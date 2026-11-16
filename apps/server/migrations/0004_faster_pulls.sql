-- Nuvio's sync_pull_watch_progress and sync_pull_watch_progress_delta copy
-- the rows they answer with into a temporary table, count them into
-- variables nothing reads, and answer from the copy. Making and dropping a
-- table on every call writes to the catalog (pg_class, pg_type,
-- pg_attribute): it was most of the time of a progress pull, and its clean-up
-- stalled pulls by most of a second now and then.
--
-- These answer the same rows in the same order, with the same arguments,
-- checks and statement timeout, straight from the query. CREATE OR REPLACE
-- keeps the functions' owner and grants. If Nuvio's own versions change,
-- this file is to be brought in line with them.

CREATE OR REPLACE FUNCTION public.sync_pull_watch_progress(p_profile_id integer DEFAULT 1, p_since_last_watched bigint DEFAULT NULL::bigint, p_limit integer DEFAULT NULL::integer) RETURNS SETOF public.watch_progress
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path TO 'public'
    AS $$
declare
  -- Kept from Nuvio's: they fail on bad claims or headers just as there.
  v_uid uuid := (select auth.uid());
  v_owner uuid;
  v_headers jsonb := coalesce(nullif(current_setting('request.headers', true), '')::jsonb, '{}'::jsonb);
  v_limit integer;
begin
  set local statement_timeout = '30s';

  v_owner := public.get_sync_owner();

  v_limit := case
    when p_limit is null or p_limit <= 0 then null
    else p_limit
  end;

  if p_since_last_watched is null then
    return query
    select *
    from public.watch_progress
    where user_id = v_owner
      and profile_id = p_profile_id
    order by last_watched desc
    limit v_limit;
  else
    return query
    select *
    from public.watch_progress
    where user_id = v_owner
      and profile_id = p_profile_id
      and last_watched > p_since_last_watched
    order by last_watched desc
    limit v_limit;
  end if;
end;
$$;

CREATE OR REPLACE FUNCTION public.sync_pull_watch_progress_delta(p_profile_id integer DEFAULT 1, p_since_event_id bigint DEFAULT 0, p_limit integer DEFAULT 1000) RETURNS TABLE(event_id bigint, operation text, progress_key text, content_id text, content_type text, video_id text, season integer, episode integer, "position" bigint, duration bigint, last_watched bigint)
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path TO 'public'
    AS $$
declare
  v_user_id uuid := public.get_sync_owner();
  v_auth_uid uuid := auth.uid();
  v_headers jsonb := coalesce(nullif(current_setting('request.headers', true), '')::jsonb, '{}'::jsonb);
  v_limit integer := least(greatest(coalesce(p_limit, 1000), 1), 1000);
begin
  return query
  select
    e.event_id,
    e.operation,
    e.progress_key,
    e.content_id,
    e.content_type,
    e.video_id,
    e.season,
    e.episode,
    e.position,
    e.duration,
    e.last_watched
  from public.watch_progress_events e
  where e.user_id = v_user_id
    and e.profile_id = p_profile_id
    and e.event_id > greatest(coalesce(p_since_event_id, 0), 0)
  order by e.event_id asc
  limit v_limit;
end;
$$;
