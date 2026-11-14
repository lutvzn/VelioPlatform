//! The routes TVs call (the Worker's `routes/tv.ts`; PLAN.md, "Signing in a
//! TV", "VelioTV pulling"): JSON, a bearer token, open CORS and no cookies.
//!
//!   POST /v1/pair       start signing in: the TV's key, its key and name sealed, and proofs
//!   GET  /v1/pair/{id}  wait (up to 20 s) for the owner; then the token and payload, once
//!   GET  /v1/sync       what the TV's profiles may read, or 304
//!   GET  /v1/events     a WebSocket: "rev N" on connecting and after each change
//!   POST /v1/signout    the TV forgets its keys: its token stops working and its sockets close

use std::time::Duration;

use anyhow::Result;
use axum::body::Bytes;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::{Path, RawQuery};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

use crate::AppState;
use crate::error::AppError;
use crate::events::{EVENTS_PROTOCOL, read_events_protocols};
use crate::http::api_problem;
use crate::limits::Limit;
use crate::time::now_ms;
use crate::tokens::{new_id, new_token, random_bytes, same_digest, sha256};
use crate::vault::{self, DevicePull};
use crate::web::Web;

type Answer = Result<Response, AppError>;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/pair", post(start_pairing))
        .route("/v1/pair/{id}", get(poll_pairing))
        .route("/v1/sync", get(sync))
        .route("/v1/signout", post(signout))
        .route("/v1/events", get(events))
}

// sync-core's pairing.ts.
pub const PAIRING_ID_LENGTH: usize = 4;
pub const PAIRING_MS: i64 = 10 * 60_000;
/// Crockford's base32: no I, L, O or U.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A sealed device info is 256 bytes padded: about 400 characters of base64.
const MAX_SEALED_LENGTH: usize = 1000;
const MAX_WAIT_S: f64 = 20.0;
const POLL_EVERY_MS: i64 = 1000;
/// last_pulled_at is written at most this often, as TVs pull every 15 minutes or on each change.
const PULLED_EVERY_MS: i64 = 10 * 60_000;

const SIGN_IN_AGAIN: &str = "Sign in again.";
const REMOVED: &str = "This TV was removed from the account. Sign in again.";
const NO_SUCH_SIGN_IN: &str = "No such sign-in.";
const PICKED_UP: &str = "This sign-in was already picked up.";

pub fn new_pairing_id() -> String {
    random_bytes::<PAIRING_ID_LENGTH>().iter().map(|byte| char::from(ALPHABET[usize::from(byte & 31)])).collect()
}

/// PAIRING_ID_PATTERN: four characters of [`ALPHABET`].
pub fn is_pairing_id(id: &str) -> bool {
    id.len() == PAIRING_ID_LENGTH && id.bytes().all(|byte| ALPHABET.contains(&byte))
}

/// 32 bytes in base64: a public key or a pairing proof (`^[A-Za-z0-9+/]{43}=$`).
fn is_base64_32(text: &str) -> bool {
    text.len() == 44
        && text.ends_with('=')
        && text[..43].bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'+' || byte == b'/')
}

/// A device or poll token's shape (`^[A-Za-z0-9_-]{20,100}$`).
fn is_token(text: &str) -> bool {
    (20..=100).contains(&text.len())
        && text.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    value.strip_prefix("Bearer ").filter(|token| is_token(token))
}

/// sync-core's isSealed: a record with a string nonce and box.
pub(crate) fn is_sealed(value: &Value) -> bool {
    value.get("nonce").is_some_and(Value::is_string) && value.get("box").is_some_and(Value::is_string)
}

/// As JavaScript counts a string's length.
fn js_length(text: &str) -> usize {
    text.encode_utf16().count()
}

pub(crate) fn json_length(value: &Value) -> usize {
    js_length(&value.to_string())
}

fn read_proofs(value: Option<&Value>) -> Option<(&str, &str)> {
    let qr = value?.get("qr")?.as_str().filter(|qr| is_base64_32(qr))?;
    let code = value?.get("code")?.as_str().filter(|code| is_base64_32(code))?;
    Some((qr, code))
}

fn read_sealed_device(value: Option<&Value>) -> Option<Value> {
    let (qr, code) = (value?.get("qr")?, value?.get("code")?);
    let fits = |sealed: &Value| is_sealed(sealed) && json_length(sealed) <= MAX_SEALED_LENGTH;
    (fits(qr) && fits(code)).then(|| json!({ "qr": qr, "code": code }))
}

struct Device {
    id: String,
    account_id: String,
    last_pulled_at: Option<i64>,
}

/// The signed-in TV a device token belongs to, in an account that isn't closed.
fn device_by_token(conn: &Connection, token: &str) -> Result<Option<Device>> {
    Ok(conn
        .query_row(
            "SELECT d.id, d.account_id, d.last_pulled_at FROM devices d JOIN accounts a ON a.id = d.account_id
             WHERE d.token_hash = ? AND a.closed_at IS NULL",
            [sha256(token)],
            |row| Ok(Device { id: row.get(0)?, account_id: row.get(1)?, last_pulled_at: row.get(2)? }),
        )
        .optional()?)
}

// --- Pairing ---

async fn start_pairing(web: Web, body: Bytes) -> Answer {
    if !web.within(Limit::Auth, &format!("pair:{}", web.ip)) {
        return Ok(api_problem(StatusCode::TOO_MANY_REQUESTS, "Too many tries. Wait a minute."));
    }
    let Ok(Value::Object(body)) = serde_json::from_slice::<Value>(&body) else {
        return Ok(api_problem(StatusCode::BAD_REQUEST, "Send JSON."));
    };
    let sealed = read_sealed_device(body.get("sealed"));
    let proofs = read_proofs(body.get("proofs"));
    let Some(public_key) = body.get("publicKey").and_then(Value::as_str).filter(|key| is_base64_32(key)) else {
        return Ok(api_problem(StatusCode::BAD_REQUEST, "No public key."));
    };
    // The TV's name is only in `sealed`, for the owner's browser to open. A
    // plain `name`, which VelioTV builds from before that still send, is
    // ignored, so the server never holds it.
    let (Some(sealed), Some((qr, code))) = (sealed, proofs) else {
        return Ok(api_problem(StatusCode::BAD_REQUEST, "No sealed details or proofs."));
    };

    let poll_token = new_token();
    let row = (sha256(&poll_token), sha256(qr), sha256(code), public_key.to_owned(), sealed.to_string());
    let now = now_ms();
    let started = web
        .state
        .db
        .call(move |conn| {
            let (poll_hash, qr_hash, code_hash, public_key, sealed) = row;
            // Ids are short, so one might be taken by another TV in its 10 minutes.
            for _ in 0..8 {
                let id = new_pairing_id();
                let tx = conn.transaction()?;
                tx.execute("DELETE FROM pairings WHERE id = ? AND expires_at <= ?", params![id, now])?;
                let inserted = tx.execute(
                    "INSERT INTO pairings (id, poll_hash, proof_qr_hash, proof_code_hash, device_id, public_key,
                       sealed_device, created_at, expires_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT (id) DO NOTHING",
                    params![id, poll_hash, qr_hash, code_hash, new_id(), public_key, sealed, now, now + PAIRING_MS],
                )?;
                tx.commit()?;
                if inserted == 1 {
                    return Ok(Some(id));
                }
            }
            Ok(None)
        })
        .await?;
    let Some(id) = started else {
        tracing::error!("No free pairing id after 8 tries");
        return Ok(api_problem(StatusCode::TOO_MANY_REQUESTS, "Velio is busy. Try again in a minute."));
    };
    let answer = json!({ "id": id, "pollToken": poll_token, "expiresAt": now + PAIRING_MS });
    Ok((StatusCode::CREATED, Json(answer)).into_response())
}

/// `?wait=` as JavaScript's `Number(...) || 0` reads it, from 0 to 20 seconds; 20 when missing.
fn wait_seconds(query: Option<&str>) -> f64 {
    let wait = form_urlencoded::parse(query.unwrap_or("").as_bytes())
        .find(|(key, _)| key == "wait")
        .map(|(_, value)| value.trim().parse::<f64>().ok().filter(|seconds| !seconds.is_nan()).unwrap_or(0.0));
    wait.unwrap_or(MAX_WAIT_S).clamp(0.0, MAX_WAIT_S)
}

enum Poll {
    NoSuchSignIn,
    PickedUp,
    Expired,
    Waiting(i64),
    Done(Value),
}

fn check_pairing(conn: &Connection, id: &str, token_hash: &[u8], until: i64) -> Result<Poll> {
    type Row = (Vec<u8>, i64, Option<i64>, Option<i64>, String, Option<String>, Option<String>, Option<String>);
    let row: Option<Row> = conn
        .query_row(
            "SELECT poll_hash, expires_at, completed_at, delivered_at, device_id, kind, payload, device_token
             FROM pairings WHERE id = ?",
            [id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .optional()?;
    let Some((poll_hash, expires_at, completed_at, delivered_at, device_id, kind, payload, device_token)) = row else {
        return Ok(Poll::NoSuchSignIn);
    };
    if !same_digest(&poll_hash, token_hash) {
        return Ok(Poll::NoSuchSignIn);
    }
    if delivered_at.is_some() {
        return Ok(Poll::PickedUp);
    }
    if let (Some(_), Some(payload), Some(device_token), Some(kind)) = (completed_at, payload, device_token, kind)
        && !payload.is_empty()
        && !device_token.is_empty()
        && !kind.is_empty()
    {
        // Hand the token over once, then forget it.
        let taken = conn.execute(
            "UPDATE pairings SET delivered_at = ?, device_token = NULL, payload = NULL WHERE id = ? AND delivered_at IS NULL",
            params![now_ms(), id],
        )?;
        if taken != 1 {
            return Ok(Poll::PickedUp);
        }
        let payload: Value = serde_json::from_str(&payload)?;
        return Ok(Poll::Done(json!({
            "status": "done",
            "deviceId": device_id,
            "deviceToken": device_token,
            "kind": kind,
            "payload": payload,
        })));
    }
    if expires_at <= now_ms() {
        return Ok(Poll::Expired);
    }
    if now_ms() + POLL_EVERY_MS > until {
        return Ok(Poll::Waiting(expires_at));
    }
    Ok(Poll::Waiting(-1))
}

async fn poll_pairing(web: Web, Path(id): Path<String>, RawQuery(query): RawQuery, headers: HeaderMap) -> Answer {
    let Some(token) = bearer(&headers).filter(|_| is_pairing_id(&id)) else {
        return Ok(api_problem(StatusCode::NOT_FOUND, NO_SUCH_SIGN_IN));
    };
    // Each poll waits up to 20 s, so a TV needs about three a minute.
    if !web.within(Limit::Auth, &format!("pair-poll:{}", web.ip)) {
        let problem = "Too many checks. Wait for each answer before asking again.";
        return Ok(api_problem(StatusCode::TOO_MANY_REQUESTS, problem));
    }
    let token_hash = sha256(token);
    let until = now_ms() + (wait_seconds(query.as_deref()) * 1000.0) as i64;
    loop {
        let poll = web.state.db.call({
            let (id, token_hash) = (id.clone(), token_hash.clone());
            move |conn| check_pairing(conn, &id, &token_hash, until)
        });
        match poll.await? {
            Poll::NoSuchSignIn => return Ok(api_problem(StatusCode::NOT_FOUND, NO_SUCH_SIGN_IN)),
            Poll::PickedUp => return Ok(api_problem(StatusCode::GONE, PICKED_UP)),
            Poll::Expired => return Ok(api_problem(StatusCode::GONE, "This sign-in expired. Start again.")),
            Poll::Done(answer) => return Ok(Json(answer).into_response()),
            Poll::Waiting(-1) => tokio::time::sleep(Duration::from_millis(POLL_EVERY_MS as u64)).await,
            Poll::Waiting(expires_at) => {
                return Ok(Json(json!({ "status": "waiting", "expiresAt": expires_at })).into_response());
            }
        }
    }
}

// --- Pulling ---

/// If-None-Match: "12" or W/"12" → 12.
fn known_rev(headers: &HeaderMap) -> Option<u64> {
    let value = headers.get(header::IF_NONE_MATCH)?.to_str().ok()?.trim();
    let quoted = value.strip_prefix("W/").unwrap_or(value);
    let digits = quoted.strip_prefix('"')?.strip_suffix('"')?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

async fn sync(web: Web, headers: HeaderMap) -> Answer {
    let Some(token) = bearer(&headers).map(str::to_owned) else {
        return Ok(api_problem(StatusCode::UNAUTHORIZED, SIGN_IN_AGAIN));
    };
    let known = known_rev(&headers);
    let pulled = web
        .state
        .db
        .call(move |conn| {
            let Some(device) = device_by_token(conn, &token)? else { return Ok(None) };
            let pull = vault::device_pull(conn, &device.account_id, &device.id, known)?;
            let now = now_ms();
            if !matches!(pull, DevicePull::Removed) && device.last_pulled_at.is_none_or(|at| now - at > PULLED_EVERY_MS)
            {
                conn.execute("UPDATE devices SET last_pulled_at = ? WHERE id = ?", params![now, device.id])?;
            }
            Ok(Some(pull))
        })
        .await?;
    let Some(pull) = pulled else { return Ok(api_problem(StatusCode::UNAUTHORIZED, SIGN_IN_AGAIN)) };
    let (rev, mut response) = match pull {
        DevicePull::Removed => return Ok(api_problem(StatusCode::UNAUTHORIZED, REMOVED)),
        DevicePull::Same(rev) => (rev, StatusCode::NOT_MODIFIED.into_response()),
        DevicePull::Pull(rev, pull) => (rev, Json(pull).into_response()),
    };
    let headers = response.headers_mut();
    headers.insert(header::ETAG, HeaderValue::try_from(format!("\"{rev}\""))?);
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

// --- Signing out ---

/// The TV signs itself out. Its token is replaced with a value no SHA-256
/// matches, so it opens nothing even if a copy was kept, and its sockets
/// close. The TV stays in the signed manifest, with the profile keys boxed
/// to it, until the owner removes it on the TVs page, which shows it as
/// signed out: only the owner can sign a change. Closed accounts are
/// included, so a TV signed out there doesn't come back if it's reopened.
async fn signout(web: Web, headers: HeaderMap) -> Answer {
    let Some(token) = bearer(&headers).map(str::to_owned) else {
        return Ok(api_problem(StatusCode::UNAUTHORIZED, SIGN_IN_AGAIN));
    };
    let device = web
        .state
        .db
        .call(move |conn| {
            let device: Option<(String, String)> = conn
                .query_row("SELECT id, account_id FROM devices WHERE token_hash = ?", [sha256(&token)], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .optional()?;
            if let Some((id, _)) = &device {
                conn.execute(
                    "UPDATE devices SET token_hash = ?, signed_out_at = ? WHERE id = ?",
                    params![format!("signed-out:{id}"), now_ms(), id],
                )?;
            }
            Ok(device)
        })
        .await?;
    let Some((device_id, account_id)) = device else {
        return Ok(api_problem(StatusCode::UNAUTHORIZED, SIGN_IN_AGAIN));
    };
    web.state.sockets.close_device(&account_id, &device_id);
    Ok(StatusCode::NO_CONTENT.into_response())
}

// --- Live updates ---

/// The TV's socket. The token comes as the subprotocol "bearer.<token>", as
/// a browser's WebSocket can't send headers. A refusal is a plain HTTP
/// answer, which the TV only sees as a failed connection: it keeps polling,
/// and /v1/sync tells it whether to sign in again.
async fn events(web: Web, headers: HeaderMap, upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>) -> Answer {
    let wants_socket = headers
        .get(header::UPGRADE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"));
    if !wants_socket {
        return Ok(api_problem(StatusCode::UPGRADE_REQUIRED, "This address takes a WebSocket."));
    }
    if !web.within(Limit::Auth, &format!("events:{}", web.ip)) {
        return Ok(api_problem(StatusCode::TOO_MANY_REQUESTS, "Too many connections. Try again in a minute."));
    }
    let offered = headers.get(header::SEC_WEBSOCKET_PROTOCOL).and_then(|value| value.to_str().ok()).unwrap_or("");
    let Some(token) = read_events_protocols(offered).filter(|token| is_token(token)).map(str::to_owned) else {
        return Ok(api_problem(StatusCode::UNAUTHORIZED, SIGN_IN_AGAIN));
    };
    let found = web
        .state
        .db
        .call({
            let token = token.clone();
            move |conn| {
                let Some(device) = device_by_token(conn, &token)? else { return Ok(None) };
                let rev = vault::device_rev(conn, &device.account_id, &device.id)?;
                Ok(Some((device, rev)))
            }
        })
        .await?;
    let Some((device, rev)) = found else { return Ok(api_problem(StatusCode::UNAUTHORIZED, SIGN_IN_AGAIN)) };
    if rev.is_none() {
        return Ok(api_problem(StatusCode::UNAUTHORIZED, REMOVED));
    }
    let upgrade = match upgrade {
        Ok(upgrade) => upgrade,
        Err(rejection) => return Ok(rejection.into_response()),
    };
    let registered = web.state.sockets.register(&device.account_id, &device.id);
    let db = web.state.db.clone();
    Ok(upgrade.protocols([EVENTS_PROTOCOL]).on_upgrade(move |socket| {
        // Read again once registered, so a change or signout meanwhile isn't missed.
        let current = async move {
            let read = db.call(move |conn| match device_by_token(conn, &token)? {
                Some(device) => vault::device_rev(conn, &device.account_id, &device.id),
                None => Ok(None),
            });
            read.await.ok().flatten()
        };
        registered.serve(socket, current)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_ids_tokens_and_waits_as_the_worker() {
        for _ in 0..50 {
            assert!(is_pairing_id(&new_pairing_id()));
        }
        assert!(!is_pairing_id("ABCI"));
        assert!(!is_pairing_id("abcd"));
        assert!(!is_pairing_id("ABCDE"));
        assert_eq!(new_id().len(), 16);
        assert!(is_base64_32(&format!("{}=", "A".repeat(43))));
        assert!(!is_base64_32(&"A".repeat(44)));
        assert!(is_token(&new_token()));
        assert!(!is_token("short"));
        assert_eq!(wait_seconds(None), 20.0);
        assert_eq!(wait_seconds(Some("wait=0")), 0.0);
        assert_eq!(wait_seconds(Some("wait=99")), 20.0);
        assert_eq!(wait_seconds(Some("wait=abc")), 0.0);
        assert_eq!(wait_seconds(Some("wait=-3")), 0.0);
    }

    #[test]
    fn reads_the_revision_a_tv_has() {
        let with = |value: &'static str| {
            let mut headers = HeaderMap::new();
            headers.insert(header::IF_NONE_MATCH, HeaderValue::from_static(value));
            known_rev(&headers)
        };
        assert_eq!(with("\"12\""), Some(12));
        assert_eq!(with("W/\"12\""), Some(12));
        assert_eq!(with("12"), None);
        assert_eq!(with("\"1x\""), None);
        assert_eq!(known_rev(&HeaderMap::new()), None);
    }
}
