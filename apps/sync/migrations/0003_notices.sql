-- Phase 8: abuse and copyright notices, and closed accounts' data deleted
-- after 30 days (PLAN.md, "Legal and abuse"). Times are milliseconds since 1970.

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

-- When a closed account's data was deleted. The row stays, with the email and
-- the reason, so a closed account can't simply sign up again.
ALTER TABLE accounts ADD COLUMN purged_at INTEGER;
CREATE INDEX accounts_closed ON accounts (closed_at) WHERE closed_at IS NOT NULL AND purged_at IS NULL;
