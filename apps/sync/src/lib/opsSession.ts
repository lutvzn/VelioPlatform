// The operator's session on velio.tv/ops: signing in with the OPS_TOKEN
// secret gives a cookie that lasts 8 hours. The cookie holds its expiry, a
// random part and an HMAC of both keyed by OPS_TOKEN, so nothing is stored,
// and changing OPS_TOKEN (wrangler secret put) ends every session at once.
//
// The cookie is HttpOnly, Secure, SameSite=Strict, and sent only to /ops.
// Calls that change something also need velio.tv's own Origin.

import { deleteCookie, getCookie, setCookie } from "hono/cookie";
import type { AppContext } from "./http.ts";
import { hmacSha256, sameDigest } from "./tokens.ts";

export const OPS_COOKIE = "__Secure-velio-ops";
export const OPS_SESSION_MS = 8 * 3_600_000;
const COOKIE_OPTIONS = { path: "/ops", secure: true, httpOnly: true, sameSite: "Strict" } as const;

const toHex = (bytes: Uint8Array) => [...bytes].map(byte => byte.toString(16).padStart(2, "0")).join("");

const mac = async (secret: string, message: string) =>
  toHex(await hmacSha256(`velio-ops-session.${secret}`, message));

/** A new cookie value for the operator, valid for OPS_SESSION_MS. */
export const newOpsSession = async (secret: string, now = Date.now()) => {
  const expires = now + OPS_SESSION_MS;
  const nonce = toHex(crypto.getRandomValues(new Uint8Array(16)));
  return `${expires}.${nonce}.${await mac(secret, `${expires}.${nonce}`)}`;
};

/** Whether a cookie value was made with this secret and hasn't expired. */
export const validOpsSession = async (secret: string, value: string | undefined, now = Date.now()) => {
  const match = /^(\d{13})\.([0-9a-f]{32})\.([0-9a-f]{64})$/.exec(value ?? "");
  if (!match) return false;
  const expires = Number(match[1]);
  if (expires <= now || expires > now + OPS_SESSION_MS) return false;
  return sameDigest(match[3] ?? "", await mac(secret, `${match[1]}.${match[2]}`));
};

export const hasOpsSession = (c: AppContext) =>
  c.env.OPS_TOKEN ? validOpsSession(c.env.OPS_TOKEN, getCookie(c, OPS_COOKIE)) : Promise.resolve(false);

export const startOpsSession = async (c: AppContext) =>
  setCookie(c, OPS_COOKIE, await newOpsSession(c.env.OPS_TOKEN), {
    ...COOKIE_OPTIONS,
    maxAge: OPS_SESSION_MS / 1000,
  });

export const endOpsSession = (c: AppContext) => deleteCookie(c, OPS_COOKIE, COOKIE_OPTIONS);
