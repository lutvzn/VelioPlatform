import { describe, expect, it } from "vitest";
import {
  EVENTS_PROTOCOL,
  eventsProtocols,
  eventsUrl,
  readEvent,
  readEventsProtocols,
  reconnectDelay,
  revEvent,
} from "../src/events.ts";

describe("live updates", () => {
  it("connect to /v1/events over wss, with the token as a subprotocol", () => {
    expect(eventsUrl("https://velio.tv")).toBe("wss://velio.tv/v1/events");
    expect(eventsUrl("http://127.0.0.1:8787/")).toBe("ws://127.0.0.1:8787/v1/events");
    expect(eventsProtocols("abc_-123")).toEqual([EVENTS_PROTOCOL, "bearer.abc_-123"]);
  });

  it("read the token back on the server, only alongside the protocol", () => {
    expect(readEventsProtocols(eventsProtocols("tok-en_1").join(", "))).toBe("tok-en_1");
    expect(readEventsProtocols("bearer.tok")).toBeUndefined();
    expect(readEventsProtocols(`${EVENTS_PROTOCOL}, bearer.`)).toBeUndefined();
    expect(readEventsProtocols(EVENTS_PROTOCOL)).toBeUndefined();
    expect(readEventsProtocols(null)).toBeUndefined();
  });

  it("read the server's messages, and nothing else", () => {
    expect(readEvent(revEvent(12))).toEqual({ type: "rev", rev: 12 });
    expect(readEvent("removed")).toEqual({ type: "removed" });
    for (const other of ["pong", "rev", "rev -1", "rev 1.5", "rev 12 ", " rev 12", 12, null])
      expect(readEvent(other)).toBeUndefined();
  });

  it("reconnect after a doubling, spread-out wait, up to 5 minutes", () => {
    expect(reconnectDelay(0, () => 0)).toBe(500);
    expect(reconnectDelay(0, () => 1)).toBe(1000);
    expect(reconnectDelay(3, () => 0)).toBe(4000);
    expect(reconnectDelay(3, () => 1)).toBe(8000);
    expect(reconnectDelay(50, () => 1)).toBe(300_000);
    expect(reconnectDelay(50, () => 0)).toBe(150_000);
    expect(reconnectDelay(-1, () => 1)).toBe(1000);
  });
});
