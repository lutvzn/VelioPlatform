//! HS256 JSON Web Tokens, as Supabase Auth signs them with JWT_SECRET: the
//! access tokens it hands out, and the legacy anon and service_role API keys.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};

use crate::tokens::{hmac_sha256, same_digest};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JwtProblem {
    /// Not a token signed with this secret.
    Invalid,
    /// Signed with this secret, but past its `exp`.
    Expired,
}

pub fn sign(claims: &Value, secret: &str) -> String {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
    let signing_input = format!("{header}.{payload}");
    let signature = URL_SAFE_NO_PAD.encode(hmac_sha256(secret, &signing_input));
    format!("{signing_input}.{signature}")
}

/// The claims of a token signed with `secret` and not expired at `now` (seconds).
pub fn verify(token: &str, secret: &str, now: i64) -> Result<Value, JwtProblem> {
    let mut parts = token.split('.');
    let (Some(header), Some(payload), Some(signature), None) = (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(JwtProblem::Invalid);
    };
    let header: Value = URL_SAFE_NO_PAD
        .decode(header)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or(JwtProblem::Invalid)?;
    if header.get("alg").and_then(Value::as_str) != Some("HS256") {
        return Err(JwtProblem::Invalid);
    }
    let signature = URL_SAFE_NO_PAD.decode(signature).map_err(|_| JwtProblem::Invalid)?;
    let expected = hmac_sha256(secret, format!("{}.{payload}", token.split('.').next().unwrap_or_default()));
    if !same_digest(signature, expected) {
        return Err(JwtProblem::Invalid);
    }
    let claims: Value = URL_SAFE_NO_PAD
        .decode(payload)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .filter(Value::is_object)
        .ok_or(JwtProblem::Invalid)?;
    if let Some(exp) = claims.get("exp") {
        let exp = exp.as_f64().ok_or(JwtProblem::Invalid)?;
        // As PostgREST, with half a minute's leeway for clocks.
        if (now as f64) > exp + 30.0 {
            return Err(JwtProblem::Expired);
        }
    }
    Ok(claims)
}

/// A legacy API key, as Supabase's self-hosting guide makes them: a token
/// with only a role. Made the same each time, so it doesn't change when the
/// server restarts.
pub fn api_key(role: &str, secret: &str) -> String {
    // 2023-11-14 to 2100-01-01.
    sign(&json!({ "role": role, "iss": "supabase", "iat": 1_700_000_000, "exp": 4_102_444_800_i64 }), secret)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "super-secret-jwt-token-with-at-least-32-characters-long";

    #[test]
    fn verifies_what_it_signs() {
        let token = sign(&json!({ "sub": "u", "exp": 2000 }), SECRET);
        assert_eq!(verify(&token, SECRET, 1000).unwrap()["sub"], "u");
        assert_eq!(verify(&token, SECRET, 2031), Err(JwtProblem::Expired));
        assert_eq!(verify(&token, "another-secret", 1000), Err(JwtProblem::Invalid));
        let mut tampered = token.clone();
        tampered.insert(token.find('.').unwrap() + 2, 'x');
        assert_eq!(verify(&tampered, SECRET, 1000), Err(JwtProblem::Invalid));
        assert_eq!(verify("a.b", SECRET, 0), Err(JwtProblem::Invalid));
    }

    #[test]
    fn reads_supabase_demo_keys() {
        // The anon key of storage-api's tests (references/storage-api/.env.test.sample), and their secret.
        let anon = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJyb2xlIjoiYW5vbiIsImlhdCI6MTYxMzUzMTk4NSwiZXhwIjoxOTI5MTA3OTg1fQ.mqfi__KnQB4v6PkIjkhzfwWrYyF94MEbSC6LnuvVniE";
        let claims = verify(anon, "f023d3db-39dc-4ac9-87b2-b2be72e9162b", 1_700_000_000).unwrap();
        assert_eq!(claims["role"], "anon");
        assert!(api_key("anon", SECRET) == api_key("anon", SECRET));
    }
}
