// Builds what the browser loads into public/static, which the server embeds
// (src/assets.rs): site.js and the operator's ops.js from client/, HTMX at
// its pinned version, and the styles. Runs before `cargo build` (the
// Dockerfile, and the e2e tests).

import { build } from "esbuild";
import { copyFile, mkdir, rm } from "node:fs/promises";
import { createRequire } from "node:module";
import path from "node:path";

const root = path.join(import.meta.dirname, "..");
const out = path.join(root, "public/static");
const require = createRequire(import.meta.url);

await rm(out, { recursive: true, force: true });
await mkdir(out, { recursive: true });

const script = name =>
  build({
    entryPoints: [path.join(root, `client/${name}.ts`)],
    outfile: path.join(out, `${name}.js`),
    bundle: true,
    format: "iife",
    target: "es2020",
    minify: true,
    sourcemap: true,
    legalComments: "none",
    logLevel: "warning",
  });

// The owner's pages, and the operator's at /ops (src/views/ops.rs).
await script("site");
await script("ops");
await copyFile(path.join(root, "client/app.css"), path.join(out, "app.css"));
await copyFile(path.join(root, "client/ops.css"), path.join(out, "ops.css"));

const htmxDir = path.dirname(require.resolve("htmx.org/package.json"));
await copyFile(path.join(htmxDir, "dist/htmx.min.js"), path.join(out, "htmx.min.js"));
