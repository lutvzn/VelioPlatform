// Phase 0's test routes (PLAN.md, Phase 0), which VelioTV's `sync` probe
// calls from the TV:
//
//   GET /v1/probe               how the request arrived: TLS version and cipher, HTTP version
//   GET /v1/probe/test-account  a made-up account's pull, to time verifying and decrypting it
//   GET /v1/probe/events        a WebSocket that sends "rev 1" to "rev 5", and echoes what it gets

import { Hono } from "hono";
import type { AppEnv } from "../lib/http.ts";
import { EVENT_COUNT, EVENT_GAP_MS } from "../probe/events.ts";
import testAccount from "../probe/test-account.json";

export const probe = new Hono<AppEnv>();

probe.get("/v1/probe", c => {
  const cf = c.req.raw.cf;
  c.header("Cache-Control", "no-store");
  return c.json({
    ok: true,
    now: Date.now(),
    tlsVersion: cf?.tlsVersion ?? null,
    tlsCipher: cf?.tlsCipher ?? null,
    httpProtocol: cf?.httpProtocol ?? null,
    colo: cf?.colo ?? null,
    userAgent: c.req.header("User-Agent") ?? null,
  });
});

probe.get("/v1/probe/test-account", c => {
  c.header("Cache-Control", "no-store");
  return c.json(testAccount);
});

probe.get("/v1/probe/events", c => {
  if (c.req.header("Upgrade")?.toLowerCase() !== "websocket")
    return c.text("This address takes a WebSocket\n", 426);
  const { 0: client, 1: server } = new WebSocketPair();
  server.accept();
  let sent = 0;
  const timer = setInterval(() => {
    sent++;
    server.send(`rev ${sent}`);
    if (sent >= EVENT_COUNT) clearInterval(timer);
  }, EVENT_GAP_MS);
  server.addEventListener("message", event => {
    server.send(`echo ${typeof event.data === "string" ? event.data : "(binary)"}`);
  });
  server.addEventListener("close", () => clearInterval(timer));
  return new Response(null, { status: 101, webSocket: client });
});
