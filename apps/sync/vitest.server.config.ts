// The Worker's tests, run against the Rust server in apps/server (PLAN.md,
// "Moving off Cloudflare", "Checking it matches"). `cloudflare:test` and
// `cloudflare:workers` are a stand-in that sends each request to the server
// (test/server/runtime.ts), so the test files stay as they are.
//
// Each phase adds the files it makes pass; `pending` names the tests that
// need a later phase. The files share one database, and the clean-up test
// moves the clock 8 days on, so they run one at a time.

import path from "node:path";
import { defineConfig } from "vitest/config";

const standIn = (file: string) => path.join(import.meta.dirname, "test/server", file);
const runtime = standIn("runtime.ts");

const files = [
  "test/probe.test.ts",
  "test/hardening.test.ts",
  "test/accounts.test.ts",
  "test/vault.test.ts",
  "test/pair.test.ts",
  "test/signout.test.ts",
  "test/settings.test.ts",
  "test/notices.test.ts",
  // The Rust server's own: not .test.ts files, so the Worker's run leaves them out.
  "test/tvLimits.rust.ts",
  "test/opsAccounts.rust.ts",
];

const pending: string[] = [];

const escape = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

export default defineConfig({
  resolve: {
    alias: [
      { find: "cloudflare:test", replacement: runtime },
      { find: "cloudflare:workers", replacement: runtime },
      { find: /^\.\.\/src\/account\.ts$/, replacement: standIn("account.ts") },
      { find: /^\.\.\/src\/lib\/cleanup\.ts$/, replacement: standIn("cleanup.ts") },
      { find: /^\.\.\/src\/lib\/closing\.ts$/, replacement: standIn("closing.ts") },
    ],
  },
  test: {
    fileParallelism: false,
    include: files,
    globalSetup: ["./test/server/globalSetup.ts"],
    testNamePattern: pending.length ? `^(?!.*(${pending.map(escape).join("|")}))` : undefined,
  },
});
