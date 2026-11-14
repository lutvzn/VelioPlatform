//! Web sessions (PLAN.md, "Routes"): a random token in the cookie
//! `__Host-velio` (HttpOnly, Secure, SameSite=Strict), stored as its SHA-256.
//! 30 days, renewed on use at most once a day.

use anyhow::Result;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rusqlite::{Connection, OptionalExtension, params};

use crate::AppState;
use crate::error::AppError;
use crate::http::is_api;
use crate::time::{DAY_MS, now_ms};
use crate::tokens::{new_token, sha256};

const COOKIE: &str = "__Host-velio";
pub const SESSION_MS: i64 = 30 * DAY_MS;

#[derive(Debug, Clone)]
pub struct Session {
    pub token_hash: Vec<u8>,
    pub account_id: String,
    pub email: String,
}

/// The session this request came with, if any; set by [`sessions`] on every web request.
#[derive(Debug, Clone, Default)]
pub struct CurrentSession(pub Option<Session>);

pub fn session_cookie(token: &str) -> HeaderValue {
    let max_age = SESSION_MS / 1000;
    HeaderValue::try_from(format!("{COOKIE}={token}; Max-Age={max_age}; Path=/; HttpOnly; Secure; SameSite=Strict"))
        .expect("A token is URL-safe base64")
}

/// Signs this browser out, as Hono's deleteCookie.
pub fn ended_cookie() -> HeaderValue {
    HeaderValue::from_static("__Host-velio=; Max-Age=0; Path=/; Secure")
}

/// A new session for the account: its token, for [`session_cookie`].
pub fn start_session(conn: &Connection, account_id: &str) -> Result<String> {
    let token = new_token();
    let now = now_ms();
    conn.execute(
        "INSERT INTO sessions (token_hash, account_id, created_at, last_used_at, expires_at) VALUES (?, ?, ?, ?, ?)",
        params![sha256(&token), account_id, now, now, now + SESSION_MS],
    )?;
    Ok(token)
}

/// Deletes a session, such as this browser's before it signs in again or out.
pub fn drop_session(conn: &Connection, session: Option<&Session>) -> Result<()> {
    if let Some(session) = session {
        conn.execute("DELETE FROM sessions WHERE token_hash = ?", [&session.token_hash])?;
    }
    Ok(())
}

fn cookie_in(headers: &HeaderMap) -> Option<String> {
    cookie_named(headers, COOKIE)
}

/// A cookie's value, unless it's empty.
pub fn cookie_named(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(name)?.strip_prefix('=').map(str::to_owned))
        .filter(|value| !value.is_empty())
}

fn sets_session_cookie(response: &Response) -> bool {
    response.headers().get_all(header::SET_COOKIE).iter().any(|value| value.as_bytes().starts_with(COOKIE.as_bytes()))
}

/// Reads the session cookie into [`CurrentSession`], renewing it once a day.
pub async fn sessions(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    // Assets need no session, so they cost no query.
    if is_api(req.uri().path()) || req.uri().path().starts_with("/static/") {
        return next.run(req).await;
    }
    let found = match cookie_in(req.headers()) {
        None => None,
        Some(token) => match state.db.call(move |conn| find_session(conn, token)).await {
            Ok(found) => found,
            Err(error) => return AppError::from(error).into_response(),
        },
    };
    let (session, renewed) = found.map_or((None, None), |(session, renewed)| (Some(session), renewed));
    req.extensions_mut().insert(CurrentSession(session));
    let mut response = next.run(req).await;
    // Unless the answer started or ended a session itself.
    if let Some(token) = renewed
        && !sets_session_cookie(&response)
    {
        response.headers_mut().append(header::SET_COOKIE, session_cookie(&token));
    }
    response
}

/// The session for a token, and the token again when it was renewed.
fn find_session(conn: &Connection, token: String) -> Result<Option<(Session, Option<String>)>> {
    let token_hash = sha256(&token);
    let now = now_ms();
    let row: Option<(String, i64, String)> = conn
        .query_row(
            "SELECT s.account_id, s.last_used_at, a.email FROM sessions s JOIN accounts a ON a.id = s.account_id
             WHERE s.token_hash = ? AND s.expires_at > ? AND a.closed_at IS NULL AND a.confirmed_at IS NOT NULL",
            params![token_hash, now],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((account_id, last_used_at, email)) = row else { return Ok(None) };
    let renewed = if now - last_used_at > DAY_MS {
        conn.execute(
            "UPDATE sessions SET last_used_at = ?, expires_at = ? WHERE token_hash = ?",
            params![now, now + SESSION_MS, token_hash],
        )?;
        Some(token)
    } else {
        None
    };
    Ok(Some((Session { token_hash, account_id, email }, renewed)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_cookie_among_others() {
        let mut headers = HeaderMap::new();
        headers.append(header::COOKIE, HeaderValue::from_static("a=1; __Host-velioX=no"));
        assert_eq!(cookie_in(&headers), None);
        headers.append(header::COOKIE, HeaderValue::from_static("b=2;  __Host-velio=tok-en_1"));
        assert_eq!(cookie_in(&headers).as_deref(), Some("tok-en_1"));
        assert_eq!(
            session_cookie("t").to_str().unwrap(),
            "__Host-velio=t; Max-Age=2592000; Path=/; HttpOnly; Secure; SameSite=Strict"
        );
    }
}
