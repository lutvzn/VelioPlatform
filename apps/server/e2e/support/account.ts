// An account's vault, read straight from the server's SQLite file
// (src/vault.rs), in the shapes the Worker's Durable Object answered with.

import { db } from "./runtime.ts";

const text = (value: unknown) => (typeof value === "string" ? value : "");
const base64 = (bytes: Uint8Array) => Buffer.from(bytes).toString("base64");

/**
 * Ciphertext as src/packed.rs stores it, read back as the JSON it was sent as:
 * a BLOB of 1, the nonce and the box, or of 2, the signature and the body;
 * or the JSON text itself.
 */
const json = (value: unknown): unknown => {
  if (!(value instanceof Uint8Array)) return JSON.parse(text(value));
  if (value[0] === 1) return { nonce: base64(value.subarray(1, 25)), box: base64(value.subarray(25)) };
  if (value[0] === 2) return { body: base64(value.subarray(65)), signature: base64(value.subarray(1, 65)) };
  throw new Error(`Not a packed value: ${Buffer.from(value).toString("hex")}`);
};

/** Every table that holds an account's vault, children first, as vault.rs deletes them. */
export const VAULT_TABLES = [
  "vault_device_keys",
  "vault_devices",
  "vault_list_keys",
  "vault_lists",
  "vault_profiles",
  "vault_manifest",
  "vault_recovery",
  "vault_owner",
];

/**
 * Every row of the account's vault, in every vault_* table, as text: hashes
 * in hex, and ciphertext as the JSON it was sent as.
 */
export const everythingStored = (accountId: string) => {
  const tables = db
    .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE 'vault\\_%' ESCAPE '\\'")
    .all()
    .map(row => String(row.name));
  return JSON.stringify(
    tables.map(table => db.prepare(`SELECT * FROM ${table} WHERE account_id = ?`).all(accountId)),
    (key, value: unknown) =>
      value instanceof Uint8Array
        ? key.endsWith("_hash")
          ? Buffer.from(value).toString("hex")
          : json(value)
        : value,
  );
};

/** The keys of each list or TV, in the order of their profiles. */
const keysBy = (table: string, column: string, accountId: string) => {
  const keys = new Map<string, { profile: string; key: unknown }[]>();
  const rows = db
    .prepare(
      `SELECT k.${column} AS owner, k.profile_id, k.key FROM ${table} k
       JOIN vault_profiles p ON p.account_id = k.account_id AND p.id = k.profile_id
       WHERE k.account_id = ? ORDER BY p.position`,
    )
    .all(accountId);
  for (const row of rows) {
    const owner = text(row.owner);
    keys.set(owner, [...(keys.get(owner) ?? []), { profile: text(row.profile_id), key: json(row.key) }]);
  }
  return keys;
};

export const accountStub = (_env: unknown, accountId: string) => ({
  accountId,

  async getOwner() {
    const row = db
      .prepare(
        "SELECT bundle, sign_public_key, box_public_key, updated_at FROM vault_owner WHERE account_id = ?",
      )
      .get(accountId);
    if (!row) return null;
    return {
      bundle: json(row.bundle),
      signPublicKey: text(row.sign_public_key),
      boxPublicKey: text(row.box_public_key),
      updatedAt: Number(row.updated_at),
    };
  },

  async getState() {
    const manifest = db.prepare("SELECT signed FROM vault_manifest WHERE account_id = ?").get(accountId);
    const listKeys = keysBy("vault_list_keys", "list_id", accountId);
    const deviceKeys = keysBy("vault_device_keys", "device_id", accountId);
    const rows = (table: string, columns: string) =>
      db.prepare(`SELECT ${columns} FROM ${table} WHERE account_id = ? ORDER BY position`).all(accountId);
    return {
      manifest: manifest ? json(manifest.signed) : null,
      profiles: rows("vault_profiles", "id, rev, sealed, key").map(row => ({
        id: text(row.id),
        rev: Number(row.rev),
        sealed: json(row.sealed),
        key: json(row.key),
      })),
      lists: rows("vault_lists", "id, rev, key_rev, sealed, key").map(row => ({
        id: text(row.id),
        rev: Number(row.rev),
        keyRev: Number(row.key_rev),
        sealed: json(row.sealed),
        key: json(row.key),
        profileKeys: listKeys.get(text(row.id)) ?? [],
      })),
      devices: rows("vault_devices", "id").map(row => ({
        id: text(row.id),
        keys: deviceKeys.get(text(row.id)) ?? [],
      })),
    };
  },

  async recoveryMadeAt() {
    const row = db.prepare("SELECT updated_at FROM vault_recovery WHERE account_id = ?").get(accountId);
    return row ? Number(row.updated_at) : null;
  },

  /**
   * As the Durable Object's deleteAll(), straight on the database: a TV with
   * a socket open hears nothing until its next pull, which the server's own
   * resets (vault.rs and Sockets::close_account) don't leave to chance.
   */
  async reset() {
    for (const table of VAULT_TABLES) db.prepare(`DELETE FROM ${table} WHERE account_id = ?`).run(accountId);
  },
});
