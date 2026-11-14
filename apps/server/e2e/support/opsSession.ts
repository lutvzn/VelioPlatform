// The operator's session cookie, made as src/ops_session.rs makes it: its
// expiry, a random part, and an HMAC of both keyed by OPS_TOKEN.

import { createHmac, randomBytes } from "node:crypto";

export const OPS_COOKIE = "__Secure-velio-ops";
export const OPS_SESSION_MS = 8 * 3_600_000;

/** A cookie value for the operator, valid for OPS_SESSION_MS from `now`. */
export const newOpsSession = async (secret: string, now = Date.now()) => {
  const expires = now + OPS_SESSION_MS;
  const nonce = randomBytes(16).toString("hex");
  const mac = createHmac("sha256", `velio-ops-session.${secret}`).update(`${expires}.${nonce}`).digest("hex");
  return `${expires}.${nonce}.${mac}`;
};
