// The web half of signing in a TV (PLAN.md, "Signing in a TV"):
//
//   GET  /pair        the pairing page, or sign-in first
//   POST /pair/find   the TV with this id, for the right proof: its sealed details
//   POST /pair/:id    the signed change that adds the TV, and the payload sealed for it
//
// The TV's half is in tv.ts.

import { Hono } from "hono";
import { isSealed, PAIRING_ID_PATTERN, type PairingKind } from "@velio/sync-core";
import { accountStub } from "../account.ts";
import { readForm, type AppContext, type AppEnv, type Session } from "../lib/http.ts";
import { withinLimit } from "../lib/limits.ts";
import { signedIn } from "../lib/session.ts";
import { newToken, sameDigest, sha256Hex } from "../lib/tokens.ts";
import { NO_KEYS, refusalText, vaultProps } from "../lib/vault.ts";
import { PAIR_TITLE, PairConfirm, PairPage } from "../views/pair.tsx";
import { SigninForm } from "../views/pages.tsx";
import { fullPage, isHtmx, problem, TOO_MANY } from "./web.tsx";

export const pair = new Hono<AppEnv>();

const MAX_CHANGE_LENGTH = 3_000_000;
const MAX_PAYLOAD_LENGTH = 2000;
const GONE =
  "No TV is waiting with this code. Codes last 10 minutes and work once: check the code, or start again on the TV.";

interface PairingRow {
  device_id: string;
  public_key: string;
  name: string;
  sealed_device: string;
  proof_qr_hash: string;
  proof_code_hash: string;
  expires_at: number;
  completed_at: number | null;
}

const isKind = (kind: string): kind is PairingKind => kind === "qr" || kind === "code";

/**
 * The pairing with this id, if it's still waiting and the proof is right
 * for its kind. Wrong ids and wrong proofs answer the same.
 */
const waitingPairing = async (c: AppContext, id: string, kind: string, proof: string) => {
  if (!PAIRING_ID_PATTERN.test(id) || !isKind(kind) || proof === "") return null;
  const row = await c.env.DB.prepare(
    `SELECT device_id, public_key, name, sealed_device, proof_qr_hash, proof_code_hash, expires_at, completed_at
     FROM pairings WHERE id = ?`,
  )
    .bind(id)
    .first<PairingRow>();
  const proofHash = await sha256Hex(proof);
  if (!row || row.completed_at !== null || row.expires_at <= Date.now()) return null;
  if (!sameDigest(kind === "qr" ? row.proof_qr_hash : row.proof_code_hash, proofHash)) return null;
  return { row, kind };
};

/**
 * The pairing page for a signed-in owner: the whole page for GET (boosted
 * links swap the body), or #main for a form's answer.
 */
const pairPage = async (c: AppContext, session: Session, signedInName?: string) => {
  const props = await vaultProps(c, session);
  if (!props) return problem(c, NO_KEYS, 500);
  const page = <PairPage {...props} signedInName={signedInName} />;
  return c.req.method === "POST" && isHtmx(c) ? c.html(page) : fullPage(c, PAIR_TITLE, page);
};

pair.get("/pair", c => {
  const session = signedIn(c);
  if (!session)
    return fullPage(
      c,
      PAIR_TITLE,
      <SigninForm next="/pair" note="Sign in to your Velio account, then choose what the TV gets." />,
    );
  return pairPage(c, session);
});

pair.post("/pair/find", async c => {
  const session = signedIn(c);
  if (!session) return problem(c, "You were signed out. Sign in again.", 401);
  if (!(await withinLimit(c.env, "AUTH_LIMIT", `pair-find:${session.accountId}`)))
    return problem(c, TOO_MANY, 429);
  const field = await readForm(c);
  const id = field("id");
  const found = await waitingPairing(c, id, field("kind"), field("proof"));
  if (!found) return problem(c, GONE, 404);
  const stored = await accountStub(c.env, session.accountId).getState();
  return c.html(
    <PairConfirm
      pairingId={id}
      deviceId={found.row.device_id}
      kind={found.kind}
      sealedJson={found.row.sealed_device}
      profileIds={stored.profiles.map(profile => profile.id)}
    />,
  );
});

pair.post("/pair/:id", async c => {
  const target = "#pair-problem";
  const session = signedIn(c);
  if (!session) return problem(c, "You were signed out. Sign in again.", 401, target);
  if (!(await withinLimit(c.env, "AUTH_LIMIT", `pair:${session.accountId}`)))
    return problem(c, TOO_MANY, 429, target);
  const id = c.req.param("id");
  const field = await readForm(c);
  const found = await waitingPairing(c, id, field("kind"), field("proof"));
  if (!found) return problem(c, GONE, 404, target);
  const { row, kind } = found;

  const changeText = field("change");
  const payloadText = field("payload");
  if (changeText.length > MAX_CHANGE_LENGTH || payloadText.length > MAX_PAYLOAD_LENGTH)
    return problem(c, "This sign-in is too big.", 413, target);
  let change: unknown;
  let payload: unknown;
  try {
    change = JSON.parse(changeText);
    payload = JSON.parse(payloadText);
  } catch {
    payload = null;
  }
  if (!isSealed(payload))
    return problem(
      c,
      "This browser didn't send a readable sign-in. Reload the page and try again.",
      400,
      target,
    );

  // Taken for this account first, so two phones can't both sign in the same TV.
  const now = Date.now();
  const claimed = await c.env.DB.prepare(
    "UPDATE pairings SET account_id = ?, completed_at = ? WHERE id = ? AND completed_at IS NULL AND expires_at > ?",
  )
    .bind(session.accountId, now, id, now)
    .run();
  if (claimed.meta.changes !== 1) return problem(c, GONE, 404, target);

  const result = await accountStub(c.env, session.accountId).change(session.accountId, change, {
    newDevices: [row.device_id],
  });
  if (!result.ok) {
    await c.env.DB.prepare("UPDATE pairings SET account_id = NULL, completed_at = NULL WHERE id = ?")
      .bind(id)
      .run();
    if (result.code === "refused")
      console.warn(`Refused a TV sign-in for ${session.accountId}: ${result.message}`);
    return problem(c, refusalText(result), result.code === "behind" ? 409 : 400, target);
  }

  // The token goes to the TV once, from the pairing; only its hash stays.
  const deviceToken = newToken();
  await c.env.DB.batch([
    c.env.DB.prepare(
      "INSERT INTO devices (id, account_id, name, public_key, token_hash, created_at) VALUES (?, ?, ?, ?, ?, ?)",
    ).bind(row.device_id, session.accountId, row.name, row.public_key, await sha256Hex(deviceToken), now),
    c.env.DB.prepare("UPDATE pairings SET kind = ?, payload = ?, device_token = ? WHERE id = ?").bind(
      kind,
      JSON.stringify(payload),
      deviceToken,
      id,
    ),
  ]);
  return pairPage(c, session, row.name);
});
