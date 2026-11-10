import { describe, expect, it } from "vitest";
import { emailProblem, normalizeEmail } from "../src/account.ts";
import { newKey, sealJson } from "../src/crypto.ts";
import { newOwnerKeys, openOwnerBundle, ownerPublicKeys, sealOwnerBundle } from "../src/owner.ts";

describe("the owner bundle", () => {
  it("opens with the unlock key, giving back the same keys", () => {
    const unlock = newKey();
    const owner = newOwnerKeys();
    const opened = openOwnerBundle(unlock, sealOwnerBundle(unlock, owner), ownerPublicKeys(owner));
    expect([...opened.accountKey]).toEqual([...owner.accountKey]);
    expect(ownerPublicKeys(opened)).toEqual(ownerPublicKeys(owner));
  });

  it("doesn't open with another key", () => {
    const owner = newOwnerKeys();
    expect(() => openOwnerBundle(newKey(), sealOwnerBundle(newKey(), owner))).toThrow(/Couldn't decrypt/);
  });

  it("is refused when the server pairs it with other public keys", () => {
    const unlock = newKey();
    const sealed = sealOwnerBundle(unlock, newOwnerKeys());
    expect(() => openOwnerBundle(unlock, sealed, ownerPublicKeys(newOwnerKeys()))).toThrow(/doesn't match/);
  });

  it("is refused when it isn't one", () => {
    const unlock = newKey();
    expect(() => openOwnerBundle(unlock, sealJson(unlock, { format: 1 }))).toThrow(/no accountKey/);
    expect(() => openOwnerBundle(unlock, sealJson(unlock, [1]))).toThrow(/Not an owner bundle/);
  });
});

describe("emails", () => {
  it("are compared trimmed and in lower case", () => {
    expect(normalizeEmail("  Someone@Example.COM ")).toBe("someone@example.com");
  });

  it("are checked loosely", () => {
    expect(emailProblem("someone@example.com")).toBeUndefined();
    expect(emailProblem("")).toMatch(/Enter/);
    expect(emailProblem("someone")).toMatch(/doesn't look/);
    expect(emailProblem(`${"a".repeat(250)}@b.co`)).toMatch(/too long/);
  });
});
