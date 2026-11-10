// Web sessions (PLAN.md, "Routes"): a random token in the cookie
// `__Host-velio` (HttpOnly, Secure, SameSite=Strict), stored as its SHA-256.
// 30 days, renewed on use at most once a day.

import { deleteCookie, getCookie, setCookie } from "hono/cookie";
import type { MiddlewareHandler } from "hono";
import type { AppContext, AppEnv, Session } from "./http.ts";
import { newToken, sha256Hex } from "./tokens.ts";

const COOKIE = "velio"; // sent as __Host-velio
const DAY_MS = 86_400_000;
export const SESSION_MS = 30 * DAY_MS;

const writeCookie = (c: AppContext, token: string) =>
  setCookie(c, COOKIE, token, {
    prefix: "host",
    httpOnly: true,
    secure: true,
    sameSite: "Strict",
    path: "/",
    maxAge: SESSION_MS / 1000,
  });

export const startSession = async (c: AppContext, accountId: string) => {
  const token = newToken();
  const now = Date.now();
  await c.env.DB.prepare(
    "INSERT INTO sessions (token_hash, account_id, created_at, last_used_at, expires_at) VALUES (?, ?, ?, ?, ?)",
  )
    .bind(await sha256Hex(token), accountId, now, now, now + SESSION_MS)
    .run();
  writeCookie(c, token);
};

/** Deletes this browser's session, if it has one, leaving the cookie for a new one to replace. */
export const dropSession = async (c: AppContext) => {
  const session = c.get("session");
  if (session)
    await c.env.DB.prepare("DELETE FROM sessions WHERE token_hash = ?").bind(session.tokenHash).run();
  c.set("session", null);
};

/** Signs this browser out. */
export const endSession = async (c: AppContext) => {
  await dropSession(c);
  deleteCookie(c, COOKIE, { prefix: "host", path: "/", secure: true });
};

/** Reads the session cookie into c.get("session"), renewing it once a day. */
export const sessions: MiddlewareHandler<AppEnv> = async (c, next) => {
  c.set("session", null);
  const token = getCookie(c, COOKIE, "host");
  if (token) {
    const tokenHash = await sha256Hex(token);
    const now = Date.now();
    const row = await c.env.DB.prepare(
      `SELECT s.account_id, s.last_used_at, a.email FROM sessions s JOIN accounts a ON a.id = s.account_id
       WHERE s.token_hash = ? AND s.expires_at > ? AND a.closed_at IS NULL AND a.confirmed_at IS NOT NULL`,
    )
      .bind(tokenHash, now)
      .first<{ account_id: string; last_used_at: number; email: string }>();
    if (row) {
      c.set("session", { tokenHash, accountId: row.account_id, email: row.email });
      if (now - row.last_used_at > DAY_MS) {
        await c.env.DB.prepare("UPDATE sessions SET last_used_at = ?, expires_at = ? WHERE token_hash = ?")
          .bind(now, now + SESSION_MS, tokenHash)
          .run();
        writeCookie(c, token);
      }
    }
  }
  return next();
};

export const signedIn = (c: AppContext): Session | null => c.get("session");
