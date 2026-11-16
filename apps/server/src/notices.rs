//! Abuse and copyright notices (the Worker's `lib/notices.ts`; PLAN.md,
//! "Legal and abuse"): sent with the /abuse form, kept in `notices`, mailed to
//! the abuse address, and handled through /ops. A notice can lead to removing
//! a list, or closing the account it names.

use anyhow::Result;
use deadpool_postgres::GenericClient;
use uuid::Uuid;

use crate::accounts::{Fields, email_problem, normalize_email};
use crate::tokens::random_bytes;

pub const MAX_NOTICE_FIELD: usize = 200;
pub const MAX_NOTICE_TEXT: usize = 10_000;
pub const MIN_NOTICE_DETAILS: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoticeInput {
    /// copyright, abuse or other.
    pub kind: &'static str,
    pub name: String,
    pub email: String,
    pub organization: String,
    pub account_email: String,
    pub work: String,
    pub sworn: bool,
    pub signature: String,
    pub details: String,
}

/// As JavaScript counts a string's length.
fn js_length(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The notice in a form, or what's wrong with it.
pub fn read_notice(form: &impl Fields) -> Result<NoticeInput, String> {
    let Some(kind) = ["copyright", "abuse", "other"].into_iter().find(|kind| *kind == form.field("kind")) else {
        return Err("Choose what the notice is about.".into());
    };
    let name = form.field("name").trim();
    if name.is_empty() || js_length(name) > MAX_NOTICE_FIELD {
        return Err("Enter your name.".into());
    }
    let email = normalize_email(form.field("email"));
    if email_problem(&email).is_some() {
        return Err("Enter your email address, so we can answer.".into());
    }
    let organization = form.field("organization").trim();
    if js_length(organization) > MAX_NOTICE_FIELD {
        return Err("Shorten the organization's name.".into());
    }
    let account_email = normalize_email(form.field("account_email"));
    if !account_email.is_empty() && email_problem(&account_email).is_some() {
        return Err("The Velio account's email address doesn't look right. Leave it empty if you don't know it.".into());
    }
    let details = form.field("details").trim();
    if js_length(details) < MIN_NOTICE_DETAILS {
        return Err("Describe what the notice is about.".into());
    }
    if js_length(details) > MAX_NOTICE_TEXT {
        return Err(format!("Keep the description under {MAX_NOTICE_TEXT} characters."));
    }
    let (work, signature, sworn) = (form.field("work").trim(), form.field("signature").trim(), form.field("sworn"));
    let copyright = kind == "copyright";
    if copyright {
        if work.is_empty() || js_length(work) > MAX_NOTICE_TEXT {
            return Err("Identify the copyrighted work.".into());
        }
        if sworn != "yes" {
            return Err("A copyright notice needs both statements.".into());
        }
        if signature.is_empty() || js_length(signature) > MAX_NOTICE_FIELD {
            return Err("Sign the notice with your full name.".into());
        }
    }
    Ok(NoticeInput {
        kind,
        name: name.into(),
        email,
        organization: organization.into(),
        account_email,
        work: if copyright { work.into() } else { String::new() },
        sworn: copyright,
        signature: if copyright { signature.into() } else { String::new() },
        details: details.into(),
    })
}

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A reference such as N-7K2Q9XHM: 40 random bits, easy to read out.
pub fn new_notice_id() -> String {
    let letters: String = random_bytes::<8>().iter().map(|byte| char::from(ALPHABET[usize::from(byte & 31)])).collect();
    format!("N-{letters}")
}

/// Keeps the notice; answers its reference, and whether the account it names was found.
pub async fn save_notice(db: &impl GenericClient, notice: &NoticeInput, now: i64) -> Result<(String, bool)> {
    let account: Option<Uuid> = if notice.account_email.is_empty() {
        None
    } else {
        db.query_opt("SELECT id FROM auth.users WHERE lower(email) = $1", &[&notice.account_email])
            .await?
            .map(|row| row.get(0))
    };
    let id = new_notice_id();
    db.execute(
        "INSERT INTO velio_private.notices (id, received_at, kind, name, email, organization, account_email,
           account_id, work, sworn, signature, details)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        &[
            &id,
            &now,
            &notice.kind,
            &notice.name,
            &notice.email,
            &notice.organization,
            &notice.account_email,
            &account,
            &notice.work,
            &notice.sworn,
            &notice.signature,
            &notice.details,
        ],
    )
    .await?;
    Ok((id, account.is_some()))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Map(Vec<(&'static str, &'static str)>);

    impl Fields for Map {
        fn field(&self, name: &str) -> &str {
            self.0.iter().find(|(key, _)| *key == name).map_or("", |(_, value)| value)
        }
    }

    fn copyright() -> Vec<(&'static str, &'static str)> {
        vec![
            ("kind", "copyright"),
            ("name", " Rights Holder "),
            ("email", "Rights@Example.invalid"),
            ("details", "This account syncs a list that streams our channel."),
            ("work", "Example Channel"),
            ("sworn", "yes"),
            ("signature", "Rights Holder"),
        ]
    }

    #[test]
    fn reads_a_notice_as_the_worker() {
        let notice = read_notice(&Map(copyright())).unwrap();
        assert_eq!(
            (notice.name.as_str(), notice.email.as_str(), notice.sworn),
            ("Rights Holder", "rights@example.invalid", true)
        );
        let mut unsworn = copyright();
        unsworn.retain(|(key, _)| *key != "sworn");
        assert_eq!(read_notice(&Map(unsworn.clone())).unwrap_err(), "A copyright notice needs both statements.");
        unsworn[0] = ("kind", "other");
        let other = read_notice(&Map(unsworn)).unwrap();
        assert_eq!((other.work.as_str(), other.signature.as_str(), other.sworn), ("", "", false));
        assert_eq!(read_notice(&Map(vec![("kind", "spam")])).unwrap_err(), "Choose what the notice is about.");
    }

    #[test]
    fn makes_readable_references() {
        let id = new_notice_id();
        assert_eq!(id.len(), 10);
        assert!(id.starts_with("N-") && id[2..].bytes().all(|byte| ALPHABET.contains(&byte)));
    }
}
