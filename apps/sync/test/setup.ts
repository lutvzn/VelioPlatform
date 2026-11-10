import { applyD1Migrations, env } from "cloudflare:test";

declare global {
  // oxlint-disable-next-line typescript/no-namespace -- Cloudflare.Env is declared as a namespace by workerd's types
  namespace Cloudflare {
    interface Env {
      TEST_MIGRATIONS: import("cloudflare:test").D1Migration[];
    }
  }
}

await applyD1Migrations(env.DB, env.TEST_MIGRATIONS);
