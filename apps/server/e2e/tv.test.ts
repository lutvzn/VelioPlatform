// TVs, end to end: signing one in with its QR code, Velio's TV limit, and
// signing one out from the devices page or from the TV itself.

import { describe, expect, it } from "vitest";
import { lit, value } from "./support/runtime.ts";
import {
  api,
  appSignIn,
  fresh,
  ops,
  post,
  refresh,
  request,
  signedInAccount,
  SITE,
  textOf,
  type Tokens,
} from "./helpers.ts";
import {
  approve,
  exchange,
  poll,
  signedIn,
  signedInTv,
  signInTv,
  startDeviceLogin,
  startTvLogin,
} from "./tv.ts";

const setLimit = (userId: string, limit: number | null | string) =>
  ops(`/accounts/${userId}/tv-limit`, { limit });

/** The session an access token stands on. */
const sessionOf = (token: string) => {
  const payload = token.split(".")[1] ?? "";
  // A JWT's payload is JSON, which the server made.
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion
  return (JSON.parse(Buffer.from(payload, "base64url").toString()) as { session_id: string }).session_id;
};

describe("a TV signing in with its QR code", () => {
  it("is approved by a signed-in browser, then gets a session of its own", async () => {
    const { browser, userId, tv } = await signedInTv();
    expect(Object.keys(tv.tokens).toSorted()).toEqual([
      "access_token",
      "expires_in",
      "refresh_token",
      "token_type",
    ]);
    const user = await api("/auth/v1/user", { token: tv.tokens.access_token });
    expect((await user.json<{ id: string }>()).id).toBe(userId);
    // It's listed with the browser, as Nuvio TV registered it.
    const devices = await textOf(await request(browser, "/devices"));
    expect(devices).toContain("Living room TV");
    expect(devices).toContain("Nuvio TV test · tv");
    expect(devices).toContain("1 of 10 TVs signed in.");
    // Its refresh token turns, as GoTrue's does.
    const next = await refresh(tv.tokens.refresh_token);
    expect(next.status).toBe(200);
    expect((await next.json<Tokens>()).refresh_token).not.toBe(tv.tokens.refresh_token);
  });

  it("opens a page that sends the visitor to sign in first, and back", async () => {
    const tv = await startTvLogin();
    expect(tv.webUrl).toBe(`${SITE}/tv-login?code=${tv.code}`);
    const page = await request(fresh(), `/tv-login?code=${tv.code}`);
    expect(page.status).toBe(302);
    expect(page.headers.get("Location")).toBe(
      `/signin?next=${encodeURIComponent(`/tv-login?code=${tv.code}`)}`,
    );
  });

  it("isn't signed in before it's approved, nor with another TV's nonce, nor twice", async () => {
    const { browser } = await signedInTv();
    const tv = await startTvLogin();
    expect((await exchange(tv)).status).toBe(400);
    expect((await approve(browser, tv)).status).toBe(200);
    expect((await exchange({ ...tv, nonce: "another-tvs-nonce-0000" })).status).toBe(400);
    expect((await exchange(tv)).status).toBe(200);
    expect((await exchange(tv)).status).toBe(400);
    // The code is used up: its page says so.
    const again = await request(browser, `/tv-login?code=${tv.code}`);
    expect(again.status).toBe(404);
    expect(await textOf(again)).toContain("This code doesn't work");
  });
});

describe("a TV signing in with a code to type", () => {
  it("is approved at /link, by its QR code or the code typed, then gets a session", async () => {
    const { browser, userId } = await signedInAccount();
    const tv = await startDeviceLogin("Kitchen TV");
    expect(tv.row.verification_uri).toBe(`${SITE}/link`);
    expect(tv.row.verification_uri_complete).toBe(`${SITE}/link?code=${tv.userCode}`);
    expect(tv.userCode).toMatch(/^[2-9A-Z]{6}$/);
    expect(await poll(tv)).toBe("pending");

    // Typed: the form asks, then shows the TV to approve.
    expect((await request(browser, "/link")).status).toBe(200);
    const page = await textOf(await request(browser, `/link?code=${tv.userCode.toLowerCase()}`));
    expect(page).toContain("Kitchen TV");
    expect((await post(browser, "/link", { code: tv.userCode })).status).toBe(200);
    expect(await poll(tv)).toBe("approved");

    const exchanged = await exchange(tv);
    expect(exchanged.status).toBe(200);
    const tokens = await exchanged.json<Tokens>();
    const user = await api("/auth/v1/user", { token: tokens.access_token });
    expect((await user.json<{ id: string }>()).id).toBe(userId);
    expect(value(`SELECT count(*) FROM velio_private.tv_sessions WHERE user_id = ${lit(userId)}`)).toBe(1);
    expect((await exchange(tv)).status).toBe(400);
  });

  it("sends the visitor to sign in first, and isn't signed in by a wrong code", async () => {
    const tv = await startDeviceLogin();
    const page = await request(fresh(), `/link?code=${tv.userCode}`);
    expect(page.status).toBe(302);
    expect(page.headers.get("Location")).toBe(
      `/signin?next=${encodeURIComponent(`/link?code=${tv.userCode}`)}`,
    );
    const { browser } = await signedInAccount();
    expect((await request(browser, "/link?code=ZZZZZZ")).status).toBe(404);
    expect(await poll(tv)).toBe("pending");
    expect((await exchange(tv)).status).toBe(400);
  });
});

describe("the TV limit", () => {
  it("stops a TV past it, and lets it in once another signs out", async () => {
    const { browser, userId, tv } = await signedInTv();
    expect((await setLimit(userId, 1)).status).toBe(200);

    const second = await startTvLogin("Bedroom TV");
    const page = await textOf(await request(browser, `/tv-login?code=${second.code}`));
    expect(page).toContain("as many as it may have");
    expect((await approve(browser, second)).status).toBe(200);
    const refused = await exchange(second);
    expect(refused.status).toBe(403);
    expect(await refused.text()).toContain("signed in on 1 TVs, its limit");

    // Signing the first out makes room; the approved code still works.
    const out = await post(browser, `/devices/${sessionOf(tv.tokens.access_token)}/signout`, {});
    expect(out.status).toBe(200);
    expect(await textOf(out)).toContain("It's signed out.");
    expect(await signedIn(tv)).toBe(false);
    expect((await exchange(second)).status).toBe(200);
  });

  it("is the default until the operator gives another, which can't pass VELIO_TV_LIMIT_MAX", async () => {
    const { userId } = await signedInTv();
    for (const limit of [0, 51, 2.5, "five"]) expect((await setLimit(userId, limit)).status).toBe(400);
    expect((await setLimit("00000000-0000-0000-0000-000000000000", 5)).status).toBe(404);
    expect((await setLimit(userId, 50)).status).toBe(200);
    expect(value(`SELECT tv_limit FROM velio_private.tv_limits WHERE user_id = ${lit(userId)}`)).toBe(50);
    // The default is kept as none.
    expect((await setLimit(userId, null)).status).toBe(200);
    expect(value(`SELECT count(*) FROM velio_private.tv_limits WHERE user_id = ${lit(userId)}`)).toBe(0);
    const unauthorized = await ops(`/accounts/${userId}/tv-limit`, { limit: 5 }, null);
    expect(unauthorized.status).toBe(401);
  });

  it("doesn't count apps or browsers, only TVs signed in with a code", async () => {
    const { email, browser, userId, another } = await signedInTv();
    await another();
    expect((await appSignIn(email)).status).toBe(200);
    expect(value(`SELECT count(*) FROM velio_private.tv_sessions WHERE user_id = ${lit(userId)}`)).toBe(1);
    expect(await textOf(await request(browser, "/devices"))).toContain("1 of 10 TVs signed in.");
  });
});

describe("a TV signing out", () => {
  it("ends its session and frees its place, as Nuvio TV signs out", async () => {
    const { userId, tv } = await signedInTv();
    const out = await api("/auth/v1/logout?scope=local", { method: "POST", token: tv.tokens.access_token });
    expect(out.status).toBe(204);
    expect(await signedIn(tv)).toBe(false);
    expect((await refresh(tv.tokens.refresh_token)).status).toBe(400);
    expect(value(`SELECT count(*) FROM velio_private.tv_sessions WHERE user_id = ${lit(userId)}`)).toBe(0);
    // Once is enough: a session that's gone is signed out.
    const again = await api("/auth/v1/logout?scope=local", { method: "POST", token: tv.tokens.access_token });
    expect(again.status).toBe(204);
  });

  it("from the devices page: only the account's own", async () => {
    const first = await signedInTv();
    const other = await signedInTv();
    const theirs = sessionOf(other.tv.tokens.access_token);
    const answer = await post(first.browser, `/devices/${theirs}/signout`, {});
    expect(await textOf(answer)).toContain("That was already signed out.");
    expect(await signedIn(other.tv)).toBe(true);
  });

  it("from signing out everywhere: every TV, app and browser", async () => {
    const { browser, tv } = await signedInTv();
    const second = await signInTv(browser, "Kitchen TV");
    const out = await api("/auth/v1/logout?scope=global", {
      method: "POST",
      token: second.tokens.access_token,
    });
    expect(out.status).toBe(204);
    expect(await signedIn(tv)).toBe(false);
    expect((await request(browser, "/account")).status).toBe(302);
  });
});
