// The page shell. Every page loads HTMX and vault.js; vault.js does
// everything secret in the browser (PLAN.md, "HTMX and end-to-end encryption").

import type { Child } from "hono/jsx";

// No inline styles from HTMX, so the CSP can keep style-src 'self'.
const HTMX_CONFIG = JSON.stringify({ includeIndicatorCSS: false });

/** data-account-id tells vault.js whose keys this tab may hold; none means signed out. */
export const Nav = ({ accountId, oob }: { accountId?: string; oob?: boolean }) => (
  <nav id="nav" hx-swap-oob={oob ? "true" : undefined} data-account-id={accountId}>
    {accountId ? (
      <>
        <a href="/lists">Lists</a>
        <a href="/profiles">Profiles</a>
        <a href="/devices">TVs</a>
        <a href="/account">Account</a>
        <form method="post" action="/signout" hx-post="/signout">
          <button type="submit" class="link">
            Sign out
          </button>
        </form>
      </>
    ) : (
      <>
        <a href="/signin">Sign in</a>
        <a href="/signup" hx-boost="false">
          Make an account
        </a>
      </>
    )}
  </nav>
);

// Links and forms are boosted, so moving between pages keeps vault.js, and the
// keys it holds, in memory. Pages with Turnstile are opened without boosting,
// as its script is in the head.
const BOOST = { "hx-boost:inherited": "true" };

export const Layout = (props: {
  title: string;
  accountId?: string;
  turnstile?: boolean;
  children: Child;
}) => (
  <html lang="en">
    <head>
      <meta charset="utf-8" />
      <meta name="viewport" content="width=device-width, initial-scale=1" />
      <meta name="htmx-config" content={HTMX_CONFIG} />
      <meta name="color-scheme" content="light dark" />
      <title>{`${props.title} · Velio`}</title>
      <link rel="stylesheet" href="/static/app.css" />
      <script src="/static/htmx.min.js" defer></script>
      <script src="/static/vault.js" defer></script>
      {props.turnstile ? (
        <script src="https://challenges.cloudflare.com/turnstile/v0/api.js" async defer></script>
      ) : null}
    </head>
    <body {...BOOST}>
      <header>
        <a href="/" class="brand">
          Velio
        </a>
        <Nav accountId={props.accountId} />
      </header>
      <main id="main">{props.children}</main>
      <footer>
        <a href="/terms">Terms</a>
        <a href="/privacy">Privacy</a>
        <a href="/security">Security</a>
        <a href="/abuse">Report abuse</a>
      </footer>
    </body>
  </html>
);
