//! Supabase Auth (GoTrue v2.189, references/gotrue) at /auth/v1: the part
//! Nuvio's apps and site use. Sign-up, guest sign-up, password and refresh
//! token sign-in, the user, sign-out, password recovery and email links.
//! Users, sessions and refresh tokens are GoTrue's own tables, written as
//! GoTrue writes them, so its tokens, Nuvio's SQL (`auth.uid()`, the session
//! checks) and its migrations all agree.
//!
//! The website calls the same functions directly, so a sign-in there is a
//! GoTrue session like any app's.

use std::net::IpAddr;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Router};
use deadpool_postgres::GenericClient;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha224};
use uuid::Uuid;

use super::{Bearer, json_response, now_seconds};
use crate::AppState;
use crate::db::Prepared;
use crate::http::ClientIp;
use crate::jwt;
use crate::limits::Limit;
use crate::mail;
use crate::tokens::random_bytes;

/// What GoTrue says it is (GET /auth/v1/health).
pub const GOTRUE_VERSION: &str = "v2.189.0";
/// How long an email link works.
pub const CONFIRM_SECONDS: i64 = 24 * 3600;
pub const RECOVERY_SECONDS: i64 = 3600;
/// GoTrue's REFRESH_TOKEN_REUSE_INTERVAL: a used refresh token still works
/// this long, for a client that didn't save the answer.
pub const REUSE_SECONDS: i64 = 10;
/// GoTrue's SMTP_MAX_FREQUENCY: one email a minute to an address.
pub const EMAIL_EVERY_SECONDS: i64 = 60;

// --- Errors, as GoTrue sends them ---

#[derive(Debug, Clone)]
pub struct AuthError {
    pub status: StatusCode,
    /// GoTrue's error_code, such as `invalid_credentials`.
    pub code: &'static str,
    pub message: String,
    /// For weak_password: why, such as `length`.
    pub reasons: Vec<&'static str>,
    /// An OAuth-style error (`{"error", "error_description"}`), as GoTrue
    /// answers a token request without its required field.
    pub oauth: bool,
}

impl AuthError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self { status, code, message: message.into(), reasons: Vec::new(), oauth: false }
    }

    fn bad(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    fn unprocessable(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, code, message)
    }

    fn forbidden(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, code, message)
    }

    fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!("Auth: {error:#}");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "unexpected_failure", "Internal server error")
    }

    fn oauth(error: &'static str, description: &str) -> Self {
        Self { oauth: true, ..Self::new(StatusCode::BAD_REQUEST, error, description) }
    }

    pub fn too_many() -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, "over_request_rate_limit", "Request rate limit reached")
    }

    /// The answer, in the shape the client's X-Supabase-Api-Version asks for.
    pub fn response(&self, version_2024: bool) -> Response {
        if self.oauth {
            return json_response(self.status, json!({ "error": self.code, "error_description": self.message }));
        }
        let weak = (!self.reasons.is_empty()).then(|| json!({ "reasons": self.reasons }));
        let mut body = if version_2024 {
            json!({ "code": self.code, "message": self.message })
        } else {
            json!({ "code": self.status.as_u16(), "error_code": self.code, "msg": self.message })
        };
        if let Some(weak) = weak {
            body["weak_password"] = weak;
        }
        let mut response = json_response(self.status, body);
        if let Ok(code) = HeaderValue::from_str(self.code) {
            response.headers_mut().insert("x-sb-error-code", code);
        }
        if version_2024 {
            response.headers_mut().insert("x-supabase-api-version", HeaderValue::from_static("2024-01-01"));
        }
        response
    }
}

impl From<tokio_postgres::Error> for AuthError {
    fn from(error: tokio_postgres::Error) -> Self {
        Self::internal(error)
    }
}

impl From<anyhow::Error> for AuthError {
    fn from(error: anyhow::Error) -> Self {
        Self::internal(error)
    }
}

/// Whether the client asked for GoTrue's 2024-01-01 error shape.
fn version_2024(headers: &HeaderMap) -> bool {
    headers
        .get("x-supabase-api-version")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|date| date.len() == 10 && date >= "2024-01-01")
}

fn reply(headers: &HeaderMap, result: Result<Response, AuthError>) -> Response {
    result.unwrap_or_else(|error| error.response(version_2024(headers)))
}

// --- Who is signing in, and from where ---

#[derive(Debug, Clone, Default)]
pub struct Meta {
    pub ip: Option<IpAddr>,
    pub user_agent: Option<String>,
}

impl Meta {
    pub fn of(ip: Option<&ClientIp>, headers: &HeaderMap) -> Self {
        let user_agent = headers
            .get(header::USER_AGENT)
            .and_then(|value| value.to_str().ok())
            .map(|agent| agent.chars().take(512).collect());
        Self { ip: ip.map(|ip| ip.ip).filter(|ip| !ip.is_unspecified()), user_agent }
    }

    fn ip_text(&self) -> Option<String> {
        self.ip.map(|ip| ip.to_string())
    }

    fn limit_key(&self, what: &str) -> String {
        format!("{what}:{}", self.ip.map(|ip| ip.to_string()).unwrap_or_default())
    }
}

// --- Emails and passwords ---

pub fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

/// GoTrue's check: an address of at most 255 characters with one @ and a dot in the domain.
pub fn valid_email(email: &str) -> bool {
    let email = email.trim();
    email.len() <= 255
        && !email.chars().any(char::is_whitespace)
        && email.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && !domain.contains('@')
                && domain.char_indices().any(|(at, c)| c == '.' && at > 0 && at + 1 < domain.len())
        })
}

fn password_problem(state: &AppState, password: &str) -> Option<AuthError> {
    let min = state.cfg.auth.password_min_length;
    if password.chars().count() < min {
        let mut error =
            AuthError::unprocessable("weak_password", format!("Password should be at least {min} characters."));
        error.reasons = vec!["length"];
        return Some(error);
    }
    if password.len() > 72 {
        return Some(AuthError::unprocessable("validation_failed", "Password cannot be longer than 72 characters"));
    }
    None
}

fn hash_password(password: &str) -> Result<String, AuthError> {
    bcrypt::hash(password, bcrypt::DEFAULT_COST).map_err(AuthError::internal)
}

/// Checked when there's no account, as long as a real check (the same cost),
/// so the answer's time doesn't tell whether the account exists.
static NO_ACCOUNT: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| bcrypt::hash("no account has this password", bcrypt::DEFAULT_COST).unwrap_or_default());

/// Makes NO_ACCOUNT's hash, which takes as long as a sign-in: called once at
/// start, off the async threads, so no request waits for it.
pub fn prepare() {
    std::sync::LazyLock::force(&NO_ACCOUNT);
}

/// A bcrypt check, in a blocking thread, as it takes a while on purpose.
async fn password_matches(password: String, hash: Option<String>) -> bool {
    tokio::task::spawn_blocking(move || match hash {
        Some(hash) if !hash.is_empty() => bcrypt::verify(password, &hash).unwrap_or(false),
        _ => {
            let _ = bcrypt::verify(password, &NO_ACCOUNT);
            false
        }
    })
    .await
    .unwrap_or(false)
}

/// GoTrue's token hash for email links: hex SHA-224 of the address and a one-time code.
pub fn token_hash(email: &str, otp: &str) -> String {
    hex::encode(Sha224::digest(format!("{email}{otp}").as_bytes()))
}

fn new_otp() -> String {
    let bytes = random_bytes::<8>();
    format!("{:06}", u64::from_le_bytes(bytes) % 1_000_000)
}

fn new_refresh_token() -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    random_bytes::<16>().iter().map(|byte| ALPHABET[usize::from(*byte) % ALPHABET.len()] as char).collect()
}

// --- Users, as GoTrue shows them ---

/// GoTrue's user JSON: what /user and every token answer carry.
const USER_JSON: &str = "jsonb_strip_nulls(jsonb_build_object(
    'id', u.id, 'aud', coalesce(u.aud, ''), 'role', coalesce(u.role, ''), 'email', coalesce(u.email, ''),
    'email_confirmed_at', u.email_confirmed_at, 'phone', coalesce(u.phone, ''),
    'confirmation_sent_at', u.confirmation_sent_at, 'confirmed_at', u.confirmed_at,
    'recovery_sent_at', u.recovery_sent_at, 'new_email', nullif(u.email_change, ''),
    'email_change_sent_at', u.email_change_sent_at, 'last_sign_in_at', u.last_sign_in_at,
    'app_metadata', coalesce(u.raw_app_meta_data, '{}'), 'user_metadata', coalesce(u.raw_user_meta_data, '{}'),
    'created_at', u.created_at, 'updated_at', u.updated_at, 'banned_until', u.banned_until,
    'is_anonymous', u.is_anonymous)) || jsonb_build_object('identities', (
        SELECT coalesce(jsonb_agg(jsonb_build_object(
            'identity_id', i.id, 'id', i.provider_id, 'user_id', i.user_id, 'identity_data', i.identity_data,
            'provider', i.provider, 'last_sign_in_at', i.last_sign_in_at, 'created_at', i.created_at,
            'updated_at', i.updated_at, 'email', i.email) ORDER BY i.created_at), '[]'::jsonb)
        FROM auth.identities i WHERE i.user_id = u.id))";

pub async fn user_json(db: &impl GenericClient, user_id: Uuid) -> Result<Option<Value>, tokio_postgres::Error> {
    let row = db.opt(&format!("SELECT {USER_JSON} FROM auth.users u WHERE u.id = $1"), &[&user_id]).await?;
    Ok(row.map(|row| row.get(0)))
}

/// A user's row, as sign-in needs it.
#[derive(Debug, Clone)]
pub struct UserRow {
    pub id: Uuid,
    pub email: String,
    pub encrypted_password: Option<String>,
    pub confirmed: bool,
    pub banned: bool,
    pub is_anonymous: bool,
}

const USER_ROW: &str = "SELECT id, coalesce(email, ''), encrypted_password, email_confirmed_at IS NOT NULL,
    coalesce(banned_until > now(), false), is_anonymous FROM auth.users";

fn user_row(row: &tokio_postgres::Row) -> UserRow {
    UserRow {
        id: row.get(0),
        email: row.get(1),
        encrypted_password: row.get(2),
        confirmed: row.get(3),
        banned: row.get(4),
        is_anonymous: row.get(5),
    }
}

pub async fn find_user_by_email(
    db: &impl GenericClient,
    email: &str,
) -> Result<Option<UserRow>, tokio_postgres::Error> {
    let row = db
        .opt(
            &format!("{USER_ROW} WHERE lower(email) = lower($1) AND NOT is_sso_user AND deleted_at IS NULL LIMIT 1"),
            &[&email],
        )
        .await?;
    Ok(row.as_ref().map(user_row))
}

pub async fn find_user(db: &impl GenericClient, id: Uuid) -> Result<Option<UserRow>, tokio_postgres::Error> {
    Ok(db.opt(&format!("{USER_ROW} WHERE id = $1"), &[&id]).await?.as_ref().map(user_row))
}

/// An address the operator closed, which can't sign up again.
pub async fn closed_email(db: &impl GenericClient, email: &str) -> Result<bool, tokio_postgres::Error> {
    Ok(db.opt("SELECT 1 FROM velio_private.closed_accounts WHERE lower(email) = lower($1)", &[&email]).await?.is_some())
}

// --- Sessions and tokens ---

/// What GoTrue answers a sign-in with (its AccessTokenResponse).
#[derive(Debug, Clone)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub expires_at: i64,
    pub user: Value,
    pub user_id: Uuid,
    pub session_id: Uuid,
}

impl Tokens {
    pub fn json(&self) -> Value {
        json!({
            "access_token": self.access_token,
            "token_type": "bearer",
            "expires_in": self.expires_in,
            "expires_at": self.expires_at,
            "refresh_token": self.refresh_token,
            "user": self.user,
        })
    }
}

/// The claims of an access token for a session, as GoTrue makes them.
pub async fn claims_for(
    state: &AppState,
    db: &impl GenericClient,
    user_id: Uuid,
    session_id: Uuid,
) -> Result<(Value, Value), AuthError> {
    // The user and the session's sign-in methods, in one query.
    let row = db
        .opt(
            &format!(
                "SELECT {USER_JSON}, (
                     SELECT coalesce(jsonb_agg(jsonb_build_object('method', c.authentication_method,
                         'timestamp', extract(epoch FROM c.created_at)::bigint) ORDER BY c.created_at DESC), '[]'::jsonb)
                     FROM auth.mfa_amr_claims c WHERE c.session_id = $2)
                 FROM auth.users u WHERE u.id = $1"
            ),
            &[&user_id, &session_id],
        )
        .await?
        .ok_or_else(|| AuthError::internal("The user just signed in is gone"))?;
    let (user, amr): (Value, Value) = (row.get(0), row.get(1));
    let now = now_seconds();
    let claims = json!({
        "iss": format!("{}/auth/v1", state.cfg.site_url),
        "sub": user_id.to_string(),
        "aud": user["aud"].as_str().filter(|aud| !aud.is_empty()).unwrap_or("authenticated"),
        "exp": now + state.cfg.auth.jwt_expiry,
        "iat": now,
        "email": user["email"],
        "phone": user["phone"],
        "app_metadata": user["app_metadata"],
        "user_metadata": user["user_metadata"],
        "role": user["role"].as_str().filter(|role| !role.is_empty()).unwrap_or("authenticated"),
        "aal": "aal1",
        "amr": amr,
        "session_id": session_id.to_string(),
        "is_anonymous": user["is_anonymous"].as_bool().unwrap_or(false),
    });
    Ok((claims, user))
}

async fn tokens_for(
    state: &AppState,
    db: &impl GenericClient,
    user_id: Uuid,
    session_id: Uuid,
    refresh_token: String,
) -> Result<Tokens, AuthError> {
    let (claims, user) = claims_for(state, db, user_id, session_id).await?;
    let expires_at = claims["exp"].as_i64().unwrap_or_default();
    Ok(Tokens {
        access_token: jwt::sign(&claims, &state.cfg.auth.jwt_secret),
        refresh_token,
        expires_in: state.cfg.auth.jwt_expiry,
        expires_at,
        user,
        user_id,
        session_id,
    })
}

/// A new session for a user who just proved who they are, by `method`
/// (`password`, `anonymous`, `otp`, `recovery`, `magiclink`), as GoTrue's
/// issueRefreshToken.
pub async fn new_session(
    state: &AppState,
    db: &impl GenericClient,
    user_id: Uuid,
    method: &str,
    meta: &Meta,
) -> Result<Tokens, AuthError> {
    let session_id = Uuid::new_v4();
    let refresh_token = new_refresh_token();
    // The session, its sign-in method and first refresh token, and the
    // sign-in times, in one statement: the foreign keys are checked at its end.
    db.exec(
        "WITH session AS (
             INSERT INTO auth.sessions (id, user_id, created_at, updated_at, aal, user_agent, ip, refreshed_at)
             VALUES ($1, $2, now(), now(), 'aal1', $3, $4::text::inet, now() AT TIME ZONE 'UTC')),
         amr AS (
             INSERT INTO auth.mfa_amr_claims (id, session_id, created_at, updated_at, authentication_method)
             VALUES (gen_random_uuid(), $1, now(), now(), $5)),
         token AS (
             INSERT INTO auth.refresh_tokens (instance_id, token, user_id, revoked, created_at, updated_at, session_id)
             VALUES ('00000000-0000-0000-0000-000000000000', $6, $7, false, now(), now(), $1)),
         identities AS (UPDATE auth.identities SET last_sign_in_at = now() WHERE user_id = $2)
         UPDATE auth.users SET last_sign_in_at = now(), updated_at = now() WHERE id = $2",
        &[&session_id, &user_id, &meta.user_agent, &meta.ip_text(), &method, &refresh_token, &user_id.to_string()],
    )
    .await?;
    tokens_for(state, db, user_id, session_id, refresh_token).await
}

/// Ends sessions: this one (`local`), the user's others (`others`), or all (`global`).
pub async fn end_sessions(
    db: &impl GenericClient,
    user_id: Uuid,
    session_id: Option<Uuid>,
    scope: &str,
) -> Result<(), tokio_postgres::Error> {
    match (scope, session_id) {
        ("local", Some(session)) => {
            db.exec("DELETE FROM auth.sessions WHERE id = $1 AND user_id = $2", &[&session, &user_id]).await?
        }
        ("others", Some(session)) => {
            db.exec("DELETE FROM auth.sessions WHERE user_id = $1 AND id <> $2", &[&user_id, &session]).await?
        }
        _ => db.exec("DELETE FROM auth.sessions WHERE user_id = $1", &[&user_id]).await?,
    };
    db.exec(
        "DELETE FROM auth.refresh_tokens WHERE user_id = $1
           AND (session_id IS NULL OR session_id NOT IN (SELECT id FROM auth.sessions WHERE user_id = $2))",
        &[&user_id.to_string(), &user_id],
    )
    .await?;
    Ok(())
}

// --- Signing up and in ---

pub enum SignedUp {
    /// Signed in at once (ENABLE_EMAIL_AUTOCONFIRM, or a guest).
    Session(Tokens),
    /// The user, waiting for the email to be confirmed.
    Pending(Value),
}

/// POST /signup with an email and password, as GoTrue.
pub async fn sign_up(
    state: &AppState,
    email: &str,
    password: &str,
    data: Option<Value>,
    meta: &Meta,
) -> Result<SignedUp, AuthError> {
    let auth = &state.cfg.auth;
    if auth.disable_signup {
        return Err(AuthError::unprocessable("signup_disabled", "Signups not allowed for this instance"));
    }
    if password.is_empty() {
        return Err(AuthError::bad("validation_failed", "Signup requires a valid password"));
    }
    if !valid_email(email) {
        return Err(AuthError::bad("validation_failed", "Unable to validate email address: invalid format"));
    }
    if let Some(problem) = password_problem(state, password) {
        return Err(problem);
    }
    if !state.limits.within(Limit::Signup, &meta.limit_key("signup")) {
        return Err(AuthError::too_many());
    }
    let email = normalize_email(email);
    let mut client = state.db.client().await?;
    let tx = client.transaction().await?;
    if closed_email(&tx, &email).await? {
        return Err(AuthError::unprocessable("signup_disabled", "Signups not allowed for this email address"));
    }
    let data = data.filter(Value::is_object).unwrap_or_else(|| json!({}));
    if let Some(existing) = find_user_by_email(&tx, &email).await? {
        if auth.autoconfirm || existing.confirmed {
            if !auth.autoconfirm {
                // As GoTrue: an answer like a new sign-up's, so it doesn't tell the address has an account.
                drop(tx);
                let _ = mail::send(state, mail::existing_account_mail(&state.cfg.site_url, &email)).await;
                return Ok(SignedUp::Pending(obfuscated_user(&email, &data)));
            }
            return Err(AuthError::unprocessable("user_already_exists", "User already registered"));
        }
        // Not confirmed yet: a new password and a new link, as GoTrue.
        let hash = hash_password(password)?;
        tx.exec(
            "UPDATE auth.users SET encrypted_password = $2, raw_user_meta_data = $3, updated_at = now() WHERE id = $1",
            &[&existing.id, &hash, &data],
        )
        .await?;
        let link = confirmation_link(state, &tx, existing.id, &email).await?;
        tx.commit().await?;
        if let Some(link) = link {
            mail::send(state, mail::confirm_mail(&state.cfg.site_url, &email, &link)).await?;
        }
        let user = user_json(&state.db.client().await?, existing.id).await?.unwrap_or_default();
        return Ok(SignedUp::Pending(user));
    }

    let id = Uuid::new_v4();
    let hash = hash_password(password)?;
    let app_metadata = json!({ "provider": "email", "providers": ["email"] });
    tx.exec(
        "INSERT INTO auth.users (instance_id, id, aud, role, email, encrypted_password, email_confirmed_at,
             raw_app_meta_data, raw_user_meta_data, created_at, updated_at, is_anonymous, is_sso_user,
             confirmation_token, recovery_token, email_change_token_new, email_change)
         VALUES ('00000000-0000-0000-0000-000000000000', $1, 'authenticated', 'authenticated', $2, $3,
             CASE WHEN $4 THEN now() END, $5, $6, now(), now(), false, false, '', '', '', '')",
        &[&id, &email, &hash, &auth.autoconfirm, &app_metadata, &data],
    )
    .await
    .map_err(|error| {
        if error.code() == Some(&tokio_postgres::error::SqlState::UNIQUE_VIOLATION) {
            AuthError::unprocessable("user_already_exists", "User already registered")
        } else {
            AuthError::internal(error)
        }
    })?;
    let identity = json!({
        "sub": id.to_string(), "email": email, "email_verified": auth.autoconfirm, "phone_verified": false,
    });
    tx.exec(
        "INSERT INTO auth.identities (id, provider_id, user_id, identity_data, provider, last_sign_in_at,
             created_at, updated_at)
         VALUES (gen_random_uuid(), $1, $2, $3, 'email', now(), now(), now())",
        &[&id.to_string(), &id, &identity],
    )
    .await?;
    if auth.autoconfirm {
        let tokens = new_session(state, &tx, id, "password", meta).await?;
        tx.commit().await?;
        return Ok(SignedUp::Session(tokens));
    }
    let link = confirmation_link(state, &tx, id, &email).await?;
    let user = user_json(&tx, id).await?.unwrap_or_default();
    tx.commit().await?;
    if let Some(link) = link {
        mail::send(state, mail::confirm_mail(&state.cfg.site_url, &email, &link)).await?;
    }
    Ok(SignedUp::Pending(user))
}

/// What GoTrue shows for an address that has an account, without saying so.
fn obfuscated_user(email: &str, data: &Value) -> Value {
    let now = crate::time::iso(crate::time::now_ms());
    json!({
        "id": Uuid::new_v4().to_string(), "aud": "authenticated", "role": "", "email": email, "phone": "",
        "confirmation_sent_at": now, "app_metadata": { "provider": "email", "providers": ["email"] },
        "user_metadata": data, "identities": [], "created_at": now, "updated_at": now, "is_anonymous": false,
    })
}

/// A guest, as Nuvio's apps start: POST /signup with no email.
pub async fn sign_up_anonymously(state: &AppState, data: Option<Value>, meta: &Meta) -> Result<Tokens, AuthError> {
    let auth = &state.cfg.auth;
    if !auth.anonymous_users {
        return Err(AuthError::unprocessable("anonymous_provider_disabled", "Anonymous sign-ins are disabled"));
    }
    if auth.disable_signup {
        return Err(AuthError::unprocessable("signup_disabled", "Signups not allowed for this instance"));
    }
    if !state.limits.within(Limit::Signup, &meta.limit_key("anonymous")) {
        return Err(AuthError::too_many());
    }
    let id = Uuid::new_v4();
    let data = data.filter(Value::is_object).unwrap_or_else(|| json!({}));
    let mut client = state.db.client().await?;
    let tx = client.transaction().await?;
    tx.exec(
        "INSERT INTO auth.users (instance_id, id, aud, role, raw_app_meta_data, raw_user_meta_data, created_at,
             updated_at, is_anonymous, is_sso_user, confirmation_token, recovery_token, email_change_token_new,
             email_change)
         VALUES ('00000000-0000-0000-0000-000000000000', $1, 'authenticated', 'authenticated', '{}', $2, now(),
             now(), true, false, '', '', '', '')",
        &[&id, &data],
    )
    .await?;
    let tokens = new_session(state, &tx, id, "anonymous", meta).await?;
    tx.commit().await?;
    Ok(tokens)
}

/// grant_type=password.
pub async fn password_grant(state: &AppState, email: &str, password: &str, meta: &Meta) -> Result<Tokens, AuthError> {
    if email.trim().is_empty() {
        return Err(AuthError::bad("validation_failed", "missing email or phone"));
    }
    if !state.limits.within(Limit::Auth, &meta.limit_key("password")) {
        return Err(AuthError::too_many());
    }
    let client = state.db.client().await?;
    let user = find_user_by_email(&client, &normalize_email(email)).await?;
    let matches = password_matches(password.to_owned(), user.as_ref().and_then(|u| u.encrypted_password.clone())).await;
    let Some(user) = user.filter(|_| matches) else {
        return Err(AuthError::bad("invalid_credentials", "Invalid login credentials"));
    };
    if user.banned {
        return Err(AuthError::bad("user_banned", "User is banned"));
    }
    if !user.confirmed {
        return Err(AuthError::bad("email_not_confirmed", "Email not confirmed"));
    }
    let mut client = client;
    let tx = client.transaction().await?;
    let tokens = new_session(state, &tx, user.id, "password", meta).await?;
    tx.commit().await?;
    Ok(tokens)
}

/// grant_type=refresh_token: the next token, as GoTrue's rotation.
pub async fn refresh_grant(state: &AppState, refresh_token: &str) -> Result<Tokens, AuthError> {
    if refresh_token.is_empty() {
        return Err(AuthError::oauth("invalid_request", "refresh_token required"));
    }
    let mut client = state.db.client().await?;
    let tx = client.transaction().await?;
    let Some(row) = tx
        .opt(
            "SELECT t.id, t.revoked, t.session_id, t.user_id,
                    extract(epoch FROM now() - t.updated_at)::bigint
             FROM auth.refresh_tokens t WHERE t.token = $1",
            &[&refresh_token],
        )
        .await?
    else {
        return Err(AuthError::bad("refresh_token_not_found", "Invalid Refresh Token: Refresh Token Not Found"));
    };
    let token_id: i64 = row.get(0);
    let revoked: Option<bool> = row.get(1);
    let session_id: Option<Uuid> = row.get(2);
    let user_id: Option<String> = row.get(3);
    let age: i64 = row.get(4);
    let user_id = user_id
        .and_then(|id| id.parse::<Uuid>().ok())
        .ok_or_else(|| AuthError::bad("refresh_token_not_found", "Invalid Refresh Token: Refresh Token Not Found"))?;
    let user = find_user(&tx, user_id)
        .await?
        .ok_or_else(|| AuthError::bad("refresh_token_not_found", "Invalid Refresh Token: Refresh Token Not Found"))?;
    if user.banned {
        return Err(AuthError::bad("user_banned", "Invalid Refresh Token: User Banned"));
    }
    // One refresh of a session at a time.
    let session = match session_id {
        Some(id) => {
            tx.opt("SELECT id, coalesce(not_after < now(), false) FROM auth.sessions WHERE id = $1 FOR UPDATE", &[&id])
                .await?
        }
        None => None,
    };
    let Some(session) = session else {
        return Err(AuthError::bad("session_not_found", "Invalid Refresh Token: No Valid Session Found"));
    };
    let session_id: Uuid = session.get(0);
    let expired: bool = session.get(1);
    if expired {
        return Err(AuthError::bad("session_expired", "Invalid Refresh Token: Session Expired"));
    }

    let mut issued: Option<String> = None;
    if revoked == Some(true) {
        let active: Option<(String, Option<String>)> = tx
            .opt(
                "SELECT token, parent FROM auth.refresh_tokens
                 WHERE session_id = $1 AND NOT coalesce(revoked, false) ORDER BY id DESC LIMIT 1",
                &[&session_id],
            )
            .await?
            .map(|row| (row.get(0), row.get(1)));
        match active {
            // The client didn't keep the answer of its last refresh: the same one again.
            Some((token, Some(parent))) if parent == refresh_token => issued = Some(token),
            _ if age <= REUSE_SECONDS => {}
            _ => {
                tx.exec(
                    "UPDATE auth.refresh_tokens SET revoked = true, updated_at = now() WHERE session_id = $1",
                    &[&session_id],
                )
                .await?;
                tx.commit().await?;
                return Err(AuthError::bad("refresh_token_already_used", "Invalid Refresh Token: Already Used"));
            }
        }
    }
    let token = match issued {
        Some(token) => {
            tx.exec(
                "UPDATE auth.sessions SET updated_at = now(), refreshed_at = now() AT TIME ZONE 'UTC' WHERE id = $1",
                &[&session_id],
            )
            .await?;
            token
        }
        None => {
            // The token used up, the next one, and the session's time, in one statement.
            let token = new_refresh_token();
            tx.exec(
                "WITH used AS (UPDATE auth.refresh_tokens SET revoked = true, updated_at = now() WHERE id = $1),
                 next AS (
                     INSERT INTO auth.refresh_tokens (instance_id, token, user_id, revoked, created_at, updated_at,
                         parent, session_id)
                     VALUES ('00000000-0000-0000-0000-000000000000', $2, $3, false, now(), now(), $4, $5))
                 UPDATE auth.sessions SET updated_at = now(), refreshed_at = now() AT TIME ZONE 'UTC' WHERE id = $5",
                &[&token_id, &token, &user_id.to_string(), &refresh_token, &session_id],
            )
            .await?;
            token
        }
    };
    let tokens = tokens_for(state, &tx, user_id, session_id, token).await?;
    tx.commit().await?;
    Ok(tokens)
}

// --- Email links ---

/// Sets a new confirmation token, unless one went out within the last
/// minute. The link for the email, or None when it's too soon.
async fn confirmation_link(
    state: &AppState,
    db: &impl GenericClient,
    user_id: Uuid,
    email: &str,
) -> Result<Option<String>, AuthError> {
    let hash = token_hash(email, &new_otp());
    let updated = db
        .exec(
            "UPDATE auth.users SET confirmation_token = $2, confirmation_sent_at = now()
             WHERE id = $1 AND (confirmation_sent_at IS NULL OR confirmation_sent_at < now() - make_interval(secs => $3))",
            &[&user_id, &hash, &(EMAIL_EVERY_SECONDS as f64)],
        )
        .await?;
    Ok((updated == 1).then(|| format!("{}/verify?type=signup&token={hash}", state.cfg.site_url)))
}

/// Sends a new confirmation link to an account not yet confirmed (POST /resend).
pub async fn resend_confirmation(state: &AppState, email: &str, meta: &Meta) -> Result<(), AuthError> {
    if !state.limits.within(Limit::Email, &meta.limit_key("resend")) {
        return Err(AuthError::too_many());
    }
    let email = normalize_email(email);
    let client = state.db.client().await?;
    if let Some(user) = find_user_by_email(&client, &email).await?
        && !user.confirmed
        && let Some(link) = confirmation_link(state, &client, user.id, &email).await?
    {
        mail::send(state, mail::confirm_mail(&state.cfg.site_url, &email, &link)).await?;
    }
    Ok(())
}

/// POST /recover: a reset link, if the address has an account. The answer is the same either way.
pub async fn request_recovery(state: &AppState, email: &str, meta: &Meta) -> Result<(), AuthError> {
    if !valid_email(email) {
        return Err(AuthError::bad("validation_failed", "Unable to validate email address: invalid format"));
    }
    if !state.limits.within(Limit::Email, &meta.limit_key("recover")) {
        return Err(AuthError::too_many());
    }
    let email = normalize_email(email);
    let client = state.db.client().await?;
    let Some(user) = find_user_by_email(&client, &email).await? else { return Ok(()) };
    if user.banned {
        return Ok(());
    }
    let hash = token_hash(&email, &new_otp());
    let updated = client
        .exec(
            "UPDATE auth.users SET recovery_token = $2, recovery_sent_at = now()
             WHERE id = $1 AND (recovery_sent_at IS NULL OR recovery_sent_at < now() - make_interval(secs => $3))",
            &[&user.id, &hash, &(EMAIL_EVERY_SECONDS as f64)],
        )
        .await?;
    if updated == 1 {
        let link = format!("{}/reset?token={hash}", state.cfg.site_url);
        mail::send(state, mail::reset_mail(&state.cfg.site_url, &email, &link)).await?;
    }
    Ok(())
}

/// The kinds of email link /verify takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkType {
    Signup,
    Recovery,
    EmailChange,
}

impl LinkType {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "signup" | "email" | "invite" => Some(Self::Signup),
            "recovery" | "magiclink" => Some(Self::Recovery),
            "email_change" => Some(Self::EmailChange),
            _ => None,
        }
    }
}

fn link_expired() -> AuthError {
    AuthError::forbidden("otp_expired", "Email link is invalid or has expired")
}

/// The user an email link is for, if it still works, without using it.
pub async fn peek_link(db: &impl GenericClient, kind: LinkType, hash: &str) -> Result<Option<UserRow>, AuthError> {
    if hash.is_empty() {
        return Ok(None);
    }
    let condition = match kind {
        LinkType::Signup => "confirmation_token = $1 AND confirmation_sent_at > now() - make_interval(secs => $2)",
        LinkType::Recovery => "recovery_token = $1 AND recovery_sent_at > now() - make_interval(secs => $2)",
        LinkType::EmailChange => {
            "email_change_token_new = $1 AND email_change_sent_at > now() - make_interval(secs => $2)"
        }
    };
    let lifetime = if kind == LinkType::Recovery { RECOVERY_SECONDS } else { CONFIRM_SECONDS } as f64;
    let row = db.opt(&format!("{USER_ROW} WHERE {condition}"), &[&hash, &lifetime]).await?;
    Ok(row.as_ref().map(user_row))
}

/// Uses an email link and signs its user in, as GoTrue's /verify.
pub async fn verify_link(state: &AppState, kind: LinkType, hash: &str, meta: &Meta) -> Result<Tokens, AuthError> {
    let mut client = state.db.client().await?;
    let tx = client.transaction().await?;
    let user = peek_link(&tx, kind, hash).await?.ok_or_else(link_expired)?;
    if user.banned {
        return Err(AuthError::bad("user_banned", "User is banned"));
    }
    let method = match kind {
        LinkType::Signup => {
            tx.exec(
                "UPDATE auth.users SET email_confirmed_at = coalesce(email_confirmed_at, now()),
                     confirmation_token = '', updated_at = now() WHERE id = $1",
                &[&user.id],
            )
            .await?;
            tx.exec(
                "UPDATE auth.identities SET identity_data = identity_data || '{\"email_verified\": true}'
                 WHERE user_id = $1 AND provider = 'email'",
                &[&user.id],
            )
            .await?;
            "otp"
        }
        LinkType::Recovery => {
            tx.exec(
                "UPDATE auth.users SET email_confirmed_at = coalesce(email_confirmed_at, now()),
                     recovery_token = '', updated_at = now() WHERE id = $1",
                &[&user.id],
            )
            .await?;
            "recovery"
        }
        LinkType::EmailChange => {
            tx.exec(
                "UPDATE auth.users SET email = email_change, email_change = '', email_change_token_new = '',
                     email_confirmed_at = now(), is_anonymous = false, updated_at = now(),
                     raw_app_meta_data = coalesce(raw_app_meta_data, '{}')
                         || '{\"provider\": \"email\", \"providers\": [\"email\"]}'
                 WHERE id = $1",
                &[&user.id],
            )
            .await?;
            let email: String = tx.one("SELECT email FROM auth.users WHERE id = $1", &[&user.id]).await?.get(0);
            upsert_email_identity(&tx, user.id, &email).await?;
            "email/signup"
        }
    };
    let tokens = new_session(state, &tx, user.id, method, meta).await?;
    tx.commit().await?;
    Ok(tokens)
}

async fn upsert_email_identity(db: &impl GenericClient, user_id: Uuid, email: &str) -> Result<(), AuthError> {
    let identity =
        json!({ "sub": user_id.to_string(), "email": email, "email_verified": true, "phone_verified": false });
    db.exec(
        "INSERT INTO auth.identities (id, provider_id, user_id, identity_data, provider, last_sign_in_at,
             created_at, updated_at)
         VALUES (gen_random_uuid(), $1, $2, $3, 'email', now(), now(), now())
         ON CONFLICT (provider_id, provider) DO UPDATE SET identity_data = excluded.identity_data, updated_at = now()",
        &[&user_id.to_string(), &user_id, &identity],
    )
    .await?;
    Ok(())
}

// --- The signed-in user ---

/// Who a bearer token is, as GoTrue's requireAuthentication checks it: a
/// valid token, for a user and a session that still exist.
#[derive(Debug, Clone)]
pub struct Authenticated {
    pub claims: Value,
    pub user_id: Uuid,
    pub session_id: Option<Uuid>,
    pub role: String,
}

pub async fn authenticate(state: &AppState, token: Option<&str>) -> Result<Authenticated, AuthError> {
    let Some(token) = token.filter(|token| !token.is_empty()) else {
        return Err(AuthError::new(
            StatusCode::UNAUTHORIZED,
            "no_authorization",
            "This endpoint requires a valid Bearer token",
        ));
    };
    let claims = jwt::verify(token, &state.cfg.auth.jwt_secret, now_seconds()).map_err(|problem| {
        let why = match problem {
            jwt::JwtProblem::Expired => "token has invalid claims: token is expired",
            jwt::JwtProblem::Invalid => "unable to parse or verify signature, token is unverifiable",
        };
        AuthError::forbidden("bad_jwt", format!("invalid JWT: {why}"))
    })?;
    let role = claims.get("role").and_then(Value::as_str).unwrap_or_default().to_owned();
    let user_id = claims.get("sub").and_then(Value::as_str).and_then(|sub| sub.parse::<Uuid>().ok());
    let session_id = claims.get("session_id").and_then(Value::as_str).and_then(|id| id.parse::<Uuid>().ok());
    let Some(user_id) = user_id else {
        return Err(AuthError::forbidden("bad_jwt", "invalid claim: missing sub claim"));
    };
    let client = state.db.client().await?;
    if find_user(&client, user_id).await?.is_none() {
        return Err(AuthError::forbidden("user_not_found", "User from sub claim in JWT does not exist"));
    }
    if let Some(session) = session_id
        && client.opt("SELECT 1 FROM auth.sessions WHERE id = $1", &[&session]).await?.is_none()
    {
        return Err(AuthError::forbidden("session_not_found", "Session from session_id claim in JWT does not exist"));
    }
    Ok(Authenticated { claims, user_id, session_id, role })
}

/// What PUT /user changes.
#[derive(Debug, Default, Deserialize)]
pub struct UserChange {
    pub email: Option<String>,
    pub password: Option<String>,
    pub data: Option<Value>,
    /// GoTrue's reauthentication nonce, not needed here (SECURE_PASSWORD_CHANGE is off).
    #[allow(dead_code)]
    pub nonce: Option<String>,
}

pub async fn update_user(state: &AppState, who: &Authenticated, change: UserChange) -> Result<Value, AuthError> {
    let mut client = state.db.client().await?;
    let tx = client.transaction().await?;
    let user = find_user(&tx, who.user_id)
        .await?
        .ok_or_else(|| AuthError::forbidden("user_not_found", "User from sub claim in JWT does not exist"))?;
    if let Some(data) = change.data.filter(Value::is_object) {
        tx.exec(
            "UPDATE auth.users SET raw_user_meta_data = coalesce(raw_user_meta_data, '{}') || $2, updated_at = now()
             WHERE id = $1",
            &[&user.id, &data],
        )
        .await?;
    }
    if let Some(password) = change.password.as_deref() {
        if let Some(problem) = password_problem(state, password) {
            return Err(problem);
        }
        if password_matches(password.to_owned(), user.encrypted_password.clone()).await {
            return Err(AuthError::unprocessable(
                "same_password",
                "New password should be different from the old password.",
            ));
        }
        let hash = hash_password(password)?;
        tx.exec("UPDATE auth.users SET encrypted_password = $2, updated_at = now() WHERE id = $1", &[&user.id, &hash])
            .await?;
        // As GoTrue: a new password ends the user's other sessions.
        end_sessions(&tx, user.id, who.session_id, "others").await?;
    }
    let mut mail_to_send = None;
    if let Some(email) = change.email.as_deref().map(normalize_email).filter(|email| *email != user.email) {
        if !valid_email(&email) {
            return Err(AuthError::bad("validation_failed", "Unable to validate email address: invalid format"));
        }
        if find_user_by_email(&tx, &email).await?.is_some() || closed_email(&tx, &email).await? {
            return Err(AuthError::unprocessable(
                "email_exists",
                "A user with this email address has already been registered",
            ));
        }
        if user.is_anonymous && state.cfg.auth.autoconfirm {
            // A guest keeping its data with an email, at once (MAILER_AUTOCONFIRM).
            tx.exec(
                "UPDATE auth.users SET email = $2, email_confirmed_at = now(), is_anonymous = false,
                     raw_app_meta_data = coalesce(raw_app_meta_data, '{}')
                         || '{\"provider\": \"email\", \"providers\": [\"email\"]}', updated_at = now()
                 WHERE id = $1",
                &[&user.id, &email],
            )
            .await?;
            upsert_email_identity(&tx, user.id, &email).await?;
        } else {
            let hash = token_hash(&email, &new_otp());
            tx.exec(
                "UPDATE auth.users SET email_change = $2, email_change_token_new = $3, email_change_sent_at = now(),
                     updated_at = now() WHERE id = $1",
                &[&user.id, &email, &hash],
            )
            .await?;
            let link = format!("{}/verify?type=email_change&token={hash}", state.cfg.site_url);
            mail_to_send = Some(mail::confirm_mail(&state.cfg.site_url, &email, &link));
        }
    }
    let json = user_json(&tx, user.id).await?.unwrap_or_default();
    tx.commit().await?;
    if let Some(mail) = mail_to_send {
        mail::send(state, mail).await?;
    }
    Ok(json)
}

/// Whether `password` is the user's, for the website's forms that ask for
/// it again before a change.
pub async fn check_password(state: &AppState, user_id: Uuid, password: &str) -> Result<bool, AuthError> {
    let client = state.db.client().await?;
    let hash = find_user(&client, user_id).await?.and_then(|user| user.encrypted_password);
    Ok(password_matches(password.to_owned(), hash).await)
}

/// Deletes a user and, by Nuvio's foreign keys, everything of theirs (Nuvio's delete-account).
pub async fn delete_user(state: &AppState, user_id: Uuid) -> Result<(), AuthError> {
    let client = state.db.client().await?;
    client.exec("DELETE FROM auth.users WHERE id = $1", &[&user_id]).await?;
    Ok(())
}

// --- The routes ---

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/v1/signup", post(signup_route))
        .route("/auth/v1/token", post(token_route))
        .route("/auth/v1/user", get(get_user_route).put(put_user_route))
        .route("/auth/v1/logout", post(logout_route))
        .route("/auth/v1/recover", post(recover_route))
        .route("/auth/v1/resend", post(resend_route))
        .route("/auth/v1/verify", get(verify_get_route).post(verify_post_route))
        .route("/auth/v1/settings", get(settings_route))
        .route("/auth/v1/health", get(health_route))
        .route("/auth/v1/.well-known/jwks.json", get(jwks_route))
        .route("/auth/v1/admin/users/{id}", delete(admin_delete_route).get(admin_get_route))
}

fn body_of(body: &[u8]) -> Result<Value, AuthError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(json!({}));
    }
    serde_json::from_slice::<Value>(body)
        .ok()
        .filter(Value::is_object)
        .ok_or_else(|| AuthError::bad("bad_json", "Could not parse request body as JSON"))
}

fn text_field<'a>(body: &'a Value, name: &str) -> &'a str {
    body.get(name).and_then(Value::as_str).unwrap_or_default()
}

/// `data`, or `options.data` as some clients send it.
fn metadata(body: &Value) -> Option<Value> {
    body.get("data").or_else(|| body.pointer("/options/data")).cloned()
}

async fn signup_route(
    State(state): State<AppState>,
    ip: Option<Extension<ClientIp>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let meta = Meta::of(ip.as_deref(), &headers);
    let result = async {
        let body = body_of(&body)?;
        let email = text_field(&body, "email");
        if !text_field(&body, "phone").is_empty() {
            return Err(AuthError::bad("phone_provider_disabled", "Phone signups are disabled"));
        }
        if email.is_empty() {
            let tokens = sign_up_anonymously(&state, metadata(&body), &meta).await?;
            return Ok(json_response(StatusCode::OK, tokens.json()));
        }
        Ok(match sign_up(&state, email, text_field(&body, "password"), metadata(&body), &meta).await? {
            SignedUp::Session(tokens) => json_response(StatusCode::OK, tokens.json()),
            SignedUp::Pending(user) => json_response(StatusCode::OK, user),
        })
    }
    .await;
    reply(&headers, result)
}

#[derive(Deserialize)]
struct GrantQuery {
    grant_type: Option<String>,
}

async fn token_route(
    State(state): State<AppState>,
    ip: Option<Extension<ClientIp>>,
    Query(query): Query<GrantQuery>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let meta = Meta::of(ip.as_deref(), &headers);
    let result = async {
        let body = body_of(&body)?;
        let tokens = match query.grant_type.as_deref() {
            Some("password") => {
                if !text_field(&body, "phone").is_empty() {
                    return Err(AuthError::unprocessable("phone_provider_disabled", "Phone logins are disabled"));
                }
                password_grant(&state, text_field(&body, "email"), text_field(&body, "password"), &meta).await?
            }
            Some("refresh_token") => refresh_grant(&state, text_field(&body, "refresh_token")).await?,
            _ => return Err(AuthError::bad("invalid_credentials", "unsupported_grant_type")),
        };
        Ok(json_response(StatusCode::OK, tokens.json()))
    }
    .await;
    reply(&headers, result)
}

async fn get_user_route(State(state): State<AppState>, bearer: Extension<Bearer>, headers: HeaderMap) -> Response {
    let result = async {
        let who = authenticate(&state, bearer.0.0.as_deref()).await?;
        let client = state.db.client().await?;
        let user = user_json(&client, who.user_id).await?.unwrap_or_default();
        Ok(json_response(StatusCode::OK, user))
    }
    .await;
    reply(&headers, result)
}

async fn put_user_route(
    State(state): State<AppState>,
    bearer: Extension<Bearer>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let result = async {
        let who = authenticate(&state, bearer.0.0.as_deref()).await?;
        let change: UserChange = serde_json::from_value(body_of(&body)?)
            .map_err(|_| AuthError::bad("bad_json", "Could not parse request body as JSON"))?;
        Ok(json_response(StatusCode::OK, update_user(&state, &who, change).await?))
    }
    .await;
    reply(&headers, result)
}

#[derive(Deserialize)]
struct LogoutQuery {
    scope: Option<String>,
}

async fn logout_route(
    State(state): State<AppState>,
    bearer: Extension<Bearer>,
    Query(query): Query<LogoutQuery>,
    headers: HeaderMap,
) -> Response {
    let result = async {
        let scope = query.scope.as_deref().unwrap_or("global");
        if !["global", "local", "others"].contains(&scope) {
            return Err(AuthError::bad("validation_failed", format!("Unsupported logout scope {scope:?}")));
        }
        let who = match authenticate(&state, bearer.0.0.as_deref()).await {
            Ok(who) => who,
            // A session that's gone already is signed out.
            Err(error) if error.code == "session_not_found" || error.code == "user_not_found" => {
                return Ok(StatusCode::NO_CONTENT.into_response());
            }
            Err(error) => return Err(error),
        };
        let client = state.db.client().await?;
        end_sessions(&client, who.user_id, who.session_id, scope).await?;
        Ok(StatusCode::NO_CONTENT.into_response())
    }
    .await;
    reply(&headers, result)
}

async fn recover_route(
    State(state): State<AppState>,
    ip: Option<Extension<ClientIp>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let meta = Meta::of(ip.as_deref(), &headers);
    let result = async {
        let body = body_of(&body)?;
        request_recovery(&state, text_field(&body, "email"), &meta).await?;
        Ok(json_response(StatusCode::OK, json!({})))
    }
    .await;
    reply(&headers, result)
}

async fn resend_route(
    State(state): State<AppState>,
    ip: Option<Extension<ClientIp>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let meta = Meta::of(ip.as_deref(), &headers);
    let result = async {
        let body = body_of(&body)?;
        match text_field(&body, "type") {
            "signup" => resend_confirmation(&state, text_field(&body, "email"), &meta).await?,
            _ => return Err(AuthError::bad("validation_failed", "Missing one of these types: signup, email_change")),
        }
        Ok(json_response(StatusCode::OK, json!({ "message_id": null })))
    }
    .await;
    reply(&headers, result)
}

#[derive(Deserialize)]
struct VerifyQuery {
    #[serde(rename = "type")]
    kind: Option<String>,
    token: Option<String>,
    redirect_to: Option<String>,
}

/// GET /verify, from an email link: signs in and goes on to `redirect_to`
/// (or the site) with the tokens in the fragment, as GoTrue.
async fn verify_get_route(
    State(state): State<AppState>,
    ip: Option<Extension<ClientIp>>,
    Query(query): Query<VerifyQuery>,
    headers: HeaderMap,
) -> Response {
    let meta = Meta::of(ip.as_deref(), &headers);
    let target = query
        .redirect_to
        .filter(|to| to.starts_with(&format!("{}/", state.cfg.site_url)) || *to == state.cfg.site_url)
        .unwrap_or_else(|| state.cfg.site_url.clone());
    let kind = query.kind.as_deref().and_then(LinkType::parse);
    let result = match kind {
        Some(kind) => verify_link(&state, kind, query.token.as_deref().unwrap_or_default(), &meta).await,
        None => Err(AuthError::bad("validation_failed", "Verify requires a verification type")),
    };
    let fragment = match result {
        Ok(tokens) => format!(
            "access_token={}&expires_at={}&expires_in={}&refresh_token={}&token_type=bearer&type={}",
            tokens.access_token,
            tokens.expires_at,
            tokens.expires_in,
            tokens.refresh_token,
            query.kind.as_deref().unwrap_or_default()
        ),
        Err(error) => form_urlencoded::Serializer::new(String::new())
            .append_pair("error", "access_denied")
            .append_pair("error_code", error.code)
            .append_pair("error_description", &error.message)
            .finish(),
    };
    Redirect::to(&format!("{target}#{fragment}")).into_response()
}

async fn verify_post_route(
    State(state): State<AppState>,
    ip: Option<Extension<ClientIp>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let meta = Meta::of(ip.as_deref(), &headers);
    let result = async {
        let body = body_of(&body)?;
        let kind = LinkType::parse(text_field(&body, "type"))
            .ok_or_else(|| AuthError::bad("validation_failed", "Verify requires a verification type"))?;
        let hash = match (text_field(&body, "token_hash"), text_field(&body, "token"), text_field(&body, "email")) {
            (hash, _, _) if !hash.is_empty() => hash.to_owned(),
            (_, token, email) if !token.is_empty() && !email.is_empty() => token_hash(&normalize_email(email), token),
            _ => return Err(AuthError::bad("validation_failed", "Verify requires either a token or a token hash")),
        };
        let tokens = verify_link(&state, kind, &hash, &meta).await?;
        Ok(json_response(StatusCode::OK, tokens.json()))
    }
    .await;
    reply(&headers, result)
}

async fn settings_route(State(state): State<AppState>) -> Response {
    let auth = &state.cfg.auth;
    let providers = [
        "anonymous_users",
        "apple",
        "azure",
        "bitbucket",
        "discord",
        "facebook",
        "figma",
        "fly",
        "github",
        "gitlab",
        "google",
        "keycloak",
        "kakao",
        "linkedin",
        "linkedin_oidc",
        "notion",
        "spotify",
        "slack",
        "slack_oidc",
        "workos",
        "twitch",
        "twitter",
        "x",
        "zoom",
        "email",
        "phone",
    ];
    let external: serde_json::Map<String, Value> = providers
        .iter()
        .map(|name| {
            let on = match *name {
                "email" => true,
                "anonymous_users" => auth.anonymous_users,
                _ => false,
            };
            ((*name).to_owned(), Value::Bool(on))
        })
        .collect();
    json_response(
        StatusCode::OK,
        json!({
            "external": external,
            "disable_signup": auth.disable_signup,
            "mailer_autoconfirm": auth.autoconfirm,
            "phone_autoconfirm": false,
            "sms_provider": "",
            "saml_enabled": false,
        }),
    )
}

async fn health_route() -> Response {
    json_response(
        StatusCode::OK,
        json!({
            "version": GOTRUE_VERSION,
            "name": "GoTrue",
            "description": "GoTrue is a user registration and authentication API",
        }),
    )
}

/// Tokens are signed with JWT_SECRET (HS256), which isn't published, so no keys.
async fn jwks_route() -> Response {
    json_response(StatusCode::OK, json!({ "keys": [] }))
}

async fn service_only(state: &AppState, bearer: &Bearer) -> Result<(), AuthError> {
    let claims = bearer
        .0
        .as_deref()
        .and_then(|token| jwt::verify(token, &state.cfg.auth.jwt_secret, now_seconds()).ok())
        .ok_or_else(|| AuthError::forbidden("bad_jwt", "invalid JWT: unable to parse or verify signature"))?;
    if claims.get("role").and_then(Value::as_str) != Some("service_role") {
        return Err(AuthError::forbidden("not_admin", "User not allowed"));
    }
    Ok(())
}

async fn admin_get_route(
    State(state): State<AppState>,
    bearer: Extension<Bearer>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let result = async {
        service_only(&state, &bearer).await?;
        let id = id.parse::<Uuid>().map_err(|_| AuthError::bad("validation_failed", "user_id must be an UUID"))?;
        let client = state.db.client().await?;
        let user = user_json(&client, id)
            .await?
            .ok_or_else(|| AuthError::new(StatusCode::NOT_FOUND, "user_not_found", "User not found"))?;
        Ok(json_response(StatusCode::OK, user))
    }
    .await;
    reply(&headers, result)
}

async fn admin_delete_route(
    State(state): State<AppState>,
    bearer: Extension<Bearer>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let result = async {
        service_only(&state, &bearer).await?;
        let id = id.parse::<Uuid>().map_err(|_| AuthError::bad("validation_failed", "user_id must be an UUID"))?;
        let client = state.db.client().await?;
        let user = user_json(&client, id)
            .await?
            .ok_or_else(|| AuthError::new(StatusCode::NOT_FOUND, "user_not_found", "User not found"))?;
        delete_user(&state, id).await?;
        Ok(json_response(StatusCode::OK, user))
    }
    .await;
    reply(&headers, result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checking_for_no_account_costs_what_a_real_check_does() {
        // The cost is the hash's second field: $2b$12$...
        let cost = |hash: &str| hash.split('$').nth(2).map(str::to_owned);
        let real = hash_password("a password").unwrap_or_default();
        assert!(cost(&real).is_some());
        assert_eq!(cost(&NO_ACCOUNT), cost(&real));
    }
}
