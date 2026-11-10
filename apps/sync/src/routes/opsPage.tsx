// The operator's page (README, "Notices and closing accounts"): velio.tv/ops.
// Signing in with the OPS_TOKEN secret gives an 8-hour session cookie
// (lib/opsSession.ts); the page (src/client/ops.ts) then calls the /ops API
// with it. Without the secret set, /ops is a 404 like any unknown address.

import { Hono } from "hono";
import { html } from "hono/html";
import type { Child } from "hono/jsx";
import { readForm, type AppContext, type AppEnv } from "../lib/http.ts";
import { hasOpsSession, startOpsSession } from "../lib/opsSession.ts";
import { OpsApp, OpsSignIn } from "../views/ops.tsx";
import { isOpsToken, wrongTry } from "./ops.ts";

export const opsPage = new Hono<AppEnv>();

const page = (c: AppContext, body: Child, status: 200 | 401 | 429 = 200) => {
  c.header("X-Robots-Tag", "noindex");
  c.header("Cache-Control", "no-store");
  return c.html(html`<!doctype html>${body}`, status);
};

opsPage.get("/ops", async c => {
  if (!c.env.OPS_TOKEN) return c.notFound();
  return (await hasOpsSession(c)) ? page(c, <OpsApp />) : page(c, <OpsSignIn />);
});

// The Origin check (sameOriginPosts) covers this form like any other.
opsPage.post("/ops", async c => {
  if (!c.env.OPS_TOKEN) return c.notFound();
  const field = await readForm(c);
  if (await isOpsToken(c.env, field("token").trim())) {
    await startOpsSession(c);
    return c.redirect("/ops", 303);
  }
  if (!(await wrongTry(c))) return page(c, <OpsSignIn problem="Too many tries. Wait a minute." />, 429);
  return page(c, <OpsSignIn problem="That isn't the operator's token." />, 401);
});
