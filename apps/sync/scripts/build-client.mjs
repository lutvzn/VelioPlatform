// Builds what the browser loads into public/ (served by Workers static
// assets): vault.js and the operator's ops.js from src/client, HTMX at its pinned version, the styles,
// and _headers for them. Runs before `wrangler dev`, `wrangler deploy` and
// the tests.

import { build } from "esbuild";
import { copyFile, mkdir, rm, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import path from "node:path";

const root = path.join(import.meta.dirname, "..");
const out = path.join(root, "public");
const require = createRequire(import.meta.url);

await rm(out, { recursive: true, force: true });
await mkdir(path.join(out, "static"), { recursive: true });

await build({
  entryPoints: [path.join(root, "src/client/vault.ts")],
  outfile: path.join(out, "static/vault.js"),
  bundle: true,
  format: "iife",
  // Owner devices: browsers with WebAssembly, for Argon2id.
  target: "es2020",
  minify: true,
  sourcemap: true,
  legalComments: "none",
  logLevel: "warning",
});

// The operator's page at /ops (src/views/ops.tsx).
await build({
  entryPoints: [path.join(root, "src/client/ops.ts")],
  outfile: path.join(out, "static/ops.js"),
  bundle: true,
  format: "iife",
  target: "es2020",
  minify: true,
  sourcemap: true,
  legalComments: "none",
  logLevel: "warning",
});
await copyFile(path.join(root, "src/client/ops.css"), path.join(out, "static/ops.css"));

const htmxDir = path.dirname(require.resolve("htmx.org/package.json"));
await copyFile(path.join(htmxDir, "dist/htmx.min.js"), path.join(out, "static/htmx.min.js"));
await copyFile(path.join(root, "src/client/app.css"), path.join(out, "static/app.css"));

// Assets skip the Worker, so their headers are set here. The names have no
// hash, so browsers check for a new version each time (an ETag makes that
// cheap): a page from a new deploy never runs with an old vault.js.
await writeFile(
  path.join(out, "_headers"),
  [
    "/static/*",
    "  X-Content-Type-Options: nosniff",
    "  Referrer-Policy: no-referrer",
    "  Strict-Transport-Security: max-age=63072000; includeSubDomains",
    "  Cache-Control: no-cache",
    "  Cross-Origin-Resource-Policy: same-origin",
    "",
  ].join("\n"),
);
