// TV limits, which only the Rust server has (PLAN.md, "TV limits"): every
// account gets VELIO_TV_LIMIT, and the operator can give one another on /ops,
// up to VELIO_TV_LIMIT_MAX (here the defaults, 10 and 50). Run by
// vitest.server.config.ts only; the Worker keeps the default.

import { withList } from "@velio/sync-core";
import { describe, expect, it } from "vitest";
import { fresh, save, SITE, worker } from "./helpers.ts";
import { attribute, codeFor, find, list, newTv, signInTv, threeProfiles } from "./tv.ts";

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

const limits = async (accountId: string) => {
  const answer = await ops(`/accounts/${accountId}`);
  expect(answer.status).toBe(200);
  const { account } = await answer.json<{
    account: {
      devices: number;
      tv_limit: number | null;
      tv_limits: { default: number; max: number; allowed: number };
    };
  }>();
  return account;
};

const setLimit = (accountId: string, limit: unknown) => ops(`/accounts/${accountId}/tv-limit`, { limit });

describe("TV limits", () => {
  it("are the default until the operator gives the account another", async () => {
    const { browser } = await threeProfiles();
    expect(await limits(browser.accountId)).toMatchObject({
      devices: 0,
      tv_limit: null,
      tv_limits: { default: 10, max: 50, allowed: 10 },
    });
    const tv = await newTv();
    const found = await find(browser, codeFor(tv, "qr"));
    expect(attribute(await found.text(), "data-tv-limit")).toBe("10");
  });

  it("hold the TVs a browser can sign in, on the server", async () => {
    const { browser, ids } = await threeProfiles();
    expect((await setLimit(browser.accountId, 1)).status).toBe(200);
    expect(await limits(browser.accountId)).toMatchObject({ tv_limit: 1, tv_limits: { allowed: 1 } });

    expect((await signInTv(browser, await newTv("One"), [ids[0]])).status).toBe(200);
    // vault.js is told the limit; a browser that doesn't check it is still refused.
    const second = await newTv("Two");
    expect(attribute(await (await find(browser, codeFor(second, "qr"))).text(), "data-tv-limit")).toBe("1");
    const refused = await signInTv(browser, second, [ids[0]]);
    expect(refused.status).toBe(400);
    expect(await refused.text()).toContain("At most 1 TV.");

    expect((await setLimit(browser.accountId, 2)).status).toBe(200);
    expect((await signInTv(browser, await newTv("Two again"), [ids[0]])).status).toBe(200);
    expect((await limits(browser.accountId)).devices).toBe(2);
  });

  it("lowered below the TVs an account has, keep them and its changes, and stop new ones", async () => {
    const { browser, ids } = await threeProfiles();
    for (const name of ["One", "Two"])
      expect((await signInTv(browser, await newTv(name), [ids[0]])).status).toBe(200);
    expect((await setLimit(browser.accountId, 1)).status).toBe(200);
    await save(browser, state => withList(state, list("Another list"), [ids[0]]).state);
    expect((await signInTv(browser, await newTv("Three"), [ids[0]])).status).toBe(400);

    // Back to the default.
    const back = await setLimit(browser.accountId, null);
    expect(back.status).toBe(200);
    expect(await limits(browser.accountId)).toMatchObject({ tv_limit: null, tv_limits: { allowed: 10 } });
    expect((await signInTv(browser, await newTv("Three"), [ids[0]])).status).toBe(200);
  });

  it("can't be given past VELIO_TV_LIMIT_MAX, or as anything but a number of TVs", async () => {
    const { browser } = await threeProfiles();
    for (const limit of [0, 51, -1, 2.5, "12", true])
      expect({ limit, status: (await setLimit(browser.accountId, limit)).status }).toEqual({
        limit,
        status: 400,
      });
    expect((await ops(`/accounts/${browser.accountId}/tv-limit`, {})).status).toBe(400);
    expect((await setLimit("no-such-account", 5)).status).toBe(404);
    expect((await setLimit(browser.accountId, 50)).status).toBe(200);
    // Giving the default is the same as none, so a later VELIO_TV_LIMIT holds.
    expect((await setLimit(browser.accountId, 10)).status).toBe(200);
    expect((await limits(browser.accountId)).tv_limit).toBeNull();
  });

  it("need the operator", async () => {
    const answer = await worker.fetch(`${SITE}/ops/accounts/x/tv-limit`, {
      method: "POST",
      headers: { "cf-connecting-ip": fresh().ip, "Content-Type": "application/json" },
      body: JSON.stringify({ limit: 50 }),
    });
    expect(answer.status).toBe(401);
  });
});
