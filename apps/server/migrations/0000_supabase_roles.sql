-- What a Supabase database has before any service runs its migrations: the
-- roles PostgREST, Auth and Storage switch to, and the extensions schema.
-- On the supabase/postgres image all of this already exists, so every step
-- here is "if missing". Velio's server connects as a role that is a member of
-- anon, authenticated and service_role, and switches to one of them for each
-- request (as PostgREST does), so the row-level policies and grants in
-- Nuvio's schema decide what each request may touch.

DO $$
DECLARE
    r text;
BEGIN
    FOREACH r IN ARRAY ARRAY[
        'anon', 'authenticated', 'service_role', 'supabase_admin',
        'supabase_auth_admin', 'supabase_storage_admin', 'supabase_monitor',
        'authenticator', 'postgres'
    ] LOOP
        IF NOT EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = r) THEN
            EXECUTE format('CREATE ROLE %I NOLOGIN', r);
        END IF;
    END LOOP;
END
$$;

-- service_role bypasses row-level security, as on Supabase.
ALTER ROLE service_role BYPASSRLS;

-- The role the server logs in as must be able to SET ROLE to each of these.
DO $$
BEGIN
    IF NOT (SELECT rolsuper FROM pg_catalog.pg_roles WHERE rolname = current_user) THEN
        EXECUTE format('GRANT anon, authenticated, service_role TO %I', current_user);
    END IF;
    EXECUTE 'GRANT anon, authenticated, service_role TO authenticator';
END
$$;

CREATE SCHEMA IF NOT EXISTS extensions;
GRANT USAGE ON SCHEMA extensions TO anon, authenticated, service_role, postgres;
CREATE EXTENSION IF NOT EXISTS pgcrypto WITH SCHEMA extensions;
CREATE EXTENSION IF NOT EXISTS "uuid-ossp" WITH SCHEMA extensions;

CREATE SCHEMA IF NOT EXISTS auth;
GRANT USAGE ON SCHEMA auth TO anon, authenticated, service_role, postgres;

-- GoTrue's own record of its migrations; 0001_gotrue.sql fills it.
CREATE TABLE IF NOT EXISTS auth.schema_migrations (
    version character varying(14) NOT NULL PRIMARY KEY
);
