// The headers on pages and on the API, and the caps on body sizes.

import { describe, expect, it } from "vitest";
import { PERMISSIONS } from "./support/constants.ts";
import { api, fresh, ops, request, SITE } from "./helpers.ts";

describe("headers", () => {
  it("lock pages down", async () => {
    const page = await request(fresh(), "/");
    expect(page.status).toBe(200);
    const headers = Object.fromEntries(page.headers);
    expect(headers).toMatchObject({
      "x-content-type-options": "nosniff",
      "referrer-policy": "no-referrer",
      "x-frame-options": "DENY",
      "cross-origin-opener-policy": "same-origin",
      "cross-origin-resource-policy": "same-origin",
      "permissions-policy": PERMISSIONS,
      "cache-control": "no-store",
    });
    expect(headers["strict-transport-security"]).toContain("max-age=63072000");
    expect(headers["content-security-policy"]).toContain("frame-ancestors 'none'");
    expect(PERMISSIONS).toContain("camera=()");
  });

  it("leave Nuvio's API open to apps on any origin, as Supabase does", async () => {
    for (const path of ["/rest/v1/rpc/sync_pull_profiles", "/auth/v1/settings", "/.well-known/nuvio"]) {
      const answer = await api(path, { headers: { Origin: "http://tv.invalid" } });
      expect(answer.headers.get("Access-Control-Allow-Origin")).toBe("*");
      expect(answer.headers.get("Content-Security-Policy")).toBeNull();
      expect(answer.headers.get("Cross-Origin-Resource-Policy")).toBeNull();
    }
    const preflight = await api("/rest/v1/profiles", {
      method: "OPTIONS",
      headers: {
        Origin: "http://tv.invalid",
        "Access-Control-Request-Method": "POST",
        "Access-Control-Request-Headers": "apikey, authorization, content-type, prefer",
      },
    });
    expect(preflight.status).toBeLessThan(300);
    expect(preflight.headers.get("Access-Control-Allow-Origin")).toBe("*");
  });
});

describe("body sizes", () => {
  it("are capped for the API, above a 5 MB avatar", async () => {
    const answer = await api("/rest/v1/rpc/sync_push_profiles", {
      body: { p_profiles: "x".repeat(9 * 1024 * 1024) },
    });
    expect(answer.status).toBe(413);
  });

  it("are capped for /ops", async () => {
    const answer = await ops("/accounts/00000000-0000-0000-0000-000000000000/close", {
      reason: "x".repeat(70_000),
    });
    expect(answer.status).toBe(413);
    expect(await answer.json()).toEqual({ problem: "This is too big to send" });
  });

  it("are capped for web forms, above the biggest form", async () => {
    const visitor = fresh();
    const answer = await request(visitor, "/signin", {
      method: "POST",
      headers: { Origin: SITE, "Content-Type": "application/x-www-form-urlencoded" },
      body: `email=${"a".repeat(2 * 1024 * 1024)}`,
    });
    expect(answer.status).toBe(413);
    // A form under the cap still gets to its route.
    const small = await request(visitor, "/signin", {
      method: "POST",
      headers: { Origin: SITE, "HX-Request": "true", "Content-Type": "application/x-www-form-urlencoded" },
      body: `email=${"a".repeat(100_000)}`,
    });
    expect(small.status).not.toBe(413);
  });
});
