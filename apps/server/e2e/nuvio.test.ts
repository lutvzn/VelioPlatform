// Nuvio's own smoke tests (smoke/run.sh, from Nuvio's self-host repository)
// against Velio: what a Nuvio app does with its server, it does with this one.
//
// They sign in straight after signing up, so they run against a second
// server on the same database that confirms sign-ups at once, as Nuvio's
// does by default; at its own address, which discovery names; and from one
// address, so it trusts no proxy.

import { spawnSync } from "node:child_process";
import path from "node:path";
import { afterAll, beforeAll, expect, inject, it } from "vitest";
import { freePort, startServer } from "./support/globalSetup.ts";

// The add-on a new account starts with; only kept, never fetched.
const CATALOG = "https://catalog.example.invalid/manifest.json";

let server: Awaited<ReturnType<typeof startServer>> | undefined;

beforeAll(async () => {
  const { VELIO_TRUSTED_PROXY: _proxy, ...env } = inject("serverEnv");
  // Discovery answers the site's address as the backend's, which the tests check.
  const port = await freePort();
  server = await startServer(inject("serverBinary"), {
    ...env,
    VELIO_LISTEN: `127.0.0.1:${port}`,
    VELIO_SITE_URL: `http://127.0.0.1:${port}`,
    ENABLE_EMAIL_AUTOCONFIRM: "true",
    DEFAULT_CATALOG_URL: CATALOG,
  });
});

afterAll(() => server?.stop());

it("passes Nuvio's smoke tests", { timeout: 120_000 }, () => {
  if (!server) throw new Error("No server");
  const script = path.join(import.meta.dirname, "../smoke/run.sh");
  const smoke = spawnSync("sh", [script], {
    env: {
      PATH: process.env.PATH ?? "",
      SUPABASE_PUBLIC_URL: server.url,
      DATABASE_URL: inject("databaseUrl"),
      DEFAULT_CATALOG_URL: CATALOG,
    },
    encoding: "utf8",
    timeout: 110_000,
  });
  // On a failure, its output says which check, and why.
  expect(smoke.status, `${smoke.stdout}\n${smoke.stderr}`).toBe(0);
});
