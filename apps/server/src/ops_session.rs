//! The operator's session on /ops (the Worker's `lib/opsSession.ts`):
//! signing in with the OPS_TOKEN secret gives a cookie that lasts 8 hours.
//! The cookie holds its expiry, a random part and an HMAC of both keyed by
//! OPS_TOKEN, so nothing is stored, and changing OPS_TOKEN ends every
//! session at once.
//!
//! The cookie is HttpOnly, Secure, SameSite=Strict, and sent only to /ops.
//! Calls that change something also need velio.tv's own Origin.

use axum::http::{HeaderMap, HeaderValue};

use crate::session::cookie_named;
use crate::time::HOUR_MS;
use crate::tokens::{hmac_sha256, random_bytes, same_digest, sha256_hex};

pub const OPS_COOKIE: &str = "__Secure-velio-ops";
pub const OPS_SESSION_MS: i64 = 8 * HOUR_MS;

/// Whether a token is the OPS_TOKEN secret, compared as SHA-256 digests in constant time.
pub fn is_ops_token(secret: &str, token: &str) -> bool {
    same_digest(&sha256_hex(token), &sha256_hex(secret))
}

fn mac(secret: &str, message: &str) -> String {
    hex::encode(hmac_sha256(format!("velio-ops-session.{secret}"), message))
}

/// A new cookie value for the operator, valid for OPS_SESSION_MS.
pub fn new_ops_session(secret: &str, now: i64) -> String {
    let expires = now + OPS_SESSION_MS;
    let nonce = hex::encode(random_bytes::<16>());
    let mac = mac(secret, &format!("{expires}.{nonce}"));
    format!("{expires}.{nonce}.{mac}")
}

fn is_hex(text: &str, length: usize) -> bool {
    text.len() == length && text.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Whether a cookie value was made with this secret and hasn't expired:
/// /^(\d{13})\.([0-9a-f]{32})\.([0-9a-f]{64})$/ and a matching HMAC.
pub fn valid_ops_session(secret: &str, value: &str, now: i64) -> bool {
    let mut parts = value.split('.');
    let (Some(expires), Some(nonce), Some(given), None) = (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    if expires.len() != 13 || !expires.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    if !is_hex(nonce, 32) || !is_hex(given, 64) {
        return false;
    }
    let Ok(expires_at) = expires.parse::<i64>() else { return false };
    if expires_at <= now || expires_at > now + OPS_SESSION_MS {
        return false;
    }
    same_digest(given, &mac(secret, &format!("{expires}.{nonce}")))
}

pub fn has_ops_session(secret: &str, headers: &HeaderMap, now: i64) -> bool {
    cookie_named(headers, OPS_COOKIE).is_some_and(|value| valid_ops_session(secret, &value, now))
}

pub fn ops_cookie(value: &str) -> HeaderValue {
    let max_age = OPS_SESSION_MS / 1000;
    HeaderValue::try_from(format!(
        "{OPS_COOKIE}={value}; Max-Age={max_age}; Path=/ops; HttpOnly; Secure; SameSite=Strict"
    ))
    .expect("Digits, dots and hex")
}

/// Ends the session, as Hono's deleteCookie with the same options.
pub fn ended_ops_cookie() -> HeaderValue {
    HeaderValue::from_static("__Secure-velio-ops=; Max-Age=0; Path=/ops; HttpOnly; Secure; SameSite=Strict")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_the_secret_and_the_expiry() {
        let now = 1_794_560_703_042;
        let value = new_ops_session("secret", now);
        assert!(valid_ops_session("secret", &value, now));
        assert!(valid_ops_session("secret", &value, now + OPS_SESSION_MS - 1));
        assert!(!valid_ops_session("secret", &value, now + OPS_SESSION_MS));
        assert!(!valid_ops_session("another", &value, now));
        assert!(!valid_ops_session("secret", &format!("{value}0"), now));
        assert!(!valid_ops_session("secret", &value.to_uppercase(), now));
        let forged = format!("{}.{}.{}", now + 60_000, "0".repeat(32), "0".repeat(64));
        assert!(!valid_ops_session("secret", &forged, now));
        assert!(is_ops_token("secret", "secret") && !is_ops_token("secret", "secret "));
    }
}
