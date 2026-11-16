//! Nuvio's backend API, as its self-host serves it through Kong
//! (references/self-host/volumes/api): Supabase Auth at /auth/v1, PostgREST
//! at /rest/v1, Storage at /storage/v1, the edge functions at /functions/v1,
//! and the discovery document at /.well-known/nuvio. A Nuvio client pointed
//! at this server works as it does with Nuvio's own.

pub mod auth;
pub mod functions;
pub mod rest;
pub mod storage;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::AppState;
use crate::db::{Ctx, Role};
use crate::jwt::{self, JwtProblem};
use crate::time::now_ms;

/// The paths Nuvio's clients call, which answer as Supabase's services do.
pub fn is_supabase(path: &str) -> bool {
    ["/auth/v1", "/rest/v1", "/storage/v1", "/functions/v1"]
        .iter()
        .any(|prefix| path == *prefix || path.starts_with(&format!("{prefix}/")))
        || path == "/.well-known/nuvio"
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/.well-known/nuvio", get(discovery))
        .merge(auth::router())
        .merge(rest::router())
        .merge(storage::router())
        .merge(functions::router())
}

/// What a Nuvio client reads first, to find this backend and its key.
async fn discovery(State(state): State<AppState>) -> Response {
    let body = json!({
        "version": 1,
        "service": "nuvio",
        "self_hosted": true,
        "backend_url": state.cfg.site_url,
        "publishable_key": state.cfg.auth.public_key(),
        "capabilities": { "email_password_auth": true, "tv_login": true },
    });
    let mut response = (StatusCode::OK, body.to_string()).into_response();
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json; charset=utf-8"));
    response
}

/// A JSON answer with a status.
pub fn json_response(status: StatusCode, body: Value) -> Response {
    (status, Json(body)).into_response()
}

/// Seconds since 1970, as tokens count time.
pub fn now_seconds() -> i64 {
    now_ms() / 1000
}

// --- Kong's rules ---

/// The paths of /auth/v1 that need no API key, as Kong's open routes.
fn open_auth_path(path: &str) -> bool {
    ["/auth/v1/verify", "/auth/v1/callback", "/auth/v1/authorize", "/auth/v1/.well-known/jwks.json"]
        .iter()
        .any(|open| path == *open || path.starts_with(&format!("{open}/")))
}

fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok()).map(str::trim).filter(|value| !value.is_empty())
}

/// The `apikey` header, or the query's `apikey`, as Kong's key-auth reads it.
fn api_key_of(req: &Request) -> Option<String> {
    if let Some(key) = header_text(req.headers(), "apikey") {
        return Some(key.to_owned());
    }
    form_urlencoded::parse(req.uri().query().unwrap_or_default().as_bytes())
        .find(|(name, _)| name == "apikey")
        .map(|(_, value)| value.into_owned())
}

/// The token a request runs with, after Kong's Authorization rule: the
/// Authorization header unless it carries an sb_ key, else the API key, with
/// the sb_ keys standing for the legacy ones.
pub fn token_of(state: &AppState, headers: &HeaderMap, api_key: Option<&str>) -> Option<String> {
    let auth = &state.cfg.auth;
    if let Some(value) = header_text(headers, "authorization")
        && !value.starts_with("Bearer sb_")
    {
        let token = value.strip_prefix("Bearer ").or_else(|| value.strip_prefix("bearer ")).unwrap_or(value);
        return Some(token.trim().to_owned());
    }
    let key = api_key?;
    if auth.secret_key.as_deref() == Some(key) {
        return Some(auth.service_role_key.clone());
    }
    if auth.publishable_key.as_deref() == Some(key) {
        return Some(auth.anon_key.clone());
    }
    Some(key.to_owned())
}

/// Kong's key-auth on /auth/v1 and /rest/v1, its CORS on every API path, and
/// the token each request carries on to the service.
pub async fn gateway(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let path = req.uri().path().to_owned();
    if !is_supabase(&path) {
        return next.run(req).await;
    }
    if req.method() == Method::OPTIONS {
        return preflight(req.headers());
    }
    let keyed = (path.starts_with("/auth/v1") && !open_auth_path(&path)) || path.starts_with("/rest/v1");
    let api_key = api_key_of(&req);
    if keyed {
        let auth = &state.cfg.auth;
        let known = api_key.as_deref().is_some_and(|key| {
            key == auth.anon_key
                || key == auth.service_role_key
                || auth.publishable_key.as_deref() == Some(key)
                || auth.secret_key.as_deref() == Some(key)
        });
        if api_key.is_none() {
            return allow_any_origin(json_response(
                StatusCode::UNAUTHORIZED,
                json!({ "message": "No API key found in request" }),
            ));
        }
        if !known {
            return allow_any_origin(json_response(
                StatusCode::UNAUTHORIZED,
                json!({ "message": "Invalid authentication credentials" }),
            ));
        }
    }
    let token = token_of(&state, req.headers(), api_key.as_deref());
    req.extensions_mut().insert(Bearer(token));
    allow_any_origin(next.run(req).await)
}

/// The token Kong passed on, if any.
#[derive(Clone, Debug)]
pub struct Bearer(pub Option<String>);

/// Kong's cors plugin, as configured with no options.
fn preflight(headers: &HeaderMap) -> Response {
    let mut response = StatusCode::OK.into_response();
    let out = response.headers_mut();
    out.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    out.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET,HEAD,PUT,PATCH,POST,DELETE,OPTIONS,TRACE,CONNECT"),
    );
    if let Some(asked) = headers.get(header::ACCESS_CONTROL_REQUEST_HEADERS) {
        out.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, asked.clone());
    }
    out.insert(header::ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("86400"));
    response
}

fn allow_any_origin(mut response: Response) -> Response {
    response.headers_mut().insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    response
}

// --- Who is calling, as PostgREST and Storage decide it ---

/// Why a request's token isn't accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenProblem {
    Invalid,
    Expired,
    /// Signed, but for a role the database doesn't have.
    UnknownRole,
}

/// The role and claims a token gives, as PostgREST reads it. No token is anon.
pub fn caller(state: &AppState, token: Option<&str>) -> Result<(Role, Value), TokenProblem> {
    let Some(token) = token.filter(|token| !token.is_empty()) else {
        return Ok((Role::Anon, json!({ "role": "anon" })));
    };
    let claims = jwt::verify(token, &state.cfg.auth.jwt_secret, now_seconds()).map_err(|problem| match problem {
        JwtProblem::Expired => TokenProblem::Expired,
        JwtProblem::Invalid => TokenProblem::Invalid,
    })?;
    let role = match claims.get("role").and_then(Value::as_str) {
        None => Role::Anon,
        Some(name) => Role::from_name(name).ok_or(TokenProblem::UnknownRole)?,
    };
    Ok((role, claims))
}

/// The request's context for the database: role, claims, and the headers,
/// method and path, as PostgREST sets them.
pub fn ctx_for(role: Role, claims: Value, headers: &HeaderMap, method: &Method, path: &str) -> Ctx {
    let mut map = serde_json::Map::new();
    for (name, value) in headers {
        if let Ok(value) = value.to_str() {
            map.insert(name.as_str().to_ascii_lowercase(), Value::String(value.to_owned()));
        }
    }
    Ctx {
        role,
        claims,
        headers: Value::Object(map),
        method: method.as_str().to_owned(),
        path: path.to_owned(),
        read_only: *method == Method::GET || *method == Method::HEAD,
    }
}
