-- The whole database (PLAN.md, "Moving off Cloudflare", "Data"): what was D1
-- (accounts, sessions, email links, TVs, pairings, notices), and what was each
-- account's Durable Object, now tables prefixed vault_ and keyed by account.
-- Times are milliseconds since 1970. Tokens are stored as their SHA-256, in hex.
-- Everything in a vault_ table is ciphertext, a public key or a hash.

CREATE TABLE accounts (
  id TEXT PRIMARY KEY,
  -- Trimmed and in lower case (normalizeEmail).
  email TEXT NOT NULL UNIQUE,
  confirmed_at INTEGER,
  -- Argon2id's salt (base64) and settings, sent back before sign-in.
  salt TEXT NOT NULL,
  kdf_memory_kib INTEGER NOT NULL,
  kdf_iterations INTEGER NOT NULL,
  kdf_parallelism INTEGER NOT NULL,
  -- SHA-256 of the auth key, in hex. The auth key is already high-entropy.
  auth_hash TEXT NOT NULL,
  terms_accepted_at INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  -- Closing an account (PLAN.md, "Legal and abuse"): when, and which notice or term.
  closed_at INTEGER,
  closed_reason TEXT,
  -- When a closed account's data was deleted. The row stays, with the email and
  -- the reason, so a closed account can't simply sign up again.
  purged_at INTEGER
);
CREATE INDEX accounts_unconfirmed ON accounts (created_at) WHERE confirmed_at IS NULL;
CREATE INDEX accounts_closed ON accounts (closed_at) WHERE closed_at IS NOT NULL AND purged_at IS NULL;

CREATE TABLE sessions (
  token_hash TEXT PRIMARY KEY,
  account_id TEXT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL,
  last_used_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL
);
CREATE INDEX sessions_account ON sessions (account_id);
CREATE INDEX sessions_expiry ON sessions (expires_at);

-- One-time links sent by email: confirming the address, and resetting the account.
CREATE TABLE email_links (
  token_hash TEXT PRIMARY KEY,
  account_id TEXT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK (kind IN ('confirm', 'reset')),
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  used_at INTEGER
);
CREATE INDEX email_links_account ON email_links (account_id, kind);
CREATE INDEX email_links_expiry ON email_links (expires_at);

-- Only written with DEV_MODE=1, in place of sending.
CREATE TABLE dev_outbox (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  sent_at INTEGER NOT NULL,
  to_address TEXT NOT NULL,
  subject TEXT NOT NULL,
  text TEXT NOT NULL
);

-- TVs signed in by QR code (PLAN.md, "Signing in a TV").
CREATE TABLE devices (
  -- The TV's id in the account's manifest.
  id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  -- As the TV named itself, such as "Samsung TV".
  name TEXT NOT NULL,
  -- The TV's box public key, base64.
  public_key TEXT NOT NULL,
  token_hash TEXT NOT NULL UNIQUE,
  created_at INTEGER NOT NULL,
  last_pulled_at INTEGER,
  -- A TV that signed itself out (POST /v1/signout). Its token is replaced, so
  -- it opens nothing, but it stays in the signed manifest until the owner
  -- removes it, as only the owner can sign a change.
  signed_out_at INTEGER
);
CREATE INDEX devices_by_account ON devices (account_id);

-- A TV waiting to be signed in. Lasts 10 minutes; the clean-up deletes old ones.
CREATE TABLE pairings (
  -- The first 4 characters of the code the TV shows.
  id TEXT PRIMARY KEY,
  -- The TV polls with this token; its SHA-256.
  poll_hash TEXT NOT NULL,
  -- SHA-256 of each proof that a phone has the QR or typed secret
  -- (sync-core pairingProof). Finding and claiming the TV need one.
  proof_qr_hash TEXT NOT NULL,
  proof_code_hash TEXT NOT NULL,
  -- The id the TV gets in the manifest, chosen by the server.
  device_id TEXT NOT NULL,
  public_key TEXT NOT NULL,
  name TEXT NOT NULL,
  -- The TV's key and name, sealed under each pairing secret (JSON).
  sealed_device TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  -- Set when the owner signs the TV in.
  account_id TEXT REFERENCES accounts (id) ON DELETE CASCADE,
  completed_at INTEGER,
  kind TEXT CHECK (kind IN ('qr', 'code')),
  -- The owner's public keys, sealed under the pairing secret (JSON), and the
  -- TV's token, until the TV picks them up.
  payload TEXT,
  device_token TEXT,
  delivered_at INTEGER
);
CREATE INDEX pairings_by_expiry ON pairings (expires_at);

-- Abuse and copyright notices (PLAN.md, "Legal and abuse").
CREATE TABLE notices (
  -- The reference shown to the sender, such as "N-7K2Q9XHM".
  id TEXT PRIMARY KEY,
  received_at INTEGER NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('copyright', 'abuse', 'other')),
  -- Who sent it.
  name TEXT NOT NULL,
  email TEXT NOT NULL,
  organization TEXT NOT NULL DEFAULT '',
  -- The Velio account it names, as given (trimmed, lower case), and the
  -- account that matched it then. No foreign key: notices outlive accounts.
  account_email TEXT NOT NULL DEFAULT '',
  account_id TEXT,
  -- Copyright: the work, and the sender's sworn statements and signature.
  work TEXT NOT NULL DEFAULT '',
  sworn INTEGER NOT NULL DEFAULT 0,
  signature TEXT NOT NULL DEFAULT '',
  details TEXT NOT NULL,
  -- What was done: open until handled through /ops.
  status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'actioned', 'rejected')),
  action TEXT NOT NULL DEFAULT '',
  handled_at INTEGER
);
CREATE INDEX notices_by_status ON notices (status, received_at);
CREATE INDEX notices_by_account ON notices (account_id);

-- --- Each account's vault: what was its Durable Object ---

CREATE TABLE vault_owner (
  account_id TEXT PRIMARY KEY REFERENCES accounts (id) ON DELETE CASCADE,
  -- The owner bundle, sealed under the unlock key from the password (JSON).
  bundle TEXT NOT NULL,
  sign_public_key TEXT NOT NULL,
  box_public_key TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE vault_recovery (
  account_id TEXT PRIMARY KEY REFERENCES accounts (id) ON DELETE CASCADE,
  -- SHA-256 of the recovery key's auth key, in hex.
  auth_hash TEXT NOT NULL,
  -- The owner bundle again, sealed under the recovery key's unlock key (JSON).
  bundle TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE vault_manifest (
  account_id TEXT PRIMARY KEY REFERENCES accounts (id) ON DELETE CASCADE,
  rev INTEGER NOT NULL,
  -- The signed manifest (JSON: body and signature).
  signed TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE vault_profiles (
  account_id TEXT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  id TEXT NOT NULL,
  rev INTEGER NOT NULL,
  -- The name and picture, sealed under the profile key (JSON).
  sealed TEXT NOT NULL,
  -- The profile key, wrapped under the account key (JSON).
  key TEXT NOT NULL,
  position INTEGER NOT NULL,
  PRIMARY KEY (account_id, id)
);

CREATE TABLE vault_lists (
  account_id TEXT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  id TEXT NOT NULL,
  rev INTEGER NOT NULL,
  key_rev INTEGER NOT NULL,
  -- The list, sealed under its list key (JSON).
  sealed TEXT NOT NULL,
  -- The list key, wrapped under the account key (JSON).
  key TEXT NOT NULL,
  position INTEGER NOT NULL,
  PRIMARY KEY (account_id, id)
);

CREATE TABLE vault_list_keys (
  account_id TEXT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  list_id TEXT NOT NULL,
  profile_id TEXT NOT NULL,
  -- The list key, wrapped under that profile's key (JSON).
  key TEXT NOT NULL,
  PRIMARY KEY (account_id, list_id, profile_id)
);

-- The TVs the manifest names. A TV removed from `devices` (signed out, or
-- forgotten after removal) can still be here until the owner signs a change.
CREATE TABLE vault_devices (
  account_id TEXT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  id TEXT NOT NULL,
  position INTEGER NOT NULL,
  PRIMARY KEY (account_id, id)
);

CREATE TABLE vault_device_keys (
  account_id TEXT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
  device_id TEXT NOT NULL,
  profile_id TEXT NOT NULL,
  -- The profile key, boxed from the owner's box key to the TV's (JSON).
  key TEXT NOT NULL,
  PRIMARY KEY (account_id, device_id, profile_id)
);
