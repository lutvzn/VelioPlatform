// Pages and the fragments HTMX swaps into #main. Neutral wording only:
// "your own IPTV subscriptions", never content or providers (PLAN.md,
// "Legal and abuse", 4).
//
// Forms marked data-velio="…" have hx-confirm, so vault.js gets the request
// before it is sent and puts keys in the body in place of the password.
// Password inputs have no name, so HTMX never sends them.

import { MIN_PASSWORD_LENGTH } from "@velio/sync-core";
import type { OwnerRecord } from "../account.ts";
import { deviceStatus, type DeviceRow } from "../lib/vault.ts";
import { OwnerBundle, RememberChoice, UnlockForm } from "./vault.tsx";

const Problem = () => <p id="form-problem" class="problem" role="alert"></p>;

const Turnstile = ({ siteKey }: { siteKey: string }) =>
  siteKey ? <div class="cf-turnstile" data-sitekey={siteKey} data-size="flexible"></div> : null;

export const Home = ({ signedIn }: { signedIn: boolean }) => (
  <section>
    <h1>Your lists, on every TV</h1>
    <p>
      Velio keeps the M3U playlists and Xtream logins of your own IPTV subscriptions in one account, and every
      VelioTV you sign in gets them. Type a login once here instead of with a remote on each TV.
    </p>
    <p>
      Your lists are encrypted in this browser with your password before they're sent. Velio's servers hold
      only ciphertext: they can't read a playlist address, a server, a username or a password.
    </p>
    <p>
      {signedIn ? (
        <a href="/account">Go to your account</a>
      ) : (
        <a href="/signup" hx-boost="false">
          Make an account
        </a>
      )}
    </p>
  </section>
);

export const SignupForm = ({ siteKey }: { siteKey: string }) => (
  <section>
    <h1>Make an account</h1>
    <form hx-post="/signup" hx-target="#main" hx-confirm="velio" data-velio="signup">
      <label>
        Email
        <input type="email" name="email" autocomplete="username" required maxlength={254} />
      </label>
      <label>
        Password
        <input
          type="password"
          data-password
          autocomplete="new-password"
          required
          minlength={MIN_PASSWORD_LENGTH}
        />
      </label>
      <label>
        Password again
        <input type="password" data-password-again autocomplete="new-password" required />
      </label>
      <p class="note">
        Your password is also the key to your lists. Velio never sees it and can't recover it: if you forget
        it, resetting the account deletes the lists, unless you made a recovery key in your account.
      </p>
      <label class="check">
        <input type="checkbox" name="terms" value="yes" required />
        <span>
          I accept the <a href="/terms">terms</a>, and will add only sources I have the right to use.
        </span>
      </label>
      <Turnstile siteKey={siteKey} />
      <Problem />
      <button type="submit">Make the account</button>
    </form>
    <p>
      Already have one? <a href="/signin">Sign in</a>
    </p>
  </section>
);

export const CheckEmail = ({ email }: { email: string }) => (
  <section>
    <h1>Check your email</h1>
    <p>
      We sent a link to <strong>{email}</strong>. Open it to finish. It works for 24 hours.
    </p>
  </section>
);

export const SigninForm = ({ email, note, next }: { email?: string; note?: string; next?: string }) => (
  <section>
    <h1>Sign in</h1>
    {note ? <p class="note">{note}</p> : null}
    <form hx-post="/signin" hx-target="#main" hx-confirm="velio" data-velio="signin">
      {next ? <input type="hidden" name="next" value={next} /> : null}
      <label>
        Email
        <input type="email" name="email" autocomplete="username" required maxlength={254} value={email} />
      </label>
      <label>
        Password
        <input type="password" data-password autocomplete="current-password" required />
      </label>
      <RememberChoice />
      <Problem />
      <button type="submit">Sign in</button>
    </form>
    <p>
      <a href="/reset" hx-boost="false">
        Forgot the password?
      </a>{" "}
      ·{" "}
      <a href="/signup" hx-boost="false">
        Make an account
      </a>
    </p>
  </section>
);

/** Links are confirmed with a button, so mail scanners that open links don't confirm them. */
export const ConfirmPage = ({ token }: { token: string }) => (
  <section>
    <h1>Confirm your email</h1>
    <form method="post" action={`/confirm/${token}`} hx-post={`/confirm/${token}`} hx-target="#main">
      <button type="submit">Confirm</button>
    </form>
  </section>
);

export const LinkProblem = () => (
  <section>
    <h1>This link doesn't work</h1>
    <p>
      It was already used, or it's too old. <a href="/signin">Sign in</a>, or ask for a new one:{" "}
      <a href="/signup" hx-boost="false">
        make the account again
      </a>{" "}
      or{" "}
      <a href="/reset" hx-boost="false">
        reset it
      </a>
      .
    </p>
  </section>
);

export const ResetRequestForm = ({ siteKey }: { siteKey: string }) => (
  <section>
    <h1>Reset your account</h1>
    <p>
      Velio can't read your lists, so it can't give them back without your password. A reset keeps your email
      and deletes your lists, profiles and signed-in TVs. You choose a new password and add them again.
    </p>
    <form hx-post="/reset" hx-target="#main">
      <label>
        Email
        <input type="email" name="email" autocomplete="username" required maxlength={254} />
      </label>
      <Turnstile siteKey={siteKey} />
      <Problem />
      <button type="submit">Send the reset link</button>
    </form>
  </section>
);

export const ResetSent = ({ email }: { email: string }) => (
  <section>
    <h1>Check your email</h1>
    <p>
      If <strong>{email}</strong> has an account, we sent it a reset link. It works for 1 hour.
    </p>
  </section>
);

/** The reset link's page: with the recovery key, a new password that keeps everything; without it, a reset. */
export const ResetForm = ({ token, recovery }: { token: string; recovery: boolean }) => (
  <section>
    <h1>Choose a new password</h1>
    {recovery ? (
      <>
        <h2>With your recovery key</h2>
        <p>Keeps your lists, profiles and TVs.</p>
        <form
          hx-post={`/reset/${token}/recover`}
          hx-target="#main"
          hx-confirm="velio"
          data-velio="recover"
          data-token={token}
        >
          <label>
            Recovery key
            <input
              type="text"
              data-recovery-key
              autocomplete="off"
              autocapitalize="characters"
              spellcheck={false}
              placeholder="K7QM-2XHF-…"
              required
            />
          </label>
          <NewPassword />
          <p id="recover-problem" class="problem" role="alert"></p>
          <button type="submit">Set the new password</button>
        </form>
        <h2>Without it</h2>
      </>
    ) : null}
    <p class="warning">This deletes your lists, profiles and signed-in TVs. It can't be undone.</p>
    <form hx-post={`/reset/${token}`} hx-target="#main" hx-confirm="velio" data-velio="reset">
      <NewPassword />
      <Problem />
      <button type="submit" class="danger">
        Reset the account
      </button>
    </form>
  </section>
);

const NewPassword = () => (
  <>
    <label>
      New password
      <input
        type="password"
        data-password
        autocomplete="new-password"
        required
        minlength={MIN_PASSWORD_LENGTH}
      />
    </label>
    <label>
      New password again
      <input type="password" data-password-again autocomplete="new-password" required />
    </label>
  </>
);

const CurrentPassword = ({ email, label = "Your password" }: { email: string; label?: string }) => (
  <>
    <input type="email" hidden value={email} autocomplete="username" />
    <label>
      {label}
      <input type="password" data-password-current autocomplete="current-password" required />
    </label>
  </>
);

/**
 * The account page. Straight after signing in it carries the owner bundle,
 * which vault.js opens with the unlock key it kept from the password.
 */
/** A day as 2026-11-10, in UTC. */
const day = (ms: number) => new Date(ms).toISOString().slice(0, 10);

const Devices = ({ devices }: { devices: DeviceRow[] }) => (
  <section>
    <h2>TVs</h2>
    {devices.length === 0 ? (
      <p>No TVs yet.</p>
    ) : (
      <ul class="rows">
        {devices.map(device => (
          <li>
            <div class="row-head">
              <strong data-device-row={device.id}>{device.name}</strong>
              <span class="note">{deviceStatus(device)}</span>
            </div>
          </li>
        ))}
      </ul>
    )}
    <p>
      <a href="/pair" class="button">
        Sign in a TV
      </a>{" "}
      {devices.length > 0 ? <a href="/devices">Change or remove TVs</a> : null}
    </p>
  </section>
);

export interface AccountViewProps {
  email: string;
  owner?: OwnerRecord;
  devices: DeviceRow[];
  /** When the recovery key was made; null when it's off. */
  recoveryMadeAt: number | null;
  /** What was just done, such as "Your password is changed." */
  notice?: string;
  /** Just after a recovery key was made: the place vault.js shows it, once. */
  showRecoveryKey?: boolean;
}

export const AccountView = (props: AccountViewProps) => (
  <>
    <section>
      <h1>Your account</h1>
      {props.notice ? (
        <p class="done" role="status">
          {props.notice}
        </p>
      ) : null}
      <p>
        Signed in as <strong>{props.email}</strong>.
      </p>
      {props.owner ? <OwnerBundle owner={props.owner} /> : null}
      <p>
        <a href="/lists">Your lists</a> · <a href="/profiles">Your profiles</a>
      </p>
    </section>
    {props.showRecoveryKey ? <RecoveryKeyShown /> : null}
    <Devices devices={props.devices} />
    <UnlockForm email={props.email} />
    <section data-needs-keys hidden>
      <p data-vault-status>Your keys are open in this browser.</p>
      <button type="button" class="link" data-forget>
        Lock this browser
      </button>
    </section>
    <PasswordSection email={props.email} />
    <RecoverySection email={props.email} madeAt={props.recoveryMadeAt} />
    <DeleteSection email={props.email} />
  </>
);

const RecoveryKeyShown = () => (
  <section class="recovery-shown" data-recovery-shown>
    <h2>Your recovery key</h2>
    <p>
      <code class="recovery-key" data-recovery-code></code>
    </p>
    <p>
      Write it down or print this page now: it's shown only once. Keep it away from this device. With it and
      your email, a forgotten password keeps your lists; anyone with both can open them.
    </p>
    <p>
      <button type="button" data-print>
        Print
      </button>
    </p>
  </section>
);

const PasswordSection = ({ email }: { email: string }) => (
  <section>
    <h2>Password</h2>
    <p data-locked class="note">
      Unlock your lists above to change the password.
    </p>
    <details data-needs-keys hidden>
      <summary>Change the password</summary>
      <p class="note">Your lists and TVs stay as they are. Other browsers are signed out.</p>
      <form hx-post="/password" hx-target="#main" hx-confirm="velio" data-velio="password" data-email={email}>
        <CurrentPassword email={email} label="Current password" />
        <NewPassword />
        <p id="password-problem" class="problem" role="alert"></p>
        <button type="submit">Change the password</button>
      </form>
    </details>
  </section>
);

const RecoverySection = ({ email, madeAt }: { email: string; madeAt: number | null }) => (
  <section>
    <h2>Recovery key</h2>
    {madeAt === null ? (
      <p>
        Off. Without one, a forgotten password means resetting the account, which deletes your lists and
        profiles.
      </p>
    ) : (
      <p>On, made {day(madeAt)}. With it and the reset email, a forgotten password keeps your lists.</p>
    )}
    <p data-locked class="note">
      Unlock your lists above to make one.
    </p>
    <details data-needs-keys hidden>
      <summary>{madeAt === null ? "Turn it on" : "Make a new one"}</summary>
      {madeAt === null ? null : <p class="note">The old key stops working.</p>}
      <form
        hx-post="/recovery"
        hx-target="#main"
        hx-confirm="velio"
        data-velio="recovery-on"
        data-email={email}
      >
        <input type="hidden" name="action" value="on" />
        <CurrentPassword email={email} />
        <p id="recovery-problem" class="problem" role="alert"></p>
        <button type="submit">Make a recovery key</button>
      </form>
    </details>
    {madeAt === null ? null : (
      <details>
        <summary>Turn it off</summary>
        <form
          hx-post="/recovery"
          hx-target="#main"
          hx-confirm="velio"
          data-velio="recovery-off"
          data-email={email}
        >
          <input type="hidden" name="action" value="off" />
          <CurrentPassword email={email} />
          <p id="recovery-off-problem" class="problem" role="alert"></p>
          <button type="submit">Turn the recovery key off</button>
        </form>
      </details>
    )}
  </section>
);

const DeleteSection = ({ email }: { email: string }) => (
  <section>
    <h2>Delete your account</h2>
    <details>
      <summary>Delete your account</summary>
      <p class="warning">
        This deletes your lists, profiles and keys from Velio, and signs out every TV. It can't be undone.
      </p>
      <form
        hx-post="/account/delete"
        hx-target="#main"
        hx-confirm="velio"
        data-velio="delete-account"
        data-email={email}
      >
        <CurrentPassword email={email} />
        <p id="delete-problem" class="problem" role="alert"></p>
        <button type="submit" class="danger">
          Delete the account
        </button>
      </form>
    </details>
  </section>
);

export const AccountDeleted = () => (
  <section>
    <h1>Your account is deleted</h1>
    <p>Its lists, profiles and keys are gone from Velio, and its TVs are signed out.</p>
    <p>
      <a href="/">Go to the start</a>
    </p>
  </section>
);

export const Terms = ({ abuseEmail }: { abuseEmail: string }) => (
  <section class="prose">
    <h1>Terms</h1>
    <p class="note">Draft. To be reviewed before Velio is open to the public.</p>
    <ol>
      <li>Velio stores and syncs the sources you add, for your own devices. It provides no content.</li>
      <li>Add only sources you have the right to use, such as your own subscriptions.</li>
      <li>
        Your lists are encrypted on your devices. Velio can't see what you store, and can't recover it if you
        lose your password and have no recovery key.
      </li>
      <li>
        An account is for one household: up to 10 TVs, 8 profiles and 50 lists. Lists can't be shared with
        other accounts.
      </li>
      <li>
        Accounts that break these terms can be closed. Accounts named in valid copyright notices more than
        once are closed.
      </li>
      <li>
        When an account is closed, its owner is emailed the reason. Its TVs are signed out, and its encrypted
        data is deleted 30 days later. To contest it, write to{" "}
        <a href={`mailto:${abuseEmail}`}>{abuseEmail}</a> before then.
      </li>
      <li>
        You can delete your account at any time, from <a href="/account">your account</a>. Everything in it is
        deleted at once.
      </li>
      <li>
        To report abuse or copyright infringement, use <a href="/abuse">the form</a> or write to{" "}
        <a href={`mailto:${abuseEmail}`}>{abuseEmail}</a>.
      </li>
    </ol>
  </section>
);

export const Privacy = () => (
  <section class="prose">
    <h1>Privacy</h1>
    <p class="note">Draft. To be reviewed before Velio is open to the public.</p>
    <h2>What Velio can't read</h2>
    <p>
      Your lists, profile names, PINs and keys are encrypted on your devices before they're sent. Velio can't
      read them, and neither can anyone who gets a copy of its databases. See{" "}
      <a href="/security">how Velio protects your lists</a>.
    </p>
    <h2>What Velio holds</h2>
    <ul>
      <li>
        Your email address, when you made the account and accepted the terms, the salt and settings your
        password is turned into keys with, and a hash of your sign-in key.
      </li>
      <li>Your sessions in browsers, and your TVs' names and when they last synced.</li>
      <li>How many profiles and lists you have, and when they changed.</li>
      <li>If you made a recovery key: a copy of your keys sealed under it, and a hash of its sign-in key.</li>
      <li>
        If you send a notice: what you write in the form, kept as a record of the notice and what was done.
      </li>
    </ul>
    <h2>How long</h2>
    <ul>
      <li>An account never confirmed is deleted after 7 days.</li>
      <li>Deleting your account deletes all of it at once.</li>
      <li>
        A closed account's data is deleted 30 days after it's closed. Its email address, the dates and the
        reason are kept, so it can't simply be made again.
      </li>
      <li>Request logs, which can include IP addresses, are kept for at most 7 days.</li>
      <li>
        Backups that Cloudflare keeps for recovering the databases can hold deleted data for up to 30 days
        more.
      </li>
    </ul>
    <h2>Who handles it</h2>
    <p>
      Velio runs on Cloudflare, which hosts the service and its databases, sends its email, and runs the check
      that you're a person on the sign-up, reset and report forms. Email is sent only for your account:
      confirming it, resetting it, and telling you if it's closed. Nothing is sold or used for ads.
    </p>
  </section>
);

/** How Velio protects the lists, for owners (Phase 7). PLAN.md has the details. */
export const Security = () => (
  <section class="prose">
    <h1>How Velio protects your lists</h1>
    <p>
      Your lists hold addresses and logins that are yours. Velio keeps them so that your TVs can have them,
      without being able to read them.
    </p>

    <h2>Your password</h2>
    <p>
      Your browser turns your password into two keys, with Argon2id (64 MiB, 3 passes) and a salt of your own.
      One key signs you in: Velio keeps only a hash of it. The other opens your account's keys, which Velio
      keeps sealed under it. Your password and that second key never leave your browser.
    </p>

    <h2>Your lists and profiles</h2>
    <p>
      Each list and each profile is encrypted in your browser with a key of its own (XSalsa20-Poly1305),
      before it's sent. Your browser signs the account's list of what exists (Ed25519), so a TV or another
      browser can tell if anything was changed, added, dropped or swapped on the way.
    </p>
    <p>
      This browser also remembers the newest version of your account it has opened, and refuses an older one.
      That stops a server from quietly undoing a change, such as bringing back a TV you removed. A browser
      that has never opened your account can't tell, so make changes from one you use often.
    </p>

    <h2>Your TVs</h2>
    <p>
      A TV makes its own keys, and gets only the keys of the profiles you give it. The QR code it shows
      carries a secret that goes from the TV to your phone without passing through Velio, so Velio can't slip
      in keys of its own. The code you can type instead is shorter: a hostile server could work it out in
      days, though not within the 10 minutes it lasts. Scan the QR code when you can.
    </p>
    <p>
      When you remove a TV or take a profile from it, your browser makes new keys for what it could read, so
      it can't read anything you change after that. It may still have what it had before.
    </p>

    <h2>A forgotten password</h2>
    <p>
      Velio can't open your lists, so it can't give them back. Without a recovery key, resetting the password
      deletes your lists and profiles. With a <a href="/account">recovery key</a> and the reset email, you
      keep everything. Keep the recovery key away from the devices you use Velio on.
    </p>

    <h2>What Velio can still do</h2>
    <p>
      Velio sends the page your browser runs. If Velio itself were taken over, the page could be changed to
      send your password elsewhere: that's true of any service you use in a browser. Pages run only code from
      velio.tv, plus Cloudflare's Turnstile check on the sign-up and reset pages, under a strict
      Content-Security-Policy, and only over HTTPS.
    </p>
    <p>
      Velio knows your email address, your TVs' names, how many profiles and lists you have, and when things
      happen. See <a href="/privacy">Privacy</a>.
    </p>

    <h2>A copy for you</h2>
    <p>
      On <a href="/lists">Your lists</a>, you can download your lists and profiles as a file. Your browser
      makes it from what it has opened. It isn't encrypted, so keep it safe.
    </p>
  </section>
);

// --- Notices (Phase 8) ---

export const AbuseForm = ({ siteKey, abuseEmail }: { siteKey: string; abuseEmail: string }) => (
  <section class="prose">
    <h1>Report abuse or copyright infringement</h1>
    <p>
      Velio syncs the lists people add for their own TVs. It provides no content, and it stores lists
      encrypted, so it can't see them, search them, or remove a single one. What it can do is close the
      account that holds them: name it by its email address if you know it.
    </p>
    <p>
      You can also write to <a href={`mailto:${abuseEmail}`}>{abuseEmail}</a>. Authorities with a legal
      request: write there.
    </p>
    <form hx-post="/abuse" hx-target="#main">
      <label>
        What it's about
        <select name="kind" required>
          <option value="copyright">Copyright infringement</option>
          <option value="abuse">Other abuse of the service</option>
          <option value="other">Something else, such as contesting a closed account</option>
        </select>
      </label>
      <label>
        Your name
        <input type="text" name="name" autocomplete="name" required maxlength={200} />
      </label>
      <label>
        Your email
        <input type="email" name="email" autocomplete="email" required maxlength={254} />
      </label>
      <label>
        Organization, if you write for one
        <input type="text" name="organization" autocomplete="organization" maxlength={200} />
      </label>
      <label>
        The Velio account's email address, if you know it
        <input type="email" name="account_email" maxlength={254} />
      </label>
      <label>
        What happened, and how you know
        <textarea name="details" required minlength={20} maxlength={10000} rows={6}></textarea>
      </label>
      <fieldset>
        <legend>For copyright notices</legend>
        <label>
          The copyrighted work, such as the channel or programme, and who owns it
          <textarea name="work" maxlength={10000} rows={3}></textarea>
        </label>
        <label class="check">
          <input type="checkbox" name="sworn" value="yes" />
          <span>
            I believe in good faith that the use described isn't authorized by the copyright owner, its agent,
            or the law. The information in this notice is accurate and, under penalty of perjury, I am the
            owner of the right concerned or authorized to act for the owner.
          </span>
        </label>
        <label>
          Signature: your full name
          <input type="text" name="signature" maxlength={200} />
        </label>
      </fieldset>
      <Turnstile siteKey={siteKey} />
      <Problem />
      <button type="submit">Send the notice</button>
    </form>
  </section>
);

export const NoticeReceived = ({ id }: { id: string }) => (
  <section>
    <h1>Notice received</h1>
    <p>
      Thank you. Its reference is <strong>{id}</strong>. We'll answer by email, and let you know what we did.
    </p>
  </section>
);

export const NotFound = () => (
  <section>
    <h1>Not found</h1>
    <p>
      <a href="/">Go to the start</a>
    </p>
  </section>
);

export interface OutboxRow {
  id: number;
  sent_at: number;
  to_address: string;
  subject: string;
  text: string;
}

export const DevOutbox = ({ mails }: { mails: OutboxRow[] }) => (
  <section>
    <h1>Outbox (DEV_MODE)</h1>
    {mails.length === 0 ? <p>No email yet.</p> : null}
    {mails.map(mail => (
      <article class="mail">
        <h2>{mail.subject}</h2>
        <p class="note">
          To {mail.to_address}, {new Date(mail.sent_at).toISOString()}
        </p>
        <pre>{mail.text}</pre>
        {(mail.text.match(/\/(?:confirm|reset)\/[\w-]+/g) ?? []).map(link => (
          <p>
            <a href={link}>{link}</a>
          </p>
        ))}
      </article>
    ))}
  </section>
);
