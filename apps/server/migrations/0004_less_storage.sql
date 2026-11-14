-- Less storage. Each change reads back the same.
--
-- Ciphertext as its bytes (src/packed.rs) rather than base64 in JSON, about a
-- third smaller. velio_pack_* are the server's own functions (src/db.rs), so
-- this runs only there.
UPDATE vault_owner SET bundle = velio_pack_bundle(bundle);
UPDATE vault_recovery SET bundle = velio_pack_bundle(bundle);
UPDATE vault_manifest SET signed = velio_pack_signed(signed);
UPDATE vault_profiles SET sealed = velio_pack_sealed(sealed), key = velio_pack_sealed(key);
UPDATE vault_lists SET sealed = velio_pack_sealed(sealed), key = velio_pack_sealed(key);

-- SHA-256 hashes as their 32 bytes, not 64 hex characters. A TV that signed
-- out keeps 'signed-out:' and its id as text, which no hash can match; a
-- closed account's deleted auth hash is the empty blob.
UPDATE accounts SET auth_hash = unhex(auth_hash);
UPDATE vault_recovery SET auth_hash = unhex(auth_hash);
UPDATE sessions SET token_hash = unhex(token_hash);
UPDATE email_links SET token_hash = unhex(token_hash);
UPDATE devices SET token_hash = unhex(token_hash) WHERE token_hash NOT LIKE 'signed-out:%';
UPDATE pairings SET
  poll_hash = unhex(poll_hash),
  proof_qr_hash = unhex(proof_qr_hash),
  proof_code_hash = unhex(proof_code_hash);

-- The small tables keyed by account and ids, without a rowid: the key is
-- then stored once, in the table, rather than again in its own index. (The
-- tables with whole lists and profiles keep their rowid, as rows that big
-- would spill out of an index's pages.)
CREATE TABLE vault_list_keys_new (
  account_id TEXT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  list_id TEXT NOT NULL,
  profile_id TEXT NOT NULL,
  -- The list key, wrapped under that profile's key.
  key BLOB NOT NULL,
  PRIMARY KEY (account_id, list_id, profile_id)
) WITHOUT ROWID;
INSERT INTO vault_list_keys_new SELECT account_id, list_id, profile_id, velio_pack_sealed(key) FROM vault_list_keys;
DROP TABLE vault_list_keys;
ALTER TABLE vault_list_keys_new RENAME TO vault_list_keys;

-- The TVs the manifest names. A TV removed from `devices` (signed out, or
-- forgotten after removal) can still be here until the owner signs a change.
CREATE TABLE vault_devices_new (
  account_id TEXT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  id TEXT NOT NULL,
  position INTEGER NOT NULL,
  PRIMARY KEY (account_id, id)
) WITHOUT ROWID;
INSERT INTO vault_devices_new SELECT account_id, id, position FROM vault_devices;
DROP TABLE vault_devices;
ALTER TABLE vault_devices_new RENAME TO vault_devices;

CREATE TABLE vault_device_keys_new (
  account_id TEXT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  device_id TEXT NOT NULL,
  profile_id TEXT NOT NULL,
  -- The profile key, boxed from the owner's box key to the TV's.
  key BLOB NOT NULL,
  PRIMARY KEY (account_id, device_id, profile_id)
) WITHOUT ROWID;
INSERT INTO vault_device_keys_new
  SELECT account_id, device_id, profile_id, velio_pack_sealed(key) FROM vault_device_keys;
DROP TABLE vault_device_keys;
ALTER TABLE vault_device_keys_new RENAME TO vault_device_keys;
