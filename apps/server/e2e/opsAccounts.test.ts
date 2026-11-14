// The list of accounts on /ops: GET /ops/accounts, newest first, by state,
// and narrowed by part of an email or an exact id.

import { describe, expect, it } from "vitest";
import { confirmedAccount, fresh, newCredentials, SITE, signedInAccount, signup, worker } from "./helpers.ts";

interface Row {
  id: string;
  email: string;
  confirmed_at: number | null;
  closed_at: number | null;
  devices: number;
  allowed: number;
  tv_limit: number | null;
  notice_count: number;
}

const ops = (path: string, body?: unknown) =>
  worker.fetch(`${SITE}/ops${path}`, {
    method: body === undefined ? "GET" : "POST",
    headers: {
      "cf-connecting-ip": fresh().ip,
      Authorization: "Bearer test-ops-token",
      ...(body === undefined ? {} : { "Content-Type": "application/json" }),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });

const listed = async (query: Record<string, string>) => {
  const answer = await ops(`/accounts?${new URLSearchParams(query).toString()}`);
  expect(answer.status).toBe(200);
  return answer.json<{ accounts: Row[]; total: number; tv_limits: { default: number; max: number } }>();
};

const emails = async (query: Record<string, string>) => (await listed(query)).accounts.map(row => row.email);

/** The states whose list has this account. */
const states = async (email: string) => {
  const found: string[] = [];
  for (const state of ["all", "open", "unconfirmed", "closed"])
    if ((await emails({ state, q: email })).includes(email)) found.push(state);
  return found;
};

describe("The operator's list of accounts", () => {
  it("lists accounts newest first, with their TVs and notices", async () => {
    const older = await signedInAccount();
    const newer = await signedInAccount();
    const prefix = older.email.split("-")[0] ?? "";
    const all = await emails({ q: `${prefix}-` });
    expect(all.indexOf(newer.email)).toBeGreaterThanOrEqual(0);
    expect(all.indexOf(newer.email)).toBeLessThan(all.indexOf(older.email));

    expect((await ops(`/accounts/${older.browser.accountId}/tv-limit`, { limit: 4 })).status).toBe(200);
    const list = await listed({ q: older.email });
    expect(list.total).toBe(1);
    expect(list.tv_limits).toEqual({ default: 10, max: 50 });
    expect(list.accounts[0]).toMatchObject({
      id: older.browser.accountId,
      email: older.email,
      devices: 0,
      tv_limit: 4,
      allowed: 4,
      notice_count: 0,
    });
  });

  it("finds an account by its id, or by part of its email, in any case", async () => {
    const { email, browser } = await signedInAccount();
    expect(await emails({ q: browser.accountId })).toEqual([email]);
    expect(await emails({ q: email.toUpperCase() })).toEqual([email]);
    expect(await emails({ q: `  ${email.slice(0, -4)}  ` })).toEqual([email]);
    // % and _ are only themselves.
    expect((await listed({ q: "%" })).total).toBe(0);
    expect((await listed({ q: "_" })).total).toBe(0);
  });

  it("shows accounts by state", async () => {
    const open = await confirmedAccount();
    const unconfirmed = fresh();
    expect((await signup({ ip: unconfirmed.ip }, unconfirmed.email, newCredentials())).status).toBe(200);
    const closed = await signedInAccount();
    const closing = await ops(`/accounts/${closed.browser.accountId}/close`, {
      reason: "Testing the list of accounts.",
      confirm: closed.email,
    });
    expect(closing.status).toBe(200);

    expect(await states(open.email)).toEqual(["all", "open"]);
    expect(await states(unconfirmed.email)).toEqual(["all", "unconfirmed"]);
    expect(await states(closed.email)).toEqual(["all", "closed"]);
  });

  it("refuses an unknown state, a search too long, and anyone but the operator", async () => {
    expect((await ops("/accounts?state=everything")).status).toBe(400);
    expect((await ops(`/accounts?q=${"a".repeat(321)}`)).status).toBe(400);
    const answer = await worker.fetch(`${SITE}/ops/accounts`, {
      headers: { "cf-connecting-ip": fresh().ip },
    });
    expect(answer.status).toBe(401);
  });
});
