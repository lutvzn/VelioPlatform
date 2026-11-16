//! Nuvio's edge functions (references/self-host/volumes/functions) at
//! /functions/v1, answering as they do. Kong checks no API key here and the
//! main function doesn't verify tokens (FUNCTIONS_VERIFY_JWT=false), so each
//! function reads the Authorization header itself.

use std::time::Instant;

use axum::Extension;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use serde_json::{Value, json};
use uuid::Uuid;

use super::auth::{self, Meta};
use crate::AppState;
use crate::http::ClientIp;
use crate::time::{iso, now_ms};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/functions/v1", any(missing_name))
        .route("/functions/v1/", any(missing_name))
        .route("/functions/v1/{name}", any(function))
        .route("/functions/v1/{name}/{*rest}", any(function_with_path))
}

/// The CORS headers each function sets on its answers.
#[derive(Clone, Copy)]
enum Cors {
    /// tv-logins-exchange and delete-account.
    ClientInfo,
    /// tracker-tv-logins-exchange.
    Tracker,
    /// health-check.
    Health,
}

fn answer(cors: Cors, status: StatusCode, body: Value) -> Response {
    let mut response = (status, body.to_string()).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    add_cors(headers, cors);
    response
}

fn add_cors(headers: &mut HeaderMap, cors: Cors) {
    let (allow_headers, methods) = match cors {
        Cors::ClientInfo => ("authorization, x-client-info, apikey, content-type", "POST, OPTIONS"),
        Cors::Tracker => ("authorization, content-type", "POST, OPTIONS"),
        Cors::Health => ("Content-Type", "GET, OPTIONS"),
    };
    headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    headers.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static(allow_headers));
    headers.insert(header::ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static(methods));
}

async fn missing_name() -> Response {
    plain_json(StatusCode::BAD_REQUEST, json!({ "msg": "missing function name in request" }))
}

fn plain_json(status: StatusCode, body: Value) -> Response {
    (status, [(header::CONTENT_TYPE, "application/json")], body.to_string()).into_response()
}

struct Call {
    state: AppState,
    ip: Option<ClientIp>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
}

async fn function(
    State(state): State<AppState>,
    ip: Option<Extension<ClientIp>>,
    Path(name): Path<String>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    run(&name, Call { state, ip: ip.map(|Extension(ip)| ip), method, headers, body }).await
}

/// The functions don't look at their path, so /functions/v1/<name>/<anything> is <name>.
async fn function_with_path(
    State(state): State<AppState>,
    ip: Option<Extension<ClientIp>>,
    Path((name, _)): Path<(String, String)>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    run(&name, Call { state, ip: ip.map(|Extension(ip)| ip), method, headers, body }).await
}

async fn run(name: &str, call: Call) -> Response {
    match name {
        "tv-logins-exchange" => tv_logins_exchange(call).await,
        "tracker-tv-logins-exchange" => tracker_tv_logins_exchange(call).await,
        "delete-account" => delete_account(call).await,
        "health-check" => health_check(call).await,
        // The edge runtime's answer when it has no such function.
        _ => plain_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "msg": format!("InvalidWorkerCreation: worker boot error: failed to bootstrap runtime: could not find an appropriate entrypoint for {name}") }),
        ),
    }
}

/// The Authorization header's token, as `Bearer <token>`.
fn bearer_token(headers: &HeaderMap, any_case: bool) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let token = if any_case {
        let (scheme, token) = value.split_once(char::is_whitespace)?;
        scheme.eq_ignore_ascii_case("bearer").then_some(token.trim_start())?
    } else {
        value.strip_prefix("Bearer ")?
    };
    (!token.is_empty()).then(|| token.to_owned())
}

// --- tv-logins-exchange ---

/// A TV that was shown a code trades it, once someone approved it, for a
/// session of the approver's. Velio also holds the account to its TV limit.
async fn tv_logins_exchange(call: Call) -> Response {
    let cors = Cors::ClientInfo;
    if call.method == Method::OPTIONS {
        return ok_text(cors);
    }
    if call.method != Method::POST {
        return answer(cors, StatusCode::METHOD_NOT_ALLOWED, json!({ "error": "Method not allowed" }));
    }
    let body: Value = match serde_json::from_slice(&call.body) {
        Ok(body) => body,
        Err(error) => return answer(cors, StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": error.to_string() })),
    };
    let code = body["code"].as_str().filter(|code| !code.is_empty());
    let nonce = body["device_nonce"].as_str().filter(|nonce| !nonce.is_empty());
    let (Some(code), Some(nonce)) = (code, nonce) else {
        return answer(cors, StatusCode::BAD_REQUEST, json!({ "error": "code and device_nonce are required" }));
    };
    let meta = Meta::of(call.ip.as_ref(), &call.headers);
    match exchange(&call.state, code, nonce, &meta).await {
        Ok(tokens) => answer(
            cors,
            StatusCode::OK,
            json!({
                "access_token": tokens.access_token,
                "refresh_token": tokens.refresh_token,
                "token_type": "bearer",
                "expires_in": tokens.expires_in,
            }),
        ),
        Err((status, message)) => answer(cors, status, json!({ "error": message })),
    }
}

async fn exchange(
    state: &AppState,
    code: &str,
    nonce: &str,
    meta: &Meta,
) -> Result<auth::Tokens, (StatusCode, String)> {
    let internal = |error: &dyn std::fmt::Display| {
        tracing::error!("tv-logins-exchange: {error:#}");
        (StatusCode::INTERNAL_SERVER_ERROR, "Session exchange failed".to_owned())
    };
    let mut client = state.db.client().await.map_err(|error| internal(&error))?;
    let tx = client.transaction().await.map_err(|error| internal(&error))?;
    let consumed =
        tx.query_opt("SELECT approved_user_id FROM public.consume_tv_login_session($1, $2)", &[&code, &nonce]).await;
    let user_id: Option<Uuid> = match consumed {
        Ok(row) => row.and_then(|row| row.get(0)),
        // The RPC's own message, as the function passes it on.
        Err(error) => {
            let message = error.as_db_error().map_or_else(|| error.to_string(), |db| db.message().to_owned());
            return Err((StatusCode::BAD_REQUEST, message));
        }
    };
    let Some(user_id) = user_id else {
        return Err((StatusCode::BAD_REQUEST, "No approved user found".into()));
    };
    let user = auth::find_user(&tx, user_id).await.map_err(|error| internal(&error))?;
    if !user.as_ref().is_some_and(|user| !user.email.is_empty()) {
        return Err((StatusCode::BAD_REQUEST, "Approved user email not found".into()));
    }
    if user.as_ref().is_some_and(|user| user.banned) {
        return Err((StatusCode::INTERNAL_SERVER_ERROR, "User is banned".into()));
    }
    // Velio's TV limit: the sessions TVs got with a code, which end when the TV signs out.
    let row = tx
        .query_one(
            "SELECT (SELECT count(*) FROM velio_private.tv_sessions WHERE user_id = $1),
                    (SELECT tv_limit::int8 FROM velio_private.tv_limits WHERE user_id = $1)",
            &[&user_id],
        )
        .await
        .map_err(|error| internal(&error))?;
    let signed_in: i64 = row.get(0);
    let limit = state.cfg.tv_limits.for_account(row.get(1));
    if signed_in >= limit as i64 {
        // Rolled back, so the code stays approved for when a TV has signed out.
        return Err((
            StatusCode::FORBIDDEN,
            format!(
                "This account is signed in on {limit} TVs, its limit. Sign one out at {}/devices, then try again.",
                state.cfg.site_url
            ),
        ));
    }
    let tokens =
        auth::new_session(state, &tx, user_id, "magiclink", meta).await.map_err(|error| internal(&error.message))?;
    tx.execute(
        "INSERT INTO velio_private.tv_sessions (session_id, user_id) VALUES ($1, $2)",
        &[&tokens.session_id, &user_id],
    )
    .await
    .map_err(|error| internal(&error))?;
    tx.commit().await.map_err(|error| internal(&error))?;
    Ok(tokens)
}

fn ok_text(cors: Cors) -> Response {
    let mut response = (StatusCode::OK, "ok").into_response();
    add_cors(response.headers_mut(), cors);
    response
}

// --- tracker-tv-logins-exchange ---

fn text_or_none(value: &Value) -> Option<String> {
    value.as_str().filter(|text| !text.is_empty()).map(str::to_owned)
}

/// The phone posts a tracker's tokens for the TV that showed the code.
async fn tracker_tv_logins_exchange(call: Call) -> Response {
    let cors = Cors::Tracker;
    if call.method == Method::OPTIONS {
        let mut response = StatusCode::NO_CONTENT.into_response();
        add_cors(response.headers_mut(), cors);
        return response;
    }
    if call.method != Method::POST {
        return answer(cors, StatusCode::METHOD_NOT_ALLOWED, json!({ "error": "method_not_allowed" }));
    }
    let Some(token) = bearer_token(&call.headers, true) else {
        return answer(cors, StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" }));
    };
    let Ok(body) = serde_json::from_slice::<Value>(&call.body) else {
        return answer(cors, StatusCode::BAD_REQUEST, json!({ "error": "bad_json" }));
    };
    let code = text_or_none(&body["code"]);
    let tracker = text_or_none(&body["tracker"]);
    let access_token = text_or_none(&body["access_token"]);
    let (Some(code), Some(tracker), Some(access_token)) = (code, tracker, access_token) else {
        return answer(cors, StatusCode::BAD_REQUEST, json!({ "error": "missing_required_field" }));
    };
    if !matches!(tracker.as_str(), "mal" | "anilist" | "kitsu") {
        return answer(cors, StatusCode::BAD_REQUEST, json!({ "error": "bad_tracker" }));
    }
    let refresh_token = text_or_none(&body["refresh_token"]);
    // A JSON number of any kind, as the function's Number.isFinite check.
    let expires_in = body["expires_in"].as_f64().filter(|n| n.is_finite());
    let tracker_user_id = text_or_none(&body["tracker_user_id"]);
    let username = text_or_none(&body["username"]);

    let Ok(who) = auth::authenticate(&call.state, Some(&token)).await else {
        return answer(cors, StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" }));
    };
    let failed = |what: &str, error: &dyn std::fmt::Display| {
        answer(cors, StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": what, "detail": error.to_string() }))
    };
    let client = match call.state.db.client().await {
        Ok(client) => client,
        Err(error) => return failed("lookup_failed", &error),
    };
    let found = client
        .query_opt(
            "SELECT owner_user_id, expires_at < now() FROM public.tracker_tv_login_sessions
             WHERE code = $1 AND tracker = $2 LIMIT 1",
            &[&code, &tracker],
        )
        .await;
    let (owner, expired): (Option<Uuid>, bool) = match found {
        Ok(Some(row)) => (row.get(0), row.get(1)),
        Ok(None) => return answer(cors, StatusCode::NOT_FOUND, json!({ "error": "session_not_found" })),
        Err(error) => return failed("lookup_failed", &error),
    };
    if expired {
        let _ = client.execute("DELETE FROM public.tracker_tv_login_sessions WHERE code = $1", &[&code]).await;
        return answer(cors, StatusCode::GONE, json!({ "error": "session_expired" }));
    }
    if owner != Some(who.user_id) {
        return answer(cors, StatusCode::FORBIDDEN, json!({ "error": "owner_mismatch" }));
    }
    let updated = client
        .execute(
            "UPDATE public.tracker_tv_login_sessions SET status = 'ready', access_token = $2, refresh_token = $3,
                 expires_in = $4::float8::int, tracker_user_id = $5, tracker_username = $6
             WHERE code = $1",
            &[&code, &access_token, &refresh_token, &expires_in, &tracker_user_id, &username],
        )
        .await;
    if let Err(error) = updated {
        return failed(
            "update_failed",
            &error.as_db_error().map_or_else(|| error.to_string(), |db| db.message().to_owned()),
        );
    }
    answer(cors, StatusCode::OK, json!({ "ok": true }))
}

// --- delete-account ---

async fn delete_account(call: Call) -> Response {
    let cors = Cors::ClientInfo;
    if call.method == Method::OPTIONS {
        return ok_text(cors);
    }
    if call.method != Method::POST {
        return answer(cors, StatusCode::METHOD_NOT_ALLOWED, json!({ "error": "Method not allowed" }));
    }
    let Some(token) = bearer_token(&call.headers, false) else {
        return answer(cors, StatusCode::UNAUTHORIZED, json!({ "error": "Missing Authorization header" }));
    };
    let Ok(who) = auth::authenticate(&call.state, Some(&token)).await else {
        return answer(cors, StatusCode::UNAUTHORIZED, json!({ "error": "Invalid or expired session" }));
    };
    if let Err(error) = auth::delete_user(&call.state, who.user_id).await {
        return answer(cors, StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": error.message }));
    }
    answer(cors, StatusCode::OK, json!({ "success": true }))
}

// --- health-check ---

async fn health_check(call: Call) -> Response {
    let cors = Cors::Health;
    if call.method == Method::OPTIONS {
        let mut response = StatusCode::NO_CONTENT.into_response();
        add_cors(response.headers_mut(), cors);
        return response;
    }
    let start = Instant::now();
    let ping = async {
        let client = call.state.db.client().await?;
        client.query_one("SELECT public.health_ping()", &[]).await?;
        anyhow::Ok(())
    }
    .await;
    let latency_ms = start.elapsed().as_millis() as u64;
    let timestamp = iso(now_ms());
    match ping {
        Ok(()) => answer(
            cors,
            StatusCode::OK,
            json!({
                "status": if latency_ms < 3000 { "healthy" } else { "slow" },
                "database": "connected",
                "latency_ms": latency_ms,
                "timestamp": timestamp,
            }),
        ),
        Err(error) => answer(
            cors,
            StatusCode::SERVICE_UNAVAILABLE,
            json!({
                "status": "degraded",
                "database": "error",
                "error": error.to_string(),
                "latency_ms": latency_ms,
                "timestamp": timestamp,
            }),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_bearer_tokens() {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("bearer abc"));
        assert_eq!(bearer_token(&headers, true).as_deref(), Some("abc"));
        assert_eq!(bearer_token(&headers, false), None);
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer abc"));
        assert_eq!(bearer_token(&headers, false).as_deref(), Some("abc"));
    }
}
