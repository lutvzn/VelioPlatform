-- What Velio adds to Nuvio's schema. It runs after Nuvio's migrations and
-- changes none of their tables or functions, so a Nuvio client sees the same
-- database it would on Nuvio's own server.
--
-- - public.velio_iptv_lists: M3U playlists and Xtream logins, which Nuvio has
--   no place for (Stremio add-ons are Nuvio's public.addons). The owner's
--   rows only, by row-level security, as Nuvio's own tables.
-- - velio_private: what only the server reads: abuse and copyright notices,
--   closed accounts, the operator's TV limits, and DEV_MODE's outbox.

-- --- IPTV lists ---

CREATE TABLE IF NOT EXISTS public.velio_iptv_lists (
    user_id uuid NOT NULL DEFAULT auth.uid() REFERENCES auth.users (id) ON DELETE CASCADE,
    -- Kept as the list's id in VelioTV, so its channel edits and favourites still match.
    id text NOT NULL CHECK (id ~ '^[A-Za-z0-9_-]{1,64}$'),
    name text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 100),
    kind text NOT NULL CHECK (kind IN ('m3u', 'xtream')),
    -- An M3U list's address.
    url text CHECK (char_length(url) <= 2048),
    -- An Xtream login.
    server text CHECK (char_length(server) <= 2048),
    username text CHECK (char_length(username) <= 256),
    password text CHECK (char_length(password) <= 256),
    guide_urls text[] NOT NULL DEFAULT '{}' CHECK (cardinality(guide_urls) <= 10),
    refresh_hours integer NOT NULL DEFAULT 24 CHECK (refresh_hours IN (6, 12, 24, 48, 168)),
    disabled boolean NOT NULL DEFAULT false,
    -- The profiles (Nuvio's profile_index) that use it; none means every profile.
    profile_ids integer[] NOT NULL DEFAULT '{}',
    sort_order integer NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, id),
    CHECK (
        (kind = 'm3u' AND url IS NOT NULL)
        OR (kind = 'xtream' AND server IS NOT NULL AND username IS NOT NULL AND password IS NOT NULL)
    )
);

ALTER TABLE public.velio_iptv_lists ENABLE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS "Owners use their IPTV lists" ON public.velio_iptv_lists;
-- The same rows as Nuvio's can_access_user_data(user_id), but auth.uid() is
-- worked out once a query (Supabase's advice for policies), so Postgres finds
-- the owner's rows by the primary key rather than calling a function on
-- every row of the table.
CREATE POLICY "Owners use their IPTV lists" ON public.velio_iptv_lists
    TO authenticated
    USING (user_id = (SELECT auth.uid()))
    WITH CHECK (user_id = (SELECT auth.uid()));
REVOKE ALL ON TABLE public.velio_iptv_lists FROM PUBLIC, anon;
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE public.velio_iptv_lists TO authenticated;
GRANT ALL ON TABLE public.velio_iptv_lists TO service_role;

DROP TRIGGER IF EXISTS velio_iptv_lists_updated_at ON public.velio_iptv_lists;
CREATE TRIGGER velio_iptv_lists_updated_at BEFORE UPDATE ON public.velio_iptv_lists
    FOR EACH ROW EXECUTE FUNCTION public.set_updated_at();

-- At most 50 lists an account, as before.
CREATE OR REPLACE FUNCTION public.velio_iptv_lists_limit() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, public AS $$
BEGIN
    IF (SELECT count(*) FROM public.velio_iptv_lists WHERE user_id = NEW.user_id) >= 50 THEN
        RAISE EXCEPTION 'An account has at most 50 lists' USING errcode = 'P0001';
    END IF;
    RETURN NEW;
END
$$;
REVOKE ALL ON FUNCTION public.velio_iptv_lists_limit() FROM PUBLIC, anon, authenticated;
DROP TRIGGER IF EXISTS velio_iptv_lists_limit ON public.velio_iptv_lists;
CREATE TRIGGER velio_iptv_lists_limit BEFORE INSERT ON public.velio_iptv_lists
    FOR EACH ROW EXECUTE FUNCTION public.velio_iptv_lists_limit();

-- A profile's lists, for a TV: those given to it, and those given to every profile.
CREATE OR REPLACE FUNCTION public.velio_pull_iptv_lists(p_profile_id integer DEFAULT 1)
    RETURNS SETOF public.velio_iptv_lists
    LANGUAGE sql STABLE SECURITY INVOKER SET search_path = pg_catalog, public AS $$
    SELECT *
    FROM public.velio_iptv_lists l
    WHERE l.user_id = public.get_sync_owner()
      AND (cardinality(l.profile_ids) = 0 OR p_profile_id = ANY (l.profile_ids))
    ORDER BY l.sort_order, l.created_at, l.id;
$$;
REVOKE ALL ON FUNCTION public.velio_pull_iptv_lists(integer) FROM PUBLIC, anon;
GRANT EXECUTE ON FUNCTION public.velio_pull_iptv_lists(integer) TO authenticated, service_role;

-- --- What only the server reads ---

CREATE SCHEMA IF NOT EXISTS velio_private;
REVOKE ALL ON SCHEMA velio_private FROM PUBLIC, anon, authenticated;

-- Velio's own migrations (0000_…, 0001_…), as nuvio_migrations.schema_migrations is Nuvio's.
CREATE TABLE IF NOT EXISTS velio_private.schema_migrations (
    version text PRIMARY KEY,
    applied_at timestamptz NOT NULL DEFAULT now()
);

-- Abuse and copyright notices (PLAN.md, "Legal and abuse"). Times are
-- milliseconds since 1970, as /ops shows them.
CREATE TABLE IF NOT EXISTS velio_private.notices (
    -- The reference shown to the sender, such as "N-7K2Q9XHM".
    id text PRIMARY KEY,
    received_at bigint NOT NULL,
    kind text NOT NULL CHECK (kind IN ('copyright', 'abuse', 'other')),
    name text NOT NULL,
    email text NOT NULL,
    organization text NOT NULL DEFAULT '',
    -- The account it names, as given, and the user that matched it then. No
    -- foreign key: notices outlive accounts.
    account_email text NOT NULL DEFAULT '',
    account_id uuid,
    work text NOT NULL DEFAULT '',
    sworn boolean NOT NULL DEFAULT false,
    signature text NOT NULL DEFAULT '',
    details text NOT NULL,
    status text NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'actioned', 'rejected')),
    action text NOT NULL DEFAULT '',
    handled_at bigint
);
CREATE INDEX IF NOT EXISTS notices_by_status ON velio_private.notices (status, received_at);
CREATE INDEX IF NOT EXISTS notices_by_account ON velio_private.notices (account_id);

-- Closed accounts. While closed, the user is banned (auth.users.banned_until),
-- so it can't sign in or refresh. After 30 days its data is deleted with the
-- user; this row stays, with the email and the reason, so the address can't
-- simply sign up again.
CREATE TABLE IF NOT EXISTS velio_private.closed_accounts (
    user_id uuid PRIMARY KEY,
    email text NOT NULL,
    closed_at bigint NOT NULL,
    reason text NOT NULL,
    notice_id text,
    purged_at bigint
);
CREATE INDEX IF NOT EXISTS closed_accounts_email ON velio_private.closed_accounts (lower(email));

-- TV limits the operator gave (PLAN.md, "TV limits"); none is VELIO_TV_LIMIT.
CREATE TABLE IF NOT EXISTS velio_private.tv_limits (
    user_id uuid PRIMARY KEY REFERENCES auth.users (id) ON DELETE CASCADE,
    tv_limit integer NOT NULL CHECK (tv_limit > 0)
);

-- The sessions TVs got by signing in with a code, which the TV limit counts.
CREATE TABLE IF NOT EXISTS velio_private.tv_sessions (
    session_id uuid PRIMARY KEY REFERENCES auth.sessions (id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES auth.users (id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS tv_sessions_user ON velio_private.tv_sessions (user_id);

-- Only written with DEV_MODE=1, in place of sending.
CREATE TABLE IF NOT EXISTS velio_private.dev_outbox (
    id bigserial PRIMARY KEY,
    sent_at bigint NOT NULL,
    to_address text NOT NULL,
    subject text NOT NULL,
    text text NOT NULL
);

-- The website's sessions: a cookie's token, stored as its SHA-256, for a
-- session of Supabase Auth's. Ending that session (signing out everywhere, a
-- new password, removing it from the devices page) ends this one too.
CREATE TABLE IF NOT EXISTS velio_private.web_sessions (
    token_hash bytea PRIMARY KEY,
    session_id uuid NOT NULL REFERENCES auth.sessions (id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES auth.users (id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now(),
    last_used_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL
);
CREATE INDEX IF NOT EXISTS web_sessions_user ON velio_private.web_sessions (user_id);
CREATE INDEX IF NOT EXISTS web_sessions_session ON velio_private.web_sessions (session_id);
