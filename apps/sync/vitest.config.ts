import path from "node:path";
import { cloudflareTest, readD1Migrations } from "@cloudflare/vitest-plugin";
import { defineConfig } from "vitest/config";

const migrations = await readD1Migrations(path.join(import.meta.dirname, "migrations"));

export default defineConfig({
  plugins: [
    cloudflareTest({
      wrangler: { configPath: "./wrangler.jsonc" },
      miniflare: {
        // DEV_MODE writes mail to dev_outbox and skips Turnstile.
        bindings: {
          DEV_MODE: "1",
          PRELOGIN_SECRET: "test-prelogin-secret",
          TURNSTILE_SITE_KEY: "",
          TURNSTILE_SECRET: "",
          RESEND_API_KEY: "",
          OPS_TOKEN: "test-ops-token",
          TEST_MIGRATIONS: migrations,
        },
      },
    }),
  ],
  test: { setupFiles: ["./test/setup.ts"] },
});
