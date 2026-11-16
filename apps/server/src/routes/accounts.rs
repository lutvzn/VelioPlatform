//! Making, confirming, signing in to and resetting an account, and the
//! account page. All of it goes through Supabase Auth's own steps
//! (`api::auth`), so an account made here signs in to Nuvio's apps, and one
//! made in Nuvio's apps signs in here.
//!
//! Signing in makes a session of Auth's, as an app's sign-in does; the
//! browser's cookie stands on it (session.rs), and it's registered as a
//! "Nuvio Web" device, so it shows on the devices page with the TVs and apps.

use axum::Router;
use axum::extract::Query;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use maud::html;
use serde::Deserialize;
use uuid::Uuid;

use crate::AppState;
use crate::accounts::{Fields, email_problem, normalize_email};
use crate::api::auth::{
    self, AuthError, Authenticated, LinkType, SignedUp, Tokens, UserChange, check_password, delete_user,
    password_grant, peek_link, request_recovery, sign_up, update_user, verify_link,
};
use crate::error::AppError;
use crate::limits::Limit;
use crate::session::{drop_session, ended_cookie, session_cookie, start_session, user_claims};
use crate::tokens::new_token;
use crate::turnstile;
use crate::views::accounts::{self as views, AccountView, LinkPage, OutboxRow};
use crate::views::{layout, pages};
use crate::web::{Form, LINK_GONE, TOO_MANY, Web, auth_words, found, page_fragment, set_header};

type Answer = Result<Response, AppError>;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/signup", get(signup_page).post(signup))
        .route("/verify", get(verify_page).post(verify))
        .route("/signin", get(signin_page).post(signin))
        .route("/signout", post(signout))
        .route("/account", get(account))
        .route("/account/email", post(change_email))
        .route("/account/password", post(change_password))
        .route("/account/delete", post(delete_account))
        .route("/forgot", get(forgot_page).post(forgot))
        .route("/reset", get(reset_page).post(reset))
        .route("/dev/outbox", get(dev_outbox))
}

const NOT_A_PERSON: &str = "The check that you're a person didn't pass. Try again.";
const WRONG_PASSWORD: &str = "That isn't your current password.";

/// Auth failed rather than refused: a 500.
fn failed(error: AuthError) -> AppError {
    anyhow::anyhow!("Auth: {} {}", error.code, error.message).into()
}

/// The new password, typed twice, checked before anything is used up.
fn new_password<'a>(web: &Web, form: &'a Form) -> Result<&'a str, String> {
    let password = form.field("password");
    let min = web.state.cfg.auth.password_min_length;
    if password.chars().count() < min {
        return Err(format!("Use at least {min} characters for the password."));
    }
    if password.len() > 72 {
        return Err("Use at most 72 characters for the password.".into());
    }
    if password != form.field("password_again") {
        return Err("The two passwords aren't the same.".into());
    }
    Ok(password)
}

// --- Making an account ---

async fn signup_page(web: Web) -> Response {
    if web.session.is_some() {
        return found("/account");
    }
    web.turnstile_page("Make an account", views::signup_form(web.site_key(), web.state.cfg.auth.password_min_length))
}

async fn signup(web: Web, form: Form) -> Answer {
    let email = normalize_email(form.field("email"));
    if let Some(problem) = email_problem(&email) {
        return Ok(web.problem(problem, StatusCode::BAD_REQUEST));
    }
    let password = match new_password(&web, &form) {
        Ok(password) => password,
        Err(problem) => return Ok(web.problem(&problem, StatusCode::BAD_REQUEST)),
    };
    if form.field("terms") != "yes" {
        return Ok(web.problem("Accept the terms to make an account.", StatusCode::BAD_REQUEST));
    }
    if !turnstile::passed(&web.state, form.field(turnstile::FIELD), web.ip).await {
        return Ok(web.problem(NOT_A_PERSON, StatusCode::BAD_REQUEST));
    }
    // The same answer whether or not the email has an account, as GoTrue's.
    match sign_up(&web.state, &email, password, None, &web.meta).await {
        Ok(SignedUp::Pending(_)) => Ok(web.answer("Check your email", views::check_email(&email))),
        // ENABLE_EMAIL_AUTOCONFIRM: signed in at once.
        Ok(SignedUp::Session(tokens)) => signed_in(&web, &tokens, "", None).await,
        Err(error) => web.auth_problem(&error),
    }
}

#[derive(Deserialize)]
struct LinkQuery {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    token: String,
}

fn link_page(kind: LinkType) -> Option<LinkPage> {
    match kind {
        LinkType::Signup => Some(LinkPage::Signup),
        LinkType::EmailChange => Some(LinkPage::EmailChange),
        LinkType::Recovery => None,
    }
}

async fn verify_page(web: Web, Query(query): Query<LinkQuery>) -> Answer {
    let Some(kind) = LinkType::parse(&query.kind) else {
        return Ok(web.page(StatusCode::NOT_FOUND, "Link", views::link_problem()));
    };
    // A reset link sent as /verify, as GoTrue's links can be.
    if kind == LinkType::Recovery {
        return reset_page(web, Query(TokenQuery { token: query.token })).await;
    }
    let client = web.state.db.client().await?;
    let user = peek_link(&client, kind, &query.token).await.map_err(failed)?;
    let (Some(_), Some(page)) = (user, link_page(kind)) else {
        return Ok(web.page(StatusCode::NOT_FOUND, "Link", views::link_problem()));
    };
    Ok(web.page(StatusCode::OK, "Confirm your email", views::verify_page(page, &query.token)))
}

/// Opening the link confirms the email and signs in, as GoTrue's /verify.
async fn verify(web: Web, form: Form) -> Answer {
    let Some(kind) = LinkType::parse(form.field("type")).filter(|kind| *kind != LinkType::Recovery) else {
        return Ok(web.problem(LINK_GONE, StatusCode::NOT_FOUND));
    };
    let notice = match kind {
        LinkType::EmailChange => "Your email is changed.",
        _ => "Your email is confirmed.",
    };
    match verify_link(&web.state, kind, form.field("token"), &web.meta).await {
        Ok(tokens) => signed_in(&web, &tokens, "", Some(notice)).await,
        Err(error) => web.auth_problem(&error),
    }
}

// --- Signing in ---

#[derive(Deserialize)]
struct NextQuery {
    #[serde(default)]
    next: String,
}

/// Where to go after signing in: only the pages that send people to sign in.
fn safe_next(next: &str) -> Option<&str> {
    let path = next.split(['?', '#']).next().unwrap_or("");
    let allowed = matches!(path, "/tv-login" | "/link" | "/account" | "/lists" | "/profiles" | "/devices");
    let plain = next.chars().all(|c| c.is_ascii_graphic()) && !next.contains('\\');
    (allowed && plain).then_some(next)
}

async fn signin_page(web: Web, Query(query): Query<NextQuery>) -> Response {
    let next = safe_next(&query.next);
    if web.session.is_some() {
        return found("/account");
    }
    let note = next.filter(|next| next.starts_with("/tv-login") || next.starts_with("/link"));
    let note = note.map(|_| "Sign in to finish signing in the TV or app.");
    web.page(StatusCode::OK, "Sign in", views::signin_form(None, note, next))
}

async fn signin(web: Web, form: Form) -> Answer {
    let email = normalize_email(form.field("email"));
    if email_problem(&email).is_some() {
        return Ok(web.problem("Enter your email address.", StatusCode::BAD_REQUEST));
    }
    match password_grant(&web.state, &email, form.field("password"), &web.meta).await {
        Ok(tokens) => signed_in(&web, &tokens, form.field("next"), None).await,
        Err(error) => web.auth_problem(&error),
    }
}

/// Starts this browser's session on Auth's new one, and answers with the
/// account page, or the page that sent the visitor to sign in.
async fn signed_in(web: &Web, tokens: &Tokens, next: &str, notice: Option<&str>) -> Answer {
    let email = tokens.user.get("email").and_then(|email| email.as_str()).unwrap_or_default().to_owned();
    let client = web.state.db.client().await?;
    drop_session(&client, web.session.as_ref()).await?;
    let cookie = start_session(&client, tokens.user_id, tokens.session_id).await?;
    drop(client);
    register_browser(web, user_claims(tokens.user_id, tokens.session_id, &email)).await?;

    let mut response = match safe_next(next) {
        Some(next) if web.htmx => {
            let mut response = StatusCode::NO_CONTENT.into_response();
            response.headers_mut().insert("HX-Redirect", HeaderValue::try_from(next)?);
            response
        }
        Some(next) => (StatusCode::SEE_OTHER, [(header::LOCATION, HeaderValue::try_from(next)?)]).into_response(),
        None => {
            let view = AccountView {
                devices: other_devices(web, user_claims(tokens.user_id, tokens.session_id, &email)).await?,
                new_email: new_email_of(web, tokens.user_id).await?,
                email,
                min_password: web.state.cfg.auth.password_min_length,
                notice: notice.map(str::to_owned),
            };
            let account_id = tokens.user_id.to_string();
            let mut response = page_fragment(
                "Your account",
                html! { (views::account_view(&view)) (layout::nav(Some(&account_id), true)) },
            );
            set_header(&mut response, "HX-Push-Url", "/account");
            response
        }
    };
    response.headers_mut().append(header::SET_COOKIE, session_cookie(&cookie));
    Ok(response)
}

/// A short name for the browser, for the devices page: "Firefox on Linux".
fn browser_name(user_agent: Option<&str>) -> String {
    let agent = user_agent.unwrap_or_default();
    let browser =
        [("Edg/", "Edge"), ("OPR/", "Opera"), ("Firefox/", "Firefox"), ("Chrome/", "Chrome"), ("Safari/", "Safari")]
            .iter()
            .find(|(mark, _)| agent.contains(mark))
            .map_or("A browser", |(_, name)| name);
    let system = [
        ("Android", "Android"),
        ("iPhone", "iOS"),
        ("iPad", "iPadOS"),
        ("Windows", "Windows"),
        ("Mac OS", "macOS"),
        ("CrOS", "ChromeOS"),
        ("Linux", "Linux"),
    ]
    .iter()
    .find(|(mark, _)| agent.contains(mark))
    .map(|(_, name)| *name);
    match system {
        Some(system) => format!("{browser} on {system}"),
        None => browser.to_owned(),
    }
}

/// Registers this browser's session as Nuvio's web app registers its own,
/// so it's listed, and can be signed out, with the TVs and apps.
async fn register_browser(web: &Web, claims: serde_json::Value) -> Result<(), AppError> {
    let installation = new_token();
    let name = browser_name(web.meta.user_agent.as_deref());
    let version = env!("CARGO_PKG_VERSION");
    let registered = web
        .as_claims(claims, async |tx| {
            tx.execute(
                "SELECT public.register_current_device(p_installation_id => $1, p_client_name => 'Nuvio Web',
                     p_client_version => $2, p_platform => 'web', p_device_name => $3)",
                &[&installation, &version, &name],
            )
            .await
        })
        .await?;
    if let Err(problem) = registered {
        tracing::warn!("Registering a browser's session: {problem}");
    }
    Ok(())
}

/// How many of the user's other sessions are signed in.
async fn other_devices(web: &Web, claims: serde_json::Value) -> Result<usize, AppError> {
    let count = web
        .as_claims(claims, async |tx| {
            let row = tx.query_one("SELECT count(*) FROM public.list_my_sessions() WHERE NOT is_current", &[]).await?;
            Ok(row.get::<_, i64>(0))
        })
        .await?;
    Ok(count.map_or(0, |count| usize::try_from(count).unwrap_or(0)))
}

async fn new_email_of(web: &Web, user_id: Uuid) -> Result<Option<String>, AppError> {
    let client = web.state.db.client().await?;
    let row = client.query_opt("SELECT nullif(email_change, '') FROM auth.users WHERE id = $1", &[&user_id]).await?;
    Ok(row.and_then(|row| row.get(0)))
}

async fn signout(web: Web) -> Answer {
    let client = web.state.db.client().await?;
    drop_session(&client, web.session.as_ref()).await?;
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

// --- The account page ---

async fn account_view(web: &Web, notice: Option<&str>) -> Result<Option<AccountView>, AppError> {
    let Some(session) = &web.session else { return Ok(None) };
    Ok(Some(AccountView {
        email: session.email.clone(),
        new_email: new_email_of(web, session.user_id).await?,
        devices: other_devices(web, session.claims()).await?,
        min_password: web.state.cfg.auth.password_min_length,
        notice: notice.map(str::to_owned),
    }))
}

async fn account(web: Web) -> Answer {
    let Some(view) = account_view(&web, None).await? else { return Ok(found("/signin?next=/account")) };
    Ok(web.page(StatusCode::OK, "Your account", views::account_view(&view)))
}

/// The account page as a fragment for #main, after a change.
async fn account_answer(web: &Web, notice: &str) -> Answer {
    let Some(view) = account_view(web, Some(notice)).await? else { return Ok(found("/signin")) };
    let mut response = web.answer("Your account", views::account_view(&view));
    if web.htmx {
        set_header(&mut response, "HX-Push-Url", "/account");
    }
    Ok(response)
}

/// The signed-in user, as Auth's changes take them, once their current
/// password is typed again.
async fn confirmed_user(
    web: &Web,
    form: &Form,
    target: &'static str,
) -> Result<Result<Authenticated, Response>, AppError> {
    let Some(session) = &web.session else { return Ok(Err(found("/signin"))) };
    if !web.within(Limit::Auth, &format!("password:{}", web.ip)) {
        return Ok(Err(web.problem_at(TOO_MANY, StatusCode::TOO_MANY_REQUESTS, target)));
    }
    if !check_password(&web.state, session.user_id, form.field("current_password")).await.map_err(failed)? {
        return Ok(Err(web.problem_at(WRONG_PASSWORD, StatusCode::FORBIDDEN, target)));
    }
    Ok(Ok(Authenticated {
        claims: session.claims(),
        user_id: session.user_id,
        session_id: Some(session.session_id),
        role: "authenticated".into(),
    }))
}

async fn change_email(web: Web, form: Form) -> Answer {
    const TARGET: &str = "#email-problem";
    let email = normalize_email(form.field("email"));
    if let Some(problem) = email_problem(&email) {
        return Ok(web.problem_at(problem, StatusCode::BAD_REQUEST, TARGET));
    }
    let who = match confirmed_user(&web, &form, TARGET).await? {
        Ok(who) => who,
        Err(response) => return Ok(response),
    };
    let change = UserChange { email: Some(email.clone()), ..UserChange::default() };
    if let Err(error) = update_user(&web.state, &who, change).await {
        let Some(words) = auth_words(&web.state, &error) else { return Err(failed(error)) };
        return Ok(web.problem_at(&words, error.status, TARGET));
    }
    account_answer(&web, &format!("We sent a link to {email}. Your email changes when it's opened.")).await
}

async fn change_password(web: Web, form: Form) -> Answer {
    const TARGET: &str = "#password-problem";
    let password = match new_password(&web, &form) {
        Ok(password) => password.to_owned(),
        Err(problem) => return Ok(web.problem_at(&problem, StatusCode::BAD_REQUEST, TARGET)),
    };
    let who = match confirmed_user(&web, &form, TARGET).await? {
        Ok(who) => who,
        Err(response) => return Ok(response),
    };
    // Auth signs out the user's other sessions, as on a password change in Nuvio's apps.
    let change = UserChange { password: Some(password), ..UserChange::default() };
    if let Err(error) = update_user(&web.state, &who, change).await {
        let Some(words) = auth_words(&web.state, &error) else { return Err(failed(error)) };
        return Ok(web.problem_at(&words, error.status, TARGET));
    }
    account_answer(&web, "Your password is changed. Everything else signed in is signed out.").await
}

async fn delete_account(web: Web, form: Form) -> Answer {
    let who = match confirmed_user(&web, &form, "#delete-problem").await? {
        Ok(who) => who,
        Err(response) => return Ok(response),
    };
    delete_user(&web.state, who.user_id).await.map_err(failed)?;
    let body = html! {
        section .auth {
            h1 { "Your account is deleted" }
            p { "Everything of yours is gone from this server, and every TV and app is signed out." }
        }
        (layout::nav(None, true))
    };
    let mut response = web.answer("Account deleted", body);
    if web.htmx {
        set_header(&mut response, "HX-Push-Url", "/");
    }
    response.headers_mut().append(header::SET_COOKIE, ended_cookie());
    Ok(response)
}

// --- A forgotten password ---

async fn forgot_page(web: Web) -> Response {
    web.turnstile_page("Forgot the password?", views::forgot_form(web.site_key()))
}

async fn forgot(web: Web, form: Form) -> Answer {
    let email = normalize_email(form.field("email"));
    if let Some(problem) = email_problem(&email) {
        return Ok(web.problem(problem, StatusCode::BAD_REQUEST));
    }
    if !turnstile::passed(&web.state, form.field(turnstile::FIELD), web.ip).await {
        return Ok(web.problem(NOT_A_PERSON, StatusCode::BAD_REQUEST));
    }
    if let Err(error) = request_recovery(&web.state, &email, &web.meta).await {
        return web.auth_problem(&error);
    }
    Ok(web.answer("Check your email", views::reset_sent(&email)))
}

#[derive(Deserialize)]
struct TokenQuery {
    #[serde(default)]
    token: String,
}

async fn reset_page(web: Web, Query(query): Query<TokenQuery>) -> Answer {
    let client = web.state.db.client().await?;
    let user = peek_link(&client, LinkType::Recovery, &query.token).await.map_err(failed)?;
    let Some(user) = user else { return Ok(web.page(StatusCode::NOT_FOUND, "Link", views::link_problem())) };
    let min = web.state.cfg.auth.password_min_length;
    Ok(web.page(StatusCode::OK, "Choose a new password", views::reset_form(&query.token, &user.email, min)))
}

/// The link signs in, as GoTrue's recovery link; the new password is then
/// set on that session, and every other session ends.
async fn reset(web: Web, form: Form) -> Answer {
    let password = match new_password(&web, &form) {
        Ok(password) => password.to_owned(),
        Err(problem) => return Ok(web.problem(&problem, StatusCode::BAD_REQUEST)),
    };
    let tokens = match verify_link(&web.state, LinkType::Recovery, form.field("token"), &web.meta).await {
        Ok(tokens) => tokens,
        Err(error) => return web.auth_problem(&error),
    };
    let email = tokens.user.get("email").and_then(|email| email.as_str()).unwrap_or_default();
    let who = Authenticated {
        claims: user_claims(tokens.user_id, tokens.session_id, email),
        user_id: tokens.user_id,
        session_id: Some(tokens.session_id),
        role: "authenticated".into(),
    };
    let change = UserChange { password: Some(password), ..UserChange::default() };
    match update_user(&web.state, &who, change).await {
        // The same password as before still ends the other sessions.
        Ok(_) => {}
        Err(error) if error.code == "same_password" => {
            let client = web.state.db.client().await?;
            auth::end_sessions(&client, tokens.user_id, Some(tokens.session_id), "others").await?;
        }
        Err(error) => return web.auth_problem(&error),
    }
    signed_in(&web, &tokens, "", Some("Your password is changed. Everything else signed in is signed out.")).await
}

// --- Development ---

async fn dev_outbox(web: Web) -> Answer {
    if !web.state.cfg.dev_mode {
        return Ok(web.page(StatusCode::NOT_FOUND, "Not found", pages::not_found()));
    }
    let client = web.state.db.client().await?;
    let rows = client
        .query("SELECT sent_at, to_address, subject, text FROM velio_private.dev_outbox ORDER BY id DESC LIMIT 50", &[])
        .await?;
    let mails: Vec<OutboxRow> = rows
        .iter()
        .map(|row| OutboxRow { sent_at: row.get(0), to_address: row.get(1), subject: row.get(2), text: row.get(3) })
        .collect();
    Ok(web.page(StatusCode::OK, "Outbox", views::dev_outbox(&mails)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_browsers() {
        let firefox = "Mozilla/5.0 (X11; Linux x86_64; rv:140.0) Gecko/20100101 Firefox/140.0";
        assert_eq!(browser_name(Some(firefox)), "Firefox on Linux");
        let chrome = "Mozilla/5.0 (Linux; Android 15) AppleWebKit/537.36 Chrome/140.0 Mobile Safari/537.36";
        assert_eq!(browser_name(Some(chrome)), "Chrome on Android");
        assert_eq!(browser_name(None), "A browser");
    }

    #[test]
    fn goes_back_only_to_its_own_pages() {
        assert_eq!(safe_next("/tv-login?code=ABCD1234"), Some("/tv-login?code=ABCD1234"));
        assert_eq!(safe_next("/link?code=x"), Some("/link?code=x"));
        assert_eq!(safe_next("//example.invalid"), None);
        assert_eq!(safe_next("https://example.invalid/tv-login"), None);
        assert_eq!(safe_next("/tv-login\\x"), None);
        assert_eq!(safe_next(""), None);
    }
}
