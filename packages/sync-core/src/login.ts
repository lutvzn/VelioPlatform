// Turning an email's password into keys (PLAN.md, "Keys"), on every platform
// that signs in with email and password: the web now, phone and desktop apps
// later. TVs never do this (they sign in by QR code), so it is its own entry
// point, `@velio/sync-core/login`, and VelioTV's build never pulls in
// hash-wasm.
//
// Argon2id(password, salt) → 32 bytes, then HKDF-SHA256 splits it in two:
// the auth key, which goes to the server, and the unlock key, which never
// leaves the device and opens the owner bundle.

import { argon2id, createHMAC, createSHA256 } from "hash-wasm";
import nacl from "tweetnacl";
import { utf8 } from "./bytes.ts";
import { DEFAULT_KDF, kdfProblem, SALT_BYTES, type KdfParams } from "./account.ts";
import { KEY_BYTES } from "./crypto.ts";

export * from "./account.ts";

const hmacSha256 = async (key: Uint8Array, data: Uint8Array) => {
  const hmac = await createHMAC(createSHA256(), key);
  hmac.init();
  hmac.update(data);
  return hmac.digest("binary");
};

/** HKDF-SHA256 (RFC 5869) for one 32-byte output. */
export const hkdfSha256 = async (ikm: Uint8Array, salt: Uint8Array, info: Uint8Array) => {
  const prk = await hmacSha256(salt, ikm);
  const block = new Uint8Array(info.length + 1);
  block.set(info);
  block[info.length] = 1;
  return hmacSha256(prk, block);
};

const HKDF_SALT = utf8("velio login v1");
const AUTH_INFO = utf8("auth key");
const UNLOCK_INFO = utf8("unlock key");

export interface LoginKeys {
  /** Sent to the server at sign-up and sign-in; it stores only its SHA-256. */
  authKey: Uint8Array;
  /** Opens the owner bundle; never leaves the device. */
  unlockKey: Uint8Array;
}

export const deriveLoginKeys = async (
  password: string,
  salt: Uint8Array,
  params: KdfParams = DEFAULT_KDF,
): Promise<LoginKeys> => {
  if (salt.length !== SALT_BYTES) throw new Error(`The salt must be ${SALT_BYTES} bytes`);
  const problem = kdfProblem(params);
  if (problem) throw new Error(`The account's Argon2id settings are refused: ${problem}`);
  return deriveWith(password, salt, params);
};

// Without the range check, so tests can use light settings.
export const deriveWith = async (
  password: string,
  salt: Uint8Array,
  params: KdfParams,
): Promise<LoginKeys> => {
  const master = await argon2id({
    // NFKC, so the same password typed on different keyboards gives the same keys.
    password: utf8(password.normalize("NFKC")),
    salt,
    memorySize: params.memoryKiB,
    iterations: params.iterations,
    parallelism: params.parallelism,
    hashLength: KEY_BYTES,
    outputType: "binary",
  });
  const [authKey, unlockKey] = await Promise.all([
    hkdfSha256(master, HKDF_SALT, AUTH_INFO),
    hkdfSha256(master, HKDF_SALT, UNLOCK_INFO),
  ]);
  return { authKey, unlockKey };
};

// --- The recovery key (PLAN.md, "Keys") ---
//
// Off by default. Turned on, the owner's device makes a random key, shows it
// once to print or write down, and sends the server a second copy of the
// owner bundle sealed under it, with the SHA-256 of its auth key. With the
// reset link and this key, a forgotten password sets a new one and keeps
// every list, profile and TV. The key is random, so HKDF alone splits it, as
// Argon2id's output is split above.

/** Crockford's base32: no I, L, O or U, so a key reads back the same. */
const RECOVERY_ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/** 32 characters of 5 bits each: 160 random bits. */
export const RECOVERY_KEY_LENGTH = 32;
const RECOVERY_PATTERN = /^[0-9A-HJKMNP-TV-Z]{32}$/;

/** A new recovery key, as 32 characters; 256 is a multiple of 32, so taking 5 bits of each byte isn't biased. */
export const newRecoveryKey = () => {
  const bytes = nacl.randomBytes(RECOVERY_KEY_LENGTH);
  let key = "";
  for (let i = 0; i < RECOVERY_KEY_LENGTH; i++) key += RECOVERY_ALPHABET.charAt(bytes[i] & 31);
  return key;
};

/** As shown and printed: 8 groups of 4, such as K7QM-2XHF-…. */
export const formatRecoveryKey = (key: string) => (key.match(/.{1,4}/g) ?? []).join("-");

/**
 * A typed recovery key in its 32-character form, or null: spaces and dashes
 * are left out, lower case is read as upper, and I, L and O as 1, 1 and 0.
 */
export const readRecoveryKey = (typed: string) => {
  const key = typed
    .toUpperCase()
    .replace(/[\s-]+/g, "")
    .replace(/[IL]/g, "1")
    .replace(/O/g, "0");
  return RECOVERY_PATTERN.test(key) ? key : null;
};

const RECOVERY_SALT = utf8("velio recovery v1");

/** The recovery key's auth key (the server stores its SHA-256) and unlock key (opens the recovery bundle). */
export const deriveRecoveryKeys = async (key: string): Promise<LoginKeys> => {
  if (!RECOVERY_PATTERN.test(key)) throw new Error("Not a recovery key");
  const [authKey, unlockKey] = await Promise.all([
    hkdfSha256(utf8(key), RECOVERY_SALT, AUTH_INFO),
    hkdfSha256(utf8(key), RECOVERY_SALT, UNLOCK_INFO),
  ]);
  return { authKey, unlockKey };
};
