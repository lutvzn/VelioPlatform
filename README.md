# Velio Platform

The server side of [VelioTV](../VelioTV): end-to-end encrypted sync of M3U and
Xtream lists across an owner's TVs, on [velio.tv](https://velio.tv). PLAN.md
is the design, with what each phase built and how it was checked.

## Layout

```
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

## Deploying

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
them. Closing needs a reason and the account's email typed again; velio.tv
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
