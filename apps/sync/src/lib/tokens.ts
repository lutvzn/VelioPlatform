// Random tokens and their hashes, on the Worker's WebCrypto.

import { toBase64 } from "@velio/sync-core";

const encoder = new TextEncoder();

export const randomBytes = (length: number) => crypto.getRandomValues(new Uint8Array(length));

/** 32 random bytes as URL-safe base64, for links and cookies. */
export const newToken = () =>
  toBase64(randomBytes(32)).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");

const toHex = (buffer: ArrayBuffer) =>
  Array.from(new Uint8Array(buffer), byte => byte.toString(16).padStart(2, "0")).join("");

export const sha256Hex = async (data: string | Uint8Array) =>
  toHex(await crypto.subtle.digest("SHA-256", typeof data === "string" ? encoder.encode(data) : data));

/** Compares two hex digests in constant time. */
export const sameDigest = (a: string, b: string) => {
  const left = encoder.encode(a);
  const right = encoder.encode(b);
  return left.byteLength === right.byteLength && crypto.subtle.timingSafeEqual(left, right);
};

export const hmacSha256 = async (secret: string, message: string) => {
  const key = await crypto.subtle.importKey(
    "raw",
    encoder.encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  return new Uint8Array(await crypto.subtle.sign("HMAC", key, encoder.encode(message)));
};
