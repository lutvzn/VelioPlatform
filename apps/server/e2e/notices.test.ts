// Phase 8, end to end (PLAN.md, Phases: "a test notice closes a
// test account and stops its TV"): the /abuse form, the operator's /ops, and
// closing, opening and finally deleting an account.

import { env } from "./support/runtime.ts";
import { eventsProtocols } from "@velio/sync-core";
import { describe, expect, it, vi } from "vitest";
import { accountStub } from "./support/account.ts";
import { cleanUp } from "./support/cleanup.ts";
import { CLOSED_KEEP_MS } from "./support/constants.ts";
import { newOpsSession, OPS_COOKIE, OPS_SESSION_MS } from "./support/opsSession.ts";
import { fresh, lastMail, post, request, signin, SITE, worker, type Visitor } from "./helpers.ts";
import { connect, pull, signedInTv } from "./tv.ts";

const ABUSE = "abuse@velio.tv";

const copyright = (accountEmail: string) => ({
  kind: "copyright",
  name: "Rights Holder",
  email: "rights@example.invalid",
  organization: "Example Studios",
  account_email: accountEmail,
  details: "This account syncs a list that streams our channel without a licence.",
  work: "Example Channel, owned by Example Studios",
  sworn: "yes",
  signature: "Rights Holder",
});

const ops = (path: string, init: { method?: string; body?: unknown; token?: string } = {}) =>
  worker.fetch(`${SITE}/ops${path}`, {
    method: init.method ?? "GET",
    headers: {
      "cf-connecting-ip": fresh().ip,
      Authorization: `Bearer ${init.token ?? "test-ops-token"}`,
      ...(init.body ? { "Content-Type": "application/json" } : {}),
    },
    body: init.body ? JSON.stringify(init.body) : undefined,
  });

const sendNotice = async (fields: Record<string, string>) => {
  const answer = await post(fresh(), "/abuse", fields);
  const text = await answer.text();
  return { status: answer.status, text, id: /N-[0-9A-Z]{8}/.exec(text)?.[0] };
};

describe("the abuse form", () => {
  it("keeps a notice, mails it to the abuse address, and answers its reference", async () => {
    expect(await (await request(fresh(), "/abuse")).text()).toContain(
      "Report abuse or copyright infringement",
    );
    const { status, id } = await sendNotice(copyright("nobody@example.invalid"));
    expect(status).toBe(200);
    if (!id) throw new Error("No reference");
    const row = await env.DB.prepare("SELECT * FROM notices WHERE id = ?").bind(id).first();
    expect(row).toMatchObject({
      kind: "copyright",
      account_email: "nobody@example.invalid",
      account_id: null,
      sworn: 1,
      status: "open",
    });
    const mail = await lastMail(ABUSE);
    expect(mail?.subject).toBe(`Velio notice ${id} (copyright)`);
    expect(mail?.text).toContain("nobody@example.invalid (no such account)");
  });

  it("needs a copyright notice's statements and signature, and a description for any notice", async () => {
    expect(await sendNotice({ ...copyright(""), sworn: "" })).toMatchObject({
      status: 400,
      text: "A copyright notice needs both statements.",
    });
    expect(await sendNotice({ ...copyright(""), signature: "" })).toMatchObject({ status: 400 });
    expect(await sendNotice({ ...copyright(""), kind: "abuse", details: "Too short" })).toMatchObject({
      status: 400,
      text: "Describe what the notice is about.",
    });
    // Other notices need no statements.
    expect(
      await sendNotice({ ...copyright(""), kind: "other", sworn: "", signature: "", work: "" }),
    ).toMatchObject({ status: 200 });
  });
});

describe("the operator's API", () => {
  it("needs the token", async () => {
    expect((await ops("/notices", { token: "wrong" })).status).toBe(401);
    expect((await ops("/notices", { token: "" })).status).toBe(401);
    // No cookie or form gets in either.
    const visitor = fresh();
    expect((await request(visitor, "/ops/notices")).status).toBe(401);
  });

  it("limits wrong tokens only, not the operator", async () => {
    const { ip } = fresh();
    const call = (token: string) =>
      worker.fetch(`${SITE}/ops/notices`, {
        headers: { "cf-connecting-ip": ip, Authorization: `Bearer ${token}` },
      });
    for (let i = 0; i < 25; i++) expect((await call("test-ops-token")).status).toBe(200);
    const wrong: number[] = [];
    for (let i = 0; i < 21; i++) wrong.push((await call("wrong")).status);
    expect(wrong.slice(0, 20).every(status => status === 401)).toBe(true);
    expect(wrong[20]).toBe(429);
    // The operator still gets in from the same address.
    expect((await call("test-ops-token")).status).toBe(200);
  });
});

const signInToOps = (visitor: Visitor, token: string) =>
  request(visitor, "/ops", {
    method: "POST",
    headers: { Origin: SITE, "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({ username: "velio-operator", token }),
  });

describe("the operator's page", () => {
  it("signs in with the token, and its session cookie works the API from velio.tv only", async () => {
    const visitor: Visitor = fresh();
    const signIn = await request(visitor, "/ops");
    expect(signIn.status).toBe(200);
    expect(signIn.headers.get("X-Robots-Tag")).toBe("noindex");
    expect(await signIn.text()).toContain('name="token"');

    const wrong = await signInToOps(visitor, "wrong");
    expect(wrong.status).toBe(401);
    expect(wrong.headers.get("Set-Cookie")).toBeNull();
    expect(await wrong.text()).toContain("That isn&#39;t the operator&#39;s token.");

    const right = await signInToOps(visitor, "test-ops-token");
    expect(right.status).toBe(303);
    expect(right.headers.get("Location")).toBe("/ops");
    const setCookie = right.headers.get("Set-Cookie") ?? "";
    for (const part of [
      "Path=/ops",
      "HttpOnly",
      "Secure",
      "SameSite=Strict",
      `Max-Age=${OPS_SESSION_MS / 1000}`,
    ])
      expect(setCookie).toContain(part);
    visitor.cookie = setCookie.split(";")[0];

    const app = await request(visitor, "/ops");
    expect(await app.text()).toContain("/static/ops.js");
    expect((await request(visitor, "/ops/notices")).status).toBe(200);

    // A change needs velio.tv's Origin with the cookie: no other site can make one.
    const handle = (origin?: string) =>
      request(visitor, "/ops/notices/N-NOTTHERE", {
        method: "POST",
        headers: { "Content-Type": "application/json", ...(origin ? { Origin: origin } : {}) },
        body: JSON.stringify({ status: "rejected", action: "Test." }),
      });
    expect((await handle()).status).toBe(403);
    expect((await handle("https://evil.example")).status).toBe(403);
    expect((await handle(SITE)).status).toBe(404);

    const out = await request(visitor, "/ops/signout", { method: "POST", headers: { Origin: SITE } });
    expect(out.status).toBe(200);
    expect(out.headers.get("Set-Cookie")).toContain("Max-Age=0");
  });

  it("refuses a forged, expired or other secret's cookie", async () => {
    const visitor: Visitor = fresh();
    const now = Date.now();
    const cookies = [
      `${OPS_COOKIE}=${now + 60_000}.${"0".repeat(32)}.${"0".repeat(64)}`,
      `${OPS_COOKIE}=${await newOpsSession("test-ops-token", now - OPS_SESSION_MS - 1000)}`,
      `${OPS_COOKIE}=${await newOpsSession("another-secret", now)}`,
    ];
    for (const cookie of cookies) {
      expect((await request({ ...visitor, cookie }, "/ops/notices")).status).toBe(401);
      expect(await (await request({ ...visitor, cookie }, "/ops")).text()).toContain('name="token"');
    }
  });
});

describe("closing an account", () => {
  it("from a notice: signs out its browser and TV, tells the owner, and can be undone", async () => {
    const { email, browser, credentials, tv, token } = await signedInTv();
    const socket = await connect(tv, eventsProtocols(token));
    expect(socket.answer.status).toBe(101);

    const notice = await sendNotice(copyright(email));
    if (!notice.id) throw new Error("No reference");
    const shown = await (
      await ops(`/notices/${notice.id}`)
    ).json<{ account: { id: string; notices: unknown[] } }>();
    expect(shown.account).toMatchObject({ id: browser.accountId, notices: [{ id: notice.id }] });

    const reason = `A valid copyright notice (${notice.id}) about a list in this account.`;
    // The account's email has to be typed again; without it, or with another, nothing changes.
    for (const confirm of [undefined, "", "someone-else@example.invalid"]) {
      const refused = await ops(`/accounts/${browser.accountId}/close`, {
        method: "POST",
        body: { reason, confirm },
      });
      expect(refused.status).toBe(400);
      expect(await refused.json()).toEqual({
        problem: "The email typed doesn't match this account (confirm). Nothing was changed.",
      });
    }
    expect((await request(browser.visitor, "/account")).status).toBe(200);
    expect(
      (
        await ops(`/accounts/${browser.accountId}/close`, {
          method: "POST",
          body: { reason, confirm: ` ${email.toUpperCase()} ` },
        })
      ).status,
    ).toBe(200);
    // Closed: the session, the TV's pull and its socket all end; signing in is refused.
    expect((await request(browser.visitor, "/account")).status).toBe(302);
    expect((await pull(tv, token)).status).toBe(401);
    await vi.waitFor(() => expect(socket.closes).toEqual([4401]));
    const refused = await signin({ ip: browser.visitor.ip }, email, credentials.authKey);
    expect(refused.status).toBe(403);
    expect(await refused.text()).toBe("This account is closed.");
    const toOwner = await lastMail(email);
    expect(toOwner?.subject).toBe("Your Velio account is closed");
    expect(toOwner?.text).toContain(reason);
    expect(toOwner?.text).toContain(ABUSE);
    // Closing twice is refused.
    expect(
      (
        await ops(`/accounts/${browser.accountId}/close`, {
          method: "POST",
          body: { reason, confirm: email },
        })
      ).status,
    ).toBe(409);

    // Opened again (a mistake, or a counter-notice): everything is still there.
    expect((await ops(`/accounts/${encodeURIComponent(email)}`)).status).toBe(200);
    expect((await ops(`/accounts/${browser.accountId}/reopen`, { method: "POST" })).status).toBe(200);
    expect((await lastMail(email))?.subject).toBe("Your Velio account is open again");
    expect((await signin({ ip: browser.visitor.ip }, email, credentials.authKey)).status).toBe(200);
    expect(await accountStub(env, browser.accountId).getOwner()).not.toBeNull();
  });

  it("marks the notice it acts on, and deletes the data after 30 days", async () => {
    const { email, browser, tv, token } = await signedInTv();
    const notice = await sendNotice(copyright(email));
    if (!notice.id) throw new Error("No reference");
    const closed = await ops(`/accounts/${browser.accountId}/close`, {
      method: "POST",
      body: { reason: "Two valid copyright notices.", notice: notice.id, confirm: email },
    });
    expect(closed.status).toBe(200);
    const { deleteAt } = await closed.json<{ deleteAt: number }>();
    const row = await env.DB.prepare("SELECT status, action FROM notices WHERE id = ?")
      .bind(notice.id)
      .first();
    expect(row).toEqual({ status: "actioned", action: "Account closed: Two valid copyright notices." });

    // Not before 30 days.
    await cleanUp(env, deleteAt - 60_000);
    expect(await accountStub(env, browser.accountId).getOwner()).not.toBeNull();
    await cleanUp(env, deleteAt + 60_000);
    expect(await accountStub(env, browser.accountId).getOwner()).toBeNull();
    const account = await env.DB.prepare(
      "SELECT email, closed_reason, purged_at, auth_hash FROM accounts WHERE id = ?",
    )
      .bind(browser.accountId)
      .first();
    expect(account).toMatchObject({ email, closed_reason: "Two valid copyright notices.", auth_hash: "" });
    expect(
      await env.DB.prepare("SELECT count(*) AS n FROM devices WHERE account_id = ?")
        .bind(browser.accountId)
        .first("n"),
    ).toBe(0);
    expect((await pull(tv, token)).status).toBe(401);
    // Too late to open it again.
    expect((await ops(`/accounts/${browser.accountId}/reopen`, { method: "POST" })).status).toBe(409);
    expect(deleteAt - Date.now()).toBeLessThanOrEqual(CLOSED_KEEP_MS);
  });

  it("can be handled without closing: a notice rejected", async () => {
    const notice = await sendNotice(copyright(""));
    if (!notice.id) throw new Error("No reference");
    expect(
      (await ops(`/notices/${notice.id}`, { method: "POST", body: { status: "rejected" } })).status,
    ).toBe(400);
    const handled = await ops(`/notices/${notice.id}`, {
      method: "POST",
      body: { status: "rejected", action: "Names no account; answered the sender." },
    });
    expect(handled.status).toBe(200);
    const open = await (await ops("/notices")).json<{ notices: { id: string }[] }>();
    expect(open.notices.map(n => n.id)).not.toContain(notice.id);
    const all = await (await ops("/notices?status=rejected")).json<{ notices: { id: string }[] }>();
    expect(all.notices.map(n => n.id)).toContain(notice.id);
  });
});
