# Velio Sync plan: end-to-end encrypted sources for VelioTV

**One account holds the user's M3U and Xtream lists and their profiles. The owner edits them on the web, encrypted on their own device, and every VelioTV signed in to the account pulls the lists its profiles may use.** The server stores only ciphertext. It never sees a playlist URL, an Xtream server, a username or a password, and so can't be at fault for what they hold.

Typing an Xtream login with a TV remote is the slowest part of setting up VelioTV. With sync, the user types it once on a phone or computer, and each TV signs in by scanning a QR code.

Status: Phase 1 (accounts) built and deployed to velio.tv, 2026-11-10. Phase 2 (profiles and lists) built, tested and deployed, 2026-11-10. Phase 3's server half (signing in TVs, `/v1/sync`) built, tested and deployed, 2026-11-10. Phase 4's web half (profile pictures and PINs) and Phase 5's server half (live updates) built, tested and deployed, 2026-11-10. Phase 6's server and web half (the TVs page, the password, the recovery key, deleting the account) built, tested and deployed, 2026-11-10. Phase 7's server and web half (security review and fixes, headers, list export, docs) built, tested and deployed, 2026-11-10. Phase 8's server and web half (notices, closing accounts, the terms and privacy drafts) built, tested and deployed, 2026-11-10, with `POST /v1/signout` for TVs; `OPS_TOKEN` set; the lawyer's review and the DMCA agent are the user's. VelioTV's half is planned in `../VelioTV/SYNC-PLAN.md`. The VelioTV halves of Phases 3 to 6 wait until the server side of every phase is done (the user's choice, 2026-11-10). Phase 0 has two items left (see its results). **2026-11-13: the server moves off Cloudflare** to a self-hostable Rust server with SQLite ("Moving off Cloudflare"); the Worker keeps serving velio.tv until the Rust server passes the same tests.

## Decided by the user

- **Hosting:** ~~Cloudflare Workers~~ a **self-hostable Rust server with SQLite** (2026-11-13), the same design: lists stored encrypted, the server holding only ciphertext. See "Moving off Cloudflare". **Web UI:** HTMX 4.
- **End-to-end encrypted**, and **the login is the key**: no separate PIN or key to unlock the lists (2026-11-10). A PIN alone would be too few guesses to protect a copied database.
- **Account-wide**, with **profiles**: an account has several profiles, and the owner decides which profiles can use each list (2026-11-10).
- **Signing in:** TVs sign in with a QR code. Other platforms, now and later (the web, phone and desktop apps), sign in with **email and password** (2026-11-10).
- **What syncs:** M3U URLs, and Xtream servers with their logins.
- **Direction:** the owner changes the lists, and VelioTV pulls them.

Everything below is a recommendation, open to change.

## Stack

The Worker's stack, as built through Phase 8. The server half is being replaced; see "Moving off Cloudflare" for the new one. The browser half (HTMX 4, `vault.js`, TweetNaCl, Argon2id) and `sync-core` stay.

| Part                | Choice                                                                                                                                                   | Why                                                                                                                                        |
| ------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| Language            | TypeScript                                                                                                                                               | Same as VelioTV, so the formats and checks can be shared                                                                                   |
| Router and HTML     | [Hono](https://hono.dev) with `hono/jsx`, rendered on the server                                                                                         | Small, made for Workers, and its JSX returns the HTML fragments HTMX swaps in                                                              |
| Web UI              | HTMX 4, plain CSS, one small client script (`vault.js`)                                                                                                  | HTMX handles pages, forms and navigation; the script handles everything secret (see "HTMX and end-to-end encryption")                      |
| Global lookups      | Cloudflare D1                                                                                                                                            | Email to account, sessions, device tokens and pairing codes. KV can take up to 60 s to show a write elsewhere, too slow for a pairing code |
| Each account's data | One Durable Object per account, with SQLite storage                                                                                                      | One writer per account, so changes are strictly ordered, and the same object holds the TVs' WebSockets to tell them something changed      |
| Encryption          | [TweetNaCl.js](https://tweetnacl.js.org) everywhere; Argon2id where email and password are typed                                                         | See "Why TweetNaCl"                                                                                                                        |
| Email               | Cloudflare Email Service if the account has it, otherwise Resend                                                                                         | Confirming the address at sign-up, and the reset link                                                                                      |
| Abuse limits        | Workers Rate Limiting binding; Turnstile on sign-up                                                                                                      | Limits password guessing, pairing-code guessing and sign-up spam                                                                           |
| Tooling             | pnpm, oxfmt, oxlint, tsc, Vitest with `@cloudflare/vitest-plugin`                                                                                        | Same as VelioTV; the plugin runs tests inside the Workers runtime with D1 and Durable Objects                                              |
| Where it's built    | On the host, in this repo (2026-11-10)                                                                                                                   | The distroboxes are for the VelioTV client only                                                                                            |
| Deploy              | Wrangler, from Forgejo CI                                                                                                                                | Same CI as VelioTV                                                                                                                         |
| Domain              | `velio.tv` itself, a custom domain on the Worker: the site, sign-in, the web UI, QR sign-in and `/v1` share one address, with no subdomains (2026-11-10) | Short enough to type when a phone can't scan; `workers.dev` addresses are long                                                             |

### Why TweetNaCl

The oldest TVs VelioTV supports run Chromium 47 (Tizen 3) and Chromium 38 (webOS 3):

- WebCrypto has no X25519 or Ed25519 before Chromium 133, and `crypto.subtle` may not be there at all in a packaged TV app's origin.
- `@noble/curves` and similar need `BigInt`, which arrived in Chromium 67.
- TweetNaCl is plain ES5 with no `BigInt`, was audited by Cure53, and only needs `crypto.getRandomValues`, which the TV's probes already use. It has everything needed: `secretbox`, `box` and `sign`.

Every platform uses the same library and the same `sync-core` code, so lists encrypted on one decrypt on all the others.

### Why TVs sign in only by QR code

Email and password turn into keys through Argon2id with 64 MiB of memory. TVs before Chromium 57 have no WebAssembly, and Argon2id in plain JavaScript at that size would take far too long and too much memory on a TV. Typing an email and a password with a remote is also what sync is meant to save. So TVs never see the password: they get their keys from a signed-in phone or computer.

## Accounts, profiles and lists

- **Account:** an email and password. Whoever signs in with them is the **owner**: they see every profile and every list, and can change everything.
- **Profiles:** such as "Living room", "Kids" or "Alex". Each has a name, a picture (one of 8 colours, with the name's first letter) and, optionally, a **profile PIN** of four digits. Every platform asks which profile to use, and skips the question when the device has only one.
- **Lists:** an M3U playlist or an Xtream login, with its guide URLs and refresh time (VelioTV's `Source`). Each list has a set of profiles that can use it.
- **Devices:** a TV signed in by QR code gets **only the profiles the owner chose** for it when it signed in, and so can only decrypt those profiles' lists. A device signed in with email and password is the owner's, with everything.

The profile PIN stops someone at a shared TV from switching profile (like VelioTV's parental PIN). It doesn't encrypt anything, as the TV holds the keys of every profile it was given. **What a device can decrypt is what limits access:** a friend's TV given only the "Friend" profile never receives the keys of the other lists.

## Encryption

```
 email + password ─Argon2id─► unlock key ─opens─► owner bundle: account key, signing key, box key
 account key ─opens─► every profile key and every list key
 profile key ─opens─► the keys of the lists that profile can use
 list key ─opens─► one list
 TV device key ─opens─► the profile keys given to that TV
```

### Keys

| Key                                         | Made                        | Kept                                                                               | Opens or proves                                     |
| ------------------------------------------- | --------------------------- | ---------------------------------------------------------------------------------- | --------------------------------------------------- |
| **Unlock key**                              | Argon2id(password, salt)    | Nowhere; derived at each sign-in                                                   | The owner bundle                                    |
| **Account key** (32 bytes)                  | At sign-up, by the owner    | In the owner bundle                                                                | Every profile key and list key                      |
| **Signing key pair** (Ed25519)              | At sign-up                  | Public half on the server and on each TV; secret half in the owner bundle          | Signs the manifest (see below)                      |
| **Box key pair** (X25519)                   | At sign-up                  | Public half on the server and on each TV; secret half in the owner bundle          | Sends profile keys to TVs, from the owner           |
| **Profile key** (32 bytes), one per profile | When the profile is made    | Encrypted under the account key, and for each TV given that profile                | The profile's name and picture, and its lists' keys |
| **List key** (32 bytes), one per list       | When the list is made       | Encrypted under the account key, and under the key of each profile that can use it | The list                                            |
| **Device key pair** (X25519), one per TV    | On the TV, when it signs in | Secret half in the TV's `localStorage`; public half on the server                  | The TV's copy of its profile keys                   |

- **Signing in with email and password:** HKDF splits the Argon2id output into an _auth key_ and the _unlock key_. Only the auth key goes to the server, which stores its SHA-256 (the auth key is already high-entropy, so a slow hash isn't needed on the Worker). The server can't open the owner bundle with it.
- **Salt:** the app first asks for the account's salt and Argon2id settings by email. For an email with no account, the server answers with a salt derived from the email and a server secret, so the answer doesn't show whether an account exists.
- **Argon2id settings:** 64 MiB, 3 passes, 1 lane, about a second on a phone. Stored per account, so they can be raised later, and the same on every platform.
- **Passwords:** at least 12 characters; a passphrase is suggested. If the database ever leaked, the password's strength is all that protects the lists.
- **Changing the password** (signed in) re-encrypts the owner bundle only. Lists, profiles and TVs are untouched.
- **Forgotten password:** without a recovery key, the email's reset link can only **reset the account**. It keeps the email and deletes every profile and list, and every TV must sign in again. Nobody, including you, can get the old lists back. The sign-up page says so.
- **Recovery key:** off by default. An owner can turn one on to print or save; it holds a second copy of the owner bundle, so a forgotten password doesn't lose the lists. It is 160 random bits, shown as 32 characters of Crockford's base32 in groups of 4; HKDF splits it into an auth key (the server keeps its SHA-256) and an unlock key, as Argon2id's output is split. Using it needs both the reset link and the key.

### Lists and the manifest

Each list is its own encrypted record:

```ts
/** Encrypted with nacl.secretbox under its list key. VelioTV's Source (src/data/sources.ts). */
interface SyncedList {
  format: 1;
  id: string; // kept, so VelioTV's channel edits and favourites still match
  rev: number; // goes up by one on each change to this list
  name: string;
  kind: "m3u" | "xtream" | "stremio"; // "stremio": a Stremio add-on, `url` its manifest (no guide)
  url?: string;
  server?: string;
  username?: string;
  password?: string;
  guideUrls?: string[];
  refreshHours: number;
  disabled?: boolean;
}

/** Signed by the owner's signing key; readable by the server, as it holds no secrets. */
interface Manifest {
  format: 1;
  account: string;
  rev: number; // goes up by one on any change in the account
  profiles: { id: string; rev: number; lists: string[] }[]; // each profile's record revision, and its lists
  lists: { id: string; rev: number; keyRev: number }[]; // every list, used by a profile or not
  devices: { id: string; profiles: string[]; publicKey?: string; name?: Sealed }[]; // name: sealed under the account key
}
```

- Each record is encrypted with `nacl.secretbox` (XSalsa20-Poly1305) and a fresh 24-byte nonce, and **padded** to a multiple of 1 KB, so its size doesn't give away the URL's length.
- **The manifest says which profile has which list, at which revision.** Only an owner device can sign it, so the server can't give a profile a list, take one away, or serve an old version; the TV checks the signature, and refuses a manifest older than the last one it applied, or a list whose `rev` doesn't match.
- **What the server can see:** how many profiles and lists there are, which profile has which list, and when they change. Not what they're called or what's in them.
- **Two owners saving at once** (two tabs, or the web and a phone): a change sends the manifest `rev` it started from. If the account has moved on, the server answers 409, and the app reloads and asks to save again.

### Changing who can use what

All of this happens on the owner's device, which then signs a new manifest:

- **Giving a profile a list:** encrypt the list key under the profile key.
- **Taking a list from a profile:** make a new list key, re-encrypt the list, and encrypt the new key for the profiles that keep it. TVs with that profile may have kept the old key, but it opens no new version.
- **Deleting a profile:** the same for each of its lists.
- **Giving a TV another profile:** send it that profile key, boxed from the owner's box key.
- **Taking a profile from a TV, or removing the TV:** delete what the TV was sent, and make new keys for each profile it had and for each list those profiles could use. The remaining TVs get the new profile keys on their next pull; each checks they came from the owner's box key, so the server can't slip in a key of its own.

Lists are a few KB each, so even removing a TV re-encrypts everything in well under a second.

### Limits per account

Each account has at most **10 TVs** (devices signed in by QR code), **8 profiles** and **50 lists**. The server enforces these, because it can count records without reading them: the Durable Object refuses a change that would go over, including the one that signs in an eleventh TV. See "Legal and abuse" for why. Since 2026-11-14 the Rust server lets the operator give an account more or fewer TVs ("TV limits", under "Moving off Cloudflare").

## Signing in a TV

1. VelioTV's first start (or Settings → Account) shows **Sign in with a phone**. The TV makes its device key pair and two **pairing secrets**: 128 bits for the QR code, and 40 bits (8 characters) to type. It sends `POST /v1/pair` with its public key, and its public key and name sealed under each secret, and a proof of each secret (a hash the server can check but not turn into the key), and gets a 4-character pairing ID and a poll token back.
2. The TV shows a QR code for `https://velio.tv/pair#<id>.<secret>`, and a code to type, such as `K7QM-2XHF-9WRT`.
3. On the phone, the page opens; if not signed in, it asks for email and password first, then carries on. The fragment after `#` never reaches the server, so **the server never sees the pairing secret.** `vault.js` keeps it in `sessionStorage` across the sign-in and takes it out of the address bar and history.
4. The page finds the TV (`POST /pair/find`) with the ID and the proof, so another account can't fetch a TV's sealed details by guessing IDs. It opens the TV's name and key with the secret.
5. The page shows the TV's name and the account's profiles, all ticked, and the owner unticks any this TV shouldn't have.
6. The page posts to `/pair/<id>`: a signed change that adds the TV to the manifest, with the chosen profile keys boxed from the owner's box key to the TV's key, and a payload sealed under the pairing secret holding the account, the TV's id and key, and the owner's signing and box public keys. The server takes the pairing for this account, applies the change (the Durable Object accepts a new TV only from a sign-in), and makes the TV's **device token**.
7. The TV, waiting on `GET /v1/pair/<id>` (a long poll of up to 20 s, repeated), gets the payload and token once, opens the payload with its secret, checks it names its own key, and keeps the keys and token in `localStorage`. Its first `/v1/sync` brings the manifest and its profile keys.
8. Pairing IDs last 10 minutes, are used once, and are rate-limited.

Only the TV and the phone know the pairing secret, so a server that tried to give the TV keys of its own would fail the decryption. A typed code carries a shorter secret than the QR, so it protects less against a hostile server: one could try all 2^40 values against the sealed details offline, in days on one computer, though not within a pairing's 10 minutes without many. Anyone else can only guess online, through the rate limits.

Future apps on phones, desktops or other TVs can offer both: **Sign in with email and password**, or **Sign in with a phone** by QR code, which gives a profile-limited device as above.

## VelioTV pulling

- **When:** at startup, when the app comes back into view, every 15 minutes (the source manager's existing timer), and at once when the account's Durable Object sends `rev N` over the TV's WebSocket.
- **How:** `GET /v1/sync` with the device token and `If-None-Match: <manifest rev>`. Usually the answer is 304. Otherwise it holds the signed manifest, the TV's boxed profile keys, the profiles' encrypted names, and the lists those profiles can use, with their encrypted keys. The server sends a TV only what its profiles may read.
- **No WebSocket?** Polling alone works; the socket only makes a change show up within seconds rather than within 15 minutes.
- **Profiles on the TV:** a profile picker at startup when the TV has more than one profile, with the profile PIN if set. Each profile has its own lineup (the lists it may use), favourites, channel edits, continue watching, reminders and settings. A list two profiles share is imported once.
- **Applying:** a profile's synced lists replace its earlier synced lists, then go through the existing code: `saveSources()`, and the source manager imports any list whose address or login changed (`changesData()`) and deletes the data of lists no profile on the TV can use any more.
- **Lists added on the TV itself** stay on that TV, and sync doesn't touch them (see "Open choices"). A synced list's form on the TV says it is edited on the web, rather than allowing edits a pull would overwrite.
- **The TV only reads.** It never sends lists back.

## HTMX and end-to-end encryption

The server can't render what it can't read, so HTMX does the parts that aren't secret, and `vault.js` does the rest:

- **Rendered by the server:** sign-up, sign-in, the page shell, the device list, the pairing page, and the structure of the profile and list pages with empty forms.
- **Lists and profile names** come from the server as HTML holding their ciphertext in data attributes. After each swap, `vault.js` decrypts them in place.
- **Saving a list or changing who can use it:** before HTMX sends the form, `vault.js` builds and encrypts the records, signs the new manifest, and replaces the request's body with them, so the plain fields never leave the page.
- **Sign-in:** the same hook swaps the password for the auth key, so the password never leaves the page either.
- **Keeping the keys across pages and reloads:** links and forms are boosted (`hx-boost:inherited` on `body`), so moving between pages keeps `vault.js` and the keys in memory. A reload asks for the password again, unless "Remember this browser" was ticked: then the owner's secret keys are kept in IndexedDB, encrypted with a non-extractable AES-GCM key, until signing out or "Lock this browser".

HTMX 4 renamed its events and changed attribute inheritance compared with HTMX 2, so pin an exact version and write the hooks against its documentation, not older examples.

## Server

### Data

D1 (lookups across accounts):

| Table      | Columns                                                                                                                                                         |
| ---------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `accounts` | id, email (unique), confirmed, salt, Argon2id settings, auth key hash, created; closed (when, why); when a closed account's data was deleted                    |
| `sessions` | token hash, account, created, last used, expires                                                                                                                |
| `devices`  | id, account, public key, token hash, signed in, last pulled, signed out on the TV                                                                               |
| `pairings` | id, poll token hash, proof hashes, device id, public key, sealed details, expires; once signed in: account, payload, token until picked up                      |
| `notices`  | reference, received, kind, sender (name, email, organization), the account named and matched, the work, statements, signature, details, status, action, handled |

Each account's Durable Object (SQLite):

| Table         | Holds                                                                                          |
| ------------- | ---------------------------------------------------------------------------------------------- |
| `owner`       | The owner bundle, the signing and box public keys                                              |
| `recovery`    | When the recovery key is on: the SHA-256 of its auth key, and the owner bundle sealed under it |
| `manifest`    | The current signed manifest                                                                    |
| `profiles`    | id; name and picture encrypted under the profile key; the profile key under the account key    |
| `lists`       | id, rev, nonce, ciphertext; the list key under the account key                                 |
| `list_keys`   | list, profile, the list key under that profile's key                                           |
| `device_keys` | device, profile, the profile key boxed for that device                                         |

Plus the open device WebSockets, held with hibernation.

### Routes

Web (HTML for HTMX; session cookie `HttpOnly`, `Secure`, `SameSite=Strict`):

| Route                                                         | Does                                                                                                         |
| ------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------ |
| `GET /`, `/signup`, `/signin`                                 | Pages                                                                                                        |
| `GET /terms`, `/privacy`, `/abuse`; `POST /abuse`             | The terms, the privacy notice, and the abuse and copyright notice form                                       |
| `/ops/notices[/:id]`, `/ops/accounts/:id[/close\|/reopen]`    | The operator's JSON API, with the `OPS_TOKEN` bearer token (`scripts/ops.mjs`)                               |
| `/ops` (GET, POST)                                            | The operator's page: signing in with `OPS_TOKEN`, then notices and accounts (`src/client/ops.ts`)            |
| `GET /security`                                               | How Velio protects the lists, for owners (Phase 7)                                                           |
| `POST /signup`, `/signin`, `/signout`; `GET /confirm/:token`  | Accounts, sessions and confirming the email                                                                  |
| `GET /prelogin?email=`                                        | Salt and Argon2id settings                                                                                   |
| `POST /reset`, `GET /reset/:token`                            | The reset link, and resetting the account                                                                    |
| `GET /profiles`, `/lists`, `/lists/new`, `/lists/:id`         | Profile and list pages, with their ciphertext                                                                |
| `POST /changes`                                               | One signed change: records, keys and the new manifest; 409 if behind                                         |
| `POST /unlock`                                                | The owner bundle again for a signed-in tab, for the auth key                                                 |
| `GET /devices`                                                | The TVs and the profiles each has; changing them and removing one are signed changes through `POST /changes` |
| `GET /pair`, `POST /pair/find`, `POST /pair/:id`              | Finding a TV by its code, choosing its profiles and sending it the keys                                      |
| `POST /password`, `/recovery`, `/account/delete`              | Account settings, each with the current password's auth key                                                  |
| `POST /reset/:token/recovery-bundle`, `/reset/:token/recover` | The recovery copy of the owner bundle, then a new password that keeps everything                             |

Apps and TVs (JSON, `Authorization: Bearer <token>`, CORS open, no cookies):

| Route              | Does                                                                                              |
| ------------------ | ------------------------------------------------------------------------------------------------- |
| `POST /v1/signin`  | Email and password sign-in for future apps: auth key → token and owner bundle                     |
| `POST /v1/pair`    | Start a sign-in: `{publicKey, name, sealed, proofs}` → `{id, pollToken, expiresAt}`               |
| `GET /v1/pair/:id` | Long poll with the poll token: waiting, or `{deviceId, deviceToken, kind, payload}` once          |
| `GET /v1/sync`     | What the device may read (see "VelioTV pulling"), or 304 for `If-None-Match`                      |
| `POST /v1/changes` | The same signed changes as the web, for future owner apps                                         |
| `GET /v1/events`   | WebSocket, the token as subprotocol `bearer.<token>`; `rev N` on connecting and after each change |
| `POST /v1/signout` | The TV signs itself out: its token stops working and its sockets close (204)                      |

Samsung doesn't enforce CORS for the packaged app (VelioTV PLAN.md); LG is unconfirmed, so the `/v1` routes send CORS headers anyway.

### Costs

A few TVs pulling every 15 minutes is under 1,000 requests a day per account. Each TV's socket pings every 45 s; the runtime answers without waking the object, but incoming messages may still count as requests, at 20 messages per request, so 10 TVs add at most about 960 requests a day. Workers' free plan allows 100,000 requests a day, and D1 and SQLite-backed Durable Objects have free tiers well above this. Hibernated WebSockets aren't billed while idle.

## Legal and abuse

Not legal advice. This is the design side of staying a neutral host for the user's own subscriptions. **Before the service goes public or takes payment, get a one-off consultation with an IP or tech lawyer in your country**, and ask specifically about hosting liability for an end-to-end encrypted sync service. Update this section from what they say.

What courts and prosecutors have acted on in IPTV cases is selling or promoting access to pirated channels, shipping players preloaded with pirated sources (the EU's _Filmspeler_ case, C-527/15), marketing that points at piracy (the US _Grokster_ case), and services built for mass distribution. The plan stays clear of each:

1. **Household scale.** The limits per account (10 TVs, 8 profiles, 50 lists) keep one account from feeding a crowd. Choosing which profile sees which list happens inside one account, like parental controls, and isn't sharing with the public.
2. **No sharing between accounts.** No public or share links, no directory or search of lists, no "import someone's list", and no way for one account to send a list to another. Lists only reach devices the owner signed in themselves.
3. **No content.** Neither VelioTV nor the sync service ships, suggests, links to or sells sources or providers. Examples and tests use the test server's made-up sources only, as in VelioTV now.
4. **Neutral wording** everywhere: the app, the website, the store listings and any posts. "Your own IPTV subscriptions"; never "free channels", "free TV", "cancel cable" or provider names.
5. **Terms of service**, accepted at sign-up and shown at `/terms`:
   - users add only sources they have the right to use;
   - the service provides no content and can't see what users store;
   - accounts that break the terms can be closed, and repeat infringers are closed.
6. **A privacy notice** at `/privacy`: what the server holds (email, salt, hashes, counts, timestamps, IP addresses in logs) and what it can't read.
7. **Abuse contact and notices.**
   - An abuse and copyright form at `/abuse`, and an `abuse@` address on the domain.
   - Each notice is recorded in `notices`, with what was done.
   - The service can't read or remove one list, but it can **close an account**: its tokens stop working, its devices stop pulling, and its encrypted data is deleted after a set period (30 days, unless the lawyer advises otherwise).
   - A repeat-infringer policy: an account named in valid notices more than once is closed.
   - If the service has users in the US, register a DMCA designated agent with the US Copyright Office. In the EU, the Digital Services Act asks hosts for a point of contact and a notice-and-action process, which the form and the `notices` table cover.
8. **Minimal data,** as already planned: the server keeps no more than it needs, so it holds little either way. Logs with IP addresses are kept for a short, set time (such as 7 days).

## Layout

```
VelioPlatform/
  PLAN.md, README.md        the plan; setup, checks, deploying, and the security model in short
  package.json, pnpm-workspace.yaml
  Dockerfile                 the distroless image
  apps/server/               the server (Rust: axum, maud, rusqlite), what velio.tv runs
    Cargo.toml, migrations/  0001_init.sql, 0002_sealed_device_names.sql, 0003_tv_limits.sql
    src/main.rs, lib.rs      settings, the app, the nightly clean-up and backups; `health`, `backup`, `clean-up`
    src/routes/*.rs          web (accounts), vault, pair, settings, tv (/v1), probe, notices, ops
    src/views/*.rs           pages and fragments for HTMX, in maud: the Worker's markup
    src/vault.rs             each account's vault: owner, manifest, profiles, lists, TVs' keys
    src/events.rs            the TVs' sockets
    src/sync_core/           the server's half of sync-core, checked against tests/vectors.json
    client/                  vault.ts, ops.ts, app.css, ops.css: the browser half, where all encryption happens
    scripts/build-client.mjs builds public/static (vault.js, ops.js, pinned HTMX, the styles), which the binary embeds
    scripts/ops.mjs          the operator's tool: notices, closing and opening accounts, TV limits
    probe/test-account.json  the made-up account GET /v1/probe/test-account serves (scripts/make-test-account.ts)
    e2e/*.test.ts            the tests over HTTP, against the binary (e2e/support/ starts it)
  packages/sync-core/        shared with VelioTV and later apps
    src/bytes.ts, crypto.ts  base64, UTF-8, sealing, wrapping, boxing, signing (TweetNaCl)
    src/account.ts, login.ts password rules, Argon2id settings; login.ts adds hash-wasm and the recovery key (web only)
    src/owner.ts             the owner keys and their bundle
    src/read.ts              field-by-field readers for JSON from outside
    src/records.ts           lists, profiles and the manifest; the list form's checks (sourceProblem())
    src/changes.ts           opening an account, the owner's edits, building and applying a signed change
    src/pairing.ts           pairing ids, secrets, codes and proofs; the sealed details and payload
    src/device.ts            a TV's pull: pullFor() on the server, openPull() on the TV
    src/events.ts            live updates: the socket's address, subprotocols, messages, close codes and reconnect delays
    src/export.ts            the owner's unencrypted copy of the lists (exportLists(), web only)
```

VelioTV is its own repo. It can take `sync-core` as a git dependency from Forgejo, or the files can be copied in. `sync-core` must build to ES5 for VelioTV's legacy build (`pnpm check:old-tvs`).

## VelioTV changes

- **Profiles:** a profile picker at startup, and per-profile favourites, channel edits, continue watching, reminders and settings. Today these are app-wide in `localStorage`, so each key gets a profile part, and the current data becomes the first profile's.
- `src/data/sync.ts`: the device keys and token (`velio.sync`); pull, verify, decrypt, apply. Pure functions tested with Vitest against a mock server, as `xtream.ts` is tested against the test server.
- `Source` gets `synced?: true` and the profiles that use it; `readSource()` keeps them, and the source form shows synced lists read-only.
- Settings → Account: Sign in with a phone (QR code and typed code, waiting, done), the account and profiles, the last pull, Pull now, and Sign out. Text through `$t()` in every language file.
- A QR encoder that runs in ES5, such as `qrcode-generator`, drawn into a texture.
- The source manager pulls before checking for refreshes, at startup and on its 15-minute timer.
- Backups (`backup.ts`) leave out the sync keys: a restored TV signs in again.
- Test server (`scripts/test-streams/`): a mock `/v1` so the TV can be tested without Cloudflare.

## Phases

| #   | Phase               | Main work                                                                                                                                              | Done when                                                                                                                                                                                               |
| --- | ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 0   | TV checks           | Probes: HTTPS to a test Worker on the custom domain; TweetNaCl `secretbox`, `box` and `sign.open` speed in the legacy build; a WebSocket to the Worker | The test TV and the oldest Chromium reach the Worker over HTTPS, and open the test account in a worker in under 1 s with the UI at 60 fps (changed from 200 ms on the main thread; see Phase 0 results) |
| 1   | Accounts            | Repo setup, Worker, D1, sign-up with email confirmation, sign-in with Argon2id, sessions, reset, rate limits                                           | An account can be made, confirmed, signed into and reset, with tests in the Workers pool                                                                                                                |
| 2   | Profiles and lists  | `sync-core`, the Durable Object, the profile and list pages (the same checks as `sourceProblem()`), giving and taking lists, the signed manifest       | Lists made in one browser show in another; the database holds only ciphertext; a taken list's old key opens no new version                                                                              |
| 3   | TV sign-in and pull | QR sign-in with profile choice, `/v1/sync`, `sync.ts`, Settings → Account, applying synced lists                                                       | A TV signs in by QR with two of three profiles, and a list added to one of them imports within 15 minutes                                                                                               |
| 4   | Profiles on the TV  | The profile picker, profile PINs, per-profile favourites, edits, watched, reminders and settings                                                       | Two profiles on one TV keep their own lineups and favourites across restarts                                                                                                                            |
| 5   | Live updates        | Durable Object WebSockets; the TV's socket, with polling as the fallback                                                                               | A change on the web starts the TV's import within seconds                                                                                                                                               |
| 6   | Devices             | The device page, changing a TV's profiles, removing a TV, key rotation, password change, recovery key, deleting the account                            | A removed TV, or a TV a profile was taken from, can't read a new version; the others carry on                                                                                                           |
| 7   | Hardening           | Security review of the crypto and routes, headers (CSP, HSTS), export of the lists, docs                                                               | Review findings fixed; a full run on the TV from sign-up to playing a synced channel                                                                                                                    |
| 8   | Legal and abuse     | Lawyer's review, terms, privacy notice, `/abuse` form and address, `notices`, closing accounts, log retention, DMCA agent if needed                    | Done before the service is public: the lawyer's points are in "Legal and abuse", and a test notice closes a test account and stops its TV                                                               |

### Risks to check first (Phase 0)

- **Certificates on old TVs:** a 2017 Tizen TV's root certificates may not include the authority behind Cloudflare's certificate for the domain. Old Chromium on a desktop uses the desktop's store, so only a real old TV can show this. If it fails, Cloudflare's certificate authority can be chosen per domain (Advanced Certificate Manager).
- **HTTPS on Chromium 38:** webOS 3's TLS should manage TLS 1.2, but confirm before the LG phase.
- **TweetNaCl speed:** `sign.open` (Ed25519) is the slowest part in plain JavaScript; the probe confirms an old TV verifies a manifest fast enough.

### Phase 0 results (2026-11-10)

In progress: speed, WebSocket and HTTPS are checked; what's left is the worker-based open for the new done check, and the certificate on old TVs. The Worker is deployed at `https://velio.tv` (Wrangler on the host, in the account that holds the zone), with its `workers.dev` address and preview URLs turned off, as everything lives on `velio.tv` itself.

**What's built:**

- This repo: a pnpm workspace with the same tooling as VelioTV (`pnpm check`: oxfmt, oxlint, tsc, Vitest). 23 tests pass.
- `packages/sync-core`: base64 and UTF-8 by hand (`bytes.ts`, no TextEncoder or `fill()`, for Chromium 38), and sealing with padding, key wrapping, boxing and signing on TweetNaCl 1.0.3 (`crypto.ts`). `testAccount.ts` makes a made-up account as a TV would pull it (3 profiles, 20 lists, a signed manifest; hosts on `example.invalid`), and `openForDevice()` is what the TV does with it. Its tests check that a manifest signed by anyone else, a list at another revision, or a key boxed by anyone but the owner are refused.
- `apps/sync`: the Worker with Hono, and Phase 0's routes: `GET /v1/probe` (how the request arrived: TLS version, cipher, HTTP version), `GET /v1/probe/test-account`, and `GET /v1/probe/events` (a WebSocket sending `rev 1` to `rev 5` and echoing). CORS is open on `/v1`. Tests run in the Workers runtime through `@cloudflare/vitest-plugin`, the new name of `@cloudflare/vitest-pool-workers`. The test account is written once by `pnpm --filter @velio/sync test-account`, as making it in the Worker would take much of a free-plan request's 10 ms of CPU.
- VelioTV: `tweetnacl` as a dependency, exposed to probes as `window.velio.nacl`, and the `sync` probe (`scripts/tv-probe/probes/sync.js`), which reaches the Worker, opens the account 5 times and tries the socket. The legacy build stays ES5 (`pnpm check:old-tvs`), and VelioTV's `pnpm check` passes.

**Opening the test account** (check the manifest's signature, open 3 boxed profile keys, unwrap and decrypt 34 lists), in the legacy build:

| Where                                       | First open (cold)                             | Median of 5 | Parsing the 42 KB pull | WebSocket                        |
| ------------------------------------------- | --------------------------------------------- | ----------- | ---------------------- | -------------------------------- |
| Chromium 47 (Tizen 3), on the dev machine   | 92 ms (signature 55, keys 17, lists 20)       | 45 ms       | not measured           | Opens in 7 ms, all 5 revisions   |
| Chromium 38 (webOS 3), on the dev machine   | 101 ms (signature 49, keys 20, lists 32)      | 59 ms       | 59 ms                  | Opens in 7 ms, all 5 revisions   |
| Samsung test TV (Tizen 9), over the network | **231 ms** (signature 141, keys 32, lists 58) | 86 ms       | 0.6 ms                 | Opens in 116 ms, all 5 revisions |

The TV opened all 34 lists with the right names and revisions, but **the first open missed the 200 ms target**, mostly on the one signature check while the code is still cold. A 2017 TV is slower than this 2024 one, so it would miss by more. The desktop runs are the oldest browsers' engines on a fast CPU, so they show the code runs there, not how fast an old TV is. (`wrangler dev` reports a made-up TLS version; the real one comes with the deploy.)

**Decision: keep TweetNaCl, but keep the work off the UI's thread and do as little as possible.**

- Most pulls are a 304, which needs no crypto at all.
- When the manifest changed, the TV decrypts only the lists whose revision changed, and keeps the rest from the last pull.
- The work runs in a Web Worker (VelioTV's import worker, or a small one of its own), so a slow old TV takes longer to apply a change but never drops frames. The legacy build already runs workers on old TVs.
- The done check becomes: **the oldest Chromium and the test TV open the account in a worker in under 1 s, while the UI stays at 60 fps.**

**Over HTTPS, against `https://velio.tv`** (2026-11-10):

| Where                                     | TLS                                | HTTP     | First open | Median | `wss://`              |
| ----------------------------------------- | ---------------------------------- | -------- | ---------- | ------ | --------------------- |
| Samsung test TV (Tizen 9)                 | 1.3, AES128-GCM                    | HTTP/2   | 263 ms     | 85 ms  | Opens in 63 ms, all 5 |
| Chromium 47 (Tizen 3), on the dev machine | 1.2, ECDHE-ECDSA-AES128-GCM-SHA256 | HTTP/2   | 91 ms      | 49 ms  | Opens in 76 ms, all 5 |
| Chromium 38 (webOS 3), on the dev machine | 1.2, ECDHE-ECDSA-AES128-GCM-SHA256 | HTTP/1.1 | 104 ms     | 59 ms  | Opens in 83 ms, all 5 |

The first open on the TV (263 ms, the signature check 165 ms of it) again misses 200 ms on the UI's thread, as decided above.

**The certificate is the open risk for 2017 TVs.** Cloudflare's Universal SSL gave `velio.tv` a Let's Encrypt **ECDSA** certificate (issuer YE1) whose chain ends at **ISRG Root X2** (2020), through ISRG Root YE, and serves no RSA one. The test TV and the old Chromiums accepted it, but the old Chromiums check against the dev box's up-to-date root store, not a TV's. A 2017 TV's store very likely lacks ISRG Root X2, and maybe ISRG Root X1 too. The likely fix is a certificate from **Google Trust Services**, whose roots are cross-signed by GlobalSign Root CA (1998), which old TVs trust. Cloudflare lets the zone's certificate authority be chosen; check whether the free plan allows it or it needs Advanced Certificate Manager. Change it only once an old TV (or its root list) shows the need.

**Still to do for Phase 0:**

1. ~~Move the opening into a worker~~ Done in VelioTV, 2026-11-10: in Chromium 47 (Tizen 3), the legacy build opened the test server's account in its sync worker in 85–89 ms (231 ms with starting the worker), with no frame over 17 ms. Still to see on the test TV (VelioTV's `SYNC-PLAN.md`, "Results").
2. Certificates on 2017 TVs: run the `sync` probe on one when one is at hand, or compare Tizen 3's and webOS 3's root lists with the chain above. Until then, the choice of certificate authority stays open.

### Phase 1 results (2026-11-10)

**Done:** an account can be made, confirmed, signed into and reset. 23 tests in the Workers pool cover each step, the rate limits, the Origin check and the daily clean-up. 33 more in `sync-core` cover the keys, including an Argon2id vector checked against argon2-cffi. The same flow also ran end to end in Chrome 157 against `wrangler dev`, with the real Argon2id: sign-up about 0.7 s, sign-in about 0.5 s, no CSP violations, and the password in none of the 11 posts. It's deployed on velio.tv, with the D1 migration applied.

How it works:

- **Signing up:** `vault.js` makes a salt and runs Argon2id (64 MiB, 3 passes) on the password, then HKDF splits the result into the auth key and the unlock key. It makes the owner keys and seals them under the unlock key. Only the salt, the settings, the auth key, the sealed bundle and the two public keys are sent. D1 stores the SHA-256 of the auth key; the account's Durable Object stores the bundle and the public keys.
- **HTMX 4.0.1:** forms marked `data-velio` have `hx-confirm`. `vault.js` handles `htmx:confirm`, fills `ctx.request.body`, then calls `issueRequest()`. Password inputs have no `name`. The server shows form problems through `HX-Retarget: #form-problem`. `includeIndicatorCSS` is off, so `style-src 'self'` holds.
- **Confirming:** the link opens a page with a button, so mail scanners don't confirm. Links are single-use and stored hashed; a new link replaces the old one. Confirm links last 24 h, reset links 1 h.
- **Signing up again:** an unconfirmed account is replaced by the newest sign-up for its email. For a confirmed one, the answer is the same "check your email", and the email says the account already exists.
- **Signing in:** `GET /prelogin` answers the salt and settings, or made-up but stable ones (HMAC of the email) when there's no account. `vault.js` refuses settings weaker than the default. A wrong key and an unknown email get the same answer. The answer carries the owner bundle; `vault.js` opens it, checks it against the public keys, and keeps the keys in the tab's memory. A reload locks them, and the account page asks for the password again.
- **Sessions:** `__Host-velio`, HttpOnly, Secure, SameSite=Strict, 30 days, renewed on use at most once a day. Posts without the site's own `Origin` are refused.
- **Resetting:** a new password, new owner keys, the Durable Object wiped, and every session ended. Opening the link also confirms an unconfirmed email.
- **Limits:** sign-in and `/prelogin` 20 a minute per IP; sign-in 5 a minute per email; emails 5 a minute per address; sign-ups and reset requests 5 a minute per IP. Turnstile (managed widget for velio.tv) on sign-up and reset requests. Without its secret the server fails closed.
- **Clean-up:** a daily cron (04:17 UTC) deletes accounts left unconfirmed for 7 days, with their Durable Object data, plus expired sessions and links.
- **Development:** `.dev.vars` sets `DEV_MODE=1`. Mail then goes to `dev_outbox`, shown at `/dev/outbox` with relative links (wrangler dev shows the Worker velio.tv as the host). Turnstile uses Cloudflare's always-pass test key. Production has no `DEV_MODE`, and `/dev/outbox` is a 404 there.
- **Secrets set** with `wrangler secret put`: `TURNSTILE_SECRET` and `PRELOGIN_SECRET`. The latter is random and kept nowhere else; changing it only changes the made-up salts.

**Still to check by hand, in a real browser on velio.tv:**

1. **Email delivery.** The first sign-up's email didn't arrive: velio.tv wasn't onboarded in Cloudflare Email Service, and Email Sending needs the Workers Paid plan ($5 a month). The Worker now sends with Resend whenever the `RESEND_API_KEY` secret is set, and with Email Service otherwise. Choose one: upgrade and onboard velio.tv in Email Service, or verify velio.tv in Resend (free tier) and set the secret. Then sign up again.
2. **Turnstile.** Headless Chrome got no token, so the server refused the sign-up, as it should. A normal browser should pass.

**Left for later phases:** Devices and lists to delete on reset (Phases 2 to 6). `/v1/signin` for apps. The terms and privacy texts are drafts until Phase 8. `vault.js` is 85 KB, mostly hash-wasm's Argon2id; trimming it can wait.

### Phase 2 results (2026-11-10)

**Done:** profiles and lists, made and changed on the web, end-to-end encrypted. The done checks hold:

- **Lists made in one browser show in another.** Checked in the Workers tests and in Chrome 157 with two browser contexts.
- **The database holds only ciphertext.** A test dumps every table of the account's Durable Object and of D1 after adding lists, and finds no name, host, username or password. In Chrome, none of the 14 posts held anything readable.
- **A taken list's old key opens no new version.** A test keeps the list key a TV with the taken profile would hold, and it fails to open the next version.

Tests: 53 in `sync-core` (20 new, for changes), 33 in the Workers pool (10 new). `pnpm check` passes.

How it works:

- **`sync-core/changes.ts`** holds both sides. The owner's device opens what the server holds (`openState`: checks the manifest's signature, that the records are the ones it names at its revisions, and that each profile's copy of a list key is that list's key). It makes edits on the opened account (`withList`, `withoutList`, `withProfile`, `withoutProfile`) and turns the difference into one change (`buildChange`): the new and changed records sealed again, the deletions, and the next manifest, signed. The server (`applyChange`) checks the change starts at the current revision ("behind" → 409), is signed by the owner, goes up by exactly one, stays within the limits, and leaves records that match the new manifest exactly.
- **The manifest** now lists every list, with `rev` and `keyRev`, and each profile names its lists by id. So a list can exist with no profile, and membership changes show as a profile's `lists` changing.
- **Taking a list from a profile, or deleting a profile,** gives the list a new key (`keyRev` + 1) and a new revision. Giving a list to another profile only adds a wrapped copy of its key.
- **The Durable Object** keeps `manifest`, `profiles`, `lists` and `list_keys` in SQLite and applies a change in one `transactionSync`. Its tables are made in the constructor, so there's no migration to run.
- **The pages** render what the server knows from the manifest (ids, which profile has which list, counts) with empty slots, and carry the ciphertext in `[data-vault-state]`. `vault.js` opens it after every swap and fills in names, summaries (kind and host only, as an address's query can hold a login) and the list form. Until the keys are open, the page shows an unlock form (`POST /unlock`, which doesn't make a new session).
- **Saving:** list and profile forms have unnamed inputs and `data-velio`, as the account forms do. `vault.js` runs the same checks as VelioTV's `sourceProblem()` (`listProblem()`), builds the change, and sends only `change` and `next`. The answer is the next page, built from the account after the change. A stale tab gets "changed in another tab or on another device" and keeps what was typed.
- **Remember this browser** (on sign-in and unlock): see "HTMX and end-to-end encryption". Keys remembered from before a reset are dropped, as the page names the account's signing key.

**Deployed** to velio.tv (version 5bfca36d), with no D1 migration. **Not yet:** TVs, pairing and the `devices` part of the manifest come with Phase 3; until then the server refuses a manifest that names a TV.

### Phase 3 results, the server half (2026-11-10)

**Done:** everything a TV needs from the server, tested with the TV simulated by `sync-core`. The done check holds on the server side: **a TV signs in by QR with two of three profiles, and a list added to one of them is in its next pull.** It's checked in the Workers tests and in Chrome 157, with the TV simulated in Node. "Imports within 15 minutes" needs VelioTV's half.

Tests: 71 in `sync-core` (18 new, for pairing and pulls), 46 in the Workers pool (13 new). `pnpm check` passes. In Chrome:

- A signed-out phone opened the QR address, signed in and gave the TV two profiles; the secret left the address bar.
- The TV picked up, pulled, got a 304, then got the new list.
- A second TV signed in by its typed code.
- None of the 43 requests held a pairing secret, a profile name, a list address or the password.

How it works:

- **`sync-core/pairing.ts`:** pairing ids and secrets, the QR address and typed code (read back forgiving case, spaces, dashes and look-alike letters), proofs, and the TV's sealed details and the phone's sealed payload.
- **`sync-core/device.ts`:** `pullFor()` picks what one TV may read: the manifest, its boxed profile keys, its profiles, and their lists with the list keys wrapped under those profiles' keys only. `openPull()` is the TV's side. It checks the owner's signature, the account, that the revision doesn't go back, that the manifest names this TV, that the profile keys come from the owner's box key, and that every record is the revision the manifest names. It reuses lists whose revision didn't change, and throws `DeviceRemoved` once the manifest stops naming the TV.
- **The manifest's `devices`** hold each TV's id and profiles. The Durable Object keeps each TV's boxed profile keys (`devices`, `device_keys`). A change may add a TV only from `POST /pair/:id`, and deleting a profile takes its keys off every TV.
- **D1 `devices`** holds the TV's name, key and token hash for `/v1/sync`. A reset deletes the TVs and pairings with the sessions.
- **`GET /v1/sync`** answers 304 for `If-None-Match` at the current revision, and 401 for an unknown token or a TV the manifest no longer names. It writes `last_pulled_at` at most every 10 minutes.
- **The account page** lists the TVs (name, signed in, last synced), with a link to `/pair`.

**Deployed** to velio.tv (version be4f53e2), after applying `0002_devices.sql` to the remote D1. **Not yet:** VelioTV's half: `src/data/sync.ts`, Settings → Account with the QR code, applying synced lists, and a mock `/v1` in the test server. Changing a TV's profiles and removing a TV are Phase 6.

### Phase 4 results, the web half (2026-11-10)

**Done:** what the TV's profile picker needs from the account. Each profile has a picture and can have a PIN, set by the owner on the web and carried to TVs inside the profile's sealed record. Per-profile favourites, channel edits, continue watching, reminders and settings stay on each TV (see "Open choices", 2), so the server has nothing more to hold for them. The done check ("two profiles on one TV keep their own lineups and favourites across restarts") is VelioTV's half.

Tests: 83 in `sync-core` (12 new), 47 in the Workers pool (1 new). `pnpm check` passes. In Chrome 157, against `wrangler dev`:

- Each profile showed its first letter on its colour; Kids was changed to purple with a PIN, and a 2-digit PIN was stopped by the browser.
- A TV given only Kids, simulated in Node, pulled it purple with its PIN; once the PIN was removed on the web, its next pull had none.
- None of the 34 requests held the PIN, a profile name, a list address, the password, or the colour choice.

How it works:

- **`ProfileRecord`** gains `color` (an index into `PROFILE_COLORS`) and `pin` (four digits, kept as typed, as VelioTV's parental PIN), both optional, so the format stays 1 and older profiles still read. `profileColor()` gives a profile made before pictures a steady colour from its id.
- **`PROFILE_COLORS`:** 8 colours, one per profile an account can hold. Each keeps a white letter at 4.9:1 contrast or more, and stands out 3:1 or more from a white page and a dark one. `profileInitial()` takes the name's first character in capitals, whole even for an emoji. Web and TV draw the same picture.
- **`withProfile(state, draft, id?)`** takes a `ProfileDraft`: a new profile gets the first colour no other profile has; a colour or PIN left out is kept, and `pin: null` removes the PIN. `profileProblem()` checks a draft.
- **The TV's view:** `openPull()`'s `DeviceProfile` now has `color` and, when set, `pin`. `profilePinMatches()` is the TV's check.
- **The profiles page:** each row shows the picture and "PIN set"; Change has the name, the 8 colours and the PIN. The picture is drawn by `vault.js` through the CSSOM, which `style-src 'self'` allows, so the page still has no inline styles. The colour radios need a `name` to work as a group, so `vault.js` now sends only `next` and the change from every vault form, whatever the form names. Pictures also show beside the profiles when signing in a TV.

**Deployed** to velio.tv with Phase 5 (version 968833ae). **Not yet:** VelioTV's half: the profile picker (skipped with one profile), asking the PIN with the existing `PinPrompt`, and a profile part in each `localStorage` key for favourites, edits, watched, reminders and settings, with today's data becoming the first profile's.

### Phase 5 results, the server half (2026-11-10)

**Done:** the account's Durable Object holds each TV's WebSocket and tells it the new revision after every change. The server side of the done check holds: **a change on the web reaches the TV within a second.** Against `wrangler dev`, with the TV simulated in Node, the TV got `rev N` about 80 to 110 ms after the click and had the new list opened about 150 to 230 ms after it. It did the same after 12 s idle, past the 10 s it takes the object to hibernate. "Starts the TV's import" is VelioTV's half.

Tests: 87 in `sync-core` (4 new), 51 in the Workers pool (4 new). `pnpm check` passes.

How it works:

- **`sync-core/events.ts`** is the protocol both sides share: `eventsUrl()`, `eventsProtocols(token)` and `readEventsProtocols()`, `readEvent()`, the close codes, and `reconnectDelay()`. The delay doubles from 1 s up to 5 minutes, and is spread over the upper half of that, so TVs that lost the server together don't all come back at once. Its header comment says what the TV does with each message.
- **The token** goes as a second subprotocol, `bearer.<token>`, beside `velio-events.1`, as a browser's WebSocket can't send an `Authorization` header. It stays out of the address, and so out of logs. The server answers with `velio-events.1` only. Connections are limited per IP, like pairing.
- **`GET /v1/events`** finds the TV by its token in D1 (the same lookup as `/v1/sync`), then hands the upgrade to the account's object with the TV's id. The object accepts the socket with hibernation, tagged with the TV's id, and sends `rev N` at once, so a TV that reconnects catches up on what it missed.
- **After each change**, the object sends `rev N` to every socket. A socket whose TV the manifest no longer names gets `removed` and close code 4401. A reset does the same to every socket, before the data is deleted.
- **Pings:** the TV sends `ping` every 45 s and the runtime answers `pong` (`setWebSocketAutoResponse`), without waking the object or billing its time. Other messages are ignored.
- **Two sockets per TV** at most: a third closes the oldest with 4409, which tells that socket not to reconnect. A reconnect can race the old socket's end, and the old one is likely dead already.
- **Closing from the TV's side** is answered by the object, so it ends with 1000 rather than 1006.

**Deployed** to velio.tv (version 968833ae); a `wss://` connection with a wrong token is refused there. **Not yet:** VelioTV's half: open the socket after signing in and when the app comes back into view, pull on `rev N` (skipping the revision it has), ping every 45 s, reconnect with `reconnectDelay()` while keeping the 15-minute poll, and sign out after a 4401 once `/v1/sync` confirms with 401. Also not yet: checking that the old TVs keep a `wss://` connection through Cloudflare for hours, not just the 5 messages of Phase 0's probe.

### Phase 6 results, the server and web half (2026-11-10)

**Done:** the TVs page, changing a TV's profiles, removing a TV, the new keys both need, changing the password, the recovery key, and deleting the account. The done check holds on the server's side: **a removed TV, or a TV a profile was taken from, can't read a new version; the others carry on.** In Chrome 157 against `wrangler dev`, with two TVs simulated in Node (Living room TV with Living room and Kids, Kids TV with Kids):

- Taking Kids from Living room TV: its socket got `rev N` and its next pull had only Living room's list, 166 ms after the click. The Kids list was then renamed: Kids TV pulled the new name, and none of the keys Living room TV had pulled before opened any of it.
- Removing Living room TV: its socket got `removed` and close 4401, `/v1/sync` answered 401, and Kids TV carried on.
- A recovery key was turned on and shown once; the password was changed (a wrong current one refused; the old one then failed at sign-in, the new one opened the lists); then, signed out, the reset link with the recovery key (typed in lower case with spaces) set a third password, and the lists, profiles and Kids TV were all still there.
- Deleting the account signed the browser out, Kids TV's pull answered 401, and signing in failed.
- None of the requests held a password or the recovery key. The console had only the 401s of the three wrong passwords tried on purpose.

Tests: 96 in `sync-core` (9 new), 59 in the Workers pool (8 new). `pnpm check` passes.

How it works:

- **The TV's public key is now in the signed manifest** (`devices[].publicKey`), written when the TV signs in. The owner boxes new profile keys to that key, never to one the server hands over, so a server can't add a TV of its own by answering with its own key.
- **`withDeviceProfiles(state, tv, profiles)`** (at least one profile; otherwise remove the TV) and **`withoutDevice(state, tv)`** give each profile the TV loses a new key and revision, and each list those profiles can use a new key (`keyRev` + 1). `buildChange()` boxes the new keys for every TV that keeps the profile, and for a TV only when it is new, gains a profile, or keeps one whose key changed.
- **The server checks it** (`applyChange` → `checkRekeyed`): each profile a TV lost, if it still exists, must be in the change with a new key, every other TV holding it must get that key, and every list the TV could reach before and can't now must have a higher `keyRev`. It keeps only the keys the manifest gives each TV, in the Durable Object too, which a Worker test caught.
- **The TVs page** (`/devices`, and "TVs" in the nav): each TV with its profiles' names, a Change form with a box per profile, and Remove. Both are signed changes through `POST /changes`. Removing deletes the TV's token from D1 straight after, and the object sends that TV's sockets `removed` and closes them with 4401.
- **`removed` comes before close 4401**, and the TV should act on it: in `wrangler dev` the close handshake was done in 20 ms, but the connection itself only ended about 10 s later, when the object went idle. `events.ts` says so.
- **Every setting needs the current password**, sent by `vault.js` as its auth key (through `/prelogin`, as signing in), so a stolen session cookie can't change who opens the lists, or delete them. Tries are limited per IP and per account.
- **Changing the password** seals the same owner keys under the new one, after checking they are the account's public keys. The new bundle goes to the object first; if D1 then fails, the old one goes back. Every other session and any reset link ends; TVs and the recovery key stay.
- **The recovery key:** made in the browser, which shows it once and sends only the auth key and the owner keys sealed under the unlock key. The reset page then offers it first. `POST /reset/:token/recovery-bundle` answers the sealed copy for the right auth key, without using the link; `vault.js` opens it, checks it against the public keys, and seals it under the new password for `POST /reset/:token/recover`, which checks the key again, uses the link and keeps everything. A reset without the key deletes the recovery copy with everything else.
- **Deleting the account** resets the object first (sockets closed as removed), then deletes the account's TVs, pairings, sessions, links and row from D1, and signs out. If D1 failed after the object, the account would still be there to try again.
- **Key rotation** here is the profile and list keys above. The account key and the owner's key pairs change only with a reset; changing them in place would mean re-sealing every key and re-signing every TV, which nothing needs yet.

**TVs signed in before Phase 6** have no public key in the manifest, so they can be removed but can't be sent new keys: changing their profiles, or taking a profile from another TV that shares one of theirs, asks for them to be signed in again. No real TVs are signed in yet (VelioTV's half isn't built), so this only matters for test accounts.

**Deployed** to velio.tv (version 18907ed7). **Not yet:** VelioTV's half: sign out on `removed` (or 4401) once `/v1/sync` confirms with 401, and take the new profile keys a pull brings (`openPull()` already does).

### Phase 7 results, the server and web half (2026-11-10)

**Done:** a security review of `sync-core`'s crypto and every route, the fixes it called for, the headers, a download of the lists, and the docs (`README.md`, and `/security` for owners, linked in the footer). The TV half of the done check (a full run on the TV from sign-up to playing a synced channel) waits for VelioTV's client.

Found and fixed:

1. **Rollback (medium).** The server can't forge a signed state, but it could send an older one, such as from before a TV was removed. An edit made on it would sign the TV back in. `vault.js` now remembers, per account and signing key, the newest revision this browser has opened (`localStorage` `velio-seen`) and refuses an older one: the page says so and shows nothing to edit. A browser that never opened the newer revision can't tell; `/security` says to make changes from a browser used often.
2. **Bad sizes from outside.** A nonce, key or signature of the wrong length, or text that isn't base64, made TweetNaCl throw its own errors. `crypto.ts` now checks each length and answers `CryptoProblem`, which `applyChange` and `openState` already turn into a refusal.
3. **Headers.** Pages now also send `Cross-Origin-Resource-Policy: same-origin` and a `Permissions-Policy` turning off camera, microphone, location, payment, USB and the like (no page uses any). Static files were cached for 5 minutes under names without a hash, so a deploy could pair new pages with an old `vault.js`; they now send `Cache-Control: no-cache` (checked with the ETag each time) and CORP.
4. **Body sizes.** Bodies were read whole before any route checked them. Now 10 MB at most for web forms (the biggest change is 3 MB of JSON, longer once form-encoded) and 64 KB for `/v1`, answering 413.
5. **Pairing polls** weren't limited. Each `GET /v1/pair/:id` waits up to 20 s with D1 reads; now 20 a minute per IP (`AUTH_LIMIT`, `pair-poll:`), so a TV has to wait for each answer.
6. **Console noise.** Turnstile warned about widgets htmx had swapped out; `vault.js` now removes them after the swap.

Reviewed and fine: the CSP (scripts only from velio.tv and Turnstile), HSTS, `nosniff`, `no-referrer`, `X-Frame-Options`, COOP; same-origin posts; the `__Host-` SameSite=Strict session cookie, new on every sign-in; tokens stored only as hashes and compared in constant time; no open redirects (`next` is a path on velio.tv); JSX escaping; the sizes and limits `applyChange` checks; every setting needing the current password.

Accepted, and written down:

- **The page's code comes from the server.** A taken-over velio.tv could serve a `vault.js` that sends the password elsewhere. That holds for any web app; VelioTV's own code doesn't come from velio.tv. Said on `/security`.
- **The typed pairing code** has a 40-bit secret a hostile server could find offline in days ("Signing in a TV"). `/security` says to scan the QR code when possible.
- **A device token sits in D1 in plain text** for at most the pairing's 10 minutes, until the TV picks it up. Hashing it would need the TV to send something the server could check instead, for little gain.
- **Turnstile's script** runs on the sign-up and reset pages, which hold no keys yet.
- **HSTS without `preload`.** Adding it is a one-way step for velio.tv; it waits for the user's choice.
- **Forgetting removed TVs** in D1 happens after the change is saved; until then the Durable Object has already dropped their keys and closed their sockets.

**The list export:** "Download a copy" on `/lists` makes `velio-lists-YYYY-MM-DD.json` in the browser from what it has opened (`exportLists()` in `sync-core`): format 1, the time, the profiles (name, colour, PIN) and the lists (all their fields, with the profiles that use them by name). Nothing is sent. The page says the file isn't encrypted.

Tests: 98 in `sync-core` (2 new), 64 in the Workers pool (5 new, for headers, body sizes and the poll limit). `pnpm check` passes. In Chrome against `wrangler dev`: the headers and `/security`; a profile and a list, then the download, with the right file name and content and no request sent; a remembered revision raised by 5, which made the lists page refuse with both revisions named; then back, and it opened. The Phase 6 run still passes, with no console warnings.

**Deployed** to velio.tv (version 157fcf94), with no D1 migration.

### Phase 8 results, the server and web half (2026-11-10)

**Done:** the abuse and copyright form, the `notices` table, closing accounts with the owner told why, opening them again, deleting a closed account's data after 30 days, the operator's tool, and new drafts of the terms and the privacy notice. The done check's second half holds: **a test notice closed a test account and stopped its TV.** In Chrome against `wrangler dev`, with a TV simulated in Node:

- A copyright notice sent from `/abuse` (linked as "Report abuse" in the footer) was refused without the sworn statements, keeping what was typed, then received with a reference such as `N-C263YW7P` and mailed to `abuse@velio.tv`.
- `node scripts/ops.mjs notices` listed it, naming the account; `notice <id>` showed the account with its TV; `close <email> "<reason>" --notice <id>` closed it. The TV's socket got `removed` at once and its pull answered 401; the notice became `actioned`; the owner was emailed the reason, the date the data goes, and where to contest; signing in answered "This account is closed."
- `reopen <email>` opened it again: signing in worked, and the notice stayed on the account's record.
- The console had only the 400 of the notice refused on purpose.

Tests: 98 in `sync-core`, 70 in the Workers pool (6 new: the form, the token, closing with a TV and its socket, opening again, deleting after 30 days, rejecting a notice). The TV helpers moved from `pair.test.ts` to `test/tv.ts` to share them. `pnpm check` passes.

How it works:

- **Notices** (`POST /abuse`): Turnstile and the sign-up limit per IP. A copyright notice needs the work, the good-faith and accuracy statements, and a signature (the elements of a DMCA notice; the DSA asks for much the same). The account is named by its email; the match is stored but never shown to the sender. Each notice gets a reference made of 40 random bits, is kept, and is mailed to `ABUSE_EMAIL`. Senders get no email, so the form can't be used to send mail to others.
- **The operator's API** (`/ops`) takes JSON with the `OPS_TOKEN` secret as a bearer token, compared as SHA-256 digests in constant time and limited per IP. It sends no CORS headers. Without the secret it is a 404. The operator's page at `/ops` (below) uses it with a session cookie instead of the token.
- **Closing** sets `closed_at` and the reason. It ends sessions, email links and pairings, closes the TVs' sockets as removed (the TV signs out), and emails the owner the reason (the DSA's statement of reasons) with the deletion date. Sign-in, sessions, TV pulls and sockets were already refused for closed accounts. TV rows stay, so opening again before the deletion brings back everything except the TVs that signed out.
- **After 30 days** the daily cron deletes the Durable Object's data, TVs, sessions, links, and the password's salt and hash. It keeps the email, the dates and the reason, so the address can't sign up again; sign-up already answers a closed account's email as if it were new, without sending anything.
- **The terms** now cover closing, the 30 days, how to contest, deleting your own account, and how to report. **The privacy notice** lists what is held and for how long:
  - logs: 7 days, Workers Logs on the Paid plan;
  - backups: up to 30 more days, D1 Time Travel and Durable Object point-in-time recovery;
  - closed accounts' email and reason;
  - notices;
  - Cloudflare as the processor.
    Both are still marked as drafts.

**For the user, before going public:**

- **The lawyer's review**, with these questions:
  - hosting liability for an end-to-end encrypted sync service;
  - the 30 days before deletion;
  - how long to keep a closed account's email and reason, and notices;
  - the sworn wording on the form;
  - whether the repeat-infringer rule ("more than once") is right;
  - the terms and privacy drafts.
- **`abuse@velio.tv` must reach a person:** a Cloudflare Email Routing rule to a private inbox. Notices are mailed there, and closed accounts are told to write there.
- **The DMCA designated agent**, if Velio has users in the US: registered with the US Copyright Office, and named on `/abuse`.
- **`OPS_TOKEN`**: `npx wrangler secret put OPS_TOKEN` with a long random value (for example `openssl rand -base64 32`), kept only by the operator. Until it is set, `/ops` is a 404 in production.

**The operator's page** (2026-11-10), at **velio.tv/ops**, so an account isn't closed by mistake. First a local page (`scripts/ops-web/`), now served by the Worker and the local one removed.

- **Signing in:** a form takes the `OPS_TOKEN` secret (compared like the bearer token; wrong tries count toward the same 20 a minute per IP) and sets `__Secure-velio-ops`: HttpOnly, Secure, SameSite=Strict, Path=/ops, 8 hours. The value is its expiry, a random part and an HMAC of both keyed by `OPS_TOKEN` (`lib/opsSession.ts`), so nothing is stored and changing the secret ends every session. **Sign out** deletes it.
- **The API with the cookie:** `/ops/*` takes the cookie as well as the bearer token; with the cookie, anything but GET needs velio.tv's own Origin (403 otherwise). The pages set `Referrer-Policy: same-origin` by meta tag, as the site's `no-referrer` makes Chrome send `Origin: null` with a plain form.
- **Closing:** `POST /ops/accounts/:id/close` now needs `confirm`, the account's email typed again; anything else is a 400 and nothing changes. The page's Close button stays off until a reason and the email are there; `ops.mjs close` shows the account and asks for the email (or `--confirm`).
- **Notices are text:** anyone can send one, so the page puts everything from the API in with `textContent`; script-src is `'self'` only.
- **Checked** in Chrome against `wrangler dev`: signing in (wrong token refused, other site 403), the cookie's flags and that the page's script can't read it, a notice with HTML shown as text, Close off with no email or another one (Enter included), closing, the notice marked, opening again, rejecting a notice, signing out (the API then 401); no console problems. Earlier the same day, `/ops` was changed to count only wrong tokens toward its limit, as the page's own calls went over it (**deployed**, version 1e106578). Tests: 76 in the Workers pool.

**Deployed** to velio.tv (version a0b0e0ed) with `POST /v1/signout`, after applying `0003_notices.sql` and `0004_device_signout.sql` to the remote D1. `OPS_TOKEN` was set the same day; `scripts/ops.mjs notices` works against velio.tv, and a missing or wrong token gets 401.

### A TV signing itself out (2026-11-10)

Added for VelioTV's Settings → Account (`../VelioTV/SYNC-PLAN.md`, step 3): `POST /v1/signout` with the device token answers 204. The token is replaced in D1 with a value no SHA-256 matches, `signed_out_at` is set (migration `0004_device_signout.sql`), and the TV's sockets get `removed` and close 4401. A second call, or an unknown token, answers 401. Closed accounts are included, so a TV signed out there doesn't come back if the account is opened again.

The TV stays in the signed manifest, with its boxed profile keys, as only the owner can sign a change. The TVs page and the account page show it as "Signed out on the TV", and the TVs page offers only Remove for it, which re-keys its profiles as before. The TV itself forgets its keys, so the copies boxed to it open nothing.

Tests: 73 in the Workers pool (3 new: the token and socket, the pages, the token check, the CORS preflight). In Chrome against `wrangler dev`, with two TVs simulated in Node: the TV signed out, its socket closed with 4401, its pull answered 401; the TVs page showed it signed out with only Remove; removing it worked, and the other TV opened its new keys. No console problems.

**Deployed** to velio.tv with Phase 8 (version a0b0e0ed). On velio.tv, `POST /v1/signout` answers 401 without a token or with a wrong one, and its CORS preflight 204.

### Stremio add-ons as lists (2026-11-12)

VelioTV can show a Stremio add-on's films and series (VelioTV's PLAN.md, "Stremio add-ons"), so a list can now be a third kind, `"stremio"`, whose `url` is the add-on's manifest. On the Lists page, the list form has "Stremio add-on" beside M3U and Xtream, with an "Add-on address" field; an install link (`stremio://…`) means `https://`, and `/manifest.json` is added when it's missing (`addonManifestUrl()`). An add-on has no guide or refresh choice. The server stores lists encrypted, so it needed no change. An app built before this can't open a pull with an add-on in it (`readList()` refuses the kind); none is released.

**Deployed** to velio.tv (version 9a41af31), with no D1 migration. `pnpm check` passed (100 tests in `sync-core`, 76 in the Workers pool), and velio.tv's `vault.js` has the new choice.

## Moving off Cloudflare (2026-11-13)

The user's choice: the server becomes **self-hostable**, written in **Rust**, with **SQLite** and the same **HTMX 4** pages, on the same design. Lists, profiles and keys stay end-to-end encrypted, and the server still holds only ciphertext, public keys and hashes. Anyone can run it for their own household; velio.tv becomes one install of it.

Decided with the user, 2026-11-13:

- **Built alongside, then the Worker deleted.** The Rust server lives in `apps/server`. velio.tv stays on the Worker until the Rust server passes the same tests, then velio.tv moves and `apps/sync` is removed. (The user chose to remove `apps/sync` first, 2026-11-14: see "`apps/sync` deleted".)
- **Starting fresh.** velio.tv holds only test accounts (no real TV is signed in, as VelioTV's half isn't released), so nothing is migrated from D1 or the Durable Objects.
- **Email over SMTP or Resend**, whichever is configured. **Turnstile is optional**: on when its keys are set (velio.tv sets them), otherwise sign-up and resets rely on the rate limits alone.
- ~~velio.tv runs on a VPS behind Caddy.~~ Changed 2026-11-14: **the server ships as a distroless container image**, and whoever runs it picks their own reverse proxy. velio.tv runs it behind a **Cloudflare Tunnel**, with its DNS staying at Cloudflare. **Backups:** a nightly copy, kept 30 days on the host and also in S3-compatible storage. Which machine runs velio.tv's container isn't chosen yet.

### What stays, and what changes

**Unchanged:** everything the browser and the TVs see. That is `sync-core` (still TypeScript, as browsers and TVs run it), `vault.js`, `ops.js`, `app.css`, pinned HTMX 4.0.1, the page markup `vault.js` hooks into, every route and its answers, the `/v1` JSON, the WebSocket protocol (`events.ts`), the cookies, the headers and the CSP. A TV or browser can't tell which server it talks to. VelioTV's `SYNC-PLAN.md` needs no change beyond the server's address being configurable.

**Rewritten in Rust:** what runs on the server.

- The Worker (`worker.tsx`, `routes/`, `lib/`), and the views (`views/*.tsx`), the same markup.
- The Durable Object (`account.ts`): the SQLite tables, and holding the TVs' sockets.
- The server's half of `sync-core`: `readChange()`, `readStoredState()`, `readManifest()`, `storedManifest()`, `checkMatches()`, `checkRekeyed()`, `applyChange()` and `pullFor()`, with their limits, sizes and id patterns, plus the pairing id pattern and the events constants.

### Stack

| Part              | Choice                                                                                                                          | Why                                                                                                                                                                                                                                                         |
| ----------------- | ------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Language          | Rust, stable                                                                                                                    | The user's choice. One static binary is easy to self-host                                                                                                                                                                                                   |
| HTTP              | `axum` on `tokio`, with `tower-http` for body limits and headers                                                                | Routes, WebSockets and long polls in one place                                                                                                                                                                                                              |
| HTML              | `maud`                                                                                                                          | Checked at compile time and escapes by default, like `hono/jsx`. Fragments are plain functions, as the JSX components were                                                                                                                                  |
| Database          | SQLite through `rusqlite` (bundled), WAL, one file                                                                              | One writer: each change runs in a `BEGIN IMMEDIATE` transaction, which keeps changes strictly ordered, as one Durable Object per account did                                                                                                                |
| Signatures        | `ed25519-dalek`, `verify_strict`                                                                                                | The server only checks the owner's signatures. Strict checking refuses anything TweetNaCl on a TV might refuse                                                                                                                                              |
| Hashes and tokens | `sha2`, `hmac`, `subtle`, `getrandom`; `base64` (standard, padded)                                                              | The same SHA-256 hashes, HMAC'd made-up salts, constant-time comparisons and token sizes as now                                                                                                                                                             |
| Email             | `lettre` for SMTP, or Resend's HTTP API through `reqwest` (rustls); the dev outbox                                              | Whichever is configured. `DEV_MODE` writes to `dev_outbox`, as now                                                                                                                                                                                          |
| Bot check         | Turnstile, optional                                                                                                             | On only when `TURNSTILE_SITE_KEY` and `TURNSTILE_SECRET` are set                                                                                                                                                                                            |
| Rate limits       | In memory, fixed windows, the same limits and keys                                                                              | One process, so no shared store is needed. A restart forgets the counts, which only errs toward a fresh minute                                                                                                                                              |
| Live updates      | `axum` WebSockets; a registry of each account's sockets                                                                         | Sends `rev N`, `removed` and the close codes as the Durable Object did. Pings are answered without touching the database                                                                                                                                    |
| Daily clean-up    | A task in the process, at 04:17 UTC                                                                                             | Replaces the Worker's cron                                                                                                                                                                                                                                  |
| Static files      | Built by `scripts/build-client.mjs` as now, then embedded in the binary (`rust-embed`)                                          | One file to ship. `Cache-Control: no-cache` with an ETag, as now                                                                                                                                                                                            |
| TLS               | A reverse proxy in front, the operator's choice; the server speaks plain HTTP                                                   | Certificates are the proxy's job. velio.tv's is a Cloudflare Tunnel, so its edge and certificates stay those Phase 0 measured. A self-hoster on Caddy can choose an RSA key and the ISRG Root X1 chain (`key_type rsa2048`, `preferred_chains`) for old TVs |
| Tooling           | `cargo fmt`, `cargo clippy -D warnings`, `cargo test`, added to `pnpm check`                                                    | One command checks both halves                                                                                                                                                                                                                              |
| Shipping          | A distroless image (`gcr.io/distroless/static-debian13:nonroot`) holding one static musl binary, built by the root `Dockerfile` | No shell or package manager, and not root. As there's no cron, the binary makes the backups and answers the health check itself                                                                                                                             |

### Data

One SQLite file (`velio.db`). The D1 tables keep their columns (`accounts`, `sessions`, `email_links`, `devices`, `pairings`, `notices`, `dev_outbox`) in one fresh migration, `0001_init.sql`, as there is nothing to migrate. Each Durable Object's tables become shared tables keyed by account, prefixed `vault_` so the manifest's TVs don't clash with the signed-in TVs in `devices`:

| Table               | Key                      | Holds                                                                |
| ------------------- | ------------------------ | -------------------------------------------------------------------- |
| `vault_owner`       | account                  | The owner bundle, the signing and box public keys, updated           |
| `vault_recovery`    | account                  | The SHA-256 of the recovery auth key, and the bundle sealed under it |
| `vault_manifest`    | account                  | The current signed manifest (body and signature)                     |
| `vault_profiles`    | account, profile         | rev, the sealed record, the profile key under the account key        |
| `vault_lists`       | account, list            | rev, keyRev, the sealed record, the list key under the account key   |
| `vault_list_keys`   | account, list, profile   | The list key under that profile's key                                |
| `vault_devices`     | account, device          | A TV the manifest names                                              |
| `vault_device_keys` | account, device, profile | The profile key boxed for that TV                                    |

Every `vault_` table references `accounts` with `ON DELETE CASCADE`, and `PRAGMA foreign_keys = ON`. What used to be two steps (the Durable Object, then D1) is now one transaction. So a reset, a deletion or a purge either happens whole or not at all, and the "if D1 failed after the object" cases in Phases 6 and 8 go away.

Migrations live in `apps/server/migrations/` and run at startup, in order, recorded in `PRAGMA user_version`.

Kept small (`0004_less_storage.sql`, `src/packed.rs`). Ciphertext is stored as its bytes rather than as base64 in JSON: a sealed value is the byte 1, the nonce and the box, and a signed manifest is the byte 2, the signature and the body. Anything that wouldn't read back exactly stays as JSON text. SHA-256 hashes are stored as 32 bytes instead of 64 hex characters. The small key tables (`vault_list_keys`, `vault_devices`, `vault_device_keys`) have no rowid, so their key is stored once. Auto-vacuum is incremental, and the clean-up gives freed pages back to the disk. The write-ahead log keeps at most 4 MiB between checkpoints. On 2,000 made-up accounts in the old format, the file went from 141 MB to 95 MB, and the API and sync-core see the same JSON as before.

### Configuration

Environment variables (`--env-file` for the container). README.md lists them for operators:

| Variable                                                                 | Needed          | Meaning                                                                                                                                   |
| ------------------------------------------------------------------------ | --------------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| `VELIO_SITE_URL`                                                         | yes             | Such as `https://velio.tv`. Used for the Origin check, links in emails, and the QR address                                                |
| `VELIO_LISTEN`                                                           | no              | Default `127.0.0.1:8787`; the image sets `0.0.0.0:8787`                                                                                   |
| `VELIO_DATABASE`                                                         | no              | Default `./velio.db`; the image sets `/data/velio.db`                                                                                     |
| `VELIO_TRUSTED_PROXY`                                                    | no              | The proxy's addresses or ranges, comma-separated. From them, the client IP comes from `X-Forwarded-For`; otherwise from the socket        |
| `VELIO_BACKUP_DIR`                                                       | no              | Nightly copies, kept 30 days; the image sets `/data/backups`. Unset, there are no backups                                                 |
| `VELIO_BACKUP_S3_URL`, `_REGION`, `_ACCESS_KEY_ID`, `_SECRET_ACCESS_KEY` | no              | A second copy in S3-compatible storage, path-style, signed with Signature Version 4. The bucket's lifecycle rule deletes it after 30 days |
| `VELIO_NETWORK_PROVIDER`                                                 | no              | Who carries traffic and ends HTTPS, such as `Cloudflare`, named on `/privacy` and `/security`                                             |
| `PRELOGIN_SECRET`                                                        | yes             | As now: the HMAC key for made-up salts                                                                                                    |
| `OPS_TOKEN`                                                              | no              | As now; `/ops` is a 404 without it                                                                                                        |
| `EMAIL_FROM`, `ABUSE_EMAIL`                                              | yes             | As now                                                                                                                                    |
| `SMTP_URL` or `RESEND_API_KEY`                                           | one, unless dev | Such as `smtps://user:pass@mail.example:465`. With neither, the server refuses to start (outside `DEV_MODE`)                              |
| `TURNSTILE_SITE_KEY`, `TURNSTILE_SECRET`                                 | no              | Both or neither. Neither turns Turnstile off: the widget and its CSP entry go too                                                         |
| `VELIO_TV_LIMIT`, `VELIO_TV_LIMIT_MAX`                                   | no              | Default 10 and 50: the TVs each account has, and the most `/ops` can give one ("TV limits")                                               |
| `DEV_MODE`                                                               | no              | As now: the dev outbox, `/dev/outbox`, and Turnstile's test keys if it's on                                                               |

The server refuses to start with a missing or malformed setting, rather than fail on the first request.

### Checking it matches

Three layers, so the port is exact rather than close:

1. **Vectors from `sync-core`.** A script (`packages/sync-core/scripts/vectors.ts`) uses the TypeScript code as the reference. It builds accounts and changes, then writes each case as JSON with the state before, the input, and the expected result: the state after, or the problem (`behind`, `refused` or `limit`). The cases cover accepted changes of every kind, every refusal in `readChange()`, `checkMatches()` and `checkRekeyed()`, the limits, bad sizes and bad base64, plus `pullFor()` for TVs with different profiles. Rust's tests replay them, and `pnpm check` regenerates and compares them, so a change on either side that isn't made on both fails.
2. **The Worker tests, over HTTP.** The 76 tests stay in `apps/sync/test` until R6, unchanged, and run twice: against the Worker (`vitest.config.ts`) and against the Rust server (`vitest.server.config.ts`, `pnpm --filter @velio/sync test:server`). The second config points `cloudflare:test` and `cloudflare:workers` at a stand-in (`test/server/runtime.ts`): its `fetch` sends each request to the server binary, which `test/server/globalSetup.ts` builds and starts on a free port with a temporary database and `DEV_MODE`, with `X-Forwarded-For` in place of `cf-connecting-ip`; an upgrade opens a real WebSocket. Its `env.DB` reads and writes the temporary database with `node:sqlite`. Each phase adds its files to the config's list, and `pending` names the tests that wait for a later phase. When all of them pass, the routes, answers, cookies and headers match. Since 2026-11-14 they are `apps/server/e2e`, without the Worker config.
3. **Rust unit tests** for what the vectors don't reach: the rate limiter, sessions, the socket registry, configuration, and the clean-up's times.

Then the same checks by hand as in Phases 1 to 8, in Chrome against the local server with a TV simulated in Node. Nothing sent may hold a password, a profile name or a list address.

### Differences, on purpose

- **The probe** (`/v1/probe`) reported TLS details from Cloudflare's `cf` object. Now it reads `X-Velio-TLS-Version`, `X-Velio-TLS-Cipher` and `X-Velio-HTTP-Protocol`, set by the proxy, and believes them only from `VELIO_TRUSTED_PROXY`. On velio.tv a request-header Transform Rule sets them from `cf.tls_version`, `cf.tls_cipher` and `http.request.version`; Caddy sets them from `{http.request.tls.version}`, `{http.request.tls.cipher_suite}` and `{http.request.proto}` (README.md). Without them, the TLS details are `null` and the HTTP version is the one the server saw. `colo` is always `null`.
- **The CSP without Turnstile** names no `challenges.cloudflare.com`, and has `frame-src 'none'`.
- **Logs and backups.** Logs go to stdout, and the host keeps them 7 days (journald with `MaxRetentionSec=7d`). Each night at 04:17 UTC, after the clean-up, the server copies the database with `VACUUM INTO` to `VELIO_BACKUP_DIR`, keeps 30 days of copies, and puts tonight's in S3 when that's set; the bucket's lifecycle rule deletes it after 30 days. `velio-server backup` makes one at once. The copies hold ciphertext for every list, but also the accounts' email addresses.
- **Who handles what follows the settings.** `/privacy` and `/security` name the network provider (`VELIO_NETWORK_PROVIDER`), Resend or "an email provider", Turnstile and the backups only when they're configured, so they stay true on any install. The terms didn't name Cloudflare and don't change. On velio.tv Cloudflare stays a processor: it carries the traffic and ends HTTPS, as it did for the Worker, and runs Turnstile; it no longer holds the data.
- **The clean-up runs at 04:17 UTC**, and once at start, where R2's ran every 24 hours from start.
- **`/healthz`** answers `ok` once the database does, for the image's health check (`velio-server health`), and isn't logged when it's fine.
- **Sockets across restarts.** A restart closes every TV's socket. TVs reconnect with `reconnectDelay()`, already spread out so they don't all come back at once, and catch up from the `rev N` sent on connecting. A Durable Object's sockets survived a deploy; these don't.
- **Ten-millisecond CPU limits are gone.** Nothing needs the test account to be precomputed any more, but it stays a fixture, as VelioTV's probe uses the same file.

### TVs' names sealed (2026-11-14)

The user's choice: the server no longer knows what a TV is called. The name the phone opens from the TV's sealed details goes into the TV's manifest entry, sealed under the account key (`{ format: 1, name }`, padded to 256 bytes) and sealed again on every change, so the server can't read it, tell when it changes, or move it to another TV, and a TV can't read the others' names. `openState` opens it; `vault.js` fills `[data-tv-name]` on the TVs page and the signed-in message. The account page, which doesn't carry the sealed state, shows only how many TVs there are. Both servers ignore a plain `name` in `POST /v1/pair`, which older VelioTV builds still send, and drop the columns (`0002_sealed_device_names.sql` here, `0005_sealed_device_names.sql` on D1). A TV signed in before has no name and shows as "TV".

### TV limits (2026-11-14)

The user's choice, in place of a "public" mode of 10 TVs and a "private" one without a limit: every account has `VELIO_TV_LIMIT` TVs (default 10), and the operator can give one another limit on `/ops` (**Accounts**, or `ops.mjs tv-limit`), from 1 to `VELIO_TV_LIMIT_MAX` (default 50). `accounts.tv_limit` keeps it (`0003_tv_limits.sql`); NULL is the default, and giving the default stores NULL, so a later `VELIO_TV_LIMIT` holds for the account too. A limit that `VELIO_TV_LIMIT_MAX` has since gone below is held to it.

- **The ceiling.** `MAX_DEVICES` in sync-core, now 50, is what any manifest may name: what still fits the manifest's 64,000 characters with 8 profiles, 50 lists and every TV on every profile (about 748 characters a TV entry; a test builds that account). `DEFAULT_DEVICE_LIMIT` is 10.
- **Where it's checked.** `applyChange()` takes the account's limit (`deviceLimit`, default 10) and refuses, as `limit`, a change that would leave more TVs than that _and_ more than before. So a limit lowered below the TVs an account has keeps them and every other change working, and stops new ones until it's under. `withDevice()` takes it too, from the pairing form's `data-tv-limit`, so the phone says so before it signs anything. The vectors cover a TV past the limit, one within a limit given, and a change to an account over its limit.
- **The Worker** kept 10, as it passed no limit.
- **VelioTV** has to be built with this sync-core before any account goes past 10 TVs: older builds' `openPull()` refuses a manifest naming more than 10.
- `/terms` says the default, from `VELIO_TV_LIMIT`.

### Phases

| #   | Phase           | Main work                                                                                                                                                                                                                                 | Done when                                                                                                                                                                                                       |
| --- | --------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| R1  | Skeleton        | `apps/server`: configuration, the database and migrations, headers and CSP, body limits, client IP, rate limits, embedded static files, the layout and the public pages, `/v1/probe`; the test harness over HTTP; `cargo` in `pnpm check` | `hardening.test.ts` and `probe.test.ts` pass against the Rust server                                                                                                                                            |
| R2  | Accounts        | Sign-up, confirming, `/prelogin`, sign-in, sessions, resets, mail (SMTP, Resend, dev outbox), optional Turnstile, the daily clean-up                                                                                                      | `accounts.test.ts` passes; sign-up to sign-in in Chrome with the real Argon2id, the password in no request                                                                                                      |
| R3  | Vault           | The Rust port of the server's half of `sync-core`, and the vectors; `/profiles`, `/lists`, `/lists/:id`, `POST /changes`, `POST /unlock`                                                                                                  | Every vector passes in Rust; `vault.test.ts` passes; a dump of the database after adding lists holds no name, host, username or password                                                                        |
| R4  | TVs             | Pairing (`/pair*`, `/v1/pair*`), `/v1/sync`, `/v1/events` with the socket registry, `/v1/signout`, `/devices`, the settings (password, recovery key, deleting the account)                                                                | `pair.test.ts`, `signout.test.ts` and `settings.test.ts` pass; the Phase 6 run in Chrome passes with two TVs simulated in Node                                                                                  |
| R5  | Notices and ops | `/abuse`, notices, the `/ops` API and page, closing, opening, the 30-day purge; `scripts/ops.mjs` unchanged                                                                                                                               | `notices.test.ts` passes, so all 76 pass; the Phase 8 run passes, with `ops.mjs` against the local server                                                                                                       |
| R6  | Moving velio.tv | The distroless image, backups in the server, proxy ranges, `/healthz`; `/privacy` and `/security` from the settings; README; running the image behind a Cloudflare Tunnel and pointing velio.tv at it                                     | velio.tv answers through the tunnel with the same headers; the full Chrome run passes there; the TV probe reaches it over HTTPS. Then `apps/sync` and the Cloudflare D1, Durable Objects and Worker are deleted |

**R1 done, 2026-11-13.** `apps/server` (axum 0.8, maud, rusqlite) serves the public pages, `/static` (embedded from `apps/sync/public/static` until R6, with ETags), `/v1/probe*` and the not-found answers, with the Worker's headers, CSP, body caps, CORS, Origin check and error answers. One SQLite file, migrations counted in `PRAGMA user_version`; rate limits in memory. `probe.test.ts` and `hardening.test.ts` pass against it, except the pairing-poll test (R4). `pnpm check` now also runs `check:server`: `cargo fmt --check`, `clippy -D warnings`, `cargo test` and `test:server`. A request for a known path with another method answers 404, as the Worker did, not 405.

**R2 done, 2026-11-13.** Sign-up to sign-in worked in Chrome on `http://localhost:8787` with the real Argon2id. Sign-up, confirming (POST only), `/prelogin`, sign-in, `__Host-velio` sessions (30 days, renewed daily), sign-out, `/account`, resets, `/dev/outbox`, Turnstile and the daily clean-up are ported with the Worker's wording, status codes and rate-limit keys. `accounts.test.ts` passes against the Rust server, alongside probe and hardening. The differences from the Worker:

- Mail links are full URLs from `VELIO_SITE_URL`, even in DEV_MODE, where the Worker used paths.
- SMTP and Resend mail is sent after the answer, and failures are logged. The dev outbox is written before the answer.
- Turnstile off (no keys) or DEV_MODE means no check.
- The clean-up runs at start and then every 24 hours. `velio-server clean-up [--now MS]` runs it once and prints how many unconfirmed accounts went.
- A form post counts as from this site when the browser sends `Sec-Fetch-Site: same-origin`, and only otherwise by `Origin`. In Chrome the Origin-only check refused sign-up: with `Referrer-Policy: no-referrer` a browser may send `Origin: null`, and the page may be open on another name for the server.
- A body over the cap is read and dropped, up to 16 MiB or 10 s, before the 413, so the client reads the answer and not a broken pipe.
- Test harness:
  - The test files run one at a time, as they share one database and the clean-up test moves the clock 8 days on.
  - `test/server/account.ts` and `cleanup.ts` stand in for the Worker modules `accounts.test.ts` imports.
  - The runtime turns the tests' made-up addresses, such as `10.1.2.3.4`, into IPv6 ones.

**R3 done, 2026-11-13.** `apps/server/src/sync_core` ports the server's half of `sync-core`: reading changes and manifests, checking the owner's Ed25519 signature, `applyChange()` with its refusals in the same words, `storedManifest()` and `pullFor()`. `vault.rs` keeps the account's records in the `vault_*` tables, one `BEGIN IMMEDIATE` transaction a change, and reads them back as the Durable Object held them. `/profiles`, `/lists`, `/lists/new`, `/lists/:id`, `/devices`, `POST /changes` and `POST /unlock` are ported with the Worker's markup, wording and status codes. Done when:

- **The vectors.** `scripts/vectors.ts` writes `apps/server/tests/vectors.json`: 111 changes (the accepted chain from an empty account to TVs given and taken, and every refusal), 18 pulls and 19 states. They're made with a seeded PRNG, so the file is the same each run. `tests/vectors.rs` replays them, and every one comes out as in TypeScript. The script runs as a vitest test (`test/vectors.test.ts`), so `pnpm check` fails when the file is out of date, and `pnpm --filter @velio/sync-core vectors` writes it. It runs in vitest because Node's type stripping can't load `changes.ts`.
- **`vault.test.ts`** passes against the Rust server. The stand-in `account.ts` gained `getState()` and `reset()`, and `runtime.ts` gained `runInDurableObject`, whose `storage.sql` answers with the account's rows in the `vault_*` tables.
- **The dump.** A unit test in `vault.rs` replays the accepted chain, checks that each state reads back exactly, then dumps every table as text. It holds no profile name, list name, host, token, username or password.

The differences from the Worker:

- Ed25519 signatures are checked with `verify_strict`, which also refuses some malformed signatures that TweetNaCl takes. A real owner never makes one.
- `serde_json` reads a few inputs differently from `JSON.parse`: lone surrogates in strings, nesting deeper than 128, and numbers such as `1e400`. Each is refused, where the Worker might have read it. No change vault.js builds has them.
- A stored manifest that can't be read is a refusal, where the Durable Object threw and the Worker answered 500.
- The account's TVs aren't told of a change yet (`TODO(R4)`, with the socket registry).

**R4's tests pass, 2026-11-14; the Chrome run is next.** `routes/tv.rs` ports `/v1/pair`, `/v1/pair/{id}` (the 20 s wait, polled once a second), `/v1/sync` (ETags and 304s), `/v1/signout` and `/v1/events`. `routes/pair.rs` ports `/pair`, `/pair/find` and `/pair/{id}`, and signing in with `next=/pair` goes on to the pairing page. `routes/settings.rs` ports the password, the recovery key, recovering from the reset link and deleting the account. `events.rs` holds the socket registry, in memory: each account's open sockets, two per TV at most, sent "rev N" after each change and closed with 4401 when a TV is removed, signs out, or the account is reset or deleted. All 67 server tests pass (`pair`, `signout` and `settings` added, and the pairing-poll test in `hardening` is no longer pending), five runs in a row. The stand-in `account.ts` gained `recoveryMadeAt()`, and `runtime.ts` returns the protocol the server picked on a 101.

The differences from the Worker:

- A socket joins the registry before the 101 goes out, as the Durable Object accepted its sockets. Its first "rev N" checks the token again, so a signout or change between the check and the upgrade isn't missed. Without this, a signout right after connecting could leave the socket open.
- A password change or a recovery sets the owner bundle, the credentials and the new session in one transaction, where the Worker wrote the Durable Object first and put the old bundle back if D1 failed.
- Signing in a TV claims the pairing, applies the change and stores the TV's token in one database call. If the change is refused, the claim is undone, as on the Worker.
- Open sockets live only as long as the process. After a restart, TVs reconnect and catch up from the first "rev N".

**R5's tests pass, 2026-11-14; the Phase 8 run with `ops.mjs` is left for the end, with R4's Chrome run.** `routes/notices.rs` ports `/abuse`, with `notices.rs` reading and keeping a notice. `routes/ops.rs` ports the `/ops` page, its sign-in and the API: the bearer token, or the page's session cookie (`ops_session.rs`), which needs this site's origin on anything but GET. `closing.rs` closes and opens accounts. Closing marks the notice acted on, ends the account's sessions, links, pairings and open sockets, and mails the owner. The daily clean-up now deletes the data of accounts closed for more than 30 days. All 76 server tests pass, three runs in a row. The stand-in `closing.ts` gives the tests `CLOSED_KEEP_MS`.

The differences from the Worker:

- Closing, marking the notice and ending the sessions, links and pairings happen in one transaction, where D1 ran them as one batch.
- A change through the cookie is judged as the web forms are, by Sec-Fetch-Site first, then Origin. The Worker checked Origin only.
- The purge runs inside the clean-up's transaction. `velio-server clean-up` still prints only the count of unconfirmed accounts deleted; how many closed accounts' data went is written to stderr.

**R4's and R5's manual runs pass, 2026-11-14,** in Chrome against the dev image (`velio-server:dev`, DEV_MODE, on `localhost:8788`), with three TVs simulated in Node.

- **Phase 6:**
  - Pairing worked by QR, and by a code typed in lower case with spaces.
  - Each change reached the TVs as "rev N" on the socket, and ping got pong.
  - Taking the Kids profile from Living room TV, and renaming the Kids list: Kids TV read the new name, and none of the earlier profile or list keys opened it.
  - Removing a TV sent it `removed`, closed its socket with 4401 and answered its pull with 401, while the other TV carried on.
  - The recovery key, a password change (a wrong current password refused; the TVs stay signed in), a reset with the recovery key typed in lower case with spaces, and deleting the account (its TV got `removed`, 4401 and 401, and signing in failed after).
  - No password or recovery key was in any request, and there were no console errors.
- **Phase 8:**
  - `/abuse` refused a notice without the work, then without the statements, and kept what was typed. The full notice was mailed to the abuse address, with the account it named found and its HTML escaped.
  - `ops.mjs`:
    - `notices`, `notice`, `account`, `close` and `reopen` all worked.
    - A wrong token got 401, a wrong `--confirm` changed nothing, and a second `reopen` got 409.
    - Closing sent the TV `removed`, then 4401 and 401, marked the notice actioned, ended the owner's browser session and mailed them the reason. The email on reopening came too.
  - `/ops` page:
    - A wrong token got 401.
    - The cookie was as designed: 8 h, `Path=/ops`, HttpOnly, Secure, SameSite=Strict, and the page's script can't read it.
    - A notice's HTML showed as text.
    - Close stayed off with another email or an empty reason. Case and outer spaces in the email were ignored.
    - Closing and opening again both worked.
    - The lookup took the email in any case, and an unknown one said "No such account".
    - Signing out worked, after which the API got 401. No console errors.
- **Found** (none new in the port; the Worker does the same):
  - **Pairing a second TV in the same page view failed** with "This code doesn't open the TV's details". `showPairing()` in `client/vault.ts` cleared the code when `#main .done[role=status]` was there, but the find only swaps `#pair-step`, so the first TV's "signed in" status was still on the page. **Fixed, 2026-11-14:** the status is marked `data-seen` once handled. Checked in Chrome against a debug server: two TVs signed in by code one after the other, without a reload. The Worker on velio.tv still has the old `vault.js` until it's deployed.
  - **The `<title>` is stale after a form post that answers another page:** `/account` after signing in keeps the sign-in page's title, and `/lists` after adding a list keeps "Add a list". These answers are fragments for `#main` with `HX-Push-Url`, as on the Worker, and have no `<title>`. HTMX 4.0.1 sets `document.title` only from a `<title>` in a `<head>` in the response, which it takes out before swapping. **Fixed in the Rust server, 2026-11-14:**
    - `web::page_fragment` puts the page's title in a `<head>` ahead of the fragment. `Web::answer` uses it for HTMX, as do the other fragments that are another page: signing in (to `/account` or `/pair`), the list and profile pages after a change, the pairing page after a TV is signed in, and the account deleted.
    - A unit test checks the markup, and the 76 shared tests still pass.
    - Checked in Chrome: "Your account" after signing in, "Lists" after adding a list, "Account deleted" after deleting it, with one `<title>` on the page each time.
    - The Worker is left as it was.
  - **A closed account can't be told it's closed.** `/prelogin` gives it a made-up salt, so signing in says the email and password don't match, and the "This account is closed." answer can't be reached from the browser. This keeps a closed account from being revealed. **Fixed, 2026-11-14:** `prelogin_for` answers a closed account its own salt until the purge empties it. That reveals nothing, as a made-up salt looks the same; only the right password reaches "This account is closed.", and after the purge it's "don't match" again.
  - **A TV's token works again after its account is reopened.** Closing refuses TVs, and their rows go only in the purge. A real TV forgets its token on the 401, so the email's "TVs have to be signed in again" holds in practice. **Fixed, 2026-11-14:** closing signs every TV out as `POST /v1/signout` does (token replaced, `signed_out_at` set), so they stay signed out after reopening and the owner sees them to remove. The status line now says "Signed out", not "Signed out on the TV", and the reopen email and dialog say the TVs stay signed out.
  - **Closing from an account found through a notice fills in that notice,** so closing an account again overwrote the reason recorded on the already-actioned notice. **Fixed, 2026-11-14:** closing adds its line to the notice's action on a new line; marking a notice by hand still sets it.
  - A password change sends the TVs no "rev N". Nothing they hold changes, so none is needed.

Phase 0's certificate question stays closed for velio.tv: behind the tunnel, TVs meet Cloudflare's edge and certificates, as they did the Worker's. Still run the `sync` probe from the test TV through the tunnel, to check the probe headers and the WebSocket.

**R6, the repo half, 2026-11-14.** The root `Dockerfile` builds the client with pnpm (`node:26-slim`), the server on `rust:1.99-alpine` (musl, so the binary is static), and copies the binary into `gcr.io/distroless/static-debian13:nonroot` with `/data` as a volume. `.dockerignore` lets in only the sources it needs, so `.dev.vars` and `PRIVATE.md` never reach a build. In the server: `backup.rs` (the nightly copy, pruning, and the S3 upload, its signing checked against AWS's documented example), `VELIO_TRUSTED_PROXY` as ranges (a proxy container's address isn't fixed), `/healthz` and `velio-server health`, and `/privacy` and `/security` built from the settings. The tests and the stand-in stay in `apps/sync/test` until `apps/sync` is deleted, as do the client sources the image builds from.

**`apps/sync` deleted, 2026-11-14**, before velio.tv moved, at the user's choice. What the Rust server still used moved to `apps/server` (the package `@velio/server`): the client sources to `client/`, `build-client.mjs` (now writing `public/static` only, without the Workers `_headers`), `ops.mjs` and `make-test-account.ts` to `scripts/`, the probe's test account to `probe/`, and the tests to `e2e/` with `vitest.config.ts`. The two Rust-only files are `.test.ts` now. The tests import their helpers by name (`e2e/support/`: `runtime.ts` for `env.DB` and `exports.default.fetch`, `account.ts`, `cleanup.ts`, `constants.ts`, `opsSession.ts`) instead of through `cloudflare:*` aliases, and the three constants they took from the Worker are copied from the Rust source. `pnpm check` runs `check:rust` (the client build first, as the binary embeds it), then every package's tests: 85 against the server. wrangler, workerd and the Workers vitest pool are gone from the lockfile. The Worker on velio.tv, its D1 database and Durable Objects are untouched, and it can only be redeployed from git history (3176ac7 or earlier).

## Open choices

The ones most likely to change:

1. **Lists added on a TV itself:** kept on that TV for all its profiles (planned), or only for the profile that added them, or offered for upload to the account.
2. **Per-profile data across devices:** favourites and continue watching stay on each TV (planned). Syncing them per profile would fit the same design later, as records under the profile key.
3. **Two-way sync:** the TV only reads (planned).
4. **Recovery key:** off by default (built so in Phase 6), or offered at sign-up.
5. **Live updates:** WebSocket plus polling (planned), or polling alone to start.
6. **Sharing `sync-core`:** a git dependency (planned), or copied files.
7. **Limits per account:** 10 TVs, 8 profiles and 50 lists (planned). Lower keeps it more clearly at household scale; higher suits big families.
