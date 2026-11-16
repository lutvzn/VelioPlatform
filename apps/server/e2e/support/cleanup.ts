// Runs `velio-server clean-up` on the tests' database, as the daily job
// does, and answers how many unconfirmed accounts it deleted.

import { execFileSync } from "node:child_process";
import { inject } from "vitest";

export const cleanUp = (now = Date.now()) => {
  const printed = execFileSync(inject("serverBinary"), ["clean-up", "--now", String(now)], {
    env: inject("serverEnv"),
    encoding: "utf8",
  });
  return Number(printed.trim());
};
