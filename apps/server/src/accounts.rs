//! Accounts and email links (the Worker's `lib/accounts.ts`, and the checks
//! from sync-core's `account.ts` that the server makes).

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;

use crate::time::{DAY_MS, HOUR_MS, now_ms};
use crate::tokens::{BASE64, decode, decodes, hmac_sha256, new_token, sha256_hex};
use base64::Engine;

pub const CONFIRM_LINK_MS: i64 = 24 * HOUR_MS;
pub const RESET_LINK_MS: i64 = HOUR_MS;
/// Unconfirmed accounts are deleted after this (the daily clean-up).
pub const UNCONFIRMED_MS: i64 = 7 * DAY_MS;

// --- From sync-core ---

pub const MAX_EMAIL_LENGTH: usize = 254;
pub const SALT_BYTES: usize = 16;
pub const MIN_PASSWORD_LENGTH: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Kdf {
    #[serde(rename = "memoryKiB")]
    pub memory_kib: i64,
    pub iterations: i64,
    pub parallelism: i64,
}

pub const DEFAULT_KDF: Kdf = Kdf { memory_kib: 65_536, iterations: 3, parallelism: 1 };

pub fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

/// Why an email can't be used. Deliberately loose: the confirmation email is the real check.
pub fn email_problem(email: &str) -> Option<&'static str> {
    let email = normalize_email(email);
    if email.is_empty() {
        return Some("Enter your email address.");
    }
    if email.encode_utf16().count() > MAX_EMAIL_LENGTH {
        return Some("That email address is too long.");
    }
    // /^[^\s@]+@[^\s@]+\.[^\s@]+$/
    let looks_right = !email.chars().any(char::is_whitespace)
        && email.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && !domain.contains('@')
                && domain.char_indices().any(|(at, c)| c == '.' && at > 0 && at + 1 < domain.len())
        });
    if !looks_right {
        return Some("That doesn't look like an email address.");
    }
    None
}

/// The settings a server accepts: never weaker than the default, never so heavy a phone can't run it.
pub fn kdf_problem(kdf: Kdf) -> Option<&'static str> {
    if !(DEFAULT_KDF.memory_kib..=1_048_576).contains(&kdf.memory_kib) {
        return Some("memory out of range");
    }
    if !(DEFAULT_KDF.iterations..=10).contains(&kdf.iterations) {
        return Some("passes out of range");
    }
    if !(1..=4).contains(&kdf.parallelism) {
        return Some("lanes out of range");
    }
    None
}

// --- Accounts ---

#[derive(Debug, Clone)]
pub struct Account {
    pub id: String,
    pub email: String,
    pub confirmed_at: Option<i64>,
    pub salt: String,
    pub kdf: Kdf,
    pub auth_hash: String,
    pub closed_at: Option<i64>,
}

const ACCOUNT_COLUMNS: &str =
    "id, email, confirmed_at, salt, kdf_memory_kib, kdf_iterations, kdf_parallelism, auth_hash, closed_at";

fn account_of(row: &Row<'_>) -> rusqlite::Result<Account> {
    Ok(Account {
        id: row.get(0)?,
        email: row.get(1)?,
        confirmed_at: row.get(2)?,
        salt: row.get(3)?,
        kdf: Kdf { memory_kib: row.get(4)?, iterations: row.get(5)?, parallelism: row.get(6)? },
        auth_hash: row.get(7)?,
        closed_at: row.get(8)?,
    })
}

pub fn find_account(conn: &Connection, email: &str) -> Result<Option<Account>> {
    let sql = format!("SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE email = ?");
    Ok(conn.query_row(&sql, [email], account_of).optional()?)
}

pub fn find_account_by_id(conn: &Connection, id: &str) -> Result<Option<Account>> {
    let sql = format!("SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE id = ?");
    Ok(conn.query_row(&sql, [id], account_of).optional()?)
}

/// What GET /prelogin answers.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Prelogin {
    pub salt: String,
    #[serde(flatten)]
    pub kdf: Kdf,
}

/// The account's salt and settings, or made-up ones that stay the same for that email.
pub fn prelogin_for(secret: &str, email: &str, account: Option<&Account>) -> Prelogin {
    match account {
        Some(account) if account.closed_at.is_none() => Prelogin { salt: account.salt.clone(), kdf: account.kdf },
        _ => {
            let salt = &hmac_sha256(secret, format!("prelogin salt\n{email}"))[..SALT_BYTES];
            Prelogin { salt: BASE64.encode(salt), kdf: DEFAULT_KDF }
        }
    }
}

/// The SHA-256 of an auth key sent as base64, or None if it isn't one.
pub fn auth_hash_of(auth_key: &str) -> Option<String> {
    decode(auth_key).filter(|key| key.len() == 32).map(sha256_hex)
}

/// An owner bundle sent as JSON (a sync-core `Sealed`), as the JSON to store, or None.
pub fn read_owner_bundle(text: &str) -> Option<String> {
    if text.encode_utf16().count() > 8192 {
        return None;
    }
    let bundle: serde_json::Value = serde_json::from_str(text).ok()?;
    let nonce = bundle.get("nonce")?.as_str()?;
    bundle.get("box")?.as_str()?;
    decodes(nonce, 24).then(|| bundle.to_string())
}

/// The keys an owner's device made, as the server keeps them.
#[derive(Debug, Clone)]
pub struct Owner {
    /// The owner bundle's JSON.
    pub bundle: String,
    pub sign_public_key: String,
    pub box_public_key: String,
}

#[derive(Debug, Clone)]
pub struct Credentials {
    pub salt: String,
    pub kdf: Kdf,
    pub auth_hash: String,
    pub owner: Owner,
}

/// A form's text fields, as Hono's `parseBody`: a missing field reads as "".
pub trait Fields {
    fn field(&self, name: &str) -> &str;
}

fn whole_number(value: &str) -> i64 {
    // /^\d{1,9}$/, else NaN, which no range holds.
    if (1..=9).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_digit()) {
        value.parse().unwrap_or(-1)
    } else {
        -1
    }
}

/// Reads what `vault.js` puts in a sign-up or reset form in place of the
/// password: the salt, settings, auth key, owner bundle and public keys.
pub fn read_credentials(form: &impl Fields) -> Result<Credentials, String> {
    let salt = form.field("salt");
    if !decodes(salt, SALT_BYTES) {
        return Err("The salt is missing or the wrong size.".into());
    }
    let kdf = Kdf {
        memory_kib: whole_number(form.field("kdf_memory_kib")),
        iterations: whole_number(form.field("kdf_iterations")),
        parallelism: whole_number(form.field("kdf_parallelism")),
    };
    if let Some(problem) = kdf_problem(kdf) {
        return Err(format!("The Argon2id settings are refused: {problem}."));
    }
    let auth_hash = auth_hash_of(form.field("auth_key")).ok_or("The auth key is missing or the wrong size.")?;
    let bundle = read_owner_bundle(form.field("bundle")).ok_or("The owner bundle is missing or not readable.")?;
    let sign_public_key = form.field("sign_public_key");
    let box_public_key = form.field("box_public_key");
    if !decodes(sign_public_key, 32) || !decodes(box_public_key, 32) {
        return Err("The public keys are missing.".into());
    }
    let owner = Owner { bundle, sign_public_key: sign_public_key.into(), box_public_key: box_public_key.into() };
    Ok(Credentials { salt: salt.into(), kdf, auth_hash, owner })
}

// --- Email links ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    Confirm,
    Reset,
}

impl LinkKind {
    const fn as_str(self) -> &'static str {
        match self {
            LinkKind::Confirm => "confirm",
            LinkKind::Reset => "reset",
        }
    }
}

/// A new one-time link for the account; earlier links of the same kind stop working.
pub fn new_email_link(conn: &Connection, account_id: &str, kind: LinkKind, lifetime_ms: i64) -> Result<String> {
    let token = new_token();
    let now = now_ms();
    conn.execute("DELETE FROM email_links WHERE account_id = ? AND kind = ?", params![account_id, kind.as_str()])?;
    conn.execute(
        "INSERT INTO email_links (token_hash, account_id, kind, created_at, expires_at) VALUES (?, ?, ?, ?, ?)",
        params![sha256_hex(&token), account_id, kind.as_str(), now, now + lifetime_ms],
    )?;
    Ok(token)
}

/// The account a link is for, if it is unused and unexpired, without using it.
pub fn peek_email_link(conn: &Connection, token: &str, kind: LinkKind) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT account_id FROM email_links
             WHERE token_hash = ? AND kind = ? AND used_at IS NULL AND expires_at > ?",
            params![sha256_hex(token), kind.as_str(), now_ms()],
            |row| row.get(0),
        )
        .optional()?)
}

/// Uses a link once: the account it was for, or None if it was used, expired or never sent.
pub fn use_email_link(conn: &Connection, token: &str, kind: LinkKind) -> Result<Option<String>> {
    let now = now_ms();
    Ok(conn
        .query_row(
            "UPDATE email_links SET used_at = ?
             WHERE token_hash = ? AND kind = ? AND used_at IS NULL AND expires_at > ?
             RETURNING account_id",
            params![now, sha256_hex(token), kind.as_str(), now],
            |row| row.get(0),
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    impl Fields for HashMap<&str, String> {
        fn field(&self, name: &str) -> &str {
            self.get(name).map_or("", String::as_str)
        }
    }

    #[test]
    fn checks_emails_as_sync_core_does() {
        assert_eq!(normalize_email("  Owner@Example.INVALID "), "owner@example.invalid");
        assert_eq!(email_problem("owner@example.invalid"), None);
        assert_eq!(email_problem(" "), Some("Enter your email address."));
        for wrong in ["owner", "owner@example", "@example.invalid", "a b@example.invalid", "a@b@c.d", "a@.d", "a@b."] {
            assert_eq!(email_problem(wrong), Some("That doesn't look like an email address."), "{wrong}");
        }
        let long = format!("{}@example.invalid", "a".repeat(250));
        assert_eq!(email_problem(&long), Some("That email address is too long."));
    }

    #[test]
    fn reads_credentials_and_refuses_weak_settings() {
        let key = BASE64.encode([1u8; 32]);
        let mut form: HashMap<&str, String> = HashMap::from([
            ("salt", BASE64.encode([2u8; 16])),
            ("kdf_memory_kib", "65536".into()),
            ("kdf_iterations", "3".into()),
            ("kdf_parallelism", "1".into()),
            ("auth_key", key.clone()),
            ("bundle", format!(r#"{{"nonce":"{}","box":"AAAA"}}"#, BASE64.encode([3u8; 24]))),
            ("sign_public_key", key.clone()),
            ("box_public_key", key),
        ]);
        let credentials = read_credentials(&form).unwrap();
        assert_eq!(credentials.kdf, DEFAULT_KDF);
        assert_eq!(credentials.auth_hash, sha256_hex([1u8; 32]));

        form.insert("kdf_memory_kib", "1024".into());
        assert!(read_credentials(&form).unwrap_err().contains("settings are refused: memory"));
        form.insert("kdf_memory_kib", "1e6".into());
        assert!(read_credentials(&form).unwrap_err().contains("settings are refused"));
        form.insert("kdf_memory_kib", "65536".into());
        form.insert("bundle", r#"{"nonce":"short","box":"AAAA"}"#.into());
        assert!(read_credentials(&form).unwrap_err().contains("owner bundle"));
    }

    #[test]
    fn makes_up_stable_salts() {
        let one = prelogin_for("secret", "nobody@example.invalid", None);
        assert_eq!(one, prelogin_for("secret", "nobody@example.invalid", None));
        assert_ne!(one.salt, prelogin_for("secret", "other@example.invalid", None).salt);
        assert!(decodes(&one.salt, SALT_BYTES));
        assert_eq!(
            serde_json::to_string(&one).unwrap(),
            format!(r#"{{"salt":"{}","memoryKiB":65536,"iterations":3,"parallelism":1}}"#, one.salt)
        );
    }
}
