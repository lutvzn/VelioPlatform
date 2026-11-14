//! Ciphertext as the database keeps it: the bytes themselves, not base64 in
//! JSON, which is about a third bigger.
//!
//! A sealed value (sync-core's `Sealed`, `{nonce, box}`) is a BLOB of the byte
//! 1, the 24-byte nonce and the box. A signed manifest (`{body, signature}`) is
//! the byte 2, the 64-byte signature and the body. Anything that wouldn't come
//! back exactly as sent, such as base64 that isn't in its usual form, stays as
//! its JSON text, which reads the same. Rows from before migration 4 are text
//! until it converts them.

use base64::Engine;
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, Value, ValueRef};

use crate::sync_core::{Sealed, Signed};
use crate::tokens::{BASE64, decode};

const SEALED: u8 = 1;
const SIGNED: u8 = 2;
const NONCE: usize = 24;
const SIGNATURE: usize = 64;

/// The bytes of base64 that encodes back to the same text.
fn exact(text: &str) -> Option<Vec<u8>> {
    let bytes = decode(text)?;
    (BASE64.encode(&bytes) == text).then_some(bytes)
}

fn tagged(tag: u8, head: &[u8], tail: &[u8]) -> Vec<u8> {
    let mut blob = Vec::with_capacity(1 + head.len() + tail.len());
    blob.push(tag);
    blob.extend_from_slice(head);
    blob.extend_from_slice(tail);
    blob
}

fn sealed_blob(sealed: &Sealed) -> Option<Vec<u8>> {
    let nonce = exact(&sealed.nonce).filter(|nonce| nonce.len() == NONCE)?;
    Some(tagged(SEALED, &nonce, &exact(&sealed.sealed)?))
}

fn to_json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("records are JSON")
}

pub fn pack_sealed(sealed: &Sealed) -> Value {
    sealed_blob(sealed).map_or_else(|| Value::Text(to_json(sealed)), Value::Blob)
}

pub fn pack_signed(signed: &Signed) -> Value {
    let blob = exact(&signed.signature)
        .filter(|signature| signature.len() == SIGNATURE)
        .zip(exact(&signed.body))
        .map(|(signature, body)| tagged(SIGNED, &signature, &body));
    blob.map_or_else(|| Value::Text(to_json(signed)), Value::Blob)
}

/// An owner bundle's JSON (`accounts::read_owner_bundle`), packed when it is
/// just a `Sealed`. It reads back as the same JSON, with its keys in
/// `Sealed`'s order.
pub fn pack_bundle(json: &str) -> Value {
    let sealed = serde_json::from_str::<serde_json::Value>(json).ok().and_then(|value| {
        let sealed = serde_json::from_value::<Sealed>(value.clone()).ok()?;
        (serde_json::to_value(&sealed).ok()? == value).then_some(sealed)
    });
    sealed.as_ref().and_then(sealed_blob).map_or_else(|| Value::Text(json.to_owned()), Value::Blob)
}

fn from_json<T: serde::de::DeserializeOwned>(text: &[u8]) -> FromSqlResult<T> {
    serde_json::from_slice(text).map_err(|error| FromSqlError::Other(Box::new(error)))
}

fn unsealed(blob: &[u8]) -> FromSqlResult<Sealed> {
    match blob {
        [SEALED, rest @ ..] if rest.len() >= NONCE => {
            let (nonce, sealed) = rest.split_at(NONCE);
            Ok(Sealed { nonce: BASE64.encode(nonce), sealed: BASE64.encode(sealed) })
        }
        _ => Err(FromSqlError::Other("Not a packed sealed value".into())),
    }
}

/// A stored `Sealed`, `Signed` or owner bundle, read from either form.
pub struct Packed<T>(pub T);

impl FromSql for Packed<Sealed> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value {
            ValueRef::Blob(blob) => unsealed(blob).map(Packed),
            ValueRef::Text(text) => from_json(text).map(Packed),
            _ => Err(FromSqlError::InvalidType),
        }
    }
}

impl FromSql for Packed<Signed> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value {
            ValueRef::Blob([SIGNED, rest @ ..]) if rest.len() >= SIGNATURE => {
                let (signature, body) = rest.split_at(SIGNATURE);
                Ok(Packed(Signed { body: BASE64.encode(body), signature: BASE64.encode(signature) }))
            }
            ValueRef::Blob(_) => Err(FromSqlError::Other("Not a packed signed manifest".into())),
            ValueRef::Text(text) => from_json(text).map(Packed),
            _ => Err(FromSqlError::InvalidType),
        }
    }
}

/// An owner bundle, as its JSON.
impl FromSql for Packed<String> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value {
            ValueRef::Blob(blob) => unsealed(blob).map(|sealed| Packed(to_json(&sealed))),
            ValueRef::Text(_) => String::column_result(value).map(Packed),
            _ => Err(FromSqlError::InvalidType),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn back<T>(value: Value) -> T
    where
        Packed<T>: FromSql,
    {
        Packed::<T>::column_result(ValueRef::from(&value)).unwrap().0
    }

    fn sealed(nonce: &[u8], sealed: &[u8]) -> Sealed {
        Sealed { nonce: BASE64.encode(nonce), sealed: BASE64.encode(sealed) }
    }

    #[test]
    fn sealed_values_are_their_bytes() {
        let value = sealed(&[7; 24], &[9; 1040]);
        let packed = pack_sealed(&value);
        let Value::Blob(blob) = &packed else { panic!("Not packed") };
        assert_eq!(blob.len(), 1 + 24 + 1040);
        assert!(blob.len() * 100 < to_json(&value).len() * 75);
        assert_eq!(back::<Sealed>(packed), value);
    }

    #[test]
    fn what_wouldnt_come_back_exactly_stays_json() {
        for value in [
            sealed(&[7; 12], &[9; 48]), // Another nonce size.
            // Without its padding.
            Sealed { nonce: BASE64.encode([7; 24]), sealed: BASE64.encode([9; 47]).trim_end_matches('=').into() },
            Sealed { nonce: BASE64.encode([7; 24]), sealed: "AAB=".into() }, // Trailing bits.
            Sealed { nonce: "not base64!".into(), sealed: String::new() },
        ] {
            let packed = pack_sealed(&value);
            assert!(matches!(packed, Value::Text(_)), "{value:?}");
            assert_eq!(back::<Sealed>(packed), value);
        }
    }

    #[test]
    fn manifests_and_bundles_come_back() {
        let signed = Signed { body: BASE64.encode(br#"{"format":1}"#), signature: BASE64.encode([5; 64]) };
        assert!(matches!(pack_signed(&signed), Value::Blob(_)));
        assert_eq!(back::<Signed>(pack_signed(&signed)), signed);
        let short = Signed { body: signed.body.clone(), signature: BASE64.encode([5; 32]) };
        assert_eq!(back::<Signed>(pack_signed(&short)), short);

        // read_owner_bundle's JSON has its keys sorted; it comes back in Sealed's order.
        let value = sealed(&[3; 24], &[4; 80]);
        let json = format!(r#"{{"box":"{}","nonce":"{}"}}"#, value.sealed, value.nonce);
        assert!(matches!(pack_bundle(&json), Value::Blob(_)));
        assert_eq!(back::<String>(pack_bundle(&json)), to_json(&value));
        let extra = format!(r#"{{"box":"{}","nonce":"{}","v":2}}"#, value.sealed, value.nonce);
        assert_eq!(pack_bundle(&extra), Value::Text(extra.clone()));
        assert_eq!(pack_bundle("{}"), Value::Text("{}".into()));
        assert_eq!(back::<String>(pack_bundle("{}")), "{}");
    }
}
