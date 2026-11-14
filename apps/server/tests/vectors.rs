//! Replays the vectors sync-core writes (packages/sync-core/scripts/vectors.ts):
//! each change gives the state the TypeScript gave, or the same problem in
//! the same words, and each TV pulls what it pulled there.

use serde_json::Value;
use velio_server::sync_core::{DEFAULT_DEVICE_LIMIT, StoredState, apply_change, pull_for};
use velio_server::tokens::decode;

fn vectors() -> Value {
    serde_json::from_str(include_str!("vectors.json")).expect("vectors.json is JSON")
}

fn state(vectors: &Value, name: &str) -> StoredState {
    serde_json::from_value(vectors["states"][name].clone()).unwrap_or_else(|error| panic!("state {name}: {error}"))
}

/// Enough of a value to see where it differs.
fn short(value: &Value) -> String {
    let text = value.to_string();
    if text.chars().count() > 400 { format!("{}…", text.chars().take(400).collect::<String>()) } else { text }
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_else(|| panic!("no {key} in {value}"))
}

#[test]
fn every_change_comes_out_as_in_typescript() {
    let vectors = vectors();
    let account = text(&vectors, "account");
    let sign_public_key = decode(text(&vectors, "signPublicKey")).expect("a key");
    let cases = vectors["changes"].as_array().expect("changes");
    let mut wrong = Vec::new();
    for case in cases {
        let name = text(case, "name");
        let stored = state(&vectors, text(case, "stored"));
        let new_devices: Vec<String> = case
            .get("newDevices")
            .map(|devices| serde_json::from_value(devices.clone()).expect("TV ids"))
            .unwrap_or_default();
        let device_limit = case.get("deviceLimit").map_or(DEFAULT_DEVICE_LIMIT, |limit| {
            usize::try_from(limit.as_u64().expect("a TV limit")).expect("a TV limit")
        });
        let got = apply_change(&stored, &case["input"], account, &sign_public_key, &new_devices, device_limit);
        let expect = &case["expect"];
        match (got, expect.get("state")) {
            (Ok(applied), Some(after)) => {
                let after = vectors["states"][after.as_str().expect("a state name")].clone();
                let state = serde_json::to_value(&applied.state).expect("a state");
                if state != after {
                    wrong.push(format!(
                        "{name}: the state after isn't the same\n  got  {}\n  want {}",
                        short(&state),
                        short(&after)
                    ));
                }
            }
            (Ok(_), None) => wrong.push(format!("{name}: accepted, but TypeScript refused it: {}", expect["problem"])),
            (Err(problem), Some(_)) => wrong.push(format!(
                "{name}: refused ({}: {}), but TypeScript took it",
                problem.code.as_str(),
                problem.message
            )),
            (Err(problem), None) => {
                let want = &expect["problem"];
                let code_matches = problem.code.as_str() == text(want, "code");
                let message_matches = match want.get("messageStartsWith") {
                    Some(start) => problem.message.starts_with(start.as_str().expect("text")),
                    None => problem.message == text(want, "message"),
                };
                if !code_matches || !message_matches {
                    wrong.push(format!(
                        "{name}: refused as\n  got  {}: {}\n  want {want}",
                        problem.code.as_str(),
                        problem.message
                    ));
                }
            }
        }
    }
    assert!(cases.len() > 100, "only {} cases", cases.len());
    assert!(wrong.is_empty(), "{} of {} changes differ:\n{}", wrong.len(), cases.len(), wrong.join("\n"));
}

#[test]
fn every_tv_pulls_as_in_typescript() {
    let vectors = vectors();
    let cases = vectors["pulls"].as_array().expect("pulls");
    for case in cases {
        let name = text(case, "name");
        let stored = state(&vectors, text(case, "stored"));
        let got = serde_json::to_value(pull_for(&stored, text(case, "device"))).expect("a pull");
        assert_eq!(got, case["expect"], "{name}");
    }
    assert!(!cases.is_empty());
}
