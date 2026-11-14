//! Readers for JSON from outside (sync-core's `read.ts`). Each fails with a
//! [`ReadProblem`] naming what didn't fit, in the same words and the same
//! order as the TypeScript, so the vectors compare exactly.
//!
//! JavaScript's `.length` counts UTF-16 units, so text lengths here do too.
//! A missing field is `None`, as `undefined` is there; JSON's `null` is not.

use std::fmt;

use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadProblem(pub String);

impl fmt::Display for ReadProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub type Read<T> = Result<T, ReadProblem>;

pub fn problem<T>(message: impl Into<String>) -> Read<T> {
    Err(ReadProblem(message.into()))
}

/// The length JavaScript gives a string.
pub fn js_length(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The largest integer a JavaScript number holds exactly.
pub const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

pub fn record<'a>(value: Option<&'a Value>, what: &str) -> Read<&'a Map<String, Value>> {
    match value {
        Some(Value::Object(map)) => Ok(map),
        _ => problem(format!("{what} isn't an object")),
    }
}

pub const DEFAULT_MAX_TEXT: usize = 4096;

pub fn text<'a>(value: Option<&'a Value>, what: &str, max_length: usize) -> Read<&'a str> {
    let Some(Value::String(text)) = value else { return problem(format!("{what} isn't text")) };
    if js_length(text) > max_length {
        return problem(format!("{what} is too long"));
    }
    Ok(text)
}

/// A safe integer, 0 or more. `1.0` and `1e2` are numbers like any other in JavaScript.
pub fn count(value: Option<&Value>, what: &str) -> Read<u64> {
    let read = match value {
        Some(Value::Number(number)) => {
            if let Some(whole) = number.as_u64() {
                Some(whole).filter(|whole| *whole <= MAX_SAFE_INTEGER)
            } else if number.is_i64() {
                None
            } else {
                number
                    .as_f64()
                    .filter(|float| float.fract() == 0.0 && *float >= 0.0 && *float <= MAX_SAFE_INTEGER as f64)
                    .map(|float| float as u64)
            }
        }
        _ => None,
    };
    read.map_or_else(|| problem(format!("{what} isn't a count")), Ok)
}

pub const DEFAULT_MAX_ITEMS: usize = 1000;

pub fn array<'a>(value: Option<&'a Value>, what: &str, max_length: usize) -> Read<&'a [Value]> {
    let Some(Value::Array(items)) = value else { return problem(format!("{what} isn't a list")) };
    if items.len() > max_length {
        return problem(format!("{what} has too many items"));
    }
    Ok(items)
}

/// An array that may be left out, as pages from before Phase 3 send no TVs.
pub fn optional_array<'a>(value: Option<&'a Value>, what: &str, max_length: usize) -> Read<&'a [Value]> {
    match value {
        None => Ok(&[]),
        Some(_) => array(value, what, max_length),
    }
}

/// `^[A-Za-z0-9_-]{1,40}$`: ids are server-visible, so they are short random text.
pub fn is_id(text: &str) -> bool {
    (1..=40).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

pub fn id(value: Option<&Value>, what: &str) -> Read<String> {
    let read = text(value, what, 40)?;
    if !is_id(read) {
        return problem(format!("{what} isn't an id"));
    }
    Ok(read.to_owned())
}

/// `format !== 1`, as JavaScript compares: 1.0 is 1.
pub fn is_format_one(value: Option<&Value>) -> bool {
    value.and_then(Value::as_f64) == Some(1.0)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn counts_as_javascript_does() {
        for good in [json!(0), json!(1.0), json!(1e2), json!(-0.0), json!(9_007_199_254_740_991u64)] {
            assert!(count(Some(&good), "It").is_ok(), "{good}");
        }
        for bad in [json!(-1), json!(1.5), json!("1"), json!(9_007_199_254_740_992u64), json!(null)] {
            assert_eq!(count(Some(&bad), "It"), problem("It isn't a count"), "{bad}");
        }
        assert_eq!(count(None, "It"), problem("It isn't a count"));
    }

    #[test]
    fn measures_text_in_utf16_units() {
        let emoji = json!("😀".repeat(20));
        assert_eq!(text(Some(&emoji), "It", 40), Ok("😀".repeat(20).as_str()));
        assert_eq!(text(Some(&emoji), "It", 39), problem("It is too long"));
        assert!(id(Some(&json!("a-B_9")), "It").is_ok());
        assert_eq!(id(Some(&json!("a b")), "It"), problem("It isn't an id"));
        assert_eq!(id(Some(&json!("")), "It"), problem("It isn't an id"));
    }
}
