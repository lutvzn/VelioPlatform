//! Checking the owner's signature (sync-core's `crypto.ts`, `bytes.ts`). The
//! errors read as JavaScript's `String(error)` does, "CryptoProblem: …", as
//! `applyChange()` puts them in its refusal.

use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::Value;

use crate::tokens::decode;

/// A JSON document and an Ed25519 signature over its exact bytes, both base64.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Signed {
    pub body: String,
    pub signature: String,
}

/// Base64 from outside, at an exact size when given.
fn bytes_of(text: &str, what: &str, length: Option<usize>) -> Result<Vec<u8>, String> {
    let bytes = decode(text).ok_or_else(|| format!("CryptoProblem: {what} isn't base64"))?;
    match length {
        Some(length) if bytes.len() != length => Err(format!("CryptoProblem: {what} must be {length} bytes")),
        _ => Ok(bytes),
    }
}

/// sync-core's `fromUtf8`: strict, with its messages.
pub fn from_utf8(bytes: &[u8]) -> Result<String, String> {
    let mut i = 0;
    while i < bytes.len() {
        let first = bytes[i];
        let (mut code, length) = match first {
            0x00..0x80 => (u32::from(first), 1),
            0xc2..0xe0 => (u32::from(first & 31), 2),
            0xe0..0xf0 => (u32::from(first & 15), 3),
            0xf0..0xf5 => (u32::from(first & 7), 4),
            _ => return Err("Error: Not UTF-8".into()),
        };
        if i + length > bytes.len() {
            return Err("Error: Not UTF-8: cut short".into());
        }
        for next in &bytes[i + 1..i + length] {
            if next & 0xc0 != 0x80 {
                return Err("Error: Not UTF-8".into());
            }
            code = (code << 6) | u32::from(next & 63);
        }
        let too_long = (length == 3 && code < 0x800) || (length == 4 && code < 0x10000);
        if too_long || code > 0x10ffff || (0xd800..=0xdfff).contains(&code) {
            return Err("Error: Not UTF-8".into());
        }
        i += length;
    }
    // Every sequence was checked as the TypeScript does, which is what Rust's check is too.
    String::from_utf8(bytes.to_vec()).map_err(|_| "Error: Not UTF-8".into())
}

/// Checks the signature, then reads the document.
pub fn verify_json(public_key: &[u8], signed: &Signed) -> Result<Value, String> {
    let key: [u8; 32] =
        public_key.try_into().map_err(|_| "CryptoProblem: A signing key must be 32 bytes".to_owned())?;
    let body = bytes_of(&signed.body, "A signed body", None)?;
    let signature = bytes_of(&signed.signature, "A signature", Some(64))?;
    let signature = Signature::from_slice(&signature).map_err(|_| "CryptoProblem: A signature must be 64 bytes")?;
    // TweetNaCl takes any key and answers false; a key that isn't a point matches nothing.
    let verified = VerifyingKey::from_bytes(&key).is_ok_and(|key| key.verify_strict(&body, &signature).is_ok());
    if !verified {
        return Err("CryptoProblem: The signature doesn't match".into());
    }
    let text = from_utf8(&body)?;
    serde_json::from_str(&text).map_err(|error| format!("SyntaxError: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_utf8_as_sync_core_does() {
        assert_eq!(from_utf8("Grüße 😀".as_bytes()), Ok("Grüße 😀".into()));
        assert_eq!(from_utf8(&[0xc0, 0x80]), Err("Error: Not UTF-8".into()));
        assert_eq!(from_utf8(&[0xe2, 0x82]), Err("Error: Not UTF-8: cut short".into()));
        assert_eq!(from_utf8(&[0xed, 0xa0, 0x80]), Err("Error: Not UTF-8".into()));
        assert_eq!(from_utf8(&[0xf4, 0x90, 0x80, 0x80]), Err("Error: Not UTF-8".into()));
    }
}
