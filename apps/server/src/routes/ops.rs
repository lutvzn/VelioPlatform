//! The operator's page and API (the Worker's `routes/opsPage.tsx` and
//! `routes/ops.ts`; PLAN.md, "Legal and abuse"), used at /ops and by
//! `scripts/ops.mjs`: notices, closing and opening accounts, and TV limits.
//! Accounts are Supabase Auth's users; their id is the user's id.
//! Without OPS_TOKEN set, /ops is a 404.
//!
//! Two ways into the API: the secret as a bearer token (the command-line
//! tool), or the operator's session cookie from signing in at /ops
//! (ops_session.rs), which also needs velio.tv's own Origin on anything but
//! GET. No CORS, so no other site's page can read the answers.
//!
//!   GET  /ops                         the page, or its sign-in
//!   POST /ops                         signs in with the token
//!   GET  /ops/notices[?status=open|actioned|rejected|all]
//!   GET  /ops/notices/{id}
//!   POST /ops/notices/{id}            {status: "actioned" | "rejected", action}
//!   GET  /ops/accounts[?state=all|open|unconfirmed|closed&q=]  the newest 200, and how many match
//!   GET  /ops/accounts/{idOrEmail}
//!   POST /ops/accounts/{id}/close     {reason, confirm, notice?}: confirm is the account's email, typed again
//!   POST /ops/accounts/{id}/reopen
//!   POST /ops/accounts/{id}/tv-limit  {limit}: a number of TVs up to VELIO_TV_LIMIT_MAX, or null for the default
//!   POST /ops/signout                 ends the page's session

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};

use anyhow::Result;
use axum::body::Bytes;
use axum::extract::{FromRequestParts, Path, Query, Request};
use axum::http::request::Parts;
use axum::http::{HeaderValue, Method, StatusCode, Uri, header};
use axum::middleware::{Next, from_fn};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use deadpool_postgres::GenericClient;
use maud::Markup;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::AppState;
use crate::accounts::{Fields, normalize_email};
use crate::closing::{ACCOUNTS, MAX_REASON_LENGTH, close_account, reopen_account, rows_json};
use crate::config::TvLimits;
use crate::error::AppError;
use crate::http::{ClientIp, api_problem, from_this_site};
use crate::limits::Limit;
use crate::mail::{self, closed_mail, reopened_mail};
use crate::ops_session::{ended_ops_cookie, has_ops_session, is_ops_token, new_ops_session, ops_cookie};
use crate::routes::not_found;
use crate::time::now_ms;
use crate::views::ops::{ops_app, ops_sign_in};
use crate::web::{Form, Web};

type Answer = Result<Response, AppError>;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/ops", get(show_page).post(sign_in))
        .route("/ops/notices", get(list_notices))
        .route("/ops/notices/{id}", get(show_notice).post(handle_notice))
        .route("/ops/accounts", get(list_accounts))
        .route("/ops/accounts/{account}", get(show_account))
        .route("/ops/accounts/{id}/close", post(close))
        .route("/ops/accounts/{id}/reopen", post(reopen))
        .route("/ops/accounts/{id}/tv-limit", post(set_tv_limit))
        .route("/ops/signout", post(sign_out))
        .route_layer(from_fn(noindex))
}

/// Pages and answers for the operator are never cached or indexed.
async fn noindex(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    let headers = response.headers_mut();
    headers.insert("X-Robots-Tag", HeaderValue::from_static("noindex"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn problem(status: StatusCode, text: &str) -> Response {
    api_problem(status, text)
}

/// Counts a wrong try toward the limit per IP; false once it's reached.
fn wrong_try(state: &AppState, ip: IpAddr) -> bool {
    state.limits.within(Limit::Auth, &format!("ops:{ip}"))
}

// --- Who may call the API ---

/// The operator, by the bearer token or the page's session cookie. Only
/// wrong tries count toward the limit, so guessing is limited but the
/// operator isn't.
pub struct Operator;

impl FromRequestParts<AppState> for Operator {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        let Some(secret) = state.cfg.ops_token.as_deref() else {
            return Err(problem(StatusCode::NOT_FOUND, "Not found"));
        };
        let authorization = parts.headers.get(header::AUTHORIZATION).and_then(|value| value.to_str().ok());
        // As /^Bearer (.+)$/.
        let bearer = authorization.and_then(|value| value.strip_prefix("Bearer ")).filter(|token| !token.is_empty());
        match bearer {
            Some(token) if is_ops_token(secret, token) => return Ok(Operator),
            Some(_) => {}
            None if has_ops_session(secret, &parts.headers, now_ms()) => {
                let safe = parts.method == Method::GET || parts.method == Method::HEAD;
                if !safe && !from_this_site(&parts.headers, &state.cfg.site_url) {
                    return Err(problem(StatusCode::FORBIDDEN, "Only from the operator's page on velio.tv"));
                }
                return Ok(Operator);
            }
            None => {}
        }
        let ip = parts.extensions.get::<ClientIp>().map_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED), |client| client.ip);
        if !wrong_try(state, ip) {
            return Err(problem(StatusCode::TOO_MANY_REQUESTS, "Too many tries. Wait a minute."));
        }
        Err(problem(StatusCode::UNAUTHORIZED, "Not the operator's token"))
    }
}

// --- The page ---

fn ops_page(status: StatusCode, body: Markup) -> Response {
    (status, Html(body.into_string())).into_response()
}

async fn show_page(web: Web, uri: Uri, req_headers: axum::http::HeaderMap) -> Response {
    let Some(secret) = web.state.cfg.ops_token.as_deref() else { return not_found(web, uri).await };
    let body = if has_ops_session(secret, &req_headers, now_ms()) { ops_app() } else { ops_sign_in(None) };
    ops_page(StatusCode::OK, body)
}

// The Origin check (same_origin_posts) covers this form like any other.
async fn sign_in(web: Web, uri: Uri, form: Form) -> Response {
    let Some(secret) = web.state.cfg.ops_token.as_deref() else { return not_found(web, uri).await };
    if is_ops_token(secret, form.field("token").trim()) {
        let cookie = ops_cookie(&new_ops_session(secret, now_ms()));
        return (
            StatusCode::SEE_OTHER,
            [(header::LOCATION, HeaderValue::from_static("/ops")), (header::SET_COOKIE, cookie)],
        )
            .into_response();
    }
    if !wrong_try(&web.state, web.ip) {
        return ops_page(StatusCode::TOO_MANY_REQUESTS, ops_sign_in(Some("Too many tries. Wait a minute.")));
    }
    ops_page(StatusCode::UNAUTHORIZED, ops_sign_in(Some("That isn't the operator's token.")))
}

async fn sign_out(_: Operator) -> Response {
    ([(header::SET_COOKIE, ended_ops_cookie())], Json(json!({ "ok": true }))).into_response()
}

// --- Reading ---

/// A JSON body's fields; anything but an object reads as none.
fn read_body(body: &Bytes) -> Map<String, Value> {
    match serde_json::from_slice(body) {
        Ok(Value::Object(fields)) => fields,
        _ => Map::new(),
    }
}

/// A field as trimmed text, or "" when it isn't text.
fn text<'a>(body: &'a Map<String, Value>, name: &str) -> &'a str {
    body.get(name).and_then(Value::as_str).map_or("", str::trim)
}

/// As JavaScript counts a string's length.
fn js_length(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The account's state and its notices, for deciding (the repeat-infringer policy counts them).
/// `tv_limit` is what the operator gave it, or null; `tv_limits` the default, the most that can be given, and what holds now.
async fn account_summary(db: &impl GenericClient, user_id: Uuid, tv_limits: TvLimits) -> Result<Option<Value>> {
    let account = rows_json(
        db,
        &format!(
            "SELECT a.*, (SELECT count(*) FROM velio_private.tv_sessions t WHERE t.user_id = a.id) AS devices
             FROM {ACCOUNTS} a WHERE a.id = $1"
        ),
        &[&user_id],
    )
    .await?;
    let Some(Value::Object(mut summary)) = account.into_iter().next() else { return Ok(None) };
    let notices = rows_json(
        db,
        "SELECT id, received_at, kind, status, action FROM velio_private.notices WHERE account_id = $1
         ORDER BY received_at",
        &[&user_id],
    )
    .await?;
    let allowed = tv_limits.for_account(summary.get("tv_limit").and_then(Value::as_i64));
    summary
        .insert("tv_limits".into(), json!({ "default": tv_limits.default, "max": tv_limits.max, "allowed": allowed }));
    summary.insert("notices".into(), notices.into());
    Ok(Some(Value::Object(summary)))
}

/// An account's id from the path, which is a user's id.
fn user_id(id: &str) -> Option<Uuid> {
    id.parse().ok()
}

async fn list_notices(_: Operator, web: Web, Query(query): Query<HashMap<String, String>>) -> Answer {
    let status = query.get("status").map_or("open", String::as_str).to_owned();
    if !matches!(status.as_str(), "all" | "open" | "actioned" | "rejected") {
        return Ok(problem(StatusCode::BAD_REQUEST, "Unknown status"));
    }
    let client = web.state.db.client().await?;
    let notices = rows_json(
        &client,
        "SELECT id, received_at, kind, name, email, account_email, account_id, status FROM velio_private.notices
         WHERE $1 = 'all' OR status = $1 ORDER BY received_at DESC LIMIT 200",
        &[&status],
    )
    .await?;
    Ok(Json(json!({ "notices": notices })).into_response())
}

async fn show_notice(_: Operator, web: Web, Path(id): Path<String>) -> Answer {
    let tv_limits = web.state.cfg.tv_limits;
    let client = web.state.db.client().await?;
    let found = rows_json(&client, "SELECT * FROM velio_private.notices WHERE id = $1", &[&id]).await?;
    let Some(notice) = found.into_iter().next() else { return Ok(problem(StatusCode::NOT_FOUND, "No such notice")) };
    let account = match notice.get("account_id").and_then(Value::as_str).and_then(user_id) {
        Some(account_id) => account_summary(&client, account_id, tv_limits).await?,
        None => None,
    };
    Ok(Json(json!({ "notice": notice, "account": account })).into_response())
}

async fn handle_notice(_: Operator, web: Web, Path(id): Path<String>, body: Bytes) -> Answer {
    let body = read_body(&body);
    let (status, action) = (text(&body, "status").to_owned(), text(&body, "action").to_owned());
    if status != "actioned" && status != "rejected" {
        return Ok(problem(StatusCode::BAD_REQUEST, "status: actioned or rejected"));
    }
    if action.is_empty() || js_length(&action) > MAX_REASON_LENGTH {
        return Ok(problem(StatusCode::BAD_REQUEST, "Say what was done (action)"));
    }
    let client = web.state.db.client().await?;
    let changed = client
        .execute(
            "UPDATE velio_private.notices SET status = $1, action = $2, handled_at = $3 WHERE id = $4",
            &[&status, &action, &now_ms(), &id],
        )
        .await?;
    if changed != 1 {
        return Ok(problem(StatusCode::NOT_FOUND, "No such notice"));
    }
    Ok(Json(json!({ "ok": true })).into_response())
}

/// The most accounts listed at once; a search narrows it.
const ACCOUNT_PAGE: i64 = 200;
const MAX_SEARCH_LENGTH: usize = 320;

/// Accounts, newest first: in a state, and with `q` in the email or as the id.
/// Each with its TVs, the TVs it may have, and how many notices name it.
async fn list_accounts(_: Operator, web: Web, Query(query): Query<HashMap<String, String>>) -> Answer {
    let state = query.get("state").map_or("all", String::as_str).to_owned();
    if !matches!(state.as_str(), "all" | "open" | "unconfirmed" | "closed") {
        return Ok(problem(StatusCode::BAD_REQUEST, "Unknown state"));
    }
    let search = query.get("q").map_or("", |q| q.trim()).to_owned();
    if js_length(&search) > MAX_SEARCH_LENGTH {
        return Ok(problem(StatusCode::BAD_REQUEST, "That search is too long"));
    }
    // % and _ are literal here.
    let pattern = search.to_lowercase().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    let tv_limits = web.state.cfg.tv_limits;
    let matching = format!(
        "FROM {ACCOUNTS} a
         WHERE ($1 = 'all'
                OR ($1 = 'open' AND a.closed_at IS NULL AND a.confirmed_at IS NOT NULL)
                OR ($1 = 'unconfirmed' AND a.closed_at IS NULL AND a.confirmed_at IS NULL)
                OR ($1 = 'closed' AND a.closed_at IS NOT NULL))
           AND ($2 = '' OR a.id::text = $2 OR lower(a.email) LIKE '%' || $3 || '%' ESCAPE '\\')"
    );
    let client = web.state.db.client().await?;
    let total: i64 =
        client.query_one(&format!("SELECT count(*) {matching}"), &[&state, &search, &pattern]).await?.get(0);
    let accounts = rows_json(
        &client,
        &format!(
            "SELECT a.*,
               (SELECT count(*) FROM velio_private.tv_sessions t WHERE t.user_id = a.id) AS devices,
               (SELECT count(*) FROM velio_private.notices n WHERE n.account_id = a.id) AS notice_count
             {matching} ORDER BY a.created_at DESC NULLS LAST, a.id LIMIT $4"
        ),
        &[&state, &search, &pattern, &ACCOUNT_PAGE],
    )
    .await?;
    let accounts: Vec<Value> = accounts
        .into_iter()
        .map(|mut account| {
            let allowed = tv_limits.for_account(account.get("tv_limit").and_then(Value::as_i64));
            if let Value::Object(fields) = &mut account {
                fields.insert("allowed".into(), allowed.into());
            }
            account
        })
        .collect();
    let tv_limits = json!({ "default": tv_limits.default, "max": tv_limits.max });
    Ok(Json(json!({ "accounts": accounts, "total": total, "tv_limits": tv_limits })).into_response())
}

/// An account by its id or its email.
async fn find_id(db: &impl GenericClient, key: &str) -> Result<Option<Uuid>> {
    if !key.contains('@') {
        return Ok(user_id(key));
    }
    let row = db
        .query_opt(&format!("SELECT id FROM {ACCOUNTS} a WHERE lower(a.email) = $1"), &[&normalize_email(key)])
        .await?;
    Ok(row.map(|row| row.get(0)))
}

async fn show_account(_: Operator, web: Web, Path(key): Path<String>) -> Answer {
    let tv_limits = web.state.cfg.tv_limits;
    let client = web.state.db.client().await?;
    let summary = match find_id(&client, &key).await? {
        Some(id) => account_summary(&client, id, tv_limits).await?,
        None => None,
    };
    let Some(account) = summary else { return Ok(problem(StatusCode::NOT_FOUND, "No such account")) };
    Ok(Json(json!({ "account": account })).into_response())
}

// --- Closing and opening ---

async fn close(_: Operator, web: Web, Path(id): Path<String>, body: Bytes) -> Answer {
    let body = read_body(&body);
    let (reason, notice) = (text(&body, "reason").to_owned(), text(&body, "notice").to_owned());
    if reason.is_empty() || js_length(&reason) > MAX_REASON_LENGTH {
        return Ok(problem(StatusCode::BAD_REQUEST, "Give the reason the owner will read (reason)"));
    }
    let confirm = normalize_email(text(&body, "confirm"));
    let Some(id) = user_id(&id) else { return Ok(problem(StatusCode::NOT_FOUND, "No such account")) };
    let mut client = web.state.db.client().await?;
    let email: Option<String> = client
        .query_opt("SELECT lower(coalesce(email, '')) FROM auth.users WHERE id = $1", &[&id])
        .await?
        .map(|row| row.get(0));
    let Some(email) = email else { return Ok(problem(StatusCode::NOT_FOUND, "No such account")) };
    // The account's email typed again, so a slip of the id can't close another account.
    if confirm != email {
        let text = "The email typed doesn't match this account (confirm). Nothing was changed.";
        return Ok(problem(StatusCode::BAD_REQUEST, text));
    }
    let known_notice = notice.is_empty()
        || client.query_opt("SELECT 1 FROM velio_private.notices WHERE id = $1", &[&notice]).await?.is_some();
    if !known_notice {
        return Ok(problem(StatusCode::NOT_FOUND, "No such notice"));
    }

    let now = now_ms();
    let notice = Some(notice.as_str()).filter(|notice| !notice.is_empty());
    let email = match close_account(&mut client, id, &reason, notice, now).await? {
        Ok(email) => email,
        Err((status, text)) => return Ok(problem(status, text)),
    };
    let delete_at = now + crate::closing::CLOSED_KEEP_MS;
    let cfg = &web.state.cfg;
    mail::send(&web.state, closed_mail(&cfg.site_url, &email, &cfg.abuse_email, &reason, delete_at)).await?;
    Ok(Json(json!({ "ok": true, "deleteAt": delete_at })).into_response())
}

async fn reopen(_: Operator, web: Web, Path(id): Path<String>) -> Answer {
    let Some(id) = user_id(&id) else { return Ok(problem(StatusCode::NOT_FOUND, "No such account")) };
    let mut client = web.state.db.client().await?;
    let email = match reopen_account(&mut client, id).await? {
        Ok(email) => email,
        Err((status, text)) => return Ok(problem(status, text)),
    };
    mail::send(&web.state, reopened_mail(&web.state.cfg.site_url, &email)).await?;
    Ok(Json(json!({ "ok": true })).into_response())
}

// --- TV limits ---

/// Gives the account a TV limit, or takes it back to the default. One
/// lower than the TVs it has keeps them, and stops new ones until it's under.
async fn set_tv_limit(_: Operator, web: Web, Path(id): Path<String>, body: Bytes) -> Answer {
    let tv_limits = web.state.cfg.tv_limits;
    let limit = match read_body(&body).get("limit") {
        Some(Value::Null) => None,
        Some(value) => match value.as_u64().and_then(|n| usize::try_from(n).ok()) {
            Some(n) if (1..=tv_limits.max).contains(&n) => Some(n),
            _ => {
                let text = format!("limit: a number of TVs from 1 to {}, or null for the default", tv_limits.max);
                return Ok(problem(StatusCode::BAD_REQUEST, &text));
            }
        },
        None => return Ok(problem(StatusCode::BAD_REQUEST, "limit: a number of TVs, or null for the default")),
    };
    let Some(id) = user_id(&id) else { return Ok(problem(StatusCode::NOT_FOUND, "No such account")) };
    let client = web.state.db.client().await?;
    if client.query_opt("SELECT 1 FROM auth.users WHERE id = $1", &[&id]).await?.is_none() {
        return Ok(problem(StatusCode::NOT_FOUND, "No such account"));
    }
    // The default is kept as none, so a later VELIO_TV_LIMIT holds for the account too.
    match limit.filter(|n| *n != tv_limits.default).map(|n| n as i32) {
        Some(stored) => {
            client
                .execute(
                    "INSERT INTO velio_private.tv_limits (user_id, tv_limit) VALUES ($1, $2)
                     ON CONFLICT (user_id) DO UPDATE SET tv_limit = excluded.tv_limit",
                    &[&id, &stored],
                )
                .await?;
            tracing::info!("TV limit for {id}: {stored}");
        }
        None => {
            client.execute("DELETE FROM velio_private.tv_limits WHERE user_id = $1", &[&id]).await?;
            tracing::info!("TV limit for {id}: the default");
        }
    }
    let Some(account) = account_summary(&client, id, tv_limits).await? else {
        return Ok(problem(StatusCode::NOT_FOUND, "No such account"));
    };
    Ok(Json(json!({ "ok": true, "account": account })).into_response())
}
