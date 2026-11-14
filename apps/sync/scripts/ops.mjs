#!/usr/bin/env node
// The operator's tool for notices and closing accounts (PLAN.md, "Legal and
// abuse"), over /ops. Needs VELIO_OPS_TOKEN (the OPS_TOKEN secret); talks to
// https://velio.tv, or VELIO_SITE (such as http://127.0.0.1:8787).
//
//   node scripts/ops.mjs notices [open|actioned|rejected|all]
//   node scripts/ops.mjs notice <id>
//   node scripts/ops.mjs handle <id> <actioned|rejected> "<what was done>"
//   node scripts/ops.mjs account <email or id>
//   node scripts/ops.mjs close <email or id> "<reason the owner reads>" [--notice <id>] [--confirm <email>]
//     (without --confirm, it shows the account and asks for its email to be typed)
//   node scripts/ops.mjs reopen <email or id>
//   node scripts/ops.mjs tv-limit <email or id> <number|default>   (the Rust server only)

import { createInterface } from "node:readline/promises";

const site = (process.env.VELIO_SITE ?? "https://velio.tv").replace(/\/$/, "");
const token = process.env.VELIO_OPS_TOKEN;
const [command, ...rest] = process.argv.slice(2);

const usage = () => {
  console.error(
    [
      "node scripts/ops.mjs notices [open|actioned|rejected|all]",
      "node scripts/ops.mjs notice <id>",
      'node scripts/ops.mjs handle <id> <actioned|rejected> "<what was done>"',
      "node scripts/ops.mjs account <email or id>",
      'node scripts/ops.mjs close <email or id> "<reason the owner reads>" [--notice <id>] [--confirm <email>]',
      "node scripts/ops.mjs reopen <email or id>",
      "node scripts/ops.mjs tv-limit <email or id> <number|default>",
    ].join("\n"),
  );
  process.exit(2);
};

const call = async (method, path, body) => {
  const response = await fetch(`${site}/ops${path}`, {
    method,
    headers: { Authorization: `Bearer ${token}`, ...(body ? { "Content-Type": "application/json" } : {}) },
    body: body ? JSON.stringify(body) : undefined,
  });
  const answer = await response.json().catch(() => ({ problem: `HTTP ${response.status}` }));
  if (!response.ok) {
    console.error(`${response.status}: ${answer.problem ?? JSON.stringify(answer)}`);
    process.exit(1);
  }
  return answer;
};

const when = time => (time ? new Date(time).toISOString().replace("T", " ").slice(0, 16) : "-");
const show = value =>
  console.log(
    JSON.stringify(value, (key, v) => (key.endsWith("_at") && typeof v === "number" ? when(v) : v), 2),
  );

/** An account id from an email or an id. */
const accountId = async key => (await call("GET", `/accounts/${encodeURIComponent(key)}`)).account.id;

if (!token) {
  console.error("Set VELIO_OPS_TOKEN to the OPS_TOKEN secret.");
  process.exit(2);
}

switch (command) {
  case "notices": {
    const { notices } = await call("GET", `/notices?status=${rest[0] ?? "open"}`);
    if (notices.length === 0) console.log("No notices.");
    for (const n of notices)
      console.log(
        `${n.id}  ${when(n.received_at)}  ${n.status.padEnd(8)}  ${n.kind.padEnd(9)}  ${n.email}  → ${n.account_email || "(no account named)"}${n.account_email && !n.account_id ? " (not found)" : ""}`,
      );
    break;
  }
  case "notice":
    if (!rest[0]) usage();
    show(await call("GET", `/notices/${encodeURIComponent(rest[0])}`));
    break;
  case "handle":
    if (rest.length < 3) usage();
    await call("POST", `/notices/${encodeURIComponent(rest[0])}`, { status: rest[1], action: rest[2] });
    console.log(`Notice ${rest[0]}: ${rest[1]}.`);
    break;
  case "account":
    if (!rest[0]) usage();
    show(await call("GET", `/accounts/${encodeURIComponent(rest[0])}`));
    break;
  case "close": {
    if (rest.length < 2) usage();
    const option = name => (rest.includes(name) ? rest[rest.indexOf(name) + 1] : undefined);
    const notice = option("--notice");
    const { account } = await call("GET", `/accounts/${encodeURIComponent(rest[0])}`);
    let confirm = option("--confirm");
    if (confirm === undefined) {
      console.log(
        `Closing ${account.email} (${account.id}), made ${when(account.created_at)}, ` +
          `${account.devices ?? 0} TV(s), named in ${account.notices.length} notice(s).\n` +
          "It is signed out everywhere now, the owner is emailed the reason, and its data is deleted in 30 days.",
      );
      const prompt = createInterface({ input: process.stdin, output: process.stdout });
      confirm = await prompt.question("Type the account's email to confirm: ");
      prompt.close();
    }
    // velio.tv checks it too, and changes nothing if it doesn't match.
    if (confirm.trim().toLowerCase() !== account.email.toLowerCase()) {
      console.error("That isn't the account's email. Nothing was changed.");
      process.exit(1);
    }
    const { deleteAt } = await call("POST", `/accounts/${account.id}/close`, {
      reason: rest[1],
      notice,
      confirm,
    });
    console.log(
      `Closed ${account.email}. Its data will be deleted on ${when(deleteAt)}; the owner was emailed the reason.`,
    );
    break;
  }
  case "reopen": {
    if (!rest[0]) usage();
    await call("POST", `/accounts/${await accountId(rest[0])}/reopen`);
    console.log(`Opened ${rest[0]} again; the owner was emailed.`);
    break;
  }
  case "tv-limit": {
    if (rest.length < 2 || !(rest[1] === "default" || /^\d+$/.test(rest[1]))) usage();
    const limit = rest[1] === "default" ? null : Number(rest[1]);
    const { account } = await call("POST", `/accounts/${await accountId(rest[0])}/tv-limit`, { limit });
    const { allowed } = account.tv_limits;
    console.log(
      `${account.email} can have ${allowed} TVs${account.tv_limit === null ? " (the default)" : ""}; it has ${account.devices}.`,
    );
    break;
  }
  default:
    usage();
}
