import { createHmac } from "node:crypto";
import { describe, expect, it } from "vitest";
import { toBase64, utf8 } from "../src/bytes.ts";
import {
  DEFAULT_KDF,
  deriveLoginKeys,
  deriveRecoveryKeys,
  deriveWith,
  formatRecoveryKey,
  hkdfSha256,
  kdfProblem,
  newRecoveryKey,
  newSalt,
  passwordProblem,
  readRecoveryKey,
} from "../src/login.ts";
import { newOwnerKeys, openOwnerBundle, ownerPublicKeys, sealOwnerBundle } from "../src/owner.ts";

const LIGHT = { memoryKiB: 64, iterations: 1, parallelism: 1 };
const hex = (bytes: Uint8Array) => Buffer.from(bytes).toString("hex");

describe("HKDF-SHA256", () => {
  it("matches RFC 5869's first test case, cut to 32 bytes", async () => {
    const ikm = new Uint8Array(22).fill(0x0b);
    const salt = Uint8Array.from(Buffer.from("000102030405060708090a0b0c", "hex"));
    const info = Uint8Array.from(Buffer.from("f0f1f2f3f4f5f6f7f8f9", "hex"));
    expect(hex(await hkdfSha256(ikm, salt, info))).toBe(
      "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf",
    );
  });

  it("agrees with Node's HMAC for the extract step", async () => {
    const ikm = utf8("input");
    const salt = utf8("salt");
    const prk = createHmac("sha256", salt).update(ikm).digest();
    const expected = createHmac("sha256", prk)
      .update(Buffer.from([...utf8("info"), 1]))
      .digest("hex");
    expect(hex(await hkdfSha256(ikm, salt, utf8("info")))).toBe(expected);
  });
});

describe("login keys", () => {
  const salt = Uint8Array.from({ length: 16 }, (_, i) => i);

  // Pinned, so every platform (and every later version of this code) derives the same keys.
  it("derive the same keys as before for the same password and salt", async () => {
    const keys = await deriveWith("correct horse battery staple", salt, LIGHT);
    expect(toBase64(keys.authKey)).toBe(PINNED.auth);
    expect(toBase64(keys.unlockKey)).toBe(PINNED.unlock);
  });

  it("give two different keys, and change with the password, salt or settings", async () => {
    const base = await deriveWith("a long enough password", salt, LIGHT);
    expect(hex(base.authKey)).not.toBe(hex(base.unlockKey));
    const others = await Promise.all([
      deriveWith("a long enough passworD", salt, LIGHT),
      deriveWith("a long enough password", newSalt(), LIGHT),
      deriveWith("a long enough password", salt, { ...LIGHT, iterations: 2 }),
    ]);
    for (const other of others) expect(hex(other.authKey)).not.toBe(hex(base.authKey));
  });

  it("treat the same password typed in different Unicode forms alike", async () => {
    const composed = await deriveWith("café au lait at noon", salt, LIGHT);
    const decomposed = await deriveWith("café au lait at noon", salt, LIGHT);
    expect(hex(composed.authKey)).toBe(hex(decomposed.authKey));
  });

  it("run with the default settings", async () => {
    const keys = await deriveLoginKeys("a long enough password", newSalt());
    expect(keys.authKey).toHaveLength(32);
    expect(keys.unlockKey).toHaveLength(32);
  });

  it("refuse weak settings and wrong salts", async () => {
    await expect(deriveLoginKeys("x", salt, LIGHT)).rejects.toThrow(/refused/);
    await expect(deriveLoginKeys("x", new Uint8Array(8))).rejects.toThrow(/16 bytes/);
  });
});

describe("checks", () => {
  it("accept the default settings, and nothing weaker or absurd", () => {
    expect(kdfProblem(DEFAULT_KDF)).toBeUndefined();
    expect(kdfProblem({ ...DEFAULT_KDF, memoryKiB: 32_768 })).toMatch(/memory/);
    expect(kdfProblem({ ...DEFAULT_KDF, iterations: 50 })).toMatch(/passes/);
    expect(kdfProblem({ ...DEFAULT_KDF, parallelism: 0 })).toMatch(/lanes/);
    expect(kdfProblem({ ...DEFAULT_KDF, memoryKiB: 70_000.5 })).toMatch(/memory/);
  });

  it("ask for 12 characters, counting letters rather than code units", () => {
    expect(passwordProblem("short")).toMatch(/12 characters/);
    expect(passwordProblem("twelve chars")).toBeUndefined();
    expect(passwordProblem("📺📺📺📺📺📺")).toMatch(/12 characters/);
  });
});

// Checked against argon2-cffi and Python's hmac (2026-11-10).
const PINNED = {
  auth: "BIj+CB8MeGh7ObwT+GKQQCJEYo61XBjieqr9V2LZsg0=",
  unlock: "xuKm1U6KoIswDXagjQgrr8QDr7FIoNcC9INluQ2yfk8=",
};

describe("recovery keys", () => {
  it("are 32 base32 characters, shown in groups of 4, and read back however they're typed", () => {
    const key = newRecoveryKey();
    expect(key).toMatch(/^[0-9A-HJKMNP-TV-Z]{32}$/);
    expect(newRecoveryKey()).not.toBe(key);
    const shown = formatRecoveryKey(key);
    expect(shown).toMatch(/^([0-9A-Z]{4}-){7}[0-9A-Z]{4}$/);
    expect(readRecoveryKey(shown)).toBe(key);
    expect(readRecoveryKey(` ${shown.toLowerCase().replaceAll("-", " ")} `)).toBe(key);
    expect(readRecoveryKey("ilo0".repeat(8))).toBe("1100".repeat(8));
    expect(readRecoveryKey(key.slice(1))).toBeNull();
    expect(readRecoveryKey(`${key.slice(1)}U`)).toBeNull();
  });

  it("make an auth key and an unlock key, apart from each other, that open a second owner bundle", async () => {
    const key = newRecoveryKey();
    const { authKey, unlockKey } = await deriveRecoveryKeys(key);
    expect(authKey).toHaveLength(32);
    expect(hex(authKey)).not.toBe(hex(unlockKey));
    expect(hex((await deriveRecoveryKeys(key)).unlockKey)).toBe(hex(unlockKey));
    const owner = newOwnerKeys();
    const sealed = sealOwnerBundle(unlockKey, owner);
    const opened = openOwnerBundle(unlockKey, sealed, ownerPublicKeys(owner));
    expect(toBase64(opened.accountKey)).toBe(toBase64(owner.accountKey));
    expect(() => openOwnerBundle(authKey, sealed)).toThrow(/wrong key/);
    await expect(deriveRecoveryKeys("not a key")).rejects.toThrow("Not a recovery key");
  });
});
