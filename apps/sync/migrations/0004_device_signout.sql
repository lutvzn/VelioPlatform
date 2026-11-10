-- A TV that signed itself out (POST /v1/signout). Its token is replaced, so
-- it opens nothing, but the TV stays in the account's signed manifest until
-- the owner removes it, as only the owner can sign a change.
ALTER TABLE devices ADD COLUMN signed_out_at INTEGER;
