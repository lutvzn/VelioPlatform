// The tests against the server (e2e/): globalSetup builds it and starts it
// on a free port with a new database, and each test talks to it over HTTP
// (PLAN.md, "Moving off Cloudflare", "Checking it matches"). The files share
// that database, and the clean-up tests move the clock days on, so they run
// one at a time.

import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    fileParallelism: false,
    include: ["e2e/*.test.ts"],
    globalSetup: ["./e2e/support/globalSetup.ts"],
  },
});
