// Email: confirming the address at sign-up, and the reset link. Sent by
// Resend when the RESEND_API_KEY secret is set, otherwise by Cloudflare Email
// Service (which needs the Workers Paid plan). With DEV_MODE=1, written to
// `dev_outbox` instead and shown at /dev/outbox. Plain text only, in neutral
// wording (PLAN.md, "Legal and abuse").

import { isDev } from "./http.ts";

export interface Mail {
  to: string;
  subject: string;
  text: string;
}

/** Sends, or logs why not. A failed send never changes the answer the visitor sees. */
export const sendMail = async (env: Env, mail: Mail) => {
  if (isDev(env)) {
    await env.DB.prepare("INSERT INTO dev_outbox (sent_at, to_address, subject, text) VALUES (?, ?, ?, ?)")
      .bind(Date.now(), mail.to, mail.subject, mail.text)
      .run();
    return;
  }
  try {
    if (env.RESEND_API_KEY) await sendWithResend(env.RESEND_API_KEY, env.EMAIL_FROM, mail);
    else
      await env.EMAIL.send({
        from: { email: env.EMAIL_FROM, name: "Velio" },
        to: mail.to,
        subject: mail.subject,
        text: mail.text,
      });
  } catch (error) {
    console.error("Couldn't send email", mail.subject, error);
  }
};

const sendWithResend = async (apiKey: string, from: string, mail: Mail) => {
  const response = await fetch("https://api.resend.com/emails", {
    method: "POST",
    headers: { Authorization: `Bearer ${apiKey}`, "Content-Type": "application/json" },
    body: JSON.stringify({ from: `Velio <${from}>`, to: [mail.to], subject: mail.subject, text: mail.text }),
  });
  if (!response.ok) throw new Error(`Resend answered ${response.status}: ${await response.text()}`);
};

const footer = (site: string) =>
  `\n\nIf you didn't ask for this, ignore this email; nothing changes.\n\nVelio · ${site}\n`;

export const confirmMail = (site: string, to: string, link: string): Mail => ({
  to,
  subject: "Confirm your email for Velio",
  text:
    `Open this link to confirm your email and finish making your Velio account:\n\n${link}\n\n` +
    `The link works for 24 hours.${footer(site)}`,
});

export const existingAccountMail = (site: string, to: string): Mail => ({
  to,
  subject: "You already have a Velio account",
  text:
    `Someone tried to make a Velio account with this email, but it already has one.\n\n` +
    `Sign in: ${site}/signin\n` +
    `Forgot the password? Reset the account: ${site}/reset${footer(site)}`,
});

export const resetMail = (site: string, to: string, link: string): Mail => ({
  to,
  subject: "Reset your Velio account",
  text:
    `Open this link to choose a new password:\n\n${link}\n\n` +
    `If you made a recovery key, the page asks for it, and your lists, profiles and TVs stay. ` +
    `Without one, Velio can't read your lists, so a reset deletes them, with your profiles and signed-in ` +
    `TVs, and you add them again afterwards.\n\nThe link works for 1 hour.${footer(site)}`,
});

// --- Notices and closed accounts (Phase 8) ---

const day = (time: number) => new Date(time).toISOString().slice(0, 10);

export interface NoticeSummary {
  id: string;
  kind: string;
  name: string;
  email: string;
  organization: string;
  accountEmail: string;
  accountFound: boolean;
  work: string;
  sworn: boolean;
  signature: string;
  details: string;
}

/** To the abuse address: a new notice, to handle with scripts/ops.mjs. */
export const noticeMail = (to: string, notice: NoticeSummary): Mail => ({
  to,
  subject: `Velio notice ${notice.id} (${notice.kind})`,
  text: [
    `Notice ${notice.id}, ${notice.kind}.`,
    "",
    `From: ${notice.name} <${notice.email}>${notice.organization ? `, ${notice.organization}` : ""}`,
    `Account named: ${notice.accountEmail || "(none)"}${notice.accountEmail ? (notice.accountFound ? " (found)" : " (no such account)") : ""}`,
    ...(notice.kind === "copyright"
      ? [
          `Work: ${notice.work}`,
          `Sworn statements: ${notice.sworn ? "yes" : "no"}`,
          `Signature: ${notice.signature}`,
        ]
      : []),
    "",
    notice.details,
    "",
    `Handle it with: node scripts/ops.mjs notice ${notice.id}`,
    "",
  ].join("\n"),
});

/** To the owner of an account just closed: the statement of reasons. */
export const closedMail = (
  site: string,
  to: string,
  abuse: string,
  reason: string,
  deleteAt: number,
): Mail => ({
  to,
  subject: "Your Velio account is closed",
  text:
    `Your Velio account (${to}) was closed, for this reason:\n\n${reason}\n\n` +
    `You can't sign in, and your TVs are signed out. Your encrypted lists, profiles and keys will be ` +
    `deleted on ${day(deleteAt)}.\n\n` +
    `If you think this is a mistake, or you want to contest a copyright notice, write to ${abuse} ` +
    `before then, with this email address and your reasons.\n\nVelio · ${site}\n`,
});

/** To the owner of an account opened again. */
export const reopenedMail = (site: string, to: string): Mail => ({
  to,
  subject: "Your Velio account is open again",
  text:
    `Your Velio account (${to}) is open again, with your lists and profiles. Sign in at ${site}/signin. ` +
    `TVs that were signed out need to be signed in again.\n\nVelio · ${site}\n`,
});
