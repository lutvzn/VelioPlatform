// Closing an account (PLAN.md, "Legal and abuse"): for a valid notice or a
// broken term, done by the operator through /ops. Velio can't read or remove
// one list, so it closes the account: sessions, links and pairings end, TVs
// are refused (their sockets are closed as removed), the owner is told why,
// and the encrypted data is deleted after 30 days. Until then it can be
// opened again. The row stays, so the email can't simply sign up again.

import { accountStub } from "../account.ts";
import { findAccountById } from "./accounts.ts";
import { closedMail, reopenedMail, sendMail } from "./mail.ts";

export const CLOSED_KEEP_MS = 30 * 86_400_000;
export const MAX_REASON_LENGTH = 2000;

export type Closing = { ok: true; deleteAt?: number } | { ok: false; status: 404 | 409; problem: string };

export const closeAccount = async (
  env: Env,
  accountId: string,
  reason: string,
  noticeId?: string,
  now = Date.now(),
): Promise<Closing> => {
  const account = await findAccountById(env.DB, accountId);
  if (!account) return { ok: false, status: 404, problem: "No such account" };
  if (account.closed_at !== null) return { ok: false, status: 409, problem: "The account is already closed" };
  const results = await env.DB.batch([
    env.DB.prepare(
      "UPDATE accounts SET closed_at = ?, closed_reason = ? WHERE id = ? AND closed_at IS NULL",
    ).bind(now, reason, accountId),
    env.DB.prepare("DELETE FROM sessions WHERE account_id = ?").bind(accountId),
    env.DB.prepare("DELETE FROM email_links WHERE account_id = ?").bind(accountId),
    env.DB.prepare("DELETE FROM pairings WHERE account_id = ?").bind(accountId),
    ...(noticeId
      ? [
          env.DB.prepare(
            "UPDATE notices SET status = 'actioned', action = ?, handled_at = ? WHERE id = ?",
          ).bind(`Account closed: ${reason}`, now, noticeId),
        ]
      : []),
  ]);
  if (results[0]?.meta.changes !== 1)
    return { ok: false, status: 409, problem: "The account is already closed" };
  // TVs' pulls and new sockets are refused from here (tv.ts checks closed_at); open sockets end now.
  await accountStub(env, accountId).closeSockets();
  const deleteAt = now + CLOSED_KEEP_MS;
  await sendMail(env, closedMail(env.SITE_URL, account.email, env.ABUSE_EMAIL, reason, deleteAt));
  return { ok: true, deleteAt };
};

export const reopenAccount = async (env: Env, accountId: string): Promise<Closing> => {
  const account = await findAccountById(env.DB, accountId);
  if (!account) return { ok: false, status: 404, problem: "No such account" };
  if (account.closed_at === null) return { ok: false, status: 409, problem: "The account isn't closed" };
  if (account.purged_at !== null)
    return {
      ok: false,
      status: 409,
      problem: "The account's data is already deleted; it can't be opened again",
    };
  await env.DB.prepare(
    "UPDATE accounts SET closed_at = NULL, closed_reason = NULL WHERE id = ? AND purged_at IS NULL",
  )
    .bind(accountId)
    .run();
  await sendMail(env, reopenedMail(env.SITE_URL, account.email));
  return { ok: true };
};

/** For the daily cron: deletes the data of accounts closed more than 30 days ago. */
export const purgeClosedAccounts = async (env: Env, now = Date.now()) => {
  const { results } = await env.DB.prepare(
    "SELECT id FROM accounts WHERE closed_at IS NOT NULL AND closed_at < ? AND purged_at IS NULL LIMIT 100",
  )
    .bind(now - CLOSED_KEEP_MS)
    .all<{ id: string }>();
  for (const { id } of results) {
    await accountStub(env, id).reset();
    await env.DB.batch([
      env.DB.prepare("DELETE FROM devices WHERE account_id = ?").bind(id),
      env.DB.prepare("DELETE FROM pairings WHERE account_id = ?").bind(id),
      env.DB.prepare("DELETE FROM sessions WHERE account_id = ?").bind(id),
      env.DB.prepare("DELETE FROM email_links WHERE account_id = ?").bind(id),
      // The email, the dates and the reason stay; the password's salt and hash go.
      env.DB.prepare("UPDATE accounts SET purged_at = ?, salt = '', auth_hash = '' WHERE id = ?").bind(
        now,
        id,
      ),
    ]);
  }
  return results.length;
};
