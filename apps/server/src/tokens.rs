//! Random tokens and their hashes, and base64 as sync-core reads it (the
//! Worker's `lib/tokens.ts`).

use base64::Engine;
use base64::alphabet;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

/// Standard base64, written with padding, as sync-core's `toBase64`.
pub const BASE64: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new()
        .with_decode_padding_mode(DecodePaddingMode::Indifferent)
        .with_decode_allow_trailing_bits(true),
);

const URL_SAFE: GeneralPurpose =
    GeneralPurpose::new(&alphabet::URL_SAFE, GeneralPurposeConfig::new().with_encode_padding(false));

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).expect("The system's random numbers are unavailable");
    bytes
}

/// 32 random bytes as URL-safe base64, for links and cookies.
pub fn new_token() -> String {
    URL_SAFE.encode(random_bytes::<32>())
}

/// 12 random bytes as URL-safe base64, as sync-core's `newId`: a TV's id in the manifest.
pub fn new_id() -> String {
    URL_SAFE.encode(random_bytes::<12>())
}

pub fn sha256_hex(data: impl AsRef<[u8]>) -> String {
    hex::encode(Sha256::digest(data.as_ref()))
}

/// Compares two hex digests in constant time.
pub fn same_digest(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

pub fn hmac_sha256(key: impl AsRef<[u8]>, message: impl AsRef<[u8]>) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_ref()).expect("HMAC takes a key of any length");
    mac.update(message.as_ref());
    mac.finalize().into_bytes().into()
}

/// Base64 that decodes to exactly `length` bytes, as sync-core's `fromBase64` reads it.
pub fn decodes(value: &str, length: usize) -> bool {
    decode(value).is_some_and(|bytes| bytes.len() == length)
}

pub fn decode(value: &str) -> Option<Vec<u8>> {
    BASE64.decode(value.trim_end_matches('=')).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_url_safe_and_hashes_compare() {
        let token = new_token();
        assert_eq!(token.len(), 43);
        assert!(token.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert_ne!(token, new_token());
        let hash = sha256_hex("abc");
        assert_eq!(hash, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert!(same_digest(&hash, &sha256_hex("abc")));
        assert!(!same_digest(&hash, &sha256_hex("abd")));
        assert!(!same_digest(&hash, "ba"));
    }

    #[test]
    fn reads_base64_as_sync_core_does() {
        let key = BASE64.encode([7u8; 32]);
        assert!(key.ends_with('='));
        assert!(decodes(&key, 32));
        assert!(decodes(key.trim_end_matches('='), 32));
        assert!(!decodes(&key, 16));
        assert!(!decodes("not base64!", 8));
        assert!(!decodes("", 32));
    }
}
