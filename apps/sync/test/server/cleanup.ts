// Stands in for the Worker's `src/lib/cleanup.ts` (vitest.server.config.ts):
// runs `velio-server clean-up` on the tests' database.

import { execFileSync } from "node:child_process";
import { inject } from "vitest";

export const cleanUp = async (_env: unknown, now = Date.now()) => {
  const printed = execFileSync(inject("serverBinary"), ["clean-up", "--now", String(now)], {
    env: inject("serverEnv"),
    encoding: "utf8",
  });
  return Number(printed.trim());
};
