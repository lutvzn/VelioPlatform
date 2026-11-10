-- Phase 3: TVs signed in by QR code (PLAN.md, "Signing in a TV", "Server", "Data").
-- Times are milliseconds since 1970. Tokens are stored as their SHA-256, in hex.

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
  last_pulled_at INTEGER
);
CREATE INDEX devices_by_account ON devices (account_id);

-- A TV waiting to be signed in. Lasts 10 minutes; the cron deletes old ones.
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
