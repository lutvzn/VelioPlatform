import { describe, expect, it } from "vitest";
import { fromBase64, toBase64, utf8 } from "../src/bytes.ts";
import {
  boxKey,
  CryptoProblem,
  newBoxKeyPair,
  newKey,
  newSigningKeyPair,
  open,
  openBoxedKey,
  openJson,
  PAD_TO,
  seal,
  sealJson,
  signJson,
  unwrapKey,
  verifyJson,
  wrapKey,
} from "../src/crypto.ts";

// Changes one bit of a base64 value.
const flip = (text: string, at = 0) => {
  const bytes = fromBase64(text);
  bytes[at] ^= 1;
  return toBase64(bytes);
};

describe("seal and open", () => {
  it("round-trips, padded to a multiple of 1 KB", () => {
    const key = newKey();
    for (const text of ["", "short", "x".repeat(PAD_TO - 4), "x".repeat(PAD_TO - 3)]) {
      const sealed = seal(key, utf8(text));
      // 16 bytes of Poly1305 tag on top of the padded length.
      expect((fromBase64(sealed.box).length - 16) % PAD_TO).toBe(0);
      expect(new TextDecoder().decode(open(key, sealed))).toBe(text);
    }
  });

  it("gives the same text a different nonce and box each time", () => {
    const key = newKey();
    const a = sealJson(key, { a: 1 });
    const b = sealJson(key, { a: 1 });
    expect(a.nonce).not.toBe(b.nonce);
    expect(a.box).not.toBe(b.box);
  });

  it("refuses the wrong key and changed data", () => {
    const key = newKey();
    const sealed = sealJson(key, { secret: "value" });
    expect(openJson(key, sealed)).toEqual({ secret: "value" });
    expect(() => open(newKey(), sealed)).toThrow(/Couldn't decrypt/);
    expect(() => open(key, { ...sealed, box: flip(sealed.box, 20) })).toThrow(/Couldn't decrypt/);
    expect(() => open(key, { ...sealed, nonce: flip(sealed.nonce) })).toThrow(/Couldn't decrypt/);
  });

  it("refuses keys of the wrong size", () => {
    expect(() => seal(new Uint8Array(16), utf8("x"))).toThrow(/32 bytes/);
  });
});

describe("wrapping keys", () => {
  it("unwraps with the wrapping key only", () => {
    const outer = newKey();
    const inner = newKey();
    const wrapped = wrapKey(outer, inner);
    expect([...unwrapKey(outer, wrapped)]).toEqual([...inner]);
    expect(() => unwrapKey(newKey(), wrapped)).toThrow(/Couldn't decrypt/);
  });
});

describe("boxing keys", () => {
  it("opens only for the recipient, and only from the expected sender", () => {
    const owner = newBoxKeyPair();
    const tv = newBoxKeyPair();
    const key = newKey();
    const boxed = boxKey(tv.publicKey, owner.secretKey, key);
    expect([...openBoxedKey(owner.publicKey, tv.secretKey, boxed)]).toEqual([...key]);

    // A key boxed by someone else (such as a server) to the same TV is refused.
    const impostor = newBoxKeyPair();
    const forged = boxKey(tv.publicKey, impostor.secretKey, newKey());
    expect(() => openBoxedKey(owner.publicKey, tv.secretKey, forged)).toThrow(/expected sender/);
    expect(() => openBoxedKey(owner.publicKey, newBoxKeyPair().secretKey, boxed)).toThrow(/expected sender/);
  });
});

describe("signing", () => {
  it("verifies what the key signed, and nothing else", () => {
    const signer = newSigningKeyPair();
    const signed = signJson(signer.secretKey, { rev: 7, lists: ["a"] });
    expect(verifyJson(signer.publicKey, signed)).toEqual({ rev: 7, lists: ["a"] });
    expect(() => verifyJson(newSigningKeyPair().publicKey, signed)).toThrow(/signature/);
    expect(() => verifyJson(signer.publicKey, { ...signed, body: flip(signed.body, 3) })).toThrow(
      /signature/,
    );
  });
});

describe("malformed input from outside", () => {
  it("throws CryptoProblem, not TweetNaCl's own errors, for bad sizes and bad base64", () => {
    const key = newKey();
    const sealed = seal(key, utf8("hello"));
    const short = toBase64(fromBase64(sealed.nonce).subarray(0, 10));
    expect(() => open(key, { ...sealed, nonce: short })).toThrow(CryptoProblem);
    expect(() => open(key, { ...sealed, box: "not base64!" })).toThrow(CryptoProblem);

    const owner = newBoxKeyPair();
    const tv = newBoxKeyPair();
    const boxed = boxKey(tv.publicKey, owner.secretKey, newKey());
    expect(() => openBoxedKey(owner.publicKey, tv.secretKey, { ...boxed, nonce: short })).toThrow(
      CryptoProblem,
    );
    expect(() => openBoxedKey(owner.publicKey.subarray(1), tv.secretKey, boxed)).toThrow(CryptoProblem);

    const signer = newSigningKeyPair();
    const signed = signJson(signer.secretKey, { a: 1 });
    const cut = { ...signed, signature: toBase64(fromBase64(signed.signature).subarray(0, 63)) };
    expect(() => verifyJson(signer.publicKey, cut)).toThrow(CryptoProblem);
    expect(() => verifyJson(signer.publicKey.subarray(1), signed)).toThrow(CryptoProblem);
  });
});
