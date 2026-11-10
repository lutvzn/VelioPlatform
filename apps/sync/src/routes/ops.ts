// The operator's API (PLAN.md, "Legal and abuse"), used by scripts/ops.mjs
// and by the operator's page at /ops (routes/opsPage.tsx): notices, and
// closing and opening accounts. JSON only. Without the OPS_TOKEN secret set,
// /ops is a 404.
//
// Two ways in: the secret as a bearer token (the command-line tool), or the
// operator's session cookie from signing in at /ops (lib/opsSession.ts),
// which also needs velio.tv's own Origin on anything but GET. No CORS, so no
// other site's page can read the answers.
//
//   GET  /ops/notices[?status=open|actioned|rejected|all]
//   GET  /ops/notices/:id
//   POST /ops/notices/:id           {status: "actioned" | "rejected", action}
//   GET  /ops/accounts/:idOrEmail
//   POST /ops/accounts/:id/close    {reason, confirm, notice?}: confirm is the account's email, typed again
//   POST /ops/accounts/:id/reopen
//   POST /ops/signout               ends the page's session

import { isRecord, normalizeEmail } from "@velio/sync-core";
import { Hono, type MiddlewareHandler } from "hono";
import { findAccount, findAccountById } from "../lib/accounts.ts";
import { closeAccount, MAX_REASON_LENGTH, reopenAccount } from "../lib/closing.ts";
import { clientIp, type AppContext, type AppEnv } from "../lib/http.ts";
import { endOpsSession, hasOpsSession } from "../lib/opsSession.ts";
import { withinLimit } from "../lib/limits.ts";
import { sameDigest, sha256Hex } from "../lib/tokens.ts";

export const ops = new Hono<AppEnv>();

const problem = (c: AppContext, status: 400 | 401 | 403 | 404 | 409 | 429, text: string) =>
  c.json({ problem: text }, status);

/** Whether a token is the OPS_TOKEN secret, compared as SHA-256 digests in constant time. */
export const isOpsToken = async (env: Env, token: string) => {
  const [given, expected] = await Promise.all([sha256Hex(token), sha256Hex(env.OPS_TOKEN)]);
  return sameDigest(given, expected);
};

/** Counts a wrong try toward the limit per IP; false once it's reached. */
export const wrongTry = (c: AppContext) => withinLimit(c.env, "AUTH_LIMIT", `ops:${clientIp(c)}`);

const onlyTheOperator: MiddlewareHandler<AppEnv> = async (c, next) => {
  // The page itself and its sign-in (routes/opsPage.tsx) aren't the API.
  if (c.req.path === "/ops") return next();
  c.header("Cache-Control", "no-store");
  c.header("X-Robots-Tag", "noindex");
  if (!c.env.OPS_TOKEN) return problem(c, 404, "Not found");
  const match = /^Bearer (.+)$/.exec(c.req.header("Authorization") ?? "");
  if (match && (await isOpsToken(c.env, match[1] ?? ""))) return next();
  if (!match && (await hasOpsSession(c))) {
    const method = c.req.method;
    if (method !== "GET" && method !== "HEAD" && c.req.header("Origin") !== new URL(c.req.url).origin)
      return problem(c, 403, "Only from the operator's page on velio.tv");
    return next();
  }
  // Only wrong tries count, so guessing is limited but the operator isn't.
  if (!(await wrongTry(c))) return problem(c, 429, "Too many tries. Wait a minute.");
  return problem(c, 401, "Not the operator's token");
};
ops.use("/ops/*", onlyTheOperator);

const readBody = async (c: AppContext) => {
  const body: unknown = await c.req.json().catch(() => null);
  return isRecord(body) ? body : {};
};
const text = (value: unknown) => (typeof value === "string" ? value.trim() : "");

interface NoticeRow {
  id: string;
  received_at: number;
  kind: string;
  name: string;
  email: string;
  organization: string;
  account_email: string;
  account_id: string | null;
  work: string;
  sworn: number;
  signature: string;
  details: string;
  status: string;
  action: string;
  handled_at: number | null;
}

const STATUSES = ["open", "actioned", "rejected"];

ops.get("/ops/notices", async c => {
  const status = c.req.query("status") ?? "open";
  if (status !== "all" && !STATUSES.includes(status)) return problem(c, 400, "Unknown status");
  const { results } = await c.env.DB.prepare(
    `SELECT id, received_at, kind, name, email, account_email, account_id, status FROM notices
     WHERE ? = 'all' OR status = ? ORDER BY received_at DESC LIMIT 200`,
  )
    .bind(status, status)
    .all();
  return c.json({ notices: results });
});

/** The account's state and its notices, for deciding (the repeat-infringer policy counts them). */
const accountSummary = async (env: Env, accountId: string) => {
  const account = await findAccountById(env.DB, accountId);
  if (!account) return null;
  const [devices, notices] = await Promise.all([
    env.DB.prepare("SELECT count(*) AS n FROM devices WHERE account_id = ?")
      .bind(accountId)
      .first<number>("n"),
    env.DB.prepare(
      "SELECT id, received_at, kind, status, action FROM notices WHERE account_id = ? ORDER BY received_at",
    )
      .bind(accountId)
      .all(),
  ]);
  const { id, email, created_at, confirmed_at, closed_at, closed_reason, purged_at } = account;
  return {
    id,
    email,
    created_at,
    confirmed_at,
    closed_at,
    closed_reason,
    purged_at,
    devices,
    notices: notices.results,
  };
};

ops.get("/ops/notices/:id", async c => {
  const notice = await c.env.DB.prepare("SELECT * FROM notices WHERE id = ?")
    .bind(c.req.param("id"))
    .first<NoticeRow>();
  if (!notice) return problem(c, 404, "No such notice");
  const account = notice.account_id ? await accountSummary(c.env, notice.account_id) : null;
  return c.json({ notice, account });
});

ops.post("/ops/notices/:id", async c => {
  const body = await readBody(c);
  const status = text(body.status);
  const action = text(body.action);
  if (status !== "actioned" && status !== "rejected") return problem(c, 400, "status: actioned or rejected");
  if (!action || action.length > MAX_REASON_LENGTH) return problem(c, 400, "Say what was done (action)");
  const done = await c.env.DB.prepare(
    "UPDATE notices SET status = ?, action = ?, handled_at = ? WHERE id = ?",
  )
    .bind(status, action, Date.now(), c.req.param("id"))
    .run();
  if (done.meta.changes !== 1) return problem(c, 404, "No such notice");
  return c.json({ ok: true });
});

ops.get("/ops/accounts/:account", async c => {
  const key = c.req.param("account");
  const found = key.includes("@") ? await findAccount(c.env.DB, normalizeEmail(key)) : null;
  const summary = await accountSummary(c.env, found?.id ?? key);
  if (!summary) return problem(c, 404, "No such account");
  return c.json({ account: summary });
});

ops.post("/ops/accounts/:id/close", async c => {
  const body = await readBody(c);
  const reason = text(body.reason);
  const notice = text(body.notice);
  if (!reason || reason.length > MAX_REASON_LENGTH)
    return problem(c, 400, "Give the reason the owner will read (reason)");
  // The account's email typed again, so a slip of the id can't close another account.
  const account = await findAccountById(c.env.DB, c.req.param("id"));
  if (!account) return problem(c, 404, "No such account");
  if (normalizeEmail(text(body.confirm)) !== account.email)
    return problem(c, 400, "The email typed doesn't match this account (confirm). Nothing was changed.");
  if (notice) {
    const known = await c.env.DB.prepare("SELECT 1 FROM notices WHERE id = ?").bind(notice).first();
    if (!known) return problem(c, 404, "No such notice");
  }
  const closed = await closeAccount(c.env, c.req.param("id"), reason, notice || undefined);
  if (!closed.ok) return problem(c, closed.status, closed.problem);
  return c.json({ ok: true, deleteAt: closed.deleteAt });
});

ops.post("/ops/signout", c => {
  endOpsSession(c);
  return c.json({ ok: true });
});

ops.post("/ops/accounts/:id/reopen", async c => {
  const opened = await reopenAccount(c.env, c.req.param("id"));
  if (!opened.ok) return problem(c, opened.status, opened.problem);
  return c.json({ ok: true });
});
