// Accounts and email links in D1 (migrations/0001_accounts.sql).

import {
  DEFAULT_KDF,
  fromBase64,
  isSealed,
  kdfProblem,
  SALT_BYTES,
  toBase64,
  type KdfParams,
} from "@velio/sync-core";
import type { OwnerRecord } from "../account.ts";
import { hmacSha256, newToken, sha256Hex } from "./tokens.ts";

export interface AccountRow {
  id: string;
  email: string;
  confirmed_at: number | null;
  salt: string;
  kdf_memory_kib: number;
  kdf_iterations: number;
  kdf_parallelism: number;
  auth_hash: string;
  terms_accepted_at: number;
  created_at: number;
  closed_at: number | null;
  closed_reason: string | null;
  purged_at: number | null;
}

export const HOUR_MS = 3_600_000;
export const CONFIRM_LINK_MS = 24 * HOUR_MS;
export const RESET_LINK_MS = HOUR_MS;
/** Unconfirmed accounts are deleted after this (the daily cron). */
export const UNCONFIRMED_MS = 7 * 24 * HOUR_MS;

export const findAccount = (db: D1Database, email: string) =>
  db.prepare("SELECT * FROM accounts WHERE email = ?").bind(email).first<AccountRow>();

export const findAccountById = (db: D1Database, id: string) =>
  db.prepare("SELECT * FROM accounts WHERE id = ?").bind(id).first<AccountRow>();

export const kdfOf = (account: AccountRow): KdfParams => ({
  memoryKiB: account.kdf_memory_kib,
  iterations: account.kdf_iterations,
  parallelism: account.kdf_parallelism,
});

/** What GET /prelogin answers: the account's salt and settings, or made-up ones that stay the same for that email. */
export const preloginFor = async (env: Env, email: string, account: AccountRow | null) => {
  if (account && account.closed_at === null) return { salt: account.salt, ...kdfOf(account) };
  if (!env.PRELOGIN_SECRET) throw new Error("PRELOGIN_SECRET isn't set");
  const salt = (await hmacSha256(env.PRELOGIN_SECRET, `prelogin salt\n${email}`)).subarray(0, SALT_BYTES);
  return { salt: toBase64(salt), ...DEFAULT_KDF };
};

const decodes = (value: string, length: number) => {
  try {
    return fromBase64(value).length === length;
  } catch {
    return false;
  }
};

/** The SHA-256 of an auth key sent as base64, or null if it isn't one. */
export const authHashOf = async (authKey: string) => {
  if (!decodes(authKey, 32)) return null;
  return sha256Hex(fromBase64(authKey));
};

/** An owner bundle sent as JSON, or null. It holds three keys, padded to 1 KB, plus the 16-byte tag. */
export const readOwnerBundle = (text: string) => {
  if (text.length > 8192) return null;
  let bundle: unknown;
  try {
    bundle = JSON.parse(text);
  } catch {
    return null;
  }
  return isSealed(bundle) && decodes(bundle.nonce, 24) ? bundle : null;
};

export interface Credentials {
  salt: string;
  kdf: KdfParams;
  authHash: string;
  owner: Omit<OwnerRecord, "updatedAt">;
}

const wholeNumber = (value: string) => (/^\d{1,9}$/.test(value) ? Number(value) : Number.NaN);

/**
 * Reads what `vault.js` puts in a sign-up or reset form in place of the
 * password: the salt, settings, auth key, owner bundle and public keys.
 */
export const readCredentials = async (field: (name: string) => string): Promise<Credentials | string> => {
  const salt = field("salt");
  if (!decodes(salt, SALT_BYTES)) return "The salt is missing or the wrong size.";
  const kdf = {
    memoryKiB: wholeNumber(field("kdf_memory_kib")),
    iterations: wholeNumber(field("kdf_iterations")),
    parallelism: wholeNumber(field("kdf_parallelism")),
  };
  const problem = kdfProblem(kdf);
  if (problem) return `The Argon2id settings are refused: ${problem}.`;
  const authHash = await authHashOf(field("auth_key"));
  if (!authHash) return "The auth key is missing or the wrong size.";
  const bundle = readOwnerBundle(field("bundle"));
  if (!bundle) return "The owner bundle is missing or not readable.";
  const signPublicKey = field("sign_public_key");
  const boxPublicKey = field("box_public_key");
  if (!decodes(signPublicKey, 32) || !decodes(boxPublicKey, 32)) return "The public keys are missing.";
  return { salt, kdf, authHash, owner: { bundle, signPublicKey, boxPublicKey } };
};

export type LinkKind = "confirm" | "reset";

/** A new one-time link for the account; earlier links of the same kind stop working. */
export const newEmailLink = async (db: D1Database, accountId: string, kind: LinkKind, lifetimeMs: number) => {
  const token = newToken();
  const now = Date.now();
  await db.batch([
    db.prepare("DELETE FROM email_links WHERE account_id = ? AND kind = ?").bind(accountId, kind),
    db
      .prepare(
        "INSERT INTO email_links (token_hash, account_id, kind, created_at, expires_at) VALUES (?, ?, ?, ?, ?)",
      )
      .bind(await sha256Hex(token), accountId, kind, now, now + lifetimeMs),
  ]);
  return token;
};

/** The account a link is for, if it is unused and unexpired, without using it. */
export const peekEmailLink = async (db: D1Database, token: string, kind: LinkKind) => {
  const row = await db
    .prepare(
      "SELECT account_id FROM email_links WHERE token_hash = ? AND kind = ? AND used_at IS NULL AND expires_at > ?",
    )
    .bind(await sha256Hex(token), kind, Date.now())
    .first<{ account_id: string }>();
  return row?.account_id ?? null;
};

/** Uses a link once: the account it was for, or null if it was used, expired or never sent. */
export const useEmailLink = async (db: D1Database, token: string, kind: LinkKind) => {
  const now = Date.now();
  const row = await db
    .prepare(
      `UPDATE email_links SET used_at = ? WHERE token_hash = ? AND kind = ? AND used_at IS NULL AND expires_at > ?
       RETURNING account_id`,
    )
    .bind(now, await sha256Hex(token), kind, now)
    .first<{ account_id: string }>();
  return row?.account_id ?? null;
};
