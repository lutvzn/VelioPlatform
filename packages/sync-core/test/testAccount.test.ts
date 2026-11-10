import { describe, expect, it } from "vitest";
import { newSigningKeyPair, signJson } from "../src/crypto.ts";
import { makeTestAccount, openForDevice, testDeviceKeys } from "../src/testAccount.ts";

describe("the test account", () => {
  const account = makeTestAccount(20);

  it("opens on its TV with every profile and list", () => {
    const opened = openForDevice(account.pull, testDeviceKeys(account));
    expect(opened.profiles.map(profile => profile.name)).toEqual(account.expect.profiles);
    expect(opened.profiles.map(profile => profile.lists.length)).toEqual([20, 10, 4]);
    expect(opened.profiles[0].lists[0].name).toBe(account.expect.firstList);
    expect(opened.profiles[0].lists[1]).toMatchObject({ kind: "xtream", username: "user2" });
  });

  it("names only example.invalid hosts", () => {
    const opened = openForDevice(account.pull, testDeviceKeys(account));
    const hosts = opened.profiles[0].lists.map(list => new URL(list.url ?? list.server!).hostname);
    expect(hosts.every(host => host.endsWith(".example.invalid"))).toBe(true);
  });

  it("is refused with a manifest someone else signed", () => {
    const pull = { ...account.pull, manifest: signJson(newSigningKeyPair().secretKey, { rev: 99 }) };
    expect(() => openForDevice(pull, testDeviceKeys(account))).toThrow(/signature/);
  });

  it("is refused when a list's revision isn't the manifest's", () => {
    const lists = account.pull.lists.map((list, i) => (i === 0 ? { ...list, rev: 2 } : list));
    expect(() => openForDevice({ ...account.pull, lists }, testDeviceKeys(account))).toThrow(/revision/);
  });

  it("is refused when a list is missing", () => {
    const pull = { ...account.pull, lists: account.pull.lists.slice(1) };
    expect(() => openForDevice(pull, testDeviceKeys(account))).toThrow(/missing/);
  });
});
