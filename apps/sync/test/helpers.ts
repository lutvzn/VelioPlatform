// Helpers for the Worker tests: requests as a browser with vault.js would
// send them.
//
// Workers can't compile WebAssembly at runtime, so these stand in for
// vault.js with random auth and unlock keys instead of running Argon2id. The
// owner bundle is real, so tests can open it.

import { env } from "cloudflare:test";
import { exports } from "cloudflare:workers";
import {
  buildChange,
  DEFAULT_KDF,
  isSealed,
  newKey,
  newOwnerKeys,
  openOwnerBundle,
  openState,
  ownerPublicKeys,
  readStoredState,
  sealOwnerBundle,
  toBase64,
  type Change,
  type OpenedState,
  type OwnerKeys,
  type OwnerPublicKeys,
} from "@velio/sync-core";
import { expect } from "vitest";
import { findAccount } from "../src/lib/accounts.ts";

export const SITE = "https://velio.tv";
// With DEV_MODE, links in emails are paths.
export const worker = exports.default;

// A different range in each test file, as rate limits count across them.
const range = Math.floor(Math.random() * 250);
let counter = 0;
/** A fresh email and IP for each test, as rate limits count across tests. */
export const fresh = () => {
  counter++;
  return {
    email: `owner${range}-${counter}@example.invalid`,
    ip: `10.${range}.${Math.floor(counter / 250)}.${counter % 250}`,
  };
};

export interface Visitor {
  ip: string;
  cookie?: string;
}

export const request = (visitor: Visitor, path: string, init: RequestInit = {}) => {
  const headers = new Headers(init.headers);
  headers.set("cf-connecting-ip", visitor.ip);
  if (visitor.cookie) headers.set("Cookie", visitor.cookie);
  return worker.fetch(`${SITE}${path}`, { ...init, headers, redirect: "manual" });
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

/** What vault.js sends in place of a new password. */
export const newCredentials = () => {
  const authKey = newKey();
  const unlockKey = newKey();
  const owner = newOwnerKeys();
  const publicKeys = ownerPublicKeys(owner);
  const fields = {
    salt: toBase64(newKey().subarray(0, 16)),
    kdf_memory_kib: String(DEFAULT_KDF.memoryKiB),
    kdf_iterations: String(DEFAULT_KDF.iterations),
    kdf_parallelism: String(DEFAULT_KDF.parallelism),
    auth_key: toBase64(authKey),
    bundle: JSON.stringify(sealOwnerBundle(unlockKey, owner)),
    sign_public_key: publicKeys.signPublicKey,
    box_public_key: publicKeys.boxPublicKey,
  };
  return { fields, authKey: fields.auth_key, unlockKey, publicKeys, owner };
};
export type Credentials = ReturnType<typeof newCredentials>;

export const lastMail = async (to: string) =>
  env.DB.prepare("SELECT subject, text FROM dev_outbox WHERE to_address = ? ORDER BY id DESC LIMIT 1")
    .bind(to)
    .first<{ subject: string; text: string }>();

export const linkIn = async (to: string, kind: "confirm" | "reset") => {
  const mail = await lastMail(to);
  const match = mail?.text.match(new RegExp(`/${kind}/([A-Za-z0-9_-]+)`));
  if (!match?.[1]) throw new Error(`No ${kind} link mailed to ${to}`);
  return match[1];
};

export const sessionCookie = (response: Response) => {
  const cookie = response.headers.getSetCookie().find(value => value.startsWith("__Host-velio="));
  if (!cookie) throw new Error("No session cookie");
  return cookie.split(";")[0];
};

/** The answer's text, with the entities JSX escapes turned back. */
export const textOf = async (response: Response) =>
  (await response.text()).replaceAll("&#39;", "'").replaceAll("&quot;", '"').replaceAll("&amp;", "&");

export const signup = (visitor: Visitor, email: string, credentials: Credentials) =>
  post(visitor, "/signup", { email, terms: "yes", ...credentials.fields });

export const confirm = async (visitor: Visitor, email: string) => {
  const token = await linkIn(email, "confirm");
  return post(visitor, `/confirm/${token}`, {});
};

export const signin = (visitor: Visitor, email: string, authKey: string) =>
  post(visitor, "/signin", { email, auth_key: authKey });

/** Makes and confirms an account, as most tests start from one. */
export const confirmedAccount = async () => {
  const { email, ip } = fresh();
  const visitor: Visitor = { ip };
  const credentials = newCredentials();
  expect((await signup(visitor, email, credentials)).status).toBe(200);
  expect((await confirm(visitor, email)).status).toBe(200);
  return { email, visitor, credentials };
};

/** The owner bundle in a signed-in answer, opened as vault.js would. */
export const openAnswer = async (response: Response, unlockKey: Uint8Array) => {
  const body = await response.text();
  const attribute = (name: string) => {
    const match = body.match(new RegExp(`${name}="([^"]*)"`));
    if (!match?.[1]) throw new Error(`No ${name} in the answer`);
    return match[1].replaceAll("&quot;", '"');
  };
  const bundle: unknown = JSON.parse(attribute("data-owner-bundle"));
  if (!isSealed(bundle)) throw new Error("The owner bundle isn't a Sealed");
  const expected: OwnerPublicKeys = {
    signPublicKey: attribute("data-sign-public-key"),
    boxPublicKey: attribute("data-box-public-key"),
  };
  return openOwnerBundle(unlockKey, bundle, expected);
};

// --- A signed-in browser with vault.js ---

export const unescape = (text: string) =>
  text
    .replaceAll("&quot;", '"')
    .replaceAll("&#39;", "'")
    .replaceAll("&lt;", "<")
    .replaceAll("&gt;", ">")
    .replaceAll("&amp;", "&");

/** The ciphertext a page carries, as vault.js reads it. */
export const stateIn = (html: string) => {
  const match = /data-vault-state="([^"]*)"/.exec(html);
  if (!match?.[1]) throw new Error("No [data-vault-state] on the page");
  return readStoredState(JSON.parse(unescape(match[1])));
};

/** A signed-in browser holding the owner's keys. */
export interface Browser {
  visitor: Visitor;
  accountId: string;
  owner: OwnerKeys;
}

export const signedInAccount = async () => {
  const { email, visitor, credentials } = await confirmedAccount();
  const answer = await signin(visitor, email, credentials.authKey);
  expect(answer.status).toBe(200);
  const account = await findAccount(env.DB, email);
  if (!account) throw new Error("No account");
  const browser: Browser = {
    visitor: { ...visitor, cookie: sessionCookie(answer) },
    accountId: account.id,
    owner: credentials.owner,
  };
  /** Another browser, signed in to the same account. */
  const another = async (): Promise<Browser> => {
    const second = await signin({ ip: visitor.ip }, email, credentials.authKey);
    return { ...browser, visitor: { ip: visitor.ip, cookie: sessionCookie(second) } };
  };
  return { email, browser, another, credentials };
};

/** Opens a page as vault.js would. */
export const openPage = async (browser: Browser, path = "/lists") => {
  const page = await request(browser.visitor, path);
  expect(page.status).toBe(200);
  const html = await page.text();
  return { html, state: openState(browser.owner, browser.accountId, stateIn(html)) };
};

export const send = (browser: Browser, change: Change, next = "/lists") =>
  post(browser.visitor, "/changes", { change: JSON.stringify(change), next });

/** Makes an edit on what the page shows and sends it, as vault.js does. */
export const save = async (browser: Browser, edit: (state: OpenedState) => OpenedState, next = "/lists") => {
  const { state } = await openPage(browser);
  const answer = await send(browser, buildChange(browser.owner, state, edit(state)), next);
  expect(answer.status).toBe(200);
  return answer;
};
