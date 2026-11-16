//! Web sessions (PLAN.md, "Routes"): a random token in the cookie
//! `__Host-velio` (HttpOnly, Secure, SameSite=Strict), stored as its SHA-256
//! in `velio_private.web_sessions`. Each is tied to a session of Supabase
//! Auth's (`auth.sessions`), the kind Nuvio's apps get, so it shows on the
//! devices page and ends when that session does. 30 days, renewed on use at
//! most once a day.

use anyhow::Result;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use deadpool_postgres::GenericClient;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::AppState;
use crate::error::AppError;
use crate::http::is_api;
use crate::time::DAY_MS;
use crate::tokens::{new_token, sha256};

const COOKIE: &str = "__Host-velio";
pub const SESSION_MS: i64 = 30 * DAY_MS;

#[derive(Debug, Clone)]
pub struct Session {
    pub token_hash: Vec<u8>,
    /// The user's id, as text.
    pub account_id: String,
    pub user_id: Uuid,
    /// The session of Supabase Auth's this one stands on.
    pub session_id: Uuid,
    pub email: String,
}

impl Session {
    /// The claims an access token for this session would carry, so the
    /// website's queries run as the user, under Nuvio's policies.
    pub fn claims(&self) -> Value {
        user_claims(self.user_id, self.session_id, &self.email)
    }
}

/// The claims an access token for a user's session carries, as far as
/// Nuvio's policies and functions read them.
pub fn user_claims(user_id: Uuid, session_id: Uuid, email: &str) -> Value {
    json!({
        "sub": user_id.to_string(),
        "role": "authenticated",
        "aud": "authenticated",
        "email": email,
        "session_id": session_id.to_string(),
        "is_anonymous": false,
        "aal": "aal1",
    })
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

/// A new web session on a session of Auth's: its token, for [`session_cookie`].
pub async fn start_session(db: &impl GenericClient, user_id: Uuid, session_id: Uuid) -> Result<String> {
    let token = new_token();
    db.execute(
        "INSERT INTO velio_private.web_sessions (token_hash, session_id, user_id, expires_at)
         VALUES ($1, $2, $3, now() + make_interval(secs => $4))",
        &[&sha256(&token), &session_id, &user_id, &((SESSION_MS / 1000) as f64)],
    )
    .await?;
    Ok(token)
}

/// Ends a session, such as this browser's before it signs in again or out,
/// with the session of Auth's it stands on.
pub async fn drop_session(db: &impl GenericClient, session: Option<&Session>) -> Result<()> {
    if let Some(session) = session {
        db.execute("DELETE FROM auth.sessions WHERE id = $1", &[&session.session_id]).await?;
        db.execute("DELETE FROM velio_private.web_sessions WHERE token_hash = $1", &[&session.token_hash]).await?;
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
    // Assets and the API need no session, so they cost no query.
    if is_api(req.uri().path()) || req.uri().path().starts_with("/static/") {
        return next.run(req).await;
    }
    let found = match cookie_in(req.headers()) {
        None => None,
        Some(token) => match find_session(&state, token).await {
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

/// The session for a token, and the token again when it was renewed. The
/// user must be confirmed and not banned (a closed account is banned), and
/// Auth's session must still be there.
async fn find_session(state: &AppState, token: String) -> Result<Option<(Session, Option<String>)>> {
    let token_hash = sha256(&token);
    let client = state.db.client().await?;
    let row = client
        .query_opt(
            "SELECT w.user_id, w.session_id, coalesce(u.email, ''), w.last_used_at < now() - interval '1 day'
             FROM velio_private.web_sessions w
             JOIN auth.users u ON u.id = w.user_id
             JOIN auth.sessions s ON s.id = w.session_id AND s.user_id = w.user_id
             WHERE w.token_hash = $1 AND w.expires_at > now()
               AND (s.not_after IS NULL OR s.not_after > now())
               AND u.email_confirmed_at IS NOT NULL AND u.deleted_at IS NULL
               AND NOT coalesce(u.banned_until > now(), false)",
            &[&token_hash],
        )
        .await?;
    let Some(row) = row else { return Ok(None) };
    let (user_id, session_id, email, stale): (Uuid, Uuid, String, bool) =
        (row.get(0), row.get(1), row.get(2), row.get(3));
    let renewed = if stale {
        client
            .execute(
                "UPDATE velio_private.web_sessions SET last_used_at = now(),
                     expires_at = now() + make_interval(secs => $2) WHERE token_hash = $1",
                &[&token_hash, &((SESSION_MS / 1000) as f64)],
            )
            .await?;
        // So the devices page shows when the browser was last used.
        client.execute("UPDATE auth.sessions SET updated_at = now() WHERE id = $1", &[&session_id]).await?;
        Some(token)
    } else {
        None
    };
    let session = Session { token_hash, account_id: user_id.to_string(), user_id, session_id, email };
    Ok(Some((session, renewed)))
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
