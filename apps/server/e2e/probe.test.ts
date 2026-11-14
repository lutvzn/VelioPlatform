import { exports } from "./support/runtime.ts";
import { openForDevice, testDeviceKeys, type TestAccount } from "@velio/sync-core";
import { describe, expect, it } from "vitest";
import { EVENT_COUNT } from "./support/constants.ts";

const worker = exports.default;
const get = (path: string, init?: RequestInit) => worker.fetch(`https://sync.test${path}`, init);

describe("GET /v1/probe", () => {
  it("answers with CORS open to any origin", async () => {
    const response = await get("/v1/probe", { headers: { Origin: "file://" } });
    expect(response.status).toBe(200);
    expect(response.headers.get("Access-Control-Allow-Origin")).toBe("*");
    expect(response.headers.get("Cache-Control")).toBe("no-store");
    expect(await response.json()).toMatchObject({ ok: true });
  });

  it("allows the headers TVs send, in a preflight", async () => {
    const response = await get("/v1/probe", {
      method: "OPTIONS",
      headers: {
        Origin: "http://tv.local",
        "Access-Control-Request-Method": "GET",
        "Access-Control-Request-Headers": "authorization, if-none-match",
      },
    });
    expect(response.status).toBe(204);
    expect(response.headers.get("Access-Control-Allow-Headers")).toMatch(/Authorization/);
  });
});

describe("GET /v1/probe/test-account", () => {
  it("serves an account its test TV can open", async () => {
    const account = await (await get("/v1/probe/test-account")).json<TestAccount>();
    const opened = openForDevice(account.pull, testDeviceKeys(account));
    expect(opened.profiles[0].lists).toHaveLength(account.expect.lists);
  });
});

describe("GET /v1/probe/events", () => {
  it("refuses a plain request", async () => {
    expect((await get("/v1/probe/events")).status).toBe(426);
  });

  it("sends each revision, and echoes", async () => {
    const response = await get("/v1/probe/events", { headers: { Upgrade: "websocket" } });
    expect(response.status).toBe(101);
    const socket = response.webSocket!;
    socket.accept();
    const messages: string[] = [];
    const done = new Promise<void>(resolve => {
      socket.addEventListener("message", event => {
        messages.push(String(event.data));
        if (messages.filter(m => m.startsWith("rev ")).length === EVENT_COUNT) resolve();
      });
    });
    socket.send("hello");
    await done;
    socket.close();
    expect(messages).toContain("echo hello");
    expect(messages.filter(m => m.startsWith("rev "))).toEqual(
      Array.from({ length: EVENT_COUNT }, (_, i) => `rev ${i + 1}`),
    );
  });
});
