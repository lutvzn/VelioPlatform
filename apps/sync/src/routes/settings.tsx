// Account settings (PLAN.md, "Keys"; Phase 6): changing the password, the
// recovery key, and deleting the account; and, from the reset link, setting
// a new password with the recovery key.
//
// Each setting needs the current password, which vault.js sends as its auth
// key, so a stolen session cookie alone can't change who opens the lists or
// delete them. The server never sees a password, the owner's keys or the
// recovery key: only auth keys' hashes and sealed owner bundles. It does
// check that a new bundle comes with the account's own public keys.

import { Hono } from "hono";
import { accountStub, type OwnerRecord } from "../account.ts";
import {
  authHashOf,
  findAccountById,
  peekEmailLink,
  readCredentials,
  readOwnerBundle,
  useEmailLink,
  type Credentials,
} from "../lib/accounts.ts";
import { clientIp, readForm, type AppContext, type AppEnv } from "../lib/http.ts";
import { withinLimit } from "../lib/limits.ts";
import { endSession, signedIn, startSession } from "../lib/session.ts";
import { sameDigest } from "../lib/tokens.ts";
import { NO_KEYS } from "../lib/vault.ts";
import { Nav } from "../views/layout.tsx";
import { AccountDeleted } from "../views/pages.tsx";
import { accountAnswer, isHtmx, problem, signedInAnswer, TOO_MANY } from "./web.tsx";

export const settings = new Hono<AppEnv>();

type Field = (name: string) => string;

const LINK_GONE = "This link was already used, or it's too old. Ask for a new one.";
const NOT_OUR_KEYS = "These aren't the account's keys. Reload the page and try again.";

/** The signed-in account, if `current_auth_key` is its password's; otherwise the problem to answer. */
const checkPassword = async (c: AppContext, field: Field, target: string) => {
  const session = signedIn(c);
  if (!session) return problem(c, "Sign in again.", 401, target);
  if (
    !(await withinLimit(c.env, "AUTH_LIMIT", `settings:${clientIp(c)}`)) ||
    !(await withinLimit(c.env, "EMAIL_LIMIT", `settings:${session.accountId}`))
  )
    return problem(c, TOO_MANY, 429, target);
  const account = await findAccountById(c.env.DB, session.accountId);
  const authHash = await authHashOf(field("current_auth_key"));
  if (!account || !authHash || !sameDigest(account.auth_hash, authHash))
    return problem(c, "That isn't your password.", 401, target);
  return session;
};

const sameKeys = (a: OwnerRecord, b: Credentials["owner"]) =>
  a.signPublicKey === b.signPublicKey && a.boxPublicKey === b.boxPublicKey;

/**
 * A new password for the same owner keys: the new owner bundle first, then
 * the salt, settings and auth hash in D1; every session and email link ends. If D1
 * fails, the old bundle goes back, so the old password still opens it.
 */
const setPassword = async (
  c: AppContext,
  accountId: string,
  before: OwnerRecord,
  credentials: Credentials,
) => {
  const stub = accountStub(c.env, accountId);
  await stub.setOwner({ ...credentials.owner, updatedAt: Date.now() });
  const { salt, kdf, authHash } = credentials;
  try {
    await c.env.DB.batch([
      // Opening a reset link proves the email, so it confirms an unconfirmed account, as a reset does.
      c.env.DB.prepare(
        `UPDATE accounts SET salt = ?, kdf_memory_kib = ?, kdf_iterations = ?, kdf_parallelism = ?, auth_hash = ?,
           confirmed_at = COALESCE(confirmed_at, ?) WHERE id = ?`,
      ).bind(salt, kdf.memoryKiB, kdf.iterations, kdf.parallelism, authHash, Date.now(), accountId),
      c.env.DB.prepare("DELETE FROM sessions WHERE account_id = ?").bind(accountId),
      // Reset links sent before stop working.
      c.env.DB.prepare("DELETE FROM email_links WHERE account_id = ?").bind(accountId),
    ]);
  } catch (error) {
    await stub.setOwner(before);
    throw error;
  }
  c.set("session", null);
  await startSession(c, accountId);
};

// --- Changing the password ---

settings.post("/password", async c => {
  const target = "#password-problem";
  const field = await readForm(c);
  const session = await checkPassword(c, field, target);
  if (session instanceof Response) return session;
  const credentials = await readCredentials(field);
  if (typeof credentials === "string") return problem(c, credentials, 400, target);
  const before = await accountStub(c.env, session.accountId).getOwner();
  if (!before) return problem(c, NO_KEYS, 500, target);
  if (!sameKeys(before, credentials.owner)) return problem(c, NOT_OUR_KEYS, 400, target);
  await setPassword(c, session.accountId, before, credentials);
  return accountAnswer(c, session.accountId, session.email, {
    notice: "Your password is changed. Other browsers are signed out; your TVs stay signed in.",
  });
});

// --- The recovery key ---

settings.post("/recovery", async c => {
  const field = await readForm(c);
  const on = field("action") === "on";
  const target = on ? "#recovery-problem" : "#recovery-off-problem";
  const session = await checkPassword(c, field, target);
  if (session instanceof Response) return session;
  const stub = accountStub(c.env, session.accountId);
  if (!on) {
    await stub.setRecovery(null);
    return accountAnswer(c, session.accountId, session.email, { notice: "The recovery key is off." });
  }
  const authHash = await authHashOf(field("recovery_auth_key"));
  const bundle = readOwnerBundle(field("recovery_bundle"));
  if (!authHash || !bundle)
    return problem(c, "This browser didn't send the recovery key's bundle.", 400, target);
  await stub.setRecovery({ authHash, bundle });
  return accountAnswer(c, session.accountId, session.email, {
    notice: "Your recovery key is on.",
    showRecoveryKey: true,
  });
});

/**
 * From the reset link, with the recovery key's auth key: the recovery copy
 * of the owner bundle, for vault.js to open and seal again under a new
 * password. Doesn't use the link.
 */
settings.post("/reset/:token/recovery-bundle", async c => {
  c.header("Cache-Control", "no-store");
  if (!(await withinLimit(c.env, "AUTH_LIMIT", `recover:${clientIp(c)}`)))
    return c.json({ problem: TOO_MANY }, 429);
  const accountId = await peekEmailLink(c.env.DB, c.req.param("token"), "reset");
  if (!accountId) return c.json({ problem: LINK_GONE }, 404);
  if (!(await withinLimit(c.env, "EMAIL_LIMIT", `recover:${accountId}`)))
    return c.json({ problem: TOO_MANY }, 429);
  const authHash = await authHashOf((await readForm(c))("recovery_auth_key"));
  const owner = authHash ? await accountStub(c.env, accountId).recoveryBundle(authHash) : null;
  if (!owner) return c.json({ problem: "That isn't this account's recovery key." }, 401);
  const { bundle, signPublicKey, boxPublicKey } = owner;
  return c.json({ bundle, signPublicKey, boxPublicKey });
});

/** A new password with the recovery key: lists, profiles, TVs and the recovery key stay. */
settings.post("/reset/:token/recover", async c => {
  const target = "#recover-problem";
  if (!(await withinLimit(c.env, "AUTH_LIMIT", `recover:${clientIp(c)}`)))
    return problem(c, TOO_MANY, 429, target);
  const field = await readForm(c);
  const credentials = await readCredentials(field);
  if (typeof credentials === "string") return problem(c, credentials, 400, target);
  const token = c.req.param("token");
  const linked = await peekEmailLink(c.env.DB, token, "reset");
  if (!linked) return problem(c, LINK_GONE, 404, target);
  const stub = accountStub(c.env, linked);
  const authHash = await authHashOf(field("recovery_auth_key"));
  const recovery = authHash ? await stub.recoveryBundle(authHash) : null;
  if (!recovery) return problem(c, "That isn't this account's recovery key.", 401, target);
  if (!sameKeys(recovery, credentials.owner)) return problem(c, NOT_OUR_KEYS, 400, target);
  const accountId = await useEmailLink(c.env.DB, token, "reset");
  if (accountId !== linked) return problem(c, LINK_GONE, 404, target);
  const account = await findAccountById(c.env.DB, accountId);
  if (!account || account.closed_at !== null) return problem(c, "This account is closed.", 403, target);
  const before = await stub.getOwner();
  if (!before) return problem(c, NO_KEYS, 500, target);
  await setPassword(c, accountId, before, credentials);
  return signedInAnswer(
    c,
    accountId,
    account.email,
    "",
    "Your new password is set. Your lists, profiles and TVs are as they were, and your recovery key still works.",
  );
});

// --- Deleting the account ---

settings.post("/account/delete", async c => {
  const field = await readForm(c);
  const session = await checkPassword(c, field, "#delete-problem");
  if (session instanceof Response) return session;
  const { accountId } = session;
  // The Durable Object first (its sockets close as removed): if D1 then
  // failed, the account is still there, with its password, to try again.
  await accountStub(c.env, accountId).reset();
  await c.env.DB.batch([
    c.env.DB.prepare("DELETE FROM devices WHERE account_id = ?").bind(accountId),
    c.env.DB.prepare("DELETE FROM pairings WHERE account_id = ?").bind(accountId),
    c.env.DB.prepare("DELETE FROM sessions WHERE account_id = ?").bind(accountId),
    c.env.DB.prepare("DELETE FROM email_links WHERE account_id = ?").bind(accountId),
    c.env.DB.prepare("DELETE FROM accounts WHERE id = ?").bind(accountId),
  ]);
  c.set("session", null);
  await endSession(c);
  if (!isHtmx(c)) return c.redirect("/", 303);
  c.header("HX-Push-Url", "/");
  return c.html(
    <>
      <AccountDeleted />
      <Nav oob />
    </>,
  );
});
