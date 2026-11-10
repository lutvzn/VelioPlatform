// The operator's page at /ops (routes/opsPage.tsx): signing in, and the shell
// that src/client/ops.ts fills from the /ops API. No HTMX or vault.js here.

import type { Child } from "hono/jsx";

const OpsLayout = (props: { title: string; script?: boolean; children: Child }) => (
  <html lang="en">
    <head>
      <meta charset="utf-8" />
      <meta name="viewport" content="width=device-width, initial-scale=1" />
      <meta name="color-scheme" content="light dark" />
      <meta name="robots" content="noindex" />
      {/* The site's no-referrer makes a browser send "Origin: null" with a plain form, which
          sameOriginPosts refuses; same-origin sends velio.tv's Origin and still nothing elsewhere. */}
      <meta name="referrer" content="same-origin" />
      <title>{`${props.title} · Velio operator`}</title>
      <link rel="stylesheet" href="/static/ops.css" />
      {props.script ? <script src="/static/ops.js" defer></script> : null}
    </head>
    <body>{props.children}</body>
  </html>
);

export const OpsSignIn = ({ problem }: { problem?: string }) => (
  <OpsLayout title="Sign in">
    <main class="narrow">
      <h1>Velio operator</h1>
      <p class="note">For handling notices and closing accounts. Sign in with the OPS_TOKEN secret.</p>
      <form method="post" action="/ops" class="stack">
        {/* For password managers, which keep the token under this name. */}
        <input type="text" name="username" value="velio-operator" autocomplete="username" hidden />
        <label>
          Operator token
          <input type="password" name="token" autocomplete="current-password" required autofocus />
        </label>
        {problem ? (
          <p class="problem" role="alert">
            {problem}
          </p>
        ) : null}
        <button type="submit">Sign in</button>
      </form>
      <p class="note">The session lasts 8 hours. Changing the secret ends every session.</p>
    </main>
  </OpsLayout>
);

export const OpsApp = () => (
  <OpsLayout title="Notices" script>
    <header>
      <strong>Velio operator</strong>
      <span class="badge live">LIVE</span>
      <nav>
        <button type="button" data-tab="notices" aria-pressed="true">
          Notices
        </button>
        <button type="button" data-tab="accounts" aria-pressed="false">
          Accounts
        </button>
        <button type="button" id="signout">
          Sign out
        </button>
      </nav>
    </header>
    <p id="status" role="status" hidden></p>

    <main>
      <section id="notices">
        <div class="bar">
          <label>
            Show
            <select id="notice-status">
              <option value="open">Open</option>
              <option value="actioned">Actioned</option>
              <option value="rejected">Rejected</option>
              <option value="all">All</option>
            </select>
          </label>
          <button type="button" id="notice-refresh">
            Refresh
          </button>
        </div>
        <table>
          <thead>
            <tr>
              <th>Reference</th>
              <th>Received</th>
              <th>Kind</th>
              <th>From</th>
              <th>Account named</th>
              <th>Status</th>
            </tr>
          </thead>
          <tbody id="notice-rows"></tbody>
        </table>
        <p id="notice-empty" class="note" hidden>
          No notices.
        </p>
        <article id="notice" class="card" hidden></article>
      </section>

      <section id="accounts" hidden>
        <form id="account-find" class="bar">
          <label>
            Email or account id
            <input id="account-key" type="text" autocomplete="off" spellcheck={false} required />
          </label>
          <button type="submit">Look up</button>
        </form>
        <article id="account" class="card" hidden></article>
      </section>
    </main>

    <dialog id="close-dialog">
      <form method="dialog" id="close-form">
        <h2>
          Close <span data-email></span>?
        </h2>
        <ul class="facts">
          <li>
            Every browser and TV signed in to it is signed out <strong>now</strong>.
          </li>
          <li>The owner is emailed your reason, and where to contest it.</li>
          <li>
            Their lists, profiles and TVs are{" "}
            <strong>
              deleted on <span data-delete-on></span>
            </strong>
            , unless you open the account again before then. After that it can't be undone.
          </li>
          <li>The email address can't sign up again.</li>
        </ul>
        <p class="note" data-account-facts></p>
        <label>
          Reason, which the owner reads
          <textarea id="close-reason" rows={4} maxlength={2000} required></textarea>
        </label>
        <label>
          Notice acted on (optional)
          <input
            id="close-notice"
            type="text"
            autocomplete="off"
            spellcheck={false}
            placeholder="N-XXXXXXXX"
          />
        </label>
        <label>
          Type the account's email to confirm
          <input id="close-confirm" type="text" autocomplete="off" spellcheck={false} />
        </label>
        <p class="problem" role="alert" data-problem></p>
        <div class="actions">
          <button type="button" data-cancel autofocus>
            Cancel
          </button>
          <button type="submit" class="danger" id="close-submit" disabled>
            Close this account
          </button>
        </div>
      </form>
    </dialog>

    <dialog id="reopen-dialog">
      <form method="dialog" id="reopen-form">
        <h2>
          Open <span data-email></span> again?
        </h2>
        <p>
          The owner can sign in again and is emailed. Their lists and profiles come back; TVs have to be
          signed in again.
        </p>
        <p class="problem" role="alert" data-problem></p>
        <div class="actions">
          <button type="button" data-cancel autofocus>
            Cancel
          </button>
          <button type="submit">Open it again</button>
        </div>
      </form>
    </dialog>

    <dialog id="handle-dialog">
      <form method="dialog" id="handle-form">
        <h2>
          Mark notice <span data-notice></span> as <span data-handle-status></span>?
        </h2>
        <p class="note">This doesn't change any account. It records what you did.</p>
        <label>
          What was done
          <textarea id="handle-action" rows={3} maxlength={2000} required></textarea>
        </label>
        <p class="problem" role="alert" data-problem></p>
        <div class="actions">
          <button type="button" data-cancel autofocus>
            Cancel
          </button>
          <button type="submit">Mark it</button>
        </div>
      </form>
    </dialog>
  </OpsLayout>
);
