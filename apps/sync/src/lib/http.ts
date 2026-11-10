// Things every web request goes through: headers, the Origin check, and
// reading forms.

import type { Context, MiddlewareHandler } from "hono";

export interface Session {
  tokenHash: string;
  accountId: string;
  email: string;
}

export type AppEnv = { Bindings: Env; Variables: { session: Session | null } };
export type AppContext = Context<AppEnv>;

export const isDev = (env: Env) => env.DEV_MODE === "1";

/** /v1 for TVs and /ops for the operator: bearer tokens, no cookies, no forms. */
export const isApi = (path: string) => path.startsWith("/v1/") || path.startsWith("/ops/");

/** Cloudflare sets this on every request that reaches the Worker. */
export const clientIp = (c: AppContext) => c.req.header("cf-connecting-ip") ?? "unknown";

// 'wasm-unsafe-eval' is for Argon2id (hash-wasm). Turnstile needs its script and frame.
const CSP = [
  "default-src 'self'",
  "script-src 'self' 'wasm-unsafe-eval' https://challenges.cloudflare.com",
  "frame-src https://challenges.cloudflare.com",
  "connect-src 'self'",
  "img-src 'self' data:",
  "style-src 'self'",
  "base-uri 'none'",
  "form-action 'self'",
  "frame-ancestors 'none'",
  "object-src 'none'",
].join("; ");

/** Browser features no page uses, turned off for the pages and anything they embed. */
export const PERMISSIONS = [
  "accelerometer",
  "camera",
  "display-capture",
  "geolocation",
  "gyroscope",
  "hid",
  "magnetometer",
  "microphone",
  "midi",
  "payment",
  "serial",
  "usb",
]
  .map(feature => `${feature}=()`)
  .join(", ");

export const securityHeaders: MiddlewareHandler<AppEnv> = async (c, next) => {
  await next();
  const headers = c.res.headers;
  headers.set("X-Content-Type-Options", "nosniff");
  headers.set("Referrer-Policy", "no-referrer");
  headers.set("Strict-Transport-Security", "max-age=63072000; includeSubDomains");
  if (!c.req.path.startsWith("/v1/")) {
    headers.set("Content-Security-Policy", CSP);
    headers.set("X-Frame-Options", "DENY");
    headers.set("Cross-Origin-Opener-Policy", "same-origin");
    headers.set("Cross-Origin-Resource-Policy", "same-origin");
    headers.set("Permissions-Policy", PERMISSIONS);
    // Pages and fragments carry ciphertext and session state, so nothing is cached.
    if (!headers.has("Cache-Control")) headers.set("Cache-Control", "no-store");
  }
};

/**
 * Refuses web form posts from other sites. The session cookie is
 * SameSite=Strict already; this also covers the forms that need no session,
 * such as sign-up. /v1 (apps and TVs) and /ops (the operator) take bearer
 * tokens and no cookies.
 */
export const sameOriginPosts: MiddlewareHandler<AppEnv> = async (c, next) => {
  const method = c.req.method;
  if (method === "GET" || method === "HEAD" || method === "OPTIONS" || isApi(c.req.path)) return next();
  const origin = c.req.header("Origin");
  if (origin !== new URL(c.req.url).origin) return c.text("This form can only be sent from velio.tv.\n", 403);
  return next();
};

/** A form's text fields; files and missing fields read as "". */
export const readForm = async (c: AppContext) => {
  const body = await c.req.parseBody();
  return (name: string) => {
    const value = body[name];
    return typeof value === "string" ? value : "";
  };
};
