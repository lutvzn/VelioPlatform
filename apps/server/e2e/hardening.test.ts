// Phase 7's hardening: the headers on pages and /v1, the caps on body sizes,
// and the limit on a TV's pairing polls.

import { newOwnerKeys, newPairingSecrets, pairRequest, toBase64 } from "@velio/sync-core";
import { describe, expect, it } from "vitest";
import { PERMISSIONS } from "./support/constants.ts";
import { fresh, request, SITE, worker } from "./helpers.ts";

const tvFetch = (ip: string, path: string, init: RequestInit = {}) => {
  const headers = new Headers(init.headers);
  headers.set("cf-connecting-ip", ip);
  return worker.fetch(`${SITE}${path}`, { ...init, headers });
};

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

  it("leave /v1 open to TV apps", async () => {
    const answer = await tvFetch(fresh().ip, "/v1/pair/nope", { headers: { Origin: "http://tv.invalid" } });
    expect(answer.headers.get("Access-Control-Allow-Origin")).toBe("*");
    expect(answer.headers.get("Content-Security-Policy")).toBeNull();
    expect(answer.headers.get("Cross-Origin-Resource-Policy")).toBeNull();
  });
});

describe("body sizes", () => {
  it("are capped for /v1", async () => {
    const answer = await tvFetch(fresh().ip, "/v1/pair", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name: "x".repeat(70_000) }),
    });
    expect(answer.status).toBe(413);
    expect(await answer.json()).toEqual({ problem: "This is too big to send" });
  });

  it("are capped for web forms, above the biggest change", async () => {
    const visitor = fresh();
    const answer = await request(visitor, "/signin", {
      method: "POST",
      headers: { Origin: SITE, "Content-Type": "application/x-www-form-urlencoded" },
      body: `email=${"a".repeat(11 * 1024 * 1024)}`,
    });
    expect(answer.status).toBe(413);
    // A form well under the cap still gets to its route.
    const small = await request(visitor, "/signin", {
      method: "POST",
      headers: { Origin: SITE, "HX-Request": "true", "Content-Type": "application/x-www-form-urlencoded" },
      body: `email=${"a".repeat(1_000_000)}`,
    });
    expect(small.status).not.toBe(413);
  });
});

describe("a TV's pairing polls", () => {
  it("are limited per address, so a TV has to wait for each answer", async () => {
    const { ip } = fresh();
    const secrets = newPairingSecrets();
    const publicKey = toBase64(newOwnerKeys().box.publicKey);
    const started = await tvFetch(ip, "/v1/pair", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(pairRequest(secrets, { format: 1, publicKey, name: "Busy TV" })),
    });
    const { id, pollToken } = await started.json<{ id: string; pollToken: string }>();
    const statuses: number[] = [];
    for (let i = 0; i < 21; i++) {
      const answer = await tvFetch(ip, `/v1/pair/${id}?wait=0`, {
        headers: { Authorization: `Bearer ${pollToken}` },
      });
      statuses.push(answer.status);
      await answer.body?.cancel();
    }
    expect(statuses.slice(0, 20).every(status => status === 200)).toBe(true);
    expect(statuses[20]).toBe(429);
  });
});
