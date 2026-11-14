-- TVs' names are sealed under the account key, in the signed manifest, so
-- only the owner's browser reads them. D1 no longer keeps them.
ALTER TABLE devices DROP COLUMN name;
ALTER TABLE pairings DROP COLUMN name;
