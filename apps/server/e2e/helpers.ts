// Helpers for the tests: the website as a browser with htmx uses it, and
// the API as Nuvio's apps use it.

import { expect } from "vitest";
import { lit, send, sql, SITE, value } from "./support/runtime.ts";

export { SITE };

// A different range in each test file, as rate limits count across them.
const range = Math.floor(Math.random() * 250);
let counter = 0;
/** A fresh email and IP for each test, as rate limits count per address. */
export const fresh = () => {
  counter++;
  return {
    email: `owner${range}-${counter}@example.invalid`,
    ip: `10.${range}.${Math.floor(counter / 250)}.${counter % 250}`,
  };
};

export const PASSWORD = "a-test-password-1";

// --- The website ---

export interface Visitor {
  ip: string;
  cookie?: string;
}

export const request = (visitor: Visitor, path: string, init: RequestInit = {}) => {
  const headers = new Headers(init.headers);
  if (visitor.cookie) headers.set("Cookie", visitor.cookie);
  return send(path, { ...init, headers }, visitor.ip);
};

export const post = (
  visitor: Visitor,
  path: string,
  fields: Record<string, string>,
  extra: Record<string, string> = {},
) =>
  request(visitor, path, {
    method: "POST",
    headers: {
      Origin: SITE,
      "HX-Request": "true",
      "Content-Type": "application/x-www-form-urlencoded",
      ...extra,
    },
    body: new URLSearchParams(fields),
  });

export const lastMail = (to: string) =>
  sql<{ subject: string; text: string }>(
    `SELECT subject, text FROM velio_private.dev_outbox WHERE to_address = ${lit(to)} ORDER BY id DESC LIMIT 1`,
  )[0];

/** The /verify or /reset link last mailed to the address, as its type and token. */
export const linkIn = (to: string, kind: "verify" | "reset") => {
  const match = lastMail(to)?.text.match(new RegExp(`/${kind}\\?([A-Za-z0-9_\\-=&%.]+)`));
  if (!match?.[1]) throw new Error(`No ${kind} link mailed to ${to}`);
  const query = new URLSearchParams(match[1]);
  return { path: `/${kind}?${match[1]}`, type: query.get("type") ?? "", token: query.get("token") ?? "" };
};

export const sessionCookie = (response: Response) => {
  const cookie = response.headers.getSetCookie().find(header => header.startsWith("__Host-velio="));
  if (!cookie) throw new Error("No session cookie");
  return cookie.split(";")[0] ?? "";
};

/** The answer's text, with the entities maud escapes turned back. */
export const textOf = async (response: Response) =>
  (await response.text())
    .replaceAll("&#39;", "'")
    .replaceAll("&quot;", '"')
    .replaceAll("&lt;", "<")
    .replaceAll("&gt;", ">")
    .replaceAll("&amp;", "&");

export const signup = (visitor: Visitor, email: string, password = PASSWORD) =>
  post(visitor, "/signup", { email, password, password_again: password, terms: "yes" });

/** Opens the link mailed to confirm the email, and presses its button, which signs in. */
export const confirm = (visitor: Visitor, email: string) => {
  const { type, token } = linkIn(email, "verify");
  return post(visitor, "/verify", { type, token });
};

export const signin = (visitor: Visitor, email: string, password = PASSWORD) =>
  post(visitor, "/signin", { email, password });

export const userIdOf = (email: string) => {
  const id = value(`SELECT id FROM auth.users WHERE lower(email) = lower(${lit(email)})`);
  if (typeof id !== "string") throw new Error(`No user for ${email}`);
  return id;
};

/** Makes and confirms an account, as most tests start from one; confirming signs the browser in. */
export const confirmedAccount = async () => {
  const { email, ip } = fresh();
  const visitor: Visitor = { ip };
  expect((await signup(visitor, email)).status).toBe(200);
  const confirmed = await confirm(visitor, email);
  expect(confirmed.status).toBe(200);
  return { email, visitor, confirmed, userId: userIdOf(email) };
};

/** A confirmed account with a browser signed in to it. */
export const signedInAccount = async () => {
  const { email, visitor, confirmed, userId } = await confirmedAccount();
  const browser: Visitor = { ...visitor, cookie: sessionCookie(confirmed) };
  /** Another browser, signed in to the same account. */
  const another = async (): Promise<Visitor> => {
    const second = await signin({ ip: visitor.ip }, email);
    expect(second.status).toBe(200);
    return { ip: visitor.ip, cookie: sessionCookie(second) };
  };
  return { email, browser, userId, another };
};

// --- The API, as Nuvio's apps call it ---

const discovery = await (await send("/.well-known/nuvio")).json<{ publishable_key: string }>();
/** The key every app sends as `apikey`. */
export const ANON_KEY = discovery.publishable_key;

export interface ApiCall {
  method?: string;
  body?: unknown;
  /** The user's access token; the anon key without one. */
  token?: string;
  ip?: string;
  headers?: Record<string, string>;
}

export const api = (path: string, call: ApiCall = {}) =>
  send(
    path,
    {
      method: call.method ?? (call.body === undefined ? "GET" : "POST"),
      headers: {
        apikey: ANON_KEY,
        Authorization: `Bearer ${call.token ?? ANON_KEY}`,
        ...(call.body === undefined ? {} : { "Content-Type": "application/json" }),
        ...call.headers,
      },
      body: call.body === undefined ? undefined : JSON.stringify(call.body),
    },
    call.ip ?? fresh().ip,
  );

export const rpc = (name: string, args: unknown, token?: string, ip?: string) =>
  api(`/rest/v1/rpc/${name}`, { body: args, token, ip });

export interface Tokens {
  access_token: string;
  refresh_token: string;
  token_type: string;
  expires_in: number;
}

/** Signs in as an app does: grant_type=password. */
export const appSignIn = (email: string, password = PASSWORD, ip?: string) =>
  api("/auth/v1/token?grant_type=password", { body: { email, password }, ip });

export const refresh = (refreshToken: string, ip?: string) =>
  api("/auth/v1/token?grant_type=refresh_token", { body: { refresh_token: refreshToken }, ip });

// --- /ops, as the operator calls it ---

export const OPS_TOKEN = "test-ops-token";

/** GET without a body, POST with one; as the operator, unless `token` is given. */
export const ops = (path: string, body?: unknown, token: string | null = OPS_TOKEN) =>
  send(
    `/ops${path}`,
    {
      method: body === undefined ? "GET" : "POST",
      headers: {
        ...(token === null ? {} : { Authorization: `Bearer ${token}` }),
        ...(body === undefined ? {} : { "Content-Type": "application/json" }),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    },
    fresh().ip,
  );
