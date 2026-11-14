// A TV signing itself out (POST /v1/signout): its token stops working and its
// socket closes, while the TV stays in the signed manifest until the owner
// removes it, as only the owner can sign a change.

import { env } from "./support/runtime.ts";
import { eventsProtocols } from "@velio/sync-core";
import { describe, expect, it, vi } from "vitest";
import { accountStub } from "./support/account.ts";
import { fresh, request } from "./helpers.ts";
import { connect, pull, signedInTv, tvFetch, type Tv } from "./tv.ts";

const signOut = (tv: Tv, token: string) =>
  tvFetch(tv.ip, "/v1/signout", { method: "POST", headers: { Authorization: `Bearer ${token}` } });

describe("a TV signing out", () => {
  it("ends its token and socket, and shows as signed out until the owner removes it", async () => {
    const { browser, tv, token, trust } = await signedInTv();
    const socket = await connect(tv, eventsProtocols(token));
    expect(socket.answer.status).toBe(101);

    const answer = await signOut(tv, token);
    expect(answer.status).toBe(204);
    expect(answer.headers.get("Access-Control-Allow-Origin")).toBe("*");
    await vi.waitFor(() => expect(socket.closes).toEqual([4401]));
    expect((await pull(tv, token)).status).toBe(401);
    expect((await connect(tv, eventsProtocols(token))).answer.status).toBe(401);
    // Once is enough: the token is gone.
    expect((await signOut(tv, token)).status).toBe(401);

    const row = await env.DB.prepare("SELECT token_hash, signed_out_at FROM devices WHERE id = ?")
      .bind(trust.device)
      .first<{ token_hash: string; signed_out_at: number | null }>();
    expect(row?.token_hash).toBe(`signed-out:${trust.device}`);
    expect(row?.signed_out_at).toBeTypeOf("number");

    // Still in the manifest, so the owner sees it and can remove it, but not change its profiles.
    const state = await accountStub(env, browser.accountId).getState();
    expect(state.devices.map(device => device.id)).toContain(trust.device);
    const devices = await (await request(browser.visitor, "/devices")).text();
    expect(devices).toContain("Signed out on the TV");
    expect(devices).toContain('data-velio="device-remove"');
    expect(devices).not.toContain('data-velio="device-profiles"');
    expect(await (await request(browser.visitor, "/account")).text()).toContain("Signed out on the TV");
  });

  it("needs the TV's token", async () => {
    const { ip } = fresh();
    const missing = await tvFetch(ip, "/v1/signout", { method: "POST" });
    expect(missing.status).toBe(401);
    const wrong = await tvFetch(ip, "/v1/signout", {
      method: "POST",
      headers: { Authorization: `Bearer ${"x".repeat(43)}` },
    });
    expect(wrong.status).toBe(401);
    expect(await wrong.json()).toEqual({ problem: "Sign in again." });
  });

  it("is allowed from a TV app's own origin", async () => {
    const preflight = await tvFetch(fresh().ip, "/v1/signout", {
      method: "OPTIONS",
      headers: {
        Origin: "file://",
        "Access-Control-Request-Method": "POST",
        "Access-Control-Request-Headers": "authorization",
      },
    });
    expect(preflight.status).toBe(204);
    expect(preflight.headers.get("Access-Control-Allow-Methods")).toContain("POST");
  });
});
