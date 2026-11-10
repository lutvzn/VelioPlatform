// Abuse and copyright notices (PLAN.md, "Legal and abuse"): sent with the
// /abuse form, kept in `notices`, mailed to the abuse address, and handled
// through /ops. Velio can't read lists, so a notice can only lead to closing
// the account it names.

import { emailProblem, normalizeEmail } from "@velio/sync-core";
import { findAccount } from "./accounts.ts";
import { noticeMail, sendMail, type NoticeSummary } from "./mail.ts";
import { randomBytes } from "./tokens.ts";

export const NOTICE_KINDS = ["copyright", "abuse", "other"] as const;
export type NoticeKind = (typeof NOTICE_KINDS)[number];

export const MAX_NOTICE_FIELD = 200;
export const MAX_NOTICE_TEXT = 10_000;
export const MIN_NOTICE_DETAILS = 20;

export interface NoticeInput {
  kind: NoticeKind;
  name: string;
  email: string;
  organization: string;
  accountEmail: string;
  work: string;
  sworn: boolean;
  signature: string;
  details: string;
}

const isKind = (value: string): value is NoticeKind => (NOTICE_KINDS as readonly string[]).includes(value);

/** The notice in a form, or what's wrong with it. */
export const readNotice = (field: (name: string) => string): NoticeInput | string => {
  const kind = field("kind");
  if (!isKind(kind)) return "Choose what the notice is about.";
  const name = field("name").trim();
  if (!name || name.length > MAX_NOTICE_FIELD) return "Enter your name.";
  const email = normalizeEmail(field("email"));
  if (emailProblem(email)) return "Enter your email address, so we can answer.";
  const organization = field("organization").trim();
  if (organization.length > MAX_NOTICE_FIELD) return "Shorten the organization's name.";
  const accountEmail = normalizeEmail(field("account_email"));
  if (accountEmail && emailProblem(accountEmail))
    return "The Velio account's email address doesn't look right. Leave it empty if you don't know it.";
  const details = field("details").trim();
  if (details.length < MIN_NOTICE_DETAILS) return "Describe what the notice is about.";
  if (details.length > MAX_NOTICE_TEXT) return `Keep the description under ${MAX_NOTICE_TEXT} characters.`;
  const work = field("work").trim();
  const signature = field("signature").trim();
  const sworn = field("sworn") === "yes";
  if (kind === "copyright") {
    if (!work || work.length > MAX_NOTICE_TEXT) return "Identify the copyrighted work.";
    if (!sworn) return "A copyright notice needs both statements.";
    if (!signature || signature.length > MAX_NOTICE_FIELD) return "Sign the notice with your full name.";
  }
  return {
    kind,
    name,
    email,
    organization,
    accountEmail,
    work: kind === "copyright" ? work : "",
    sworn: kind === "copyright" && sworn,
    signature: kind === "copyright" ? signature : "",
    details,
  };
};

const ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/** A reference such as N-7K2Q9XHM: 40 random bits, easy to read out. */
export const newNoticeId = () =>
  `N-${Array.from(randomBytes(8), byte => ALPHABET.charAt(byte & 31)).join("")}`;

/** Keeps the notice and mails it to the abuse address; answers its reference. */
export const saveNotice = async (env: Env, notice: NoticeInput, now = Date.now()) => {
  const account = notice.accountEmail ? await findAccount(env.DB, notice.accountEmail) : null;
  const id = newNoticeId();
  await env.DB.prepare(
    `INSERT INTO notices (id, received_at, kind, name, email, organization, account_email, account_id,
       work, sworn, signature, details)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
  )
    .bind(
      id,
      now,
      notice.kind,
      notice.name,
      notice.email,
      notice.organization,
      notice.accountEmail,
      account?.id ?? null,
      notice.work,
      notice.sworn ? 1 : 0,
      notice.signature,
      notice.details,
    )
    .run();
  const summary: NoticeSummary = { id, ...notice, accountFound: account !== null };
  await sendMail(env, noticeMail(env.ABUSE_EMAIL, summary));
  return id;
};
