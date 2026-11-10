-- Phase 1: accounts, sessions and email links (PLAN.md, "Server", "Data").
-- Times are milliseconds since 1970. Tokens are stored as their SHA-256, in hex.

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
  closed_reason TEXT
);
CREATE INDEX accounts_unconfirmed ON accounts (created_at) WHERE confirmed_at IS NULL;

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

-- Only written with DEV_MODE=1 (wrangler dev and tests), in place of sending.
CREATE TABLE dev_outbox (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  sent_at INTEGER NOT NULL,
  to_address TEXT NOT NULL,
  subject TEXT NOT NULL,
  text TEXT NOT NULL
);
