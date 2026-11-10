// Writes the made-up account that GET /v1/probe/test-account serves
// (src/probe/test-account.json). It is made here, once, rather than in the
// Worker, as signing and boxing it in plain JavaScript would take much of a
// free-plan request's 10 ms of CPU.
//
//   pnpm --filter @velio/sync test-account [lists]

import { writeFileSync } from "node:fs";
import path from "node:path";
import { makeTestAccount } from "@velio/sync-core";

const lists = Number(process.argv[2] ?? 20);
const file = path.join(import.meta.dirname, "../src/probe/test-account.json");
writeFileSync(file, `${JSON.stringify(makeTestAccount(lists), null, 2)}\n`);
console.log(`Wrote a test account with ${lists} lists to ${file}`);
