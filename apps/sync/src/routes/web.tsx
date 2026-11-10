// The web routes for accounts (PLAN.md, "Routes"; Phase 1). Forms are sent
// by HTMX, and answers are fragments for #main. A form problem answers 4xx
// with HX-Retarget to the form's #form-problem, so what was typed stays put.

import { Hono } from "hono";
import { html } from "hono/html";
import type { Child } from "hono/jsx";
import { emailProblem, normalizeEmail } from "@velio/sync-core";
import { accountStub } from "../account.ts";
import {
  authHashOf,
  CONFIRM_LINK_MS,
  findAccount,
  findAccountById,
  newEmailLink,
  peekEmailLink,
  preloginFor,
  readCredentials,
  RESET_LINK_MS,
  useEmailLink,
} from "../lib/accounts.ts";
import { clientIp, isDev, readForm, type AppContext, type AppEnv } from "../lib/http.ts";
import { withinLimit } from "../lib/limits.ts";
import { readNotice, saveNotice } from "../lib/notices.ts";
import { confirmMail, existingAccountMail, resetMail, sendMail } from "../lib/mail.ts";
import { dropSession, endSession, signedIn, startSession } from "../lib/session.ts";
import { sameDigest } from "../lib/tokens.ts";
import { passedTurnstile, TURNSTILE_FIELD } from "../lib/turnstile.ts";
import { devicesOf, NO_KEYS, vaultProps } from "../lib/vault.ts";
import { Layout, Nav } from "../views/layout.tsx";
import { PairPage } from "../views/pair.tsx";
import { OwnerBundle } from "../views/vault.tsx";
import {
  AccountView,
  type AccountViewProps,
  CheckEmail,
  AbuseForm,
  ConfirmPage,
  DevOutbox,
  Home,
  LinkProblem,
  NoticeReceived,
  NotFound,
  Privacy,
  ResetForm,
  ResetRequestForm,
  ResetSent,
  Security,
  SigninForm,
  SignupForm,
  Terms,
  type OutboxRow,
} from "../views/pages.tsx";

export const web = new Hono<AppEnv>();

export const isHtmx = (c: AppContext) => c.req.header("HX-Request") === "true";

/**
 * Links in emails: velio.tv, or with DEV_MODE, paths only, as wrangler dev
 * shows the Worker the route's host rather than where it runs.
 */
const siteUrl = (c: AppContext) => (isDev(c.env) ? "" : c.env.SITE_URL);

export const fullPage = (
  c: AppContext,
  title: string,
  body: Child,
  options: { status?: 200 | 400 | 403 | 404 | 500; turnstile?: boolean } = {},
) =>
  c.html(
    html`<!doctype html>${(
        <Layout title={title} accountId={signedIn(c)?.accountId} turnstile={options.turnstile}>
          {body}
        </Layout>
      )}`,
    options.status ?? 200,
  );

/** A fragment for #main, or the full page when the form was sent without HTMX. */
export const answer = (c: AppContext, title: string, body: Child) =>
  isHtmx(c) ? c.html(<>{body}</>) : fullPage(c, title, body);

type ProblemStatus = 400 | 401 | 403 | 404 | 409 | 413 | 429 | 500;

/** A form's problem, shown in its #form-problem (or the given target), leaving what was typed. */
export const problem = (
  c: AppContext,
  message: string,
  status: ProblemStatus = 400,
  target = "#form-problem",
) => {
  if (!isHtmx(c))
    return c.html(
      html`<!doctype html>${(
          <Layout title="Problem" accountId={signedIn(c)?.accountId}>
            <section>
              <h1>That didn't work</h1>
              <p class="problem">{message}</p>
            </section>
          </Layout>
        )}`,
      status,
    );
  c.header("HX-Retarget", target);
  c.header("HX-Reswap", "innerHTML");
  return c.html(<>{message}</>, status);
};

export const TOO_MANY = "Too many tries. Wait a minute, then try again.";

const turnstileOn = (c: AppContext) => c.env.TURNSTILE_SITE_KEY !== "";

/** What the account page shows, besides what was just done. */
const accountProps = async (c: AppContext, accountId: string, email: string) => {
  const [devices, recoveryMadeAt] = await Promise.all([
    devicesOf(c.env.DB, accountId),
    accountStub(c.env, accountId).recoveryMadeAt(),
  ]);
  return { email, devices, recoveryMadeAt };
};

/** The account page as a fragment for #main, after a setting changed. */
export const accountAnswer = async (
  c: AppContext,
  accountId: string,
  email: string,
  extra: Pick<AccountViewProps, "notice" | "showRecoveryKey"> = {},
) => {
  if (isHtmx(c)) c.header("HX-Push-Url", "/account");
  return answer(c, "Your account", <AccountView {...await accountProps(c, accountId, email)} {...extra} />);
};

/**
 * Signed in: the account page, or the pairing page when signing in on the
 * way there, with the owner bundle for vault.js to open.
 */
export const signedInAnswer = async (
  c: AppContext,
  accountId: string,
  email: string,
  next = "",
  notice?: string,
) => {
  const owner = await accountStub(c.env, accountId).getOwner();
  if (!owner) return problem(c, NO_KEYS, 500);
  if (next === "/pair") {
    const props = await vaultProps(c, { accountId, email });
    if (!props) return problem(c, NO_KEYS, 500);
    c.header("HX-Push-Url", "/pair");
    return c.html(
      <>
        <OwnerBundle owner={owner} />
        <PairPage {...props} />
        <Nav accountId={accountId} oob />
      </>,
    );
  }
  c.header("HX-Push-Url", "/account");
  return c.html(
    <>
      <AccountView {...await accountProps(c, accountId, email)} owner={owner} notice={notice} />
      <Nav accountId={accountId} oob />
    </>,
  );
};

web.get("/", c => fullPage(c, "Your lists, on every TV", <Home signedIn={signedIn(c) !== null} />));

web.get("/terms", c => fullPage(c, "Terms", <Terms abuseEmail={c.env.ABUSE_EMAIL} />));
web.get("/privacy", c => fullPage(c, "Privacy", <Privacy />));
web.get("/security", c => fullPage(c, "Security", <Security />));

// --- Notices (PLAN.md, "Legal and abuse") ---

web.get("/abuse", c =>
  fullPage(
    c,
    "Report abuse",
    <AbuseForm siteKey={c.env.TURNSTILE_SITE_KEY} abuseEmail={c.env.ABUSE_EMAIL} />,
    {
      turnstile: turnstileOn(c),
    },
  ),
);

web.post("/abuse", async c => {
  const ip = clientIp(c);
  if (!(await withinLimit(c.env, "SIGNUP_LIMIT", `abuse:${ip}`))) return problem(c, TOO_MANY, 429);
  const field = await readForm(c);
  const notice = readNotice(field);
  if (typeof notice === "string") return problem(c, notice);
  if (!(await passedTurnstile(c.env, field(TURNSTILE_FIELD), ip)))
    return problem(c, "The check that you're a person didn't pass. Try again.");
  const id = await saveNotice(c.env, notice);
  return answer(c, "Notice received", <NoticeReceived id={id} />);
});

// --- Making an account ---

web.get("/signup", c => {
  if (signedIn(c)) return c.redirect("/account");
  return fullPage(c, "Make an account", <SignupForm siteKey={c.env.TURNSTILE_SITE_KEY} />, {
    turnstile: turnstileOn(c),
  });
});

web.post("/signup", async c => {
  const ip = clientIp(c);
  if (!(await withinLimit(c.env, "SIGNUP_LIMIT", `signup:${ip}`))) return problem(c, TOO_MANY, 429);
  const field = await readForm(c);
  const email = normalizeEmail(field("email"));
  const emailWrong = emailProblem(email);
  if (emailWrong) return problem(c, emailWrong);
  if (field("terms") !== "yes") return problem(c, "Accept the terms to make an account.");
  if (!(await passedTurnstile(c.env, field(TURNSTILE_FIELD), ip)))
    return problem(c, "The check that you're a person didn't pass. Try again.");
  const credentials = await readCredentials(field);
  if (typeof credentials === "string") return problem(c, credentials);
  if (!(await withinLimit(c.env, "EMAIL_LIMIT", `mail:${email}`))) return problem(c, TOO_MANY, 429);

  const done = () => answer(c, "Check your email", <CheckEmail email={email} />);
  const existing = await findAccount(c.env.DB, email);
  const site = siteUrl(c);
  // The same answer whether or not the email has an account, so the form doesn't tell.
  if (existing && (existing.confirmed_at !== null || existing.closed_at !== null)) {
    if (existing.closed_at === null) await sendMail(c.env, existingAccountMail(site, email));
    return done();
  }

  // A new account, or one never confirmed, which this replaces: whoever can
  // read the confirmation email gets the address.
  const now = Date.now();
  const { salt, kdf, authHash, owner } = credentials;
  const accountId = existing?.id ?? crypto.randomUUID();
  const write = existing
    ? c.env.DB.prepare(
        `UPDATE accounts SET salt = ?, kdf_memory_kib = ?, kdf_iterations = ?, kdf_parallelism = ?, auth_hash = ?,
           terms_accepted_at = ?, created_at = ? WHERE id = ? AND confirmed_at IS NULL`,
      ).bind(salt, kdf.memoryKiB, kdf.iterations, kdf.parallelism, authHash, now, now, accountId)
    : c.env.DB.prepare(
        `INSERT INTO accounts (id, email, salt, kdf_memory_kib, kdf_iterations, kdf_parallelism, auth_hash,
           terms_accepted_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT (email) DO NOTHING`,
      ).bind(accountId, email, salt, kdf.memoryKiB, kdf.iterations, kdf.parallelism, authHash, now, now);
  const result = await write.run();
  // Confirmed, or made by another request, in the meantime.
  if (result.meta.changes === 0) return done();

  const stub = accountStub(c.env, accountId);
  await stub.reset();
  await stub.setOwner({ ...owner, updatedAt: now });
  const token = await newEmailLink(c.env.DB, accountId, "confirm", CONFIRM_LINK_MS);
  await sendMail(c.env, confirmMail(site, email, `${site}/confirm/${token}`));
  return done();
});

web.get("/confirm/:token", async c => {
  const accountId = await peekEmailLink(c.env.DB, c.req.param("token"), "confirm");
  if (!accountId) return fullPage(c, "Link", <LinkProblem />, { status: 404 });
  return fullPage(c, "Confirm your email", <ConfirmPage token={c.req.param("token")} />);
});

web.post("/confirm/:token", async c => {
  const accountId = await useEmailLink(c.env.DB, c.req.param("token"), "confirm");
  if (!accountId) return answer(c, "Link", <LinkProblem />);
  await c.env.DB.prepare("UPDATE accounts SET confirmed_at = ? WHERE id = ? AND confirmed_at IS NULL")
    .bind(Date.now(), accountId)
    .run();
  const account = await findAccountById(c.env.DB, accountId);
  if (isHtmx(c)) c.header("HX-Push-Url", "/signin");
  return answer(
    c,
    "Sign in",
    <SigninForm email={account?.email} note="Your email is confirmed. Sign in to start." />,
  );
});

// --- Signing in ---

web.get("/signin", c => {
  if (signedIn(c)) return c.redirect("/account");
  return fullPage(c, "Sign in", <SigninForm />);
});

/** The salt and Argon2id settings for an email; made-up but stable ones when it has no account. */
web.get("/prelogin", async c => {
  c.header("Cache-Control", "no-store");
  if (!(await withinLimit(c.env, "AUTH_LIMIT", `prelogin:${clientIp(c)}`)))
    return c.json({ problem: TOO_MANY }, 429);
  const email = normalizeEmail(c.req.query("email") ?? "");
  const emailWrong = emailProblem(email);
  if (emailWrong) return c.json({ problem: emailWrong }, 400);
  return c.json(await preloginFor(c.env, email, await findAccount(c.env.DB, email)));
});

web.post("/signin", async c => {
  if (!(await withinLimit(c.env, "AUTH_LIMIT", `signin:${clientIp(c)}`))) return problem(c, TOO_MANY, 429);
  const field = await readForm(c);
  const email = normalizeEmail(field("email"));
  if (emailProblem(email)) return problem(c, "Enter your email address.");
  if (!(await withinLimit(c.env, "EMAIL_LIMIT", `signin:${email}`))) return problem(c, TOO_MANY, 429);
  const authHash = await authHashOf(field("auth_key"));
  if (!authHash) return problem(c, "This browser didn't send a sign-in key. Reload the page and try again.");
  const account = await findAccount(c.env.DB, email);
  if (!account || !sameDigest(account.auth_hash, authHash))
    return problem(c, "That email and password don't match an account.", 401);
  if (account.closed_at !== null) return problem(c, "This account is closed.", 403);
  if (account.confirmed_at === null)
    return problem(
      c,
      "Confirm your email first, with the link we sent. For a new link, make the account again.",
      403,
    );
  await dropSession(c);
  await startSession(c, account.id);
  return signedInAnswer(c, account.id, account.email, field("next"));
});

web.post("/signout", async c => {
  await endSession(c);
  if (!isHtmx(c)) return c.redirect("/", 303);
  c.header("HX-Redirect", "/");
  return c.body(null, 204);
});

web.get("/account", async c => {
  const session = signedIn(c);
  if (!session) return c.redirect("/signin");
  return fullPage(
    c,
    "Your account",
    <AccountView {...await accountProps(c, session.accountId, session.email)} />,
  );
});

// --- Resetting ---

web.get("/reset", c =>
  fullPage(c, "Reset your account", <ResetRequestForm siteKey={c.env.TURNSTILE_SITE_KEY} />, {
    turnstile: turnstileOn(c),
  }),
);

web.post("/reset", async c => {
  const ip = clientIp(c);
  if (!(await withinLimit(c.env, "SIGNUP_LIMIT", `reset:${ip}`))) return problem(c, TOO_MANY, 429);
  const field = await readForm(c);
  const email = normalizeEmail(field("email"));
  const emailWrong = emailProblem(email);
  if (emailWrong) return problem(c, emailWrong);
  if (!(await passedTurnstile(c.env, field(TURNSTILE_FIELD), ip)))
    return problem(c, "The check that you're a person didn't pass. Try again.");
  if (!(await withinLimit(c.env, "EMAIL_LIMIT", `mail:${email}`))) return problem(c, TOO_MANY, 429);
  const account = await findAccount(c.env.DB, email);
  if (account && account.closed_at === null) {
    const token = await newEmailLink(c.env.DB, account.id, "reset", RESET_LINK_MS);
    const site = siteUrl(c);
    await sendMail(c.env, resetMail(site, email, `${site}/reset/${token}`));
  }
  return answer(c, "Check your email", <ResetSent email={email} />);
});

web.get("/reset/:token", async c => {
  const accountId = await peekEmailLink(c.env.DB, c.req.param("token"), "reset");
  if (!accountId) return fullPage(c, "Link", <LinkProblem />, { status: 404 });
  const recovery = (await accountStub(c.env, accountId).recoveryMadeAt()) !== null;
  return fullPage(c, "Choose a new password", <ResetForm token={c.req.param("token")} recovery={recovery} />);
});

web.post("/reset/:token", async c => {
  if (!(await withinLimit(c.env, "AUTH_LIMIT", `reset-link:${clientIp(c)}`)))
    return problem(c, TOO_MANY, 429);
  const credentials = await readCredentials(await readForm(c));
  if (typeof credentials === "string") return problem(c, credentials);
  const accountId = await useEmailLink(c.env.DB, c.req.param("token"), "reset");
  if (!accountId) return problem(c, "This link was already used, or it's too old. Ask for a new one.", 404);
  const account = await findAccountById(c.env.DB, accountId);
  if (!account || account.closed_at !== null) return problem(c, "This account is closed.", 403);

  // New credentials; every session and TV is signed out, as the old keys no longer open anything.
  // Opening the link also proves the email, so it confirms an unconfirmed account.
  const now = Date.now();
  const { salt, kdf, authHash, owner } = credentials;
  await c.env.DB.batch([
    c.env.DB.prepare(
      `UPDATE accounts SET salt = ?, kdf_memory_kib = ?, kdf_iterations = ?, kdf_parallelism = ?, auth_hash = ?,
         confirmed_at = COALESCE(confirmed_at, ?) WHERE id = ?`,
    ).bind(salt, kdf.memoryKiB, kdf.iterations, kdf.parallelism, authHash, now, accountId),
    c.env.DB.prepare("DELETE FROM sessions WHERE account_id = ?").bind(accountId),
    c.env.DB.prepare("DELETE FROM email_links WHERE account_id = ?").bind(accountId),
    c.env.DB.prepare("DELETE FROM devices WHERE account_id = ?").bind(accountId),
    c.env.DB.prepare("DELETE FROM pairings WHERE account_id = ?").bind(accountId),
  ]);
  const stub = accountStub(c.env, accountId);
  await stub.reset();
  await stub.setOwner({ ...owner, updatedAt: now });
  c.set("session", null);
  await startSession(c, accountId);
  return signedInAnswer(c, accountId, account.email);
});

// --- Development ---

web.get("/dev/outbox", async c => {
  if (!isDev(c.env)) return fullPage(c, "Not found", <NotFound />, { status: 404 });
  const { results } = await c.env.DB.prepare(
    "SELECT * FROM dev_outbox ORDER BY id DESC LIMIT 50",
  ).all<OutboxRow>();
  return fullPage(c, "Outbox", <DevOutbox mails={results} />);
});
