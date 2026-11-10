// The routes TVs call (PLAN.md, "Signing in a TV", "VelioTV pulling"): JSON,
// a bearer token, open CORS and no cookies.
//
//   POST /v1/pair       start signing in: the TV's key and name, sealed and plain, and proofs
//   GET  /v1/pair/:id   wait (up to 20 s) for the owner; then the token and payload, once
//   GET  /v1/sync       what the TV's profiles may read, or 304
//   GET  /v1/events     a WebSocket: "rev N" on connecting and after each change (Phase 5)
//   POST /v1/signout    the TV forgets its keys: its token stops working and its sockets close

import { Hono } from "hono";
import {
  isRecord,
  readEventsProtocols,
  isSealed,
  MAX_DEVICE_NAME_LENGTH,
  newId,
  newPairingId,
  PAIRING_ID_PATTERN,
  PAIRING_MS,
  type SealedDeviceInfo,
} from "@velio/sync-core";
import { accountStub, EVENTS_DEVICE_HEADER } from "../account.ts";
import { clientIp, type AppContext, type AppEnv } from "../lib/http.ts";
import { withinLimit } from "../lib/limits.ts";
import { newToken, sameDigest, sha256Hex } from "../lib/tokens.ts";

export const tv = new Hono<AppEnv>();

/** 32 bytes in base64: a public key or a pairing proof. */
export const BASE64_32 = /^[A-Za-z0-9+/]{43}=$/;
/** A sealed device info is 256 bytes padded: about 400 characters of base64. */
const MAX_SEALED_LENGTH = 1000;
const MAX_WAIT_S = 20;
const POLL_EVERY_MS = 1000;
/** last_pulled_at is written at most this often, as TVs pull every 15 minutes or on each change. */
const PULLED_EVERY_MS = 10 * 60_000;

const problem = (c: AppContext, status: 400 | 401 | 404 | 410 | 426 | 429, message: string) =>
  c.json({ problem: message }, status);

const TOKEN_PATTERN = /^[A-Za-z0-9_-]{20,100}$/;

const bearer = (c: AppContext) =>
  /^Bearer ([A-Za-z0-9_-]{20,100})$/.exec(c.req.header("Authorization") ?? "")?.[1];

/** The signed-in TV a device token belongs to, in an account that isn't closed. */
const deviceByToken = async (c: AppContext, token: string) =>
  c.env.DB.prepare(
    `SELECT d.id, d.account_id, d.last_pulled_at FROM devices d JOIN accounts a ON a.id = d.account_id
     WHERE d.token_hash = ? AND a.closed_at IS NULL`,
  )
    .bind(await sha256Hex(token))
    .first<{ id: string; account_id: string; last_pulled_at: number | null }>();

const readProofs = (value: unknown) =>
  isRecord(value) &&
  typeof value.qr === "string" &&
  BASE64_32.test(value.qr) &&
  typeof value.code === "string" &&
  BASE64_32.test(value.code)
    ? { qr: value.qr, code: value.code }
    : null;

const readSealedDevice = (value: unknown): SealedDeviceInfo | null => {
  if (!isRecord(value) || !isSealed(value.qr) || !isSealed(value.code)) return null;
  const { qr, code } = value;
  if (JSON.stringify(qr).length > MAX_SEALED_LENGTH || JSON.stringify(code).length > MAX_SEALED_LENGTH)
    return null;
  return { qr, code };
};

tv.post("/v1/pair", async c => {
  if (!(await withinLimit(c.env, "AUTH_LIMIT", `pair:${clientIp(c)}`)))
    return problem(c, 429, "Too many tries. Wait a minute.");
  const body: unknown = await c.req.json().catch(() => null);
  if (!isRecord(body)) return problem(c, 400, "Send JSON.");
  const { publicKey, name } = body;
  const sealed = readSealedDevice(body.sealed);
  const proofs = readProofs(body.proofs);
  if (typeof publicKey !== "string" || !BASE64_32.test(publicKey)) return problem(c, 400, "No public key.");
  if (typeof name !== "string" || name.trim() === "" || name.length > MAX_DEVICE_NAME_LENGTH)
    return problem(c, 400, `Name the TV, in at most ${MAX_DEVICE_NAME_LENGTH} characters.`);
  if (!sealed || !proofs) return problem(c, 400, "No sealed details or proofs.");

  const now = Date.now();
  const pollToken = newToken();
  const [pollHash, qrHash, codeHash] = await Promise.all([
    sha256Hex(pollToken),
    sha256Hex(proofs.qr),
    sha256Hex(proofs.code),
  ]);
  // Ids are short, so one might be taken by another TV in its 10 minutes.
  for (let attempt = 0; attempt < 8; attempt++) {
    const id = newPairingId();
    const results = await c.env.DB.batch([
      c.env.DB.prepare("DELETE FROM pairings WHERE id = ? AND expires_at <= ?").bind(id, now),
      c.env.DB.prepare(
        `INSERT INTO pairings (id, poll_hash, proof_qr_hash, proof_code_hash, device_id, public_key, name,
           sealed_device, created_at, expires_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT (id) DO NOTHING`,
      ).bind(
        id,
        pollHash,
        qrHash,
        codeHash,
        newId(),
        publicKey,
        name.trim(),
        JSON.stringify(sealed),
        now,
        now + PAIRING_MS,
      ),
    ]);
    if (results[1]?.meta.changes === 1) return c.json({ id, pollToken, expiresAt: now + PAIRING_MS }, 201);
  }
  console.error("No free pairing id after 8 tries");
  return problem(c, 429, "Velio is busy. Try again in a minute.");
});

interface PairingRow {
  poll_hash: string;
  expires_at: number;
  completed_at: number | null;
  delivered_at: number | null;
  device_id: string;
  kind: string | null;
  payload: string | null;
  device_token: string | null;
}

const wait = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));

tv.get("/v1/pair/:id", async c => {
  const id = c.req.param("id");
  const token = bearer(c);
  if (!PAIRING_ID_PATTERN.test(id) || !token) return problem(c, 404, "No such sign-in.");
  // Each poll waits up to 20 s, so a TV needs about three a minute.
  if (!(await withinLimit(c.env, "AUTH_LIMIT", `pair-poll:${clientIp(c)}`)))
    return problem(c, 429, "Too many checks. Wait for each answer before asking again.");
  const tokenHash = await sha256Hex(token);
  const seconds = Math.min(MAX_WAIT_S, Math.max(0, Number(c.req.query("wait") ?? MAX_WAIT_S) || 0));
  const until = Date.now() + seconds * 1000;
  for (;;) {
    const row = await c.env.DB.prepare(
      `SELECT poll_hash, expires_at, completed_at, delivered_at, device_id, kind, payload, device_token
       FROM pairings WHERE id = ?`,
    )
      .bind(id)
      .first<PairingRow>();
    if (!row || !sameDigest(row.poll_hash, tokenHash)) return problem(c, 404, "No such sign-in.");
    if (row.delivered_at !== null) return problem(c, 410, "This sign-in was already picked up.");
    if (row.completed_at !== null && row.payload && row.device_token && row.kind) {
      // Hand the token over once, then forget it.
      const taken = await c.env.DB.prepare(
        "UPDATE pairings SET delivered_at = ?, device_token = NULL, payload = NULL WHERE id = ? AND delivered_at IS NULL",
      )
        .bind(Date.now(), id)
        .run();
      if (taken.meta.changes !== 1) return problem(c, 410, "This sign-in was already picked up.");
      return c.json({
        status: "done",
        deviceId: row.device_id,
        deviceToken: row.device_token,
        kind: row.kind,
        payload: JSON.parse(row.payload),
      });
    }
    if (row.expires_at <= Date.now()) return problem(c, 410, "This sign-in expired. Start again.");
    if (Date.now() + POLL_EVERY_MS > until) return c.json({ status: "waiting", expiresAt: row.expires_at });
    await wait(POLL_EVERY_MS);
  }
});

/** If-None-Match: "12" or W/"12" → 12. */
const knownRev = (header: string | undefined) => {
  const match = /^(?:W\/)?"(\d+)"$/.exec(header?.trim() ?? "");
  return match ? Number(match[1]) : undefined;
};

tv.get("/v1/sync", async c => {
  const token = bearer(c);
  if (!token) return problem(c, 401, "Sign in again.");
  const device = await deviceByToken(c, token);
  if (!device) return problem(c, 401, "Sign in again.");

  const result = await accountStub(c.env, device.account_id).devicePull(
    device.id,
    knownRev(c.req.header("If-None-Match")),
  );
  if (result.status === "removed")
    return problem(c, 401, "This TV was removed from the account. Sign in again.");

  const now = Date.now();
  if (device.last_pulled_at === null || now - device.last_pulled_at > PULLED_EVERY_MS)
    c.executionCtx.waitUntil(
      c.env.DB.prepare("UPDATE devices SET last_pulled_at = ? WHERE id = ?").bind(now, device.id).run(),
    );
  c.header("ETag", `"${result.rev}"`);
  c.header("Cache-Control", "no-store");
  if (result.status === "same") return c.body(null, 304);
  return c.json(result.pull);
});

/**
 * The TV signs itself out. Its token is replaced with a value no SHA-256
 * matches, so it opens nothing even if a copy was kept, and its sockets
 * close. The TV stays in the signed manifest, with the profile keys boxed
 * to it, until the owner removes it on the TVs page, which shows it as
 * signed out: only the owner can sign a change. Closed accounts are
 * included, so a TV signed out there doesn't come back if it's reopened.
 */
tv.post("/v1/signout", async c => {
  const token = bearer(c);
  if (!token) return problem(c, 401, "Sign in again.");
  const device = await c.env.DB.prepare("SELECT id, account_id FROM devices WHERE token_hash = ?")
    .bind(await sha256Hex(token))
    .first<{ id: string; account_id: string }>();
  if (!device) return problem(c, 401, "Sign in again.");
  await c.env.DB.prepare("UPDATE devices SET token_hash = ?, signed_out_at = ? WHERE id = ?")
    .bind(`signed-out:${device.id}`, Date.now(), device.id)
    .run();
  await accountStub(c.env, device.account_id).closeDevice(device.id);
  return c.body(null, 204);
});

/**
 * The TV's socket. The token comes as the subprotocol "bearer.<token>", as
 * a browser's WebSocket can't send headers. A refusal is a plain HTTP
 * answer, which the TV only sees as a failed connection: it keeps polling,
 * and /v1/sync tells it whether to sign in again.
 */
tv.get("/v1/events", async c => {
  if (c.req.header("Upgrade")?.toLowerCase() !== "websocket")
    return problem(c, 426, "This address takes a WebSocket.");
  if (!(await withinLimit(c.env, "AUTH_LIMIT", `events:${clientIp(c)}`)))
    return problem(c, 429, "Too many connections. Try again in a minute.");
  const token = readEventsProtocols(c.req.header("Sec-WebSocket-Protocol"));
  if (!token || !TOKEN_PATTERN.test(token)) return problem(c, 401, "Sign in again.");
  const device = await deviceByToken(c, token);
  if (!device) return problem(c, 401, "Sign in again.");

  const forwarded = new Headers({ Upgrade: "websocket", [EVENTS_DEVICE_HEADER]: device.id });
  const answer = await accountStub(c.env, device.account_id).fetch("https://account/events", {
    headers: forwarded,
  });
  if (answer.status !== 101 || !answer.webSocket)
    return problem(c, 401, "This TV was removed from the account. Sign in again.");
  // A new Response, as the one from the object has headers the middleware can't change.
  return new Response(null, { status: 101, webSocket: answer.webSocket, headers: answer.headers });
});
