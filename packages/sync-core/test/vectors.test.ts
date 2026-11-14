import { readFileSync, writeFileSync } from "node:fs";
import { expect, it } from "vitest";
import { vectorCounts, vectorsText, VECTORS_FILE } from "../scripts/vectors.ts";

// The Rust server replays these (apps/server/tests/vectors.rs).
it("writes the vectors the Rust server replays", () => {
  if (process.env.VECTORS === "write") writeFileSync(VECTORS_FILE, vectorsText);
  let written = "";
  try {
    written = readFileSync(VECTORS_FILE, "utf8");
  } catch {
    // Missing, so different.
  }
  expect(vectorCounts.changes).toBeGreaterThan(100);
  expect(written === vectorsText, "apps/server/tests/vectors.json is out of date: run pnpm vectors").toBe(
    true,
  );
});
