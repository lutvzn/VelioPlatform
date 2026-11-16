# Velio Platform

The server side of [VelioTV](../VelioTV), on [velio.tv](https://velio.tv):
accounts, profiles, add-ons and M3U and Xtream lists, which an owner's TVs
pull. It speaks Nuvio's API, so a Nuvio app can use it in place of Nuvio's
own server, and it runs on Nuvio's database schema, unchanged. PLAN.md is the
design, with what each phase built and how it was checked.

## What it serves

- `/.well-known/nuvio`: where a Nuvio app finds the backend and its key.
- `/auth/v1`: Supabase Auth (GoTrue): sign-up, password and refresh grants,
  anonymous guests, recovery, sessions and sign-out.
- `/rest/v1`: PostgREST's dialect over Nuvio's tables and RPCs, run as the
  caller's role, so Nuvio's row-level policies decide what each one sees.
- `/storage/v1`: Supabase Storage, for profile pictures, on the local disk.
- `/functions/v1`: Nuvio's edge functions, such as `tv-logins-exchange`.
- The website: sign-up, profiles, add-ons, lists, TVs, `/abuse` and `/ops`,
  written on the same tables and RPCs.

Velio's own additions live beside Nuvio's schema, never in it: IPTV lists
(`public.velio_iptv_lists`, read by `velio_pull_iptv_lists`), TV limits,
closed accounts and notices (`velio_private`).

## Layout

```
apps/server/           the server (Rust: axum, maud, tokio-postgres): what velio.tv runs, and anyone can self-host
  src/api/             Nuvio's API: auth, rest, storage, functions
  src/routes/          the website: accounts, profiles, lists, devices, TV approval, notices, /ops
  src/views/           pages, written with maud
  migrations/          Supabase's roles, Auth and Storage; nuvio/ (Nuvio's, unchanged); then Velio's
  client/              site.js, ops.js and the styles
  scripts/             build-client.mjs (builds public/static, which the binary embeds), ops.mjs,
                       supabase-migrations.mjs (brings the schema over from references/)
  e2e/                 tests over HTTP, against the server binary and a throwaway Postgres
  smoke/               Nuvio's own smoke tests, run against this server
references/            Nuvio's self-host, GoTrue and Storage, to check against
Dockerfile             the distroless image
```

## Working on it

Run everything on the host (not in VelioTV's distrobox). Node 22 or newer,
Rust (the version in the Dockerfile), and Postgres 17 or newer (`initdb` and
`postgres` on `PATH`, or in `PG_BIN`) for the tests.

```sh
npx -y pnpm@12.14.0 install
npx -y pnpm@12.14.0 check      # format, lint, types, the Rust checks, all tests
```

The tests make a Postgres cluster of their own in a temporary folder, or a
database on the server `VELIO_TEST_DATABASE_URL` names (as a superuser), and
delete it after. They include Nuvio's smoke tests (`smoke/run.sh`), which
can also be run against any server:

```sh
SUPABASE_PUBLIC_URL=http://localhost:8787 DATABASE_URL=postgres://... sh apps/server/smoke/run.sh
```

Local server, at http://localhost:8787 (emails go to `/dev/outbox`). Build
the client first: the binary embeds it, so `cargo` fails without it, and a
change to `client/` needs a new build and a new `cargo run`.

```sh
createdb velio
npx -y pnpm@12.14.0 --filter @velio/server build
cd apps/server
DEV_MODE=1 VELIO_SITE_URL=http://localhost:8787 DATABASE_URL=postgres:///velio JWT_SECRET=dev \
  OPS_TOKEN=dev-ops-token EMAIL_FROM=no-reply@velio.test ABUSE_EMAIL=abuse@velio.test cargo run
```

Profile pictures go to `apps/server/storage`. Use `localhost`, not
`127.0.0.1`: the `/ops` cookie is `Secure`, and over plain HTTP only
`localhost` keeps it.

### Benchmark

`examples/bench.rs` measures the release binary on a throwaway Postgres: it
signs up accounts, fills them as Nuvio's apps and VelioTV do, then runs each
scenario (discovery, a TV's pull, Nuvio's pulls and pushes, PostgREST reads
and upserts, `/auth/v1/user`, refreshes, password sign-ins, the home page)
at a fixed concurrency. It gives requests a second, p50/p90/p99, CPU per
1000 requests for the server and for Postgres, and RSS; the binary's and
database's sizes (empty, per account, after the run, the largest tables);
and Postgres's memory and the start time.

```sh
cd apps/server
cargo build --release
cargo run --release --example bench -- --baseline bench/baseline.json
cargo run --release --example bench -- --only pull_progress,refresh --statements
```

`--statements` adds what Postgres ran per request and the costliest
statements (pg_stat_statements), and `--postgres name=value` passes Postgres a
setting and keeps its log in `target/bench/postgres.log` (with
`session_preload_libraries=auto_explain` and
`auto_explain.log_min_duration=0`, every plan). The options are at the top of
the file. `bench/baseline.json` is a run at the defaults (100 accounts, 8 s,
32 in flight, on 4 cores) and `bench/before-optimizing.json` one from before
the work in `PLAN.md` ("Performance"). Compare runs on one machine, not
across machines.

## Running the server

The server ships as a distroless container image: one static binary, no
shell, running as a non-root user. It speaks plain HTTP on port 8787; put
the reverse proxy of your choice in front of it for HTTPS. It needs a
Postgres database, 17 or newer (Nuvio's schema is a Postgres 17 dump), which it migrates on start; the user it
connects as must be a superuser the first time, as Supabase's roles are made
then (`BYPASSRLS` for `service_role`).

```sh
docker build -t velio-server .        # or podman build
docker run -d --name velio -p 127.0.0.1:8787:8787 -v velio-data:/data --env-file velio.env velio-server
```

`/data` holds Storage's files (`storage/`); everything else is in Postgres,
so back it up with `pg_dump` or the host's own backups. The image's health
check runs `velio-server health`, which asks `/healthz`. `docker exec velio
/velio-server clean-up` runs the daily clean-up at once.

### Settings

Set in the environment (`--env-file`). The server refuses to start with a
missing or malformed one. The Auth settings take the names Nuvio's
self-host gives them.

| Variable                                          | Needed         | Meaning                                                                                                                                                       |
| ------------------------------------------------- | -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `VELIO_SITE_URL`                                  | yes            | The address people use, such as `https://velio.tv`. Also the backend `/.well-known/nuvio` gives apps                                                          |
| `DATABASE_URL`                                    | yes            | The Postgres database, such as `postgres://velio:...@db/velio`                                                                                                |
| `JWT_SECRET`                                      | yes            | At least 32 characters: `openssl rand -base64 48`. Signs access tokens and the API keys; changing it signs everyone out                                       |
| `EMAIL_FROM`, `ABUSE_EMAIL`                       | yes            | The sender of account emails, and where notices from `/abuse` go                                                                                              |
| `SMTP_URL` or `RESEND_API_KEY`                    | one            | Such as `smtps://user:pass@mail.example:465`                                                                                                                  |
| `ANON_KEY`, `SERVICE_ROLE_KEY`                    | no             | Tokens for the `anon` and `service_role` roles, signed with `JWT_SECRET`. Made from it when not set                                                           |
| `SUPABASE_PUBLISHABLE_KEY`, `SUPABASE_SECRET_KEY` | no             | The newer `sb_publishable_` and `sb_secret_` keys, standing for those two. Apps are given the publishable one when it's set                                   |
| `JWT_EXPIRY`                                      | no             | Default 3600. How long an access token lasts, in seconds                                                                                                      |
| `DISABLE_SIGNUP`                                  | no             | Default `false`. `true` stops new accounts                                                                                                                    |
| `ENABLE_ANONYMOUS_USERS`                          | no             | Default `true`. Nuvio's apps start as a guest                                                                                                                 |
| `ENABLE_EMAIL_AUTOCONFIRM`                        | no             | Default `true`, as Nuvio's. `false` makes accounts confirm their email before signing in                                                                      |
| `PASSWORD_MIN_LENGTH`                             | no             | The shortest password accepted                                                                                                                                |
| `DEFAULT_CATALOG_URL`                             | no             | The catalog add-on new profiles get. Default Nuvio's, `https://catalog.nuvio.tv/manifest.json`                                                                |
| `VELIO_TRUSTED_PROXY`                             | behind a proxy | The proxy's addresses or ranges, comma-separated, such as `172.30.0.0/24`. Requests from them give the client's IP in `X-Forwarded-For` (the rightmost entry) |
| `OPS_TOKEN`                                       | no             | At least 32 characters. Turns on `/ops`, the operator's page                                                                                                  |
| `TURNSTILE_SITE_KEY`, `TURNSTILE_SECRET`          | no             | Both or neither. Turns on Cloudflare's Turnstile check on sign-up, resets and `/abuse`                                                                        |
| `VELIO_NETWORK_PROVIDER`                          | no             | Who carries traffic to the server and ends HTTPS, if not you, such as `Cloudflare`. Named on `/privacy` and `/security`                                       |
| `VELIO_TV_LIMIT`                                  | no             | Default 10. How many TVs an account may have, unless it's given another limit on `/ops`                                                                       |
| `VELIO_TV_LIMIT_MAX`                              | no             | Default 50. The highest limit `/ops` can give an account                                                                                                      |
| `VELIO_LISTEN`, `VELIO_STORAGE_DIR`               | no             | The image sets `0.0.0.0:8787` and `/data/storage`                                                                                                             |
| `VELIO_DB_POOL`                                   | no             | Default 16. Connections to Postgres; each takes about 8 MB there. On a host of 1 or 2 cores, 8 does as well (README, "Benchmark")                             |
| `DEV_MODE`                                        | no             | `1` for a local server: email goes to `/dev/outbox`, and Turnstile is skipped. Never in production                                                            |

`/privacy` and `/security` follow these settings: they name the network
provider, Resend or an email provider, and Turnstile, as configured.

What `/privacy` promises is yours to keep:

- **Logs** (one line per request, with the client's IP) must go after 7 days.
  With journald (`docker run --log-driver journald`, or Podman's default on
  systemd hosts), set `MaxRetentionSec=7d` in `/etc/systemd/journald.conf`.
- **Database backups** must go after 30 days, so a closed account's data
  doesn't outlive its purge for long.

### Behind a reverse proxy

Any proxy works. It must send `X-Forwarded-For`, and the server must trust
it (`VELIO_TRUSTED_PROXY`). Caddy:

```caddyfile
velio.example {
	reverse_proxy 127.0.0.1:8787
}
```

With `VELIO_TRUSTED_PROXY=127.0.0.1` (or the bridge gateway, such as
`10.88.0.1` under Podman, when Caddy runs on the host and the port is
published).

### velio.tv: a Cloudflare Tunnel

velio.tv runs the image with `cloudflared` beside it, so no port is open on
the host, and HTTPS ends at Cloudflare's edge, as it did for the Worker.

```yaml
# compose.yaml
services:
  velio:
    image: velio-server
    env_file: velio.env # with VELIO_TRUSTED_PROXY=172.30.0.0/24, VELIO_NETWORK_PROVIDER=Cloudflare and DATABASE_URL=postgres://velio:...@db/velio
    volumes: [velio-data:/data]
    networks: [velio]
    depends_on: [db]
    restart: unless-stopped
  db:
    image: postgres:17
    environment: [POSTGRES_USER=velio, POSTGRES_PASSWORD, POSTGRES_DB=velio]
    volumes: [velio-db:/var/lib/postgresql/data]
    networks: [velio]
    restart: unless-stopped
  cloudflared:
    image: cloudflare/cloudflared
    command: tunnel --no-autoupdate run
    environment: [TUNNEL_TOKEN] # from the tunnel's page in the Zero Trust dashboard
    networks: [velio]
    restart: unless-stopped
networks:
  velio:
    ipam:
      config: [{ subnet: 172.30.0.0/24 }]
volumes:
  velio-data:
  velio-db:
```

In the dashboard:

1. **Tunnel**: a public hostname `velio.tv` to the service `http://velio:8787`.
2. Turnstile keys for `velio.tv`.

The velio container trusts the whole subnet, which only it, the database
and `cloudflared` are on, and publishes no port. Check a request's line in
`docker logs velio`: it should show your own IP, not `172.30.0.x`.

## Notices and closing accounts

Notices from `/abuse` are mailed to `ABUSE_EMAIL` and kept in the database. Handle
them at **https://velio.tv/ops**: sign in with `OPS_TOKEN` (a
password manager can keep it under the user name `velio-operator`). It lists
the notices, shows the account each one names, and closes, opens and marks
them. **Accounts** lists every account, newest first, by state, and
narrowed by part of an email or an id; it also sets an account's TV limit, from 1 to
`VELIO_TV_LIMIT_MAX`, or back to the default (`VELIO_TV_LIMIT`); a limit
below the TVs an account has keeps them and stops new ones. Closing needs a reason and the account's email typed again; velio.tv
checks the email too and changes nothing if it doesn't match. The session is
a cookie for `/ops` only, lasting 8 hours; **Sign out** ends it, and changing
`OPS_TOKEN` ends every session. Locally it is http://localhost:8787/ops.

The same from a terminal:

```sh
cd apps/server
export VELIO_OPS_TOKEN=...                       # OPS_TOKEN
node scripts/ops.mjs notices                     # open notices
node scripts/ops.mjs notice N-XXXXXXXX           # one notice, with the account it names
node scripts/ops.mjs close owner@example.com "Reason the owner reads" --notice N-XXXXXXXX
node scripts/ops.mjs handle N-XXXXXXXX rejected "What was done"
node scripts/ops.mjs reopen owner@example.com    # within 30 days
node scripts/ops.mjs tv-limit owner@example.com 20   # or "default"
```

`close` shows the account and asks for its email to be typed (or takes
`--confirm <email>`). `VELIO_SITE=http://localhost:8787` points the tool at
a local server.

A closed account's data is deleted by the nightly clean-up 30 days
after closing.

## Security model, in short

- Lists, profiles and add-ons are kept as written, not end-to-end
  encrypted: the server, and whoever runs it, can read them, as Nuvio's can.
  `/security` says so to owners.
- Every API call runs as the caller's Postgres role, `anon` or
  `authenticated` with the token's claims, so Nuvio's row-level policies
  decide what it reads and writes. The website writes as the signed-in
  user the same way.
- Passwords are hashed with bcrypt, as GoTrue does. Refresh tokens turn on
  each use, and one used twice ends its session.
- TVs sign in only by QR code, approved by a signed-in browser; an account's
  TVs are counted against its TV limit.
- The website checks same-origin posts, uses `__Host-` SameSite=Strict
  cookies, rate limits per IP, and sends a strict CSP on every page.

## Licence

AGPL-3.0 (`LICENSE`). Anyone who runs a changed copy of this server for
others must offer them its source. Parts come from Supabase and Nuvio under
MIT and Apache-2.0; `THIRD_PARTY_NOTICES.md` lists them.
