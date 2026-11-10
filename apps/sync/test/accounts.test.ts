// Phase 1, end to end in the Workers runtime: making, confirming, signing
// in to and resetting an account (PLAN.md, Phases: "Done when").
//
// The test stands in for vault.js (see helpers.ts); the owner bundle is
// real, so it also checks that it comes back openable.

import { env } from "cloudflare:test";
import { DEFAULT_KDF, fromBase64, newKey, ownerPublicKeys, toBase64 } from "@velio/sync-core";
import { describe, expect, it } from "vitest";
import { accountStub } from "../src/account.ts";
import { cleanUp } from "../src/lib/cleanup.ts";
import {
  confirm,
  confirmedAccount,
  fresh,
  lastMail,
  linkIn,
  newCredentials,
  openAnswer,
  post,
  request,
  sessionCookie,
  signin,
  signup,
  textOf,
} from "./helpers.ts";

describe("making an account", () => {
  it("emails a link, which confirms it once, with a button", async () => {
    const { email, ip } = fresh();
    const visitor = { ip };
    const response = await signup(visitor, email, newCredentials());
    expect(response.status).toBe(200);
    expect(await response.text()).toContain("Check your email");
    expect((await lastMail(email))?.subject).toBe("Confirm your email for Velio");

    const token = await linkIn(email, "confirm");
    // Opening the link only shows the button, so a mail scanner confirms nothing.
    const page = await request(visitor, `/confirm/${token}`);
    expect(page.status).toBe(200);
    expect(await page.text()).toContain(`action="/confirm/${token}"`);
    const account = await env.DB.prepare("SELECT confirmed_at FROM accounts WHERE email = ?")
      .bind(email)
      .first<{ confirmed_at: number | null }>();
    expect(account?.confirmed_at).toBeNull();

    const confirmed = await post(visitor, `/confirm/${token}`, {});
    expect(await confirmed.text()).toContain("Your email is confirmed");
    expect((await post(visitor, `/confirm/${token}`, {})).status).toBe(200);
    expect(await textOf(await post(visitor, `/confirm/${token}`, {}))).toContain("This link doesn't work");
    expect((await request(visitor, `/confirm/${token}`)).status).toBe(404);
  });

  it("can't be signed into before it's confirmed", async () => {
    const { email, ip } = fresh();
    const credentials = newCredentials();
    await signup({ ip }, email, credentials);
    const response = await signin({ ip }, email, credentials.authKey);
    expect(response.status).toBe(403);
    expect(await response.text()).toContain("Confirm your email first");
  });

  it("replaces an unconfirmed account made with the same email, and only the newest link works", async () => {
    const { email, ip } = fresh();
    const first = newCredentials();
    await signup({ ip }, email, first);
    const firstToken = await linkIn(email, "confirm");
    const second = newCredentials();
    await signup({ ip: `${ip}0` }, email, second);
    expect(await textOf(await post({ ip }, `/confirm/${firstToken}`, {}))).toContain("doesn't work");
    await confirm({ ip }, email);
    expect((await signin({ ip }, email, first.authKey)).status).toBe(401);
    const response = await signin({ ip }, email, second.authKey);
    expect(response.status).toBe(200);
    await openAnswer(response, second.unlockKey);
  });

  it("gives the same answer for an email that already has an account, and changes nothing", async () => {
    const { email, visitor, credentials } = await confirmedAccount();
    const response = await signup({ ip: `${visitor.ip}0` }, email, newCredentials());
    expect(response.status).toBe(200);
    expect(await response.text()).toContain("Check your email");
    expect((await lastMail(email))?.subject).toBe("You already have a Velio account");
    expect((await signin(visitor, email, credentials.authKey)).status).toBe(200);
  });

  it("refuses forms without the terms, a valid email, or sound keys", async () => {
    const { email, ip } = fresh();
    const { fields } = newCredentials();
    const cases: [Record<string, string>, RegExp][] = [
      [{ email, ...fields }, /Accept the terms/],
      [{ email: "nope", terms: "yes", ...fields }, /doesn't look like an email/],
      [{ email, terms: "yes", ...fields, kdf_memory_kib: "1024" }, /settings are refused/],
      [{ email, terms: "yes", ...fields, auth_key: "short" }, /auth key/],
      [{ email, terms: "yes", ...fields, salt: "" }, /salt/],
      [{ email, terms: "yes", ...fields, bundle: "{}" }, /owner bundle/],
    ];
    for (const [index, [form, message]] of cases.entries()) {
      const response = await post({ ip: `${ip}.${index}` }, "/signup", form);
      expect(response.status).toBe(400);
      // Shown in the form's problem line, so what was typed stays.
      expect(response.headers.get("HX-Retarget")).toBe("#form-problem");
      expect(await textOf(response)).toMatch(message);
    }
  });
});

describe("signing in", () => {
  it("answers the account's salt and settings before sign-in", async () => {
    const { email, visitor, credentials } = await confirmedAccount();
    const response = await request(visitor, `/prelogin?email=${encodeURIComponent(email.toUpperCase())}`);
    expect(await response.json()).toEqual({ salt: credentials.fields.salt, ...DEFAULT_KDF });
  });

  it("answers made-up but stable settings for an email with no account", async () => {
    const { ip } = fresh();
    const ask = async (email: string): Promise<{ salt: string; memoryKiB: number }> =>
      (await request({ ip }, `/prelogin?email=${email}`)).json();
    const first = await ask("nobody@example.invalid");
    expect(fromBase64(first.salt)).toHaveLength(16);
    expect(first.memoryKiB).toBe(DEFAULT_KDF.memoryKiB);
    expect((await ask("nobody@example.invalid")).salt).toBe(first.salt);
    expect((await ask("someone-else@example.invalid")).salt).not.toBe(first.salt);
  });

  it("starts a session and sends back the owner bundle, which opens with the unlock key", async () => {
    const { email, visitor, credentials } = await confirmedAccount();
    const response = await signin(visitor, email, credentials.authKey);
    expect(response.status).toBe(200);
    expect(response.headers.get("HX-Push-Url")).toBe("/account");
    const setCookie = response.headers.getSetCookie().join("\n");
    expect(setCookie).toMatch(/__Host-velio=.+; Max-Age=2592000; Path=\/; HttpOnly; Secure; SameSite=Strict/);
    const owner = await openAnswer(response, credentials.unlockKey);
    expect(ownerPublicKeys(owner)).toEqual(credentials.publicKeys);

    const cookie = sessionCookie(response);
    const account = await request({ ...visitor, cookie }, "/account");
    expect(account.status).toBe(200);
    expect(await account.text()).toContain(email);
  });

  it("refuses a wrong key and an unknown email alike", async () => {
    const { email, visitor } = await confirmedAccount();
    const wrong = await signin(visitor, email, toBase64(newKey()));
    const unknown = await signin(visitor, "nobody-here@example.invalid", toBase64(newKey()));
    expect(wrong.status).toBe(401);
    expect(unknown.status).toBe(401);
    expect(await wrong.text()).toBe(await unknown.text());
  });

  it("sends the account page to sign-in without a session, and signing out ends it", async () => {
    const { email, visitor, credentials } = await confirmedAccount();
    expect((await request(visitor, "/account")).headers.get("Location")).toBe("/signin");
    const cookie = sessionCookie(await signin(visitor, email, credentials.authKey));
    const out = await post({ ...visitor, cookie }, "/signout", {});
    expect(out.headers.get("HX-Redirect")).toBe("/");
    expect((await request({ ...visitor, cookie }, "/account")).status).toBe(302);
  });
});

describe("resetting", () => {
  it("emails a link that sets a new password, deletes the old keys and ends every session", async () => {
    const { email, visitor, credentials } = await confirmedAccount();
    const oldCookie = sessionCookie(await signin(visitor, email, credentials.authKey));

    const asked = await post(visitor, "/reset", { email });
    expect(await asked.text()).toContain("Check your email");
    const token = await linkIn(email, "reset");
    expect(await (await request(visitor, `/reset/${token}`)).text()).toContain("Choose a new password");

    const next = newCredentials();
    const reset = await post(visitor, `/reset/${token}`, next.fields);
    expect(reset.status).toBe(200);
    const newCookie = sessionCookie(reset);
    expect(ownerPublicKeys(await openAnswer(reset, next.unlockKey))).toEqual(next.publicKeys);

    expect((await request({ ...visitor, cookie: oldCookie }, "/account")).status).toBe(302);
    expect((await request({ ...visitor, cookie: newCookie }, "/account")).status).toBe(200);
    expect((await signin(visitor, email, credentials.authKey)).status).toBe(401);
    expect((await signin(visitor, email, next.authKey)).status).toBe(200);
    expect((await post(visitor, `/reset/${token}`, newCredentials().fields)).status).toBe(404);
  });

  it("gives the same answer for an email with no account, and sends nothing", async () => {
    const { email, ip } = fresh();
    const response = await post({ ip }, "/reset", { email });
    expect(await response.text()).toContain("If <strong>");
    expect(await lastMail(email)).toBeNull();
  });
});

describe("abuse limits", () => {
  it("refuses posts from other sites, or with no Origin", async () => {
    const { email, ip } = fresh();
    const fields = { email, terms: "yes", ...newCredentials().fields };
    expect((await post({ ip }, "/signup", fields, { Origin: "https://elsewhere.example" })).status).toBe(403);
    const noOrigin = await request({ ip }, "/signup", { method: "POST", body: new URLSearchParams(fields) });
    expect(noOrigin.status).toBe(403);
  });

  it("limits sign-in tries per IP address", async () => {
    const { ip } = fresh();
    const statuses: number[] = [];
    for (let i = 0; i < 22; i++)
      statuses.push((await signin({ ip }, `guess${i}-${ip}@example.invalid`, toBase64(newKey()))).status);
    expect(statuses.slice(0, 20).every(status => status === 401)).toBe(true);
    expect(statuses.at(-1)).toBe(429);
  });

  it("limits sign-in tries per email, from any address", async () => {
    const { email, visitor } = await confirmedAccount();
    const statuses: number[] = [];
    for (let i = 0; i < 6; i++)
      statuses.push((await signin({ ip: `${visitor.ip}.${i}` }, email, toBase64(newKey()))).status);
    expect(statuses.slice(0, 5)).toEqual([401, 401, 401, 401, 401]);
    expect(statuses[5]).toBe(429);
  });
});

describe("pages", () => {
  it("are sent with a strict CSP and no caching", async () => {
    const response = await request({ ip: "203.0.113.1" }, "/");
    expect(response.status).toBe(200);
    expect(await response.text()).toMatch(/^<!doctype html><html lang="en">/);
    const csp = response.headers.get("Content-Security-Policy") ?? "";
    expect(csp).toContain("script-src 'self' 'wasm-unsafe-eval' https://challenges.cloudflare.com");
    expect(csp).toContain("frame-ancestors 'none'");
    expect(response.headers.get("X-Content-Type-Options")).toBe("nosniff");
    expect(response.headers.get("Cache-Control")).toBe("no-store");
  });

  it("show the dev outbox only with DEV_MODE", async () => {
    expect((await request({ ip: "203.0.113.2" }, "/dev/outbox")).status).toBe(200);
  });
});

describe("the daily clean-up", () => {
  it("deletes accounts left unconfirmed for 7 days, with their keys", async () => {
    const { email, ip } = fresh();
    await signup({ ip }, email, newCredentials());
    const row = await env.DB.prepare("SELECT id FROM accounts WHERE email = ?")
      .bind(email)
      .first<{ id: string }>();
    if (!row) throw new Error("No account");
    expect(await accountStub(env, row.id).getOwner()).not.toBeNull();

    await cleanUp(env, Date.now() + 6 * 86_400_000);
    expect(await env.DB.prepare("SELECT id FROM accounts WHERE id = ?").bind(row.id).first()).not.toBeNull();
    await cleanUp(env, Date.now() + 8 * 86_400_000);
    expect(await env.DB.prepare("SELECT id FROM accounts WHERE id = ?").bind(row.id).first()).toBeNull();
    expect(await accountStub(env, row.id).getOwner()).toBeNull();
  });
});
