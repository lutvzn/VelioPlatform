# Velio Platform

The server side of [VelioTV](../VelioTV): end-to-end encrypted sync of M3U and
Xtream lists across an owner's TVs, on [velio.tv](https://velio.tv). PLAN.md
is the design, with what each phase built and how it was checked.

## Layout

```
apps/server/           the Rust server (axum, maud, SQLite): what velio.tv runs, and anyone can self-host
  src/routes/          web pages, vault, pairing, settings, /v1 for TVs, notices, /ops
  src/views/           pages, written with maud: the Worker's markup
  src/sync_core/       the server's half of sync-core, checked against tests/vectors.json
  migrations/          the one SQLite database
Dockerfile             the distroless image
apps/sync/             the Cloudflare Worker: the web pages on velio.tv and /v1 for TVs
  src/worker.tsx       middleware (headers, body limits, sessions, same-origin posts) and routes
  src/account.ts       the Account Durable Object: one SQLite database per account
  src/routes/          web (sign-up, sign-in, resets), vault (lists, profiles), pair, settings, tv (/v1)
  src/views/           pages, written with hono/jsx
  src/client/          vault.js and app.css: the browser half, where all encryption happens
  migrations/          D1 (accounts, sessions, email links, devices, pairings, notices)
  test/                Worker tests, in the Workers runtime
packages/sync-core/    shared by the browser, the Worker and (later) VelioTV:
                       keys, sealing, signing, records, changes, pairing, pulls, export
```

## Working on it

Run everything on the host (not in VelioTV's distrobox). Node 22 or newer.

```sh
npx -y pnpm@12.14.0 install
cp apps/sync/.dev.vars.example apps/sync/.dev.vars   # DEV_MODE, PRELOGIN_SECRET, TURNSTILE_SECRET
npx -y pnpm@12.14.0 check                            # format, lint, types, all tests
```

Local server, at http://127.0.0.1:8787 (emails go to `/dev/outbox`):

```sh
cd apps/sync
npx wrangler d1 migrations apply DB --local
npm run build && npx wrangler dev --port 8787 --ip 127.0.0.1
```

## Running the server

The server ships as a distroless container image: one static binary, no
shell, running as a non-root user. It speaks plain HTTP on port 8787; put
the reverse proxy of your choice in front of it for HTTPS.

```sh
docker build -t velio-server .        # or podman build
docker run -d --name velio -p 127.0.0.1:8787:8787 -v velio-data:/data --env-file velio.env velio-server
```

`/data` holds the database (`velio.db`) and the nightly backups
(`backups/`). The image's health check runs `velio-server health`, which asks
`/healthz`. `docker exec velio /velio-server backup` makes a backup at once.

### Settings

Set in the environment (`--env-file`). The server refuses to start with a
missing or malformed one.

| Variable                                                             | Needed         | Meaning                                                                                                                                                       |
| -------------------------------------------------------------------- | -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `VELIO_SITE_URL`                                                     | yes            | The address people use, such as `https://velio.tv`                                                                                                            |
| `PRELOGIN_SECRET`                                                    | yes            | At least 32 characters: `openssl rand -base64 32`. Keep it: changing it changes the made-up salts unknown emails get                                          |
| `EMAIL_FROM`, `ABUSE_EMAIL`                                          | yes            | The sender of account emails, and where notices from `/abuse` go                                                                                              |
| `SMTP_URL` or `RESEND_API_KEY`                                       | one            | Such as `smtps://user:pass@mail.example:465`                                                                                                                  |
| `VELIO_TRUSTED_PROXY`                                                | behind a proxy | The proxy's addresses or ranges, comma-separated, such as `172.30.0.0/24`. Requests from them give the client's IP in `X-Forwarded-For` (the rightmost entry) |
| `OPS_TOKEN`                                                          | no             | At least 32 characters. Turns on `/ops`, the operator's page                                                                                                  |
| `TURNSTILE_SITE_KEY`, `TURNSTILE_SECRET`                             | no             | Both or neither. Turns on Cloudflare's Turnstile check on sign-up, resets and `/abuse`                                                                        |
| `VELIO_BACKUP_DIR`                                                   | no             | The image sets `/data/backups`. A copy of the database is made there each night at 04:17 UTC and kept 30 days                                                 |
| `VELIO_BACKUP_S3_URL`                                                | no             | S3-compatible storage for a second copy, path-style: `https://<endpoint>/<bucket>[/<prefix>]`                                                                 |
| `VELIO_BACKUP_S3_ACCESS_KEY_ID`, `VELIO_BACKUP_S3_SECRET_ACCESS_KEY` | with the URL   | A key that can put objects in that bucket                                                                                                                     |
| `VELIO_BACKUP_S3_REGION`                                             | no             | Default `auto` (Cloudflare R2); the bucket's region elsewhere                                                                                                 |
| `VELIO_NETWORK_PROVIDER`                                             | no             | Who carries traffic to the server and ends HTTPS, if not you, such as `Cloudflare`. Named on `/privacy` and `/security`                                       |
| `VELIO_TV_LIMIT`                                                     | no             | Default 10. How many TVs an account may have, unless it's given another limit on `/ops`                                                                       |
| `VELIO_TV_LIMIT_MAX`                                                 | no             | Default 50, the most a manifest can hold. The highest limit `/ops` can give an account                                                                        |
| `VELIO_LISTEN`, `VELIO_DATABASE`                                     | no             | The image sets `0.0.0.0:8787` and `/data/velio.db`                                                                                                            |
| `DEV_MODE`                                                           | no             | `1` for a local server: email goes to `/dev/outbox`, and Turnstile is skipped. Never in production                                                            |

`/privacy` and `/security` follow these settings: they name the network
provider, Resend or an email provider, Turnstile, and the backups as
configured.

What `/privacy` promises is yours to keep:

- **Logs** (one line per request, with the client's IP) must go after 7 days.
  With journald (`docker run --log-driver journald`, or Podman's default on
  systemd hosts), set `MaxRetentionSec=7d` in `/etc/systemd/journald.conf`.
- **S3 backups** must go after 30 days. Give the bucket a lifecycle rule that
  deletes objects 30 days after they're made. The server deletes only its
  local copies.

### Behind a reverse proxy

Any proxy works. It must:

- send `X-Forwarded-For`, and the server must trust it (`VELIO_TRUSTED_PROXY`);
- pass WebSocket upgrades (`/v1/events`);
- set, not append, `X-Velio-TLS-Version`, `X-Velio-TLS-Cipher` and
  `X-Velio-HTTP-Protocol`, if you want `/v1/probe` to report what the TV
  negotiated. Without them those fields are `null`.

Caddy:

```caddyfile
velio.example {
	reverse_proxy 127.0.0.1:8787 {
		header_up X-Velio-TLS-Version {http.request.tls.version}
		header_up X-Velio-TLS-Cipher {http.request.tls.cipher_suite}
		header_up X-Velio-HTTP-Protocol {http.request.proto}
	}
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
    env_file: velio.env # with VELIO_TRUSTED_PROXY=172.30.0.0/24 and VELIO_NETWORK_PROVIDER=Cloudflare
    volumes: [velio-data:/data]
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
```

In the dashboard:

1. **Tunnel**: a public hostname `velio.tv` to the service `http://velio:8787`.
2. **Rules, Transform Rules, Request Header**: for `velio.tv`, set dynamic
   `X-Velio-TLS-Version` to `cf.tls_version`, `X-Velio-TLS-Cipher` to
   `cf.tls_cipher`, and `X-Velio-HTTP-Protocol` to `http.request.version`.
3. Turnstile keys for `velio.tv`, and an R2 bucket for the backups with a
   30-day lifecycle rule, and an API token that can write to it.

The velio container trusts the whole subnet, which only it and
`cloudflared` are on, and publishes no port. Check a request's line in
`docker logs velio`: it should show your own IP, not `172.30.0.x`.

## Deploying the Worker (until R6 is done)

Only to velio.tv (no workers.dev, no subdomains). Apply new D1 migrations
first, then deploy:

```sh
cd apps/sync
npx wrangler d1 migrations list DB --remote     # then `apply` if any are listed
npm run build && npx wrangler deploy
```

Secrets are set with `npx wrangler secret put`: `PRELOGIN_SECRET`,
`TURNSTILE_SECRET`, `OPS_TOKEN` (the operator's token for `/ops`), and
optionally `RESEND_API_KEY`.

## Notices and closing accounts

Notices from `/abuse` are mailed to `abuse@velio.tv` and kept in D1. Handle
them at **https://velio.tv/ops**: sign in with the `OPS_TOKEN` secret (a
password manager can keep it under the user name `velio-operator`). It lists
the notices, shows the account each one names, and closes, opens and marks
them. **Accounts** lists every account, newest first, by state, and
narrowed by part of an email or an id (on the Rust server); it also sets an account's TV limit, from 1 to
`VELIO_TV_LIMIT_MAX`, or back to the default (`VELIO_TV_LIMIT`); a limit
below the TVs an account has keeps them and stops new ones. Closing needs a reason and the account's email typed again; velio.tv
checks the email too and changes nothing if it doesn't match. The session is
a cookie for `/ops` only, lasting 8 hours; **Sign out** ends it, and changing
`OPS_TOKEN` ends every session. Locally it is http://127.0.0.1:8787/ops, with
the token in `.dev.vars`.

The same from a terminal:

```sh
cd apps/sync
export VELIO_OPS_TOKEN=...                       # the OPS_TOKEN secret
node scripts/ops.mjs notices                     # open notices
node scripts/ops.mjs notice N-XXXXXXXX           # one notice, with the account it names
node scripts/ops.mjs close owner@example.com "Reason the owner reads" --notice N-XXXXXXXX
node scripts/ops.mjs handle N-XXXXXXXX rejected "What was done"
node scripts/ops.mjs reopen owner@example.com    # within 30 days
node scripts/ops.mjs tv-limit owner@example.com 20   # or "default"; the Rust server only
```

`close` shows the account and asks for its email to be typed (or takes
`--confirm <email>`). `VELIO_SITE=http://127.0.0.1:8787` points the tool at
`wrangler dev`.

A closed account's data is deleted by the daily cron 30 days
after closing.

## Security model, in short

- The password never leaves the browser. Argon2id and HKDF make a sign-in
  key (the server keeps its SHA-256) and an unlock key, which opens the
  owner's keys, kept on the server sealed.
- Lists and profiles are sealed with their own keys (TweetNaCl secretbox).
  The owner's browser signs the manifest (Ed25519) naming every record at its
  revision; browsers and TVs refuse anything that doesn't match it.
- TVs get their profile keys boxed to their own key. Pairing secrets travel
  in the QR code's fragment or are typed, never through the server.
- Removing a TV, or a profile from it, re-keys what it could read; the server
  refuses a change that doesn't.
- The owner's browser remembers the newest revision it opened and refuses
  older ones (rollback).
- The server checks what it can without keys: signatures, revisions, sizes,
  rate limits, same-origin posts, `__Host-` SameSite=Strict cookies, and a
  strict CSP on every page.

What the server can still do, and other accepted limits, are in PLAN.md
("Phase 7 results"). The owner-facing version is the page at `/security`.
