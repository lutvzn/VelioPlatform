//! Email: confirming the address at sign-up, the reset link, notices and
//! closed accounts (the Worker's
//! `lib/mail.ts`). Sent over SMTP or by Resend; with DEV_MODE, written to
//! `dev_outbox` instead and shown at /dev/outbox. Plain text only, in neutral
//! wording (PLAN.md, "Legal and abuse").

use std::time::Duration;

use anyhow::{Context, Result, bail};
use lettre::message::Mailbox;
use lettre::message::header::ContentType;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use rusqlite::params;
use serde_json::json;

use crate::AppState;
use crate::config::{self, Config};
use crate::time::{day, now_ms};

#[derive(Debug, Clone)]
pub struct Mail {
    pub to: String,
    pub subject: String,
    pub text: String,
}

/// How mail leaves, built once at start, so a wrong SMTP_URL or EMAIL_FROM stops the server.
pub enum Mailer {
    DevOutbox,
    Smtp { transport: AsyncSmtpTransport<Tokio1Executor>, from: Mailbox },
    Resend { client: reqwest::Client, key: String, from: String },
}

impl Mailer {
    pub fn new(cfg: &Config, client: reqwest::Client) -> Result<Self> {
        // EMAIL_FROM is an address, or a name and an address.
        let from =
            if cfg.email_from.contains('<') { cfg.email_from.clone() } else { format!("Velio <{}>", cfg.email_from) };
        let mailbox: Mailbox = from.parse().with_context(|| format!("EMAIL_FROM isn't an address: {from}"))?;
        Ok(match &cfg.mail {
            config::Mail::DevOutbox => Mailer::DevOutbox,
            config::Mail::Smtp(url) => {
                let transport = AsyncSmtpTransport::<Tokio1Executor>::from_url(url)
                    .context("SMTP_URL isn't readable")?
                    .timeout(Some(Duration::from_secs(30)))
                    .build();
                Mailer::Smtp { transport, from: mailbox }
            }
            config::Mail::Resend(key) => Mailer::Resend { client, key: key.clone(), from },
        })
    }
}

/// Sends, or logs why not. A failed send never changes the answer the
/// visitor sees, and real mail is sent after the answer, so how long it takes
/// doesn't tell whether an email has an account.
pub async fn send(state: &AppState, mail: Mail) -> Result<()> {
    if let Mailer::DevOutbox = *state.mailer {
        return state
            .db
            .call(move |conn| {
                conn.execute(
                    "INSERT INTO dev_outbox (sent_at, to_address, subject, text) VALUES (?, ?, ?, ?)",
                    params![now_ms(), mail.to, mail.subject, mail.text],
                )?;
                Ok(())
            })
            .await;
    }
    let mailer = state.mailer.clone();
    tokio::spawn(async move {
        if let Err(error) = deliver(&mailer, &mail).await {
            tracing::error!("Couldn't send the email {:?}: {error:#}", mail.subject);
        }
    });
    Ok(())
}

async fn deliver(mailer: &Mailer, mail: &Mail) -> Result<()> {
    match mailer {
        Mailer::DevOutbox => unreachable!("Written to the outbox without delivering"),
        Mailer::Smtp { transport, from } => {
            let message = Message::builder()
                .from(from.clone())
                .to(mail.to.parse().context("The address")?)
                .subject(mail.subject.clone())
                .header(ContentType::TEXT_PLAIN)
                .body(mail.text.clone())?;
            transport.send(message).await?;
        }
        Mailer::Resend { client, key, from } => {
            let response = client
                .post("https://api.resend.com/emails")
                .bearer_auth(key)
                .json(&json!({ "from": from, "to": [mail.to], "subject": mail.subject, "text": mail.text }))
                .send()
                .await?;
            if !response.status().is_success() {
                bail!("Resend answered {}: {}", response.status(), response.text().await.unwrap_or_default());
            }
        }
    }
    Ok(())
}

fn footer(site: &str) -> String {
    format!("\n\nIf you didn't ask for this, ignore this email; nothing changes.\n\nVelio · {site}\n")
}

pub fn confirm_mail(site: &str, to: &str, link: &str) -> Mail {
    Mail {
        to: to.into(),
        subject: "Confirm your email for Velio".into(),
        text: format!(
            "Open this link to confirm your email and finish making your Velio account:\n\n{link}\n\n\
             The link works for 24 hours.{}",
            footer(site)
        ),
    }
}

pub fn existing_account_mail(site: &str, to: &str) -> Mail {
    Mail {
        to: to.into(),
        subject: "You already have a Velio account".into(),
        text: format!(
            "Someone tried to make a Velio account with this email, but it already has one.\n\n\
             Sign in: {site}/signin\n\
             Forgot the password? Reset the account: {site}/reset{}",
            footer(site)
        ),
    }
}

pub fn reset_mail(site: &str, to: &str, link: &str) -> Mail {
    Mail {
        to: to.into(),
        subject: "Reset your Velio account".into(),
        text: format!(
            "Open this link to choose a new password:\n\n{link}\n\n\
             If you made a recovery key, the page asks for it, and your lists, profiles and TVs stay. \
             Without one, Velio can't read your lists, so a reset deletes them, with your profiles and signed-in \
             TVs, and you add them again afterwards.\n\nThe link works for 1 hour.{}",
            footer(site)
        ),
    }
}

/// A notice sent with /abuse, as the abuse address reads it.
pub struct NoticeSummary<'a> {
    pub id: &'a str,
    pub notice: &'a crate::notices::NoticeInput,
    pub account_found: bool,
}

/// To the abuse address: a new notice, to handle with scripts/ops.mjs.
pub fn notice_mail(to: &str, summary: &NoticeSummary<'_>) -> Mail {
    let (id, notice) = (summary.id, summary.notice);
    let organization =
        if notice.organization.is_empty() { String::new() } else { format!(", {}", notice.organization) };
    let named = match (notice.account_email.is_empty(), summary.account_found) {
        (true, _) => "(none)".to_owned(),
        (false, true) => format!("{} (found)", notice.account_email),
        (false, false) => format!("{} (no such account)", notice.account_email),
    };
    let mut lines = vec![
        format!("Notice {id}, {}.", notice.kind),
        String::new(),
        format!("From: {} <{}>{organization}", notice.name, notice.email),
        format!("Account named: {named}"),
    ];
    if notice.kind == "copyright" {
        lines.push(format!("Work: {}", notice.work));
        lines.push(format!("Sworn statements: {}", if notice.sworn { "yes" } else { "no" }));
        lines.push(format!("Signature: {}", notice.signature));
    }
    lines.extend([
        String::new(),
        notice.details.clone(),
        String::new(),
        format!("Handle it with: node scripts/ops.mjs notice {id}"),
        String::new(),
    ]);
    Mail { to: to.into(), subject: format!("Velio notice {id} ({})", notice.kind), text: lines.join("\n") }
}

/// To the owner of an account just closed: the statement of reasons.
pub fn closed_mail(site: &str, to: &str, abuse: &str, reason: &str, delete_at: i64) -> Mail {
    Mail {
        to: to.into(),
        subject: "Your Velio account is closed".into(),
        text: format!(
            "Your Velio account ({to}) was closed, for this reason:\n\n{reason}\n\n\
             You can't sign in, and your TVs are signed out. Your encrypted lists, profiles and keys will be \
             deleted on {}.\n\n\
             If you think this is a mistake, or you want to contest a copyright notice, write to {abuse} \
             before then, with this email address and your reasons.\n\nVelio · {site}\n",
            day(delete_at)
        ),
    }
}

/// To the owner of an account opened again.
pub fn reopened_mail(site: &str, to: &str) -> Mail {
    Mail {
        to: to.into(),
        subject: "Your Velio account is open again".into(),
        text: format!(
            "Your Velio account ({to}) is open again, with your lists and profiles. Sign in at {site}/signin. \
             Your TVs were signed out when it closed: remove them on the TVs page, and sign them in again.\n\nVelio · {site}\n"
        ),
    }
}

/// The links /dev/outbox shows under each email, as /\/(?:confirm|reset)\/[\w-]+/g finds them.
pub fn links_in(text: &str) -> Vec<&str> {
    let mut links = Vec::new();
    let mut at = 0;
    while at < text.len() {
        let rest = &text[at..];
        let prefix = ["/confirm/", "/reset/"].into_iter().find(|prefix| rest.starts_with(prefix));
        if let Some(prefix) = prefix {
            let token = rest[prefix.len()..]
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
                .unwrap_or(rest.len() - prefix.len());
            if token > 0 {
                links.push(&rest[..prefix.len() + token]);
                at += prefix.len() + token;
                continue;
            }
        }
        at += rest.chars().next().map_or(1, char::len_utf8);
    }
    links
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_links_in_an_email() {
        let mail = confirm_mail("https://velio.tv", "a@example.invalid", "https://velio.tv/confirm/ab_C-9");
        assert_eq!(links_in(&mail.text), ["/confirm/ab_C-9"]);
        let existing = existing_account_mail("https://velio.tv", "a@example.invalid");
        assert!(links_in(&existing.text).is_empty());
        assert_eq!(links_in("· /reset/x1 and /confirm/ /reset/y"), ["/reset/x1", "/reset/y"]);
    }
}
