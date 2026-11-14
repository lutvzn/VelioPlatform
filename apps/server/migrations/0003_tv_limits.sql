-- The TV limit the operator gave an account on /ops (PLAN.md, "TV limits").
-- NULL is VELIO_TV_LIMIT, the default.
ALTER TABLE accounts ADD COLUMN tv_limit INTEGER CHECK (tv_limit IS NULL OR tv_limit > 0);
