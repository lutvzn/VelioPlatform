// Accounts on the website, end to end: making, confirming, signing in to,
// changing and resetting one. Each goes through Supabase Auth's own steps,
// so an account made here is one Nuvio's apps sign in to, and the other way
// round.

import { describe, expect, it } from "vitest";
import { cleanUp } from "./support/cleanup.ts";
import { UNCONFIRMED_KEEP_MS } from "./support/constants.ts";
import { exec, lit, value } from "./support/runtime.ts";
import {
  api,
  appSignIn,
  confirm,
  confirmedAccount,
  fresh,
  lastMail,
  linkIn,
  PASSWORD,
  post,
  refresh,
  request,
  sessionCookie,
  signedInAccount,
  signin,
  signup,
  textOf,
  userIdOf,
  type Tokens,
} from "./helpers.ts";

const confirmedAt = (email: string) =>
  value(`SELECT email_confirmed_at FROM auth.users WHERE lower(email) = lower(${lit(email)})`);

describe("making an account", () => {
  it("emails a link, whose page confirms it with a button, once", async () => {
    const { email, ip } = fresh();
    const visitor = { ip };
    const answer = await signup(visitor, email);
    expect(answer.status).toBe(200);
    expect(await textOf(answer)).toContain("Check your email");
    expect(lastMail(email)?.subject).toBe("Confirm your email for Velio");

    const link = linkIn(email, "verify");
    expect(link.type).toBe("signup");
    // Opening the link only shows the button, so a mail scanner confirms nothing.
    const page = await request(visitor, link.path);
    expect(page.status).toBe(200);
    expect(await page.text()).toContain('action="/verify"');
    expect(confirmedAt(email)).toBeNull();

    const confirmed = await confirm(visitor, email);
    expect(confirmed.status).toBe(200);
    expect(await textOf(confirmed)).toContain("Your email is confirmed.");
    expect(sessionCookie(confirmed)).toMatch(/^__Host-velio=/);
    expect(confirmedAt(email)).not.toBeNull();

    expect((await confirm(visitor, email)).status).not.toBe(200);
    expect((await request(visitor, link.path)).status).toBe(404);
  });

  it("can't be signed into before it's confirmed", async () => {
    const { email, ip } = fresh();
    await signup({ ip }, email);
    const answer = await signin({ ip }, email);
    expect(answer.status).toBe(400);
    expect(await textOf(answer)).toContain("Confirm your email first");
    expect((await appSignIn(email)).status).toBe(400);
  });

  it("gives the same answer for an email that already has an account, and mails its owner", async () => {
    const { email } = await confirmedAccount();
    const again = await signup(fresh(), email, "another-password-2");
    expect(again.status).toBe(200);
    expect(await textOf(again)).toContain("Check your email");
    expect(lastMail(email)?.subject).toBe("You already have a Velio account");
    // The password is still the first one.
    expect((await appSignIn(email)).status).toBe(200);
    expect((await appSignIn(email, "another-password-2")).status).toBe(400);
  });

  it("refuses forms without the terms, a valid email, or a sound password", async () => {
    const { email, ip } = fresh();
    const cases: [Record<string, string>, string][] = [
      [{ email, password: PASSWORD, password_again: PASSWORD }, "Accept the terms"],
      [{ email: "not-an-email", password: PASSWORD, password_again: PASSWORD, terms: "yes" }, "email"],
      [{ email, password: "short", password_again: "short", terms: "yes" }, "characters for the password"],
      [{ email, password: PASSWORD, password_again: "something-else", terms: "yes" }, "aren't the same"],
      [{ email, password: "x".repeat(73), password_again: "x".repeat(73), terms: "yes" }, "at most 72"],
    ];
    for (const [fields, words] of cases) {
      const answer = await post({ ip }, "/signup", fields);
      expect(answer.status).toBe(400);
      expect(answer.headers.get("HX-Retarget")).toBe("#form-problem");
      expect(await textOf(answer)).toContain(words);
    }
    expect(value(`SELECT count(*) FROM auth.users WHERE email = ${lit(email)}`)).toBe(0);
  });
});

describe("one account for the site and the apps", () => {
  it("made here, signs in to the API as Nuvio's apps do", async () => {
    const { email, userId } = await confirmedAccount();
    const signedIn = await appSignIn(email);
    expect(signedIn.status).toBe(200);
    const tokens = await signedIn.json<Tokens & { user: { id: string; email: string } }>();
    expect(tokens.user).toMatchObject({ id: userId, email });
    const profiles = await api("/rest/v1/rpc/sync_pull_profiles", { body: {}, token: tokens.access_token });
    expect(profiles.status).toBe(200);
  });

  it("made in an app, is confirmed by the site's link and signs in here", async () => {
    const { email, ip } = fresh();
    const made = await api("/auth/v1/signup", { body: { email, password: PASSWORD }, ip });
    expect(made.status).toBe(200);
    // Auth mails the site's /verify link, as GoTrue mails its own.
    expect(lastMail(email)?.subject).toBe("Confirm your email for Velio");
    expect((await confirm({ ip }, email)).status).toBe(200);
    const answer = await signin({ ip }, email);
    expect(answer.status).toBe(200);
    expect(await textOf(answer)).toContain(`Signed in as <strong>${email}</strong>`);
  });
});

describe("signing in", () => {
  it("starts a session, which the account page needs", async () => {
    const { email, ip } = fresh();
    await signup({ ip }, email);
    await confirm({ ip }, email);
    expect((await request({ ip }, "/account")).status).toBe(302);
    const answer = await signin({ ip }, email);
    expect(answer.status).toBe(200);
    expect(answer.headers.get("HX-Push-Url")).toBe("/account");
    const cookie = sessionCookie(answer);
    for (const part of ["Path=/", "HttpOnly", "Secure", "SameSite=Strict"])
      expect(answer.headers.getSetCookie().join(";")).toContain(part);
    const account = await request({ ip, cookie }, "/account");
    expect(account.status).toBe(200);
    expect(await textOf(account)).toContain(`Signed in as <strong>${email}</strong>`);
  });

  it("goes back to the page that sent the visitor, and only to one of those", async () => {
    const {
      email,
      visitor: { ip },
    } = await confirmedAccount();
    const back = await post({ ip }, "/signin", {
      email,
      password: PASSWORD,
      next: "/tv-login?code=ABCD1234",
    });
    expect(back.status).toBe(204);
    expect(back.headers.get("HX-Redirect")).toBe("/tv-login?code=ABCD1234");
    const elsewhere = await post({ ip }, "/signin", {
      email,
      password: PASSWORD,
      next: "https://evil.example/",
    });
    expect(elsewhere.status).toBe(200);
    expect(elsewhere.headers.get("HX-Redirect")).toBeNull();
  });

  it("refuses a wrong password and an unknown email alike", async () => {
    const {
      email,
      visitor: { ip },
    } = await confirmedAccount();
    const wrong = await signin({ ip }, email, "not-the-password");
    const unknown = await signin({ ip }, fresh().email);
    expect(wrong.status).toBe(400);
    expect(unknown.status).toBe(400);
    const words = await textOf(wrong);
    expect(await textOf(unknown)).toBe(words);
    expect(words).toContain("That email and password don't match an account.");
  });

  it("ends with signing out, and only that browser's session", async () => {
    const { browser, another } = await signedInAccount();
    const second = await another();
    const out = await post(browser, "/signout", {});
    expect(out.status).toBe(204);
    expect(out.headers.get("HX-Redirect")).toBe("/");
    expect(out.headers.getSetCookie().join(";")).toContain("Max-Age=0");
    expect((await request(browser, "/account")).status).toBe(302);
    expect((await request(second, "/account")).status).toBe(200);
  });
});

describe("the account page", () => {
  it("changes the password with the current one, and signs out everything else", async () => {
    const { email, browser, another } = await signedInAccount();
    const second = await another();
    const app = await (await appSignIn(email)).json<Tokens>();
    const wrong = await post(browser, "/account/password", {
      current_password: "not-it",
      password: "a-new-password-2",
      password_again: "a-new-password-2",
    });
    expect(wrong.status).toBe(403);
    const changed = await post(browser, "/account/password", {
      current_password: PASSWORD,
      password: "a-new-password-2",
      password_again: "a-new-password-2",
    });
    expect(changed.status).toBe(200);
    expect((await request(browser, "/account")).status).toBe(200);
    expect((await request(second, "/account")).status).toBe(302);
    expect((await api("/auth/v1/user", { token: app.access_token })).status).toBe(403);
    expect((await appSignIn(email)).status).toBe(400);
    expect((await appSignIn(email, "a-new-password-2")).status).toBe(200);
  });

  it("deletes the account with its password", async () => {
    const { email, browser, userId } = await signedInAccount();
    const wrong = await post(browser, "/account/delete", { current_password: "not-it" });
    expect(wrong.status).toBe(403);
    const deleted = await post(browser, "/account/delete", { current_password: PASSWORD });
    expect(deleted.status).toBe(200);
    expect(await textOf(deleted)).toContain("Your account is deleted");
    expect(value(`SELECT count(*) FROM auth.users WHERE id = ${lit(userId)}`)).toBe(0);
    expect((await appSignIn(email)).status).toBe(400);
  });
});

describe("resetting", () => {
  it("emails a link that sets a new password, and ends every other session", async () => {
    const { email, browser } = await signedInAccount();
    const app = await (await appSignIn(email)).json<Tokens>();
    const { ip } = fresh();
    const asked = await post({ ip }, "/forgot", { email });
    expect(asked.status).toBe(200);
    expect(lastMail(email)?.subject).toBe("Choose a new Velio password");
    const link = linkIn(email, "reset");
    const page = await request({ ip }, link.path);
    expect(page.status).toBe(200);
    expect(await page.text()).toContain('name="password_again"');

    const reset = await post({ ip }, "/reset", {
      token: link.token,
      password: "a-reset-password-3",
      password_again: "a-reset-password-3",
    });
    expect(reset.status).toBe(200);
    expect(await textOf(reset)).toContain("Your password is changed.");
    expect((await request(browser, "/account")).status).toBe(302);
    expect(await refreshFails(app.refresh_token)).toBe(true);
    expect((await appSignIn(email, "a-reset-password-3")).status).toBe(200);
    // The link works once.
    expect((await request({ ip }, link.path)).status).toBe(404);
  });

  it("gives the same answer for an email with no account, and sends nothing", async () => {
    const { email, ip } = fresh();
    const asked = await post({ ip }, "/forgot", { email });
    expect(asked.status).toBe(200);
    expect(await textOf(asked)).toContain("If <strong>");
    expect(lastMail(email)).toBeUndefined();
  });
});

const refreshFails = async (refreshToken: string) =>
  (await api("/auth/v1/token?grant_type=refresh_token", { body: { refresh_token: refreshToken } })).status !==
  200;

describe("abuse limits", () => {
  it("refuse posts from other sites, or with no Origin", async () => {
    const { email, ip } = fresh();
    for (const origin of ["https://evil.example", undefined]) {
      const answer = await request({ ip }, "/signin", {
        method: "POST",
        headers: {
          "Content-Type": "application/x-www-form-urlencoded",
          ...(origin ? { Origin: origin } : {}),
        },
        body: new URLSearchParams({ email, password: PASSWORD }),
      });
      expect(answer.status).toBe(403);
    }
  });

  it("limit sign-in tries per address", async () => {
    const { email } = fresh();
    const { ip } = fresh();
    const statuses: number[] = [];
    for (let i = 0; i < 21; i++) statuses.push((await signin({ ip }, email, "wrong")).status);
    expect(statuses.slice(0, 20).every(status => status === 400)).toBe(true);
    expect(statuses[20]).toBe(429);
  });
});

describe("the daily clean-up", () => {
  it("deletes accounts left unconfirmed for 7 days, and only those", async () => {
    const { email, ip } = fresh();
    await signup({ ip }, email);
    const kept = await confirmedAccount();
    const id = userIdOf(email);
    const users = () => value(`SELECT count(*) FROM auth.users WHERE id = ${lit(id)}`);
    cleanUp(Date.now() + UNCONFIRMED_KEEP_MS - 60_000);
    expect(users()).toBe(1);
    expect(cleanUp(Date.now() + UNCONFIRMED_KEEP_MS + 60_000)).toBeGreaterThanOrEqual(1);
    expect(users()).toBe(0);
    expect(value(`SELECT count(*) FROM auth.users WHERE id = ${lit(kept.userId)}`)).toBe(1);
  });

  it("deletes refresh tokens used up a day ago, as GoTrue's does, and keeps the session going", async () => {
    const { email } = await confirmedAccount();
    const first = await (await appSignIn(email)).json<Tokens>();
    const second = await (await refresh(first.refresh_token)).json<Tokens>();
    const tokens = (revoked: boolean) =>
      value(`SELECT count(*) FROM auth.refresh_tokens WHERE revoked = ${revoked} AND session_id =
               (SELECT session_id FROM auth.refresh_tokens WHERE token = ${lit(second.refresh_token)})`);
    expect(tokens(true)).toBe(1);
    cleanUp();
    expect(tokens(true)).toBe(1);
    exec(`UPDATE auth.refresh_tokens SET updated_at = now() - interval '25 hours'
            WHERE token = ${lit(first.refresh_token)}`);
    cleanUp();
    expect(tokens(true)).toBe(0);
    expect(tokens(false)).toBe(1);
    expect((await refresh(second.refresh_token)).status).toBe(200);
  });
});

describe("pages", () => {
  it("show the dev outbox only with DEV_MODE, which the tests run with", async () => {
    const page = await request(fresh(), "/dev/outbox");
    expect(page.status).toBe(200);
  });
});
