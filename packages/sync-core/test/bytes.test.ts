import { describe, expect, it } from "vitest";
import { fromBase64, fromUtf8, toBase64, utf8 } from "../src/bytes.ts";

describe("base64", () => {
  it("matches the platform's encoding for every length up to 64", () => {
    for (let length = 0; length <= 64; length++) {
      const bytes = new Uint8Array(length).map((_, i) => (i * 37 + length) & 255);
      const expected = Buffer.from(bytes).toString("base64");
      expect(toBase64(bytes)).toBe(expected);
      expect([...fromBase64(expected)]).toEqual([...bytes]);
    }
  });

  it("reads base64 without padding", () => {
    expect([...fromBase64("AQI")]).toEqual([1, 2]);
  });

  it("refuses what isn't base64", () => {
    expect(() => fromBase64("A")).toThrow(/wrong length/);
    expect(() => fromBase64("AB$=")).toThrow(/bad character/);
    expect(() => fromBase64("ABé=")).toThrow(/bad character/);
  });
});

describe("utf8", () => {
  const samples = ["", "plain", "Café, naïve, Ærøskøbing", "Δοκιμή", "日本語", "emoji 📺🎬", "a\u0000b"];

  it("matches TextEncoder and TextDecoder", () => {
    for (const text of samples) {
      const bytes = utf8(text);
      expect([...bytes]).toEqual([...new TextEncoder().encode(text)]);
      expect(fromUtf8(bytes)).toBe(text);
    }
  });

  it("turns a lone surrogate into U+FFFD, as TextEncoder does", () => {
    expect(fromUtf8(utf8("a\ud800b"))).toBe("a�b");
    expect([...utf8("\udc00")]).toEqual([...new TextEncoder().encode("\udc00")]);
  });

  it("refuses bytes that aren't UTF-8", () => {
    for (const bad of [[0xff], [0xc0, 0x80], [0xe2, 0x82], [0xed, 0xa0, 0x80], [0xf4, 0x90, 0x80, 0x80]])
      expect(() => fromUtf8(new Uint8Array(bad))).toThrow(/Not UTF-8/);
  });
});
