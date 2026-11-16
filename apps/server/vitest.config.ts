// The tests against the server (e2e/): globalSetup builds it and starts it
// on a free port with a new database, and each test talks to it over HTTP
// (smoke/run.sh, Nuvio's own tests, runs as one of them). The files share
// that database, and the clean-up tests move the clock days on, so they run
// one at a time.

import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    fileParallelism: false,
    // Every sign-in hashes a password.
    testTimeout: 20_000,
    include: ["e2e/*.test.ts"],
    globalSetup: ["./e2e/support/globalSetup.ts"],
  },
});
