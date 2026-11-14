//! Making, confirming, signing in to and resetting an account (the Worker's
//! `routes/web.tsx`; PLAN.md, "Routes"). The password never reaches the
//! server: vault.js sends the auth key, salt, settings and owner bundle.

use anyhow::Result;
use axum::Json;
use axum::Router;
use axum::extract::{Path, RawQuery};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use maud::html;
use rusqlite::{Connection, params};

use crate::AppState;
use crate::accounts::{
    CONFIRM_LINK_MS, Credentials, Fields, LinkKind, RESET_LINK_MS, auth_hash_of, email_problem, find_account,
    find_account_by_id, new_email_link, normalize_email, peek_email_link, prelogin_for, read_credentials,
    use_email_link,
};
use crate::error::AppError;
use crate::http::api_problem;
use crate::limits::Limit;
use crate::mail::{self, confirm_mail, existing_account_mail, reset_mail};
use crate::routes::vault::vault_props;
use crate::session::{drop_session, ended_cookie, session_cookie, start_session};
use crate::time::now_ms;
use crate::tokens::same_digest;
use crate::turnstile;
use crate::vault::{self, NO_KEYS};
use crate::views::accounts::{self as views, AccountView, OutboxRow};
use crate::views::pair::{PAIR_TITLE, pair_page};
use crate::views::vault as vault_views;
use crate::views::{layout, pages};
use crate::web::{Form, TOO_MANY, Web, found, page_fragment, set_header};

type Answer = Result<Response, AppError>;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/signup", get(signup_page).post(signup))
        .route("/confirm/{token}", get(confirm_page).post(confirm))
        .route("/signin", get(signin_page).post(signin))
        .route("/prelogin", get(prelogin))
        .route("/signout", post(signout))
        .route("/account", get(account))
        .route("/reset", get(reset_request_page).post(reset_request))
        .route("/reset/{token}", get(reset_page).post(reset))
        .route("/dev/outbox", get(dev_outbox))
}

const NOT_A_PERSON: &str = "The check that you're a person didn't pass. Try again.";
pub(crate) const LINK_GONE: &str = "This link was already used, or it's too old. Ask for a new one.";
pub(crate) const CLOSED: &str = "This account is closed.";

// --- Making an account ---

async fn signup_page(web: Web) -> Response {
    if web.session.is_some() {
        return found("/account");
    }
    web.turnstile_page("Make an account", views::signup_form(web.site_key()))
}

enum Signup {
    /// A new account, or one never confirmed, which this replaces: its confirmation link.
    Made(String),
    /// The email has a confirmed account.
    Existing,
    /// Closed, or confirmed or made by another request in the meantime.
    Nothing,
}

async fn signup(web: Web, form: Form) -> Answer {
    if !web.within(Limit::Signup, &format!("signup:{}", web.ip)) {
        return Ok(web.problem(TOO_MANY, StatusCode::TOO_MANY_REQUESTS));
    }
    let email = normalize_email(form.field("email"));
    if let Some(problem) = email_problem(&email) {
        return Ok(web.problem(problem, StatusCode::BAD_REQUEST));
    }
    if form.field("terms") != "yes" {
        return Ok(web.problem("Accept the terms to make an account.", StatusCode::BAD_REQUEST));
    }
    if !turnstile::passed(&web.state, form.field(turnstile::FIELD), web.ip).await {
        return Ok(web.problem(NOT_A_PERSON, StatusCode::BAD_REQUEST));
    }
    let credentials = match read_credentials(&form) {
        Ok(credentials) => credentials,
        Err(problem) => return Ok(web.problem(&problem, StatusCode::BAD_REQUEST)),
    };
    if !web.within(Limit::Email, &format!("mail:{email}")) {
        return Ok(web.problem(TOO_MANY, StatusCode::TOO_MANY_REQUESTS));
    }

    let outcome = web.state.db.call({
        let email = email.clone();
        move |conn| make_account(conn, &email, &credentials)
    });
    let site = &web.state.cfg.site_url;
    // The same answer whether or not the email has an account, so the form doesn't tell.
    match outcome.await? {
        Signup::Made(token) => {
            mail::send(&web.state, confirm_mail(site, &email, &format!("{site}/confirm/{token}"))).await?;
        }
        Signup::Existing => mail::send(&web.state, existing_account_mail(site, &email)).await?,
        Signup::Nothing => {}
    }
    Ok(web.answer("Check your email", views::check_email(&email)))
}

/// Whoever can read the confirmation email gets the address, so an unconfirmed account is replaced.
fn make_account(conn: &mut Connection, email: &str, credentials: &Credentials) -> Result<Signup> {
    let tx = conn.transaction()?;
    let existing = find_account(&tx, email)?;
    if let Some(account) = &existing
        && (account.confirmed_at.is_some() || account.closed_at.is_some())
    {
        return Ok(if account.closed_at.is_none() { Signup::Existing } else { Signup::Nothing });
    }
    let now = now_ms();
    let Credentials { salt, kdf, auth_hash, owner } = credentials;
    let (account_id, changes) = match existing {
        Some(account) => {
            let changes = tx.execute(
                "UPDATE accounts SET salt = ?, kdf_memory_kib = ?, kdf_iterations = ?, kdf_parallelism = ?,
                   auth_hash = ?, terms_accepted_at = ?, created_at = ? WHERE id = ? AND confirmed_at IS NULL",
                params![salt, kdf.memory_kib, kdf.iterations, kdf.parallelism, auth_hash, now, now, account.id],
            )?;
            (account.id, changes)
        }
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            let changes = tx.execute(
                "INSERT INTO accounts (id, email, salt, kdf_memory_kib, kdf_iterations, kdf_parallelism, auth_hash,
                   terms_accepted_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT (email) DO NOTHING",
                params![id, email, salt, kdf.memory_kib, kdf.iterations, kdf.parallelism, auth_hash, now, now],
            )?;
            (id, changes)
        }
    };
    if changes == 0 {
        return Ok(Signup::Nothing);
    }
    vault::reset(&tx, &account_id)?;
    vault::set_owner(&tx, &account_id, owner)?;
    let token = new_email_link(&tx, &account_id, LinkKind::Confirm, CONFIRM_LINK_MS)?;
    tx.commit()?;
    Ok(Signup::Made(token))
}

async fn confirm_page(web: Web, Path(token): Path<String>) -> Answer {
    let found = web.state.db.call({
        let token = token.clone();
        move |conn| peek_email_link(conn, &token, LinkKind::Confirm)
    });
    if found.await?.is_none() {
        return Ok(web.page(StatusCode::NOT_FOUND, "Link", views::link_problem()));
    }
    Ok(web.page(StatusCode::OK, "Confirm your email", views::confirm_page(&token)))
}

async fn confirm(web: Web, Path(token): Path<String>) -> Answer {
    let email = web
        .state
        .db
        .call(move |conn| {
            let Some(account_id) = use_email_link(conn, &token, LinkKind::Confirm)? else { return Ok(None) };
            conn.execute(
                "UPDATE accounts SET confirmed_at = ? WHERE id = ? AND confirmed_at IS NULL",
                params![now_ms(), account_id],
            )?;
            Ok(Some(find_account_by_id(conn, &account_id)?.map(|account| account.email)))
        })
        .await?;
    let Some(email) = email else { return Ok(web.answer("Link", views::link_problem())) };
    let note = "Your email is confirmed. Sign in to start.";
    let mut response = web.answer("Sign in", views::signin_form(email.as_deref(), Some(note), None));
    if web.htmx {
        set_header(&mut response, "HX-Push-Url", "/signin");
    }
    Ok(response)
}

// --- Signing in ---

async fn signin_page(web: Web) -> Response {
    if web.session.is_some() {
        return found("/account");
    }
    web.page(StatusCode::OK, "Sign in", views::signin_form(None, None, None))
}

/// The salt and Argon2id settings for an email; made-up but stable ones when it has no account.
async fn prelogin(web: Web, RawQuery(query): RawQuery) -> Answer {
    let mut response = prelogin_answer(&web, query.as_deref().unwrap_or("")).await?;
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

async fn prelogin_answer(web: &Web, query: &str) -> Answer {
    if !web.within(Limit::Auth, &format!("prelogin:{}", web.ip)) {
        return Ok(api_problem(StatusCode::TOO_MANY_REQUESTS, TOO_MANY));
    }
    let email = form_urlencoded::parse(query.as_bytes())
        .filter(|(key, _)| key == "email")
        .map(|(_, value)| normalize_email(&value))
        .next()
        .unwrap_or_default();
    if let Some(problem) = email_problem(&email) {
        return Ok(api_problem(StatusCode::BAD_REQUEST, problem));
    }
    let secret = web.state.cfg.prelogin_secret.clone();
    let answer = web.state.db.call(move |conn| Ok(prelogin_for(&secret, &email, find_account(conn, &email)?.as_ref())));
    Ok(Json(answer.await?).into_response())
}

async fn signin(web: Web, form: Form) -> Answer {
    if !web.within(Limit::Auth, &format!("signin:{}", web.ip)) {
        return Ok(web.problem(TOO_MANY, StatusCode::TOO_MANY_REQUESTS));
    }
    let email = normalize_email(form.field("email"));
    if email_problem(&email).is_some() {
        return Ok(web.problem("Enter your email address.", StatusCode::BAD_REQUEST));
    }
    if !web.within(Limit::Email, &format!("signin:{email}")) {
        return Ok(web.problem(TOO_MANY, StatusCode::TOO_MANY_REQUESTS));
    }
    let Some(auth_hash) = auth_hash_of(form.field("auth_key")) else {
        let problem = "This browser didn't send a sign-in key. Reload the page and try again.";
        return Ok(web.problem(problem, StatusCode::BAD_REQUEST));
    };
    let account = web.state.db.call(move |conn| find_account(conn, &email)).await?;
    let Some(account) = account.filter(|account| same_digest(&account.auth_hash, &auth_hash)) else {
        return Ok(web.problem("That email and password don't match an account.", StatusCode::UNAUTHORIZED));
    };
    if account.closed_at.is_some() {
        return Ok(web.problem(CLOSED, StatusCode::FORBIDDEN));
    }
    if account.confirmed_at.is_none() {
        let problem = "Confirm your email first, with the link we sent. For a new link, make the account again.";
        return Ok(web.problem(problem, StatusCode::FORBIDDEN));
    }
    let token = web
        .state
        .db
        .call({
            let (old, account_id) = (web.session.clone(), account.id.clone());
            move |conn| {
                drop_session(conn, old.as_ref())?;
                start_session(conn, &account_id)
            }
        })
        .await?;
    signed_in_answer(&web, &account.id, &account.email, &token, form.field("next"), None).await
}

/// Signed in: the account page, or the pairing page when signing in on the
/// way there, with the owner bundle for vault.js to open, and the new
/// session's cookie.
pub(crate) async fn signed_in_answer(
    web: &Web,
    account_id: &str,
    email: &str,
    token: &str,
    next: &str,
    notice: Option<&str>,
) -> Answer {
    let mut response = signed_in_page(web, account_id, email, next, notice).await?;
    response.headers_mut().append(header::SET_COOKIE, session_cookie(token));
    Ok(response)
}

async fn signed_in_page(web: &Web, account_id: &str, email: &str, next: &str, notice: Option<&str>) -> Answer {
    let id = account_id.to_owned();
    let (owner, devices, recovery_made_at) = web
        .state
        .db
        .call(move |conn| {
            Ok((vault::get_owner(conn, &id)?, vault::devices_of(conn, &id)?, vault::recovery_made_at(conn, &id)?))
        })
        .await?;
    let Some(owner) = owner else { return Ok(web.problem(NO_KEYS, StatusCode::INTERNAL_SERVER_ERROR)) };
    if next == "/pair" {
        let Some(props) = vault_props(web, account_id, email).await? else {
            return Ok(web.problem(NO_KEYS, StatusCode::INTERNAL_SERVER_ERROR));
        };
        let mut response = page_fragment(
            PAIR_TITLE,
            html! {
                (vault_views::owner_bundle(&owner))
                (pair_page(&props, None))
                (layout::nav(Some(account_id), true))
            },
        );
        set_header(&mut response, "HX-Push-Url", "/pair");
        return Ok(response);
    }
    let view = AccountView {
        email,
        owner: Some(&owner),
        devices: &devices,
        recovery_made_at,
        notice,
        show_recovery_key: false,
    };
    let mut response =
        page_fragment("Your account", html! { (views::account_view(&view)) (layout::nav(Some(account_id), true)) });
    set_header(&mut response, "HX-Push-Url", "/account");
    Ok(response)
}

/// The account page as a fragment for #main, after a setting changed.
pub(crate) async fn account_answer(
    web: &Web,
    account_id: &str,
    email: &str,
    notice: Option<&str>,
    show_recovery_key: bool,
) -> Answer {
    let id = account_id.to_owned();
    let (devices, recovery_made_at) =
        web.state.db.call(move |conn| Ok((vault::devices_of(conn, &id)?, vault::recovery_made_at(conn, &id)?))).await?;
    let view = AccountView { email, owner: None, devices: &devices, recovery_made_at, notice, show_recovery_key };
    let mut response = web.answer("Your account", views::account_view(&view));
    if web.htmx {
        set_header(&mut response, "HX-Push-Url", "/account");
    }
    Ok(response)
}

async fn signout(web: Web) -> Answer {
    let session = web.session.clone();
    web.state.db.call(move |conn| drop_session(conn, session.as_ref())).await?;
    let mut response = if web.htmx {
        let mut response = StatusCode::NO_CONTENT.into_response();
        set_header(&mut response, "HX-Redirect", "/");
        response
    } else {
        (StatusCode::SEE_OTHER, [(header::LOCATION, "/")]).into_response()
    };
    response.headers_mut().append(header::SET_COOKIE, ended_cookie());
    Ok(response)
}

async fn account(web: Web) -> Answer {
    let Some(session) = web.session.clone() else { return Ok(found("/signin")) };
    let id = session.account_id.clone();
    let (devices, recovery_made_at) =
        web.state.db.call(move |conn| Ok((vault::devices_of(conn, &id)?, vault::recovery_made_at(conn, &id)?))).await?;
    let view = AccountView {
        email: &session.email,
        owner: None,
        devices: &devices,
        recovery_made_at,
        notice: None,
        show_recovery_key: false,
    };
    Ok(web.page(StatusCode::OK, "Your account", views::account_view(&view)))
}

// --- Resetting ---

async fn reset_request_page(web: Web) -> Response {
    web.turnstile_page("Reset your account", views::reset_request_form(web.site_key()))
}

async fn reset_request(web: Web, form: Form) -> Answer {
    if !web.within(Limit::Signup, &format!("reset:{}", web.ip)) {
        return Ok(web.problem(TOO_MANY, StatusCode::TOO_MANY_REQUESTS));
    }
    let email = normalize_email(form.field("email"));
    if let Some(problem) = email_problem(&email) {
        return Ok(web.problem(problem, StatusCode::BAD_REQUEST));
    }
    if !turnstile::passed(&web.state, form.field(turnstile::FIELD), web.ip).await {
        return Ok(web.problem(NOT_A_PERSON, StatusCode::BAD_REQUEST));
    }
    if !web.within(Limit::Email, &format!("mail:{email}")) {
        return Ok(web.problem(TOO_MANY, StatusCode::TOO_MANY_REQUESTS));
    }
    let token = web.state.db.call({
        let email = email.clone();
        move |conn| match find_account(conn, &email)? {
            Some(account) if account.closed_at.is_none() => {
                Ok(Some(new_email_link(conn, &account.id, LinkKind::Reset, RESET_LINK_MS)?))
            }
            _ => Ok(None),
        }
    });
    if let Some(token) = token.await? {
        let site = &web.state.cfg.site_url;
        mail::send(&web.state, reset_mail(site, &email, &format!("{site}/reset/{token}"))).await?;
    }
    Ok(web.answer("Check your email", views::reset_sent(&email)))
}

async fn reset_page(web: Web, Path(token): Path<String>) -> Answer {
    let recovery = web.state.db.call({
        let token = token.clone();
        move |conn| match peek_email_link(conn, &token, LinkKind::Reset)? {
            Some(account_id) => Ok(Some(vault::recovery_made_at(conn, &account_id)?.is_some())),
            None => Ok(None),
        }
    });
    let Some(recovery) = recovery.await? else {
        return Ok(web.page(StatusCode::NOT_FOUND, "Link", views::link_problem()));
    };
    Ok(web.page(StatusCode::OK, "Choose a new password", views::reset_form(&token, recovery)))
}

enum Reset {
    /// The account, its email, and the new session's token.
    Done(String, String, String),
    LinkGone,
    Closed,
}

async fn reset(web: Web, Path(token): Path<String>, form: Form) -> Answer {
    if !web.within(Limit::Auth, &format!("reset-link:{}", web.ip)) {
        return Ok(web.problem(TOO_MANY, StatusCode::TOO_MANY_REQUESTS));
    }
    let credentials = match read_credentials(&form) {
        Ok(credentials) => credentials,
        Err(problem) => return Ok(web.problem(&problem, StatusCode::BAD_REQUEST)),
    };
    match web.state.db.call(move |conn| reset_account(conn, &token, &credentials)).await? {
        Reset::LinkGone => Ok(web.problem(LINK_GONE, StatusCode::NOT_FOUND)),
        Reset::Closed => Ok(web.problem(CLOSED, StatusCode::FORBIDDEN)),
        Reset::Done(account_id, email, session) => {
            // Its TVs were signed out with the old keys.
            web.state.sockets.close_account(&account_id);
            signed_in_answer(&web, &account_id, &email, &session, "", None).await
        }
    }
}

/// New credentials; every session and TV is signed out, as the old keys no
/// longer open anything. Opening the link also proves the email, so it
/// confirms an unconfirmed account.
fn reset_account(conn: &mut Connection, token: &str, credentials: &Credentials) -> Result<Reset> {
    let tx = conn.transaction()?;
    let Some(account_id) = use_email_link(&tx, token, LinkKind::Reset)? else { return Ok(Reset::LinkGone) };
    let account = find_account_by_id(&tx, &account_id)?;
    let Some(account) = account.filter(|account| account.closed_at.is_none()) else {
        tx.commit()?; // The link is used up, as on the Worker.
        return Ok(Reset::Closed);
    };
    let Credentials { salt, kdf, auth_hash, owner } = credentials;
    tx.execute(
        "UPDATE accounts SET salt = ?, kdf_memory_kib = ?, kdf_iterations = ?, kdf_parallelism = ?, auth_hash = ?,
           confirmed_at = COALESCE(confirmed_at, ?) WHERE id = ?",
        params![salt, kdf.memory_kib, kdf.iterations, kdf.parallelism, auth_hash, now_ms(), account_id],
    )?;
    for table in ["sessions", "email_links", "devices", "pairings"] {
        tx.execute(&format!("DELETE FROM {table} WHERE account_id = ?"), [&account_id])?;
    }
    vault::reset(&tx, &account_id)?;
    vault::set_owner(&tx, &account_id, owner)?;
    let session = start_session(&tx, &account_id)?;
    tx.commit()?;
    Ok(Reset::Done(account_id, account.email, session))
}

// --- Development ---

async fn dev_outbox(web: Web) -> Answer {
    if !web.state.cfg.dev_mode {
        return Ok(web.page(StatusCode::NOT_FOUND, "Not found", pages::not_found()));
    }
    let mails = web
        .state
        .db
        .call(|conn| {
            let mut statement =
                conn.prepare("SELECT sent_at, to_address, subject, text FROM dev_outbox ORDER BY id DESC LIMIT 50")?;
            let rows = statement.query_map([], |row| {
                Ok(OutboxRow { sent_at: row.get(0)?, to_address: row.get(1)?, subject: row.get(2)?, text: row.get(3)? })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await?;
    Ok(web.page(StatusCode::OK, "Outbox", views::dev_outbox(&mails)))
}
