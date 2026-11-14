// Writes the made-up account that GET /v1/probe/test-account serves
// (probe/test-account.json). It is made here, once, and the server
// embeds it (src/routes/probe.rs).
//
//   pnpm --filter @velio/server test-account [lists]

import { writeFileSync } from "node:fs";
import path from "node:path";
import { makeTestAccount } from "@velio/sync-core";

const lists = Number(process.argv[2] ?? 20);
const file = path.join(import.meta.dirname, "../probe/test-account.json");
writeFileSync(file, `${JSON.stringify(makeTestAccount(lists), null, 2)}\n`);
console.log(`Wrote a test account with ${lists} lists to ${file}`);
