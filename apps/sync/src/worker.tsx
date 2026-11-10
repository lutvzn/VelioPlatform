// The Velio Sync Worker (PLAN.md, "Server"): the web UI on velio.tv, and
// /v1 for TVs and apps. Phase 1: accounts. Phase 2: profiles and lists. Phase 3: signing in TVs and their pulls. Phase 5: live updates. Phase 6: the TVs page and account settings. Phase 8: notices at /abuse, and /ops for the operator. Phase 0's probe routes stay
// under /v1/probe.

import { Hono } from "hono";
import { bodyLimit } from "hono/body-limit";
import { cors } from "hono/cors";
import { cleanUp } from "./lib/cleanup.ts";
import { isApi, sameOriginPosts, securityHeaders, type AppEnv } from "./lib/http.ts";
import { sessions } from "./lib/session.ts";
import { pair } from "./routes/pair.tsx";
import { ops } from "./routes/ops.ts";
import { opsPage } from "./routes/opsPage.tsx";
import { probe } from "./routes/probe.ts";
import { settings } from "./routes/settings.tsx";
import { tv } from "./routes/tv.ts";
import { vault } from "./routes/vault.tsx";
import { fullPage, web } from "./routes/web.tsx";
import { NotFound } from "./views/pages.tsx";

export { Account } from "./account.ts";

const app = new Hono<AppEnv>();

app.use(securityHeaders);

// Bodies are read whole, so their size is capped before any route reads one.
// The biggest web form is a change of up to 3 MB of JSON, which form
// encoding can make longer; /v1 and /ops only take small JSON.
const webBodies = bodyLimit({
  maxSize: 10 * 1024 * 1024,
  onError: c => c.text("This is too big to send.\n", 413),
});
const apiBodies = bodyLimit({
  maxSize: 64 * 1024,
  onError: c => c.json({ problem: "This is too big to send" }, 413),
});
app.use((c, next) => (isApi(c.req.path) ? apiBodies(c, next) : webBodies(c, next)));

// TVs call /v1 from their app's own origin, with a bearer token and no cookies.
app.use(
  "/v1/*",
  cors({
    origin: "*",
    allowHeaders: ["Authorization", "Content-Type", "If-None-Match"],
    exposeHeaders: ["ETag"],
    maxAge: 86_400,
  }),
);
app.use(async (c, next) => {
  if (isApi(c.req.path)) {
    c.set("session", null);
    return next();
  }
  return sessions(c, next);
});
app.use(sameOriginPosts);

app.route("/", probe);
app.route("/", web);
app.route("/", settings);
app.route("/", vault);
app.route("/", pair);
app.route("/", tv);
app.route("/", opsPage);
app.route("/", ops);

app.notFound(c =>
  isApi(c.req.path)
    ? c.json({ problem: "Not found" }, 404)
    : fullPage(c, "Not found", <NotFound />, { status: 404 }),
);

app.onError((error, c) => {
  console.error(error);
  if (isApi(c.req.path)) return c.json({ problem: "Something went wrong on the server" }, 500);
  return c.text("Something went wrong on the server. Try again in a minute.\n", 500);
});

export default {
  fetch: app.fetch,
  async scheduled(_controller, env, ctx) {
    ctx.waitUntil(cleanUp(env).then(count => console.log(`Cleaned up ${count} unconfirmed accounts`)));
  },
} satisfies ExportedHandler<Env>;
