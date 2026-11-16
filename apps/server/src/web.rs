//! How web handlers answer (the Worker's `fullPage`, `answer` and `problem`
//! in `routes/web.tsx`). Forms are sent by HTMX, and answers are fragments
//! for #main. A form problem answers 4xx with HX-Retarget to the form's
//! #form-problem, so what was typed stays put.

use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr};

use axum::body::Bytes;
use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use maud::{Markup, html};

use crate::AppState;
use crate::accounts::Fields;
use crate::api::auth::{AuthError, Meta};
use crate::db::{self, Ctx};
use crate::error::AppError;
use crate::http::ClientIp;
use crate::limits::Limit;
use crate::session::{CurrentSession, Session};
use crate::views::layout::{Page, title_tag};
use crate::views::{accounts, full_page};

pub const TOO_MANY: &str = "Too many tries. Wait a minute, then try again.";
pub const LINK_GONE: &str = "This link was already used, or it's too old. Ask for a new one.";
pub const CLOSED: &str = "This account is closed.";

/// What a web handler needs to know about the request.
pub struct Web {
    pub state: AppState,
    pub session: Option<Session>,
    /// Sent by HTMX, so the answer is a fragment.
    pub htmx: bool,
    pub ip: IpAddr,
    /// The IP and browser, which Auth keeps with each session.
    pub meta: Meta,
}

impl FromRequestParts<AppState> for Web {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Infallible> {
        Ok(Self {
            state: state.clone(),
            session: parts.extensions.get::<CurrentSession>().and_then(|current| current.0.clone()),
            htmx: parts.headers.get("HX-Request").is_some_and(|value| value == "true"),
            ip: parts.extensions.get::<ClientIp>().map_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED), |client| client.ip),
            meta: Meta::of(parts.extensions.get::<ClientIp>(), &parts.headers),
        })
    }
}

impl Web {
    pub fn account_id(&self) -> Option<&str> {
        self.session.as_ref().map(|session| session.account_id.as_str())
    }

    /// Turnstile's site key, when it's on.
    pub fn site_key(&self) -> Option<&str> {
        self.state.cfg.turnstile.as_ref().map(|turnstile| turnstile.site_key.as_str())
    }

    pub fn within(&self, limit: Limit, key: &str) -> bool {
        self.state.limits.within(limit, key)
    }

    /// Runs `work` in a transaction as the signed-in user, so Nuvio's
    /// policies and functions decide, as they do for Nuvio's apps. A refusal
    /// of the database's with words for a person (Nuvio's functions raise
    /// them) comes back as `Err` of those words; the caller shows it.
    pub async fn as_user<T>(
        &self,
        work: impl AsyncFnOnce(&deadpool_postgres::Transaction<'_>) -> Result<T, tokio_postgres::Error>,
    ) -> Result<Result<T, String>, AppError> {
        let Some(session) = &self.session else { return Ok(Err("Sign in first.".into())) };
        self.as_claims(session.claims(), work).await
    }

    /// [`Web::as_user`] for a user with these claims, such as one signing in.
    pub async fn as_claims<T>(
        &self,
        claims: serde_json::Value,
        work: impl AsyncFnOnce(&deadpool_postgres::Transaction<'_>) -> Result<T, tokio_postgres::Error>,
    ) -> Result<Result<T, String>, AppError> {
        let mut client = self.state.db.client().await?;
        let tx = db::begin(&mut client, &Ctx::user(claims)).await?;
        match work(&tx).await {
            Ok(value) => {
                tx.commit().await?;
                Ok(Ok(value))
            }
            Err(error) => match refusal(&error) {
                Some(words) => {
                    tracing::debug!(error = ?error.as_db_error(), "The database refused a change");
                    Ok(Err(words))
                }
                None => Err(error.into()),
            },
        }
    }

    /// Auth's refusal, for a form: its words, or a 500 for a failure.
    pub fn auth_problem(&self, error: &AuthError) -> Result<Response, AppError> {
        let words = match auth_words(&self.state, error) {
            Some(words) => words,
            None => return Err(anyhow::anyhow!("Auth: {} {}", error.code, error.message).into()),
        };
        let status = if error.status.is_server_error() { StatusCode::BAD_REQUEST } else { error.status };
        Ok(self.problem(&words, status))
    }

    /// A whole page in the layout.
    pub fn page(&self, status: StatusCode, title: &str, body: Markup) -> Response {
        full_page(status, Page::new(title, self.account_id()), body)
    }

    /// A whole page with Turnstile's script, when it's on.
    pub fn turnstile_page(&self, title: &str, body: Markup) -> Response {
        let page = Page { turnstile: self.site_key().is_some(), ..Page::new(title, self.account_id()) };
        full_page(StatusCode::OK, page, body)
    }

    /// A fragment for #main, or the full page when the form was sent without HTMX.
    pub fn answer(&self, title: &str, body: Markup) -> Response {
        if self.htmx { page_fragment(title, body) } else { self.page(StatusCode::OK, title, body) }
    }

    /// A form's problem, shown in its #form-problem, leaving what was typed.
    pub fn problem(&self, message: &str, status: StatusCode) -> Response {
        self.problem_at(message, status, "#form-problem")
    }

    /// A problem shown in another element than the form's #form-problem.
    pub fn problem_at(&self, message: &str, status: StatusCode, target: &'static str) -> Response {
        if !self.htmx {
            return self.page(status, "Problem", accounts::problem_page(message));
        }
        let mut response = (status, fragment(html! { (message) })).into_response();
        let headers = response.headers_mut();
        headers.insert("HX-Retarget", HeaderValue::from_static(target));
        headers.insert("HX-Reswap", HeaderValue::from_static("innerHTML"));
        response
    }
}

/// The words of a database refusal meant for a person: what Nuvio's
/// functions raise (P0001, 22023, …), a broken check, or a policy's no.
pub fn refusal(error: &tokio_postgres::Error) -> Option<String> {
    let db = error.as_db_error()?;
    let code = db.code().code();
    match code {
        "42501" => Some("That isn't yours to change.".into()),
        "23505" => Some("That's there already.".into()),
        "23514" | "23502" | "22001" => Some("Something in the form isn't right. Check it and try again.".into()),
        _ if code.starts_with("P0") || code.starts_with("22") => Some(sentence(db.message())),
        _ => None,
    }
}

/// A message as a sentence: a capital first, and a full stop.
fn sentence(message: &str) -> String {
    let message = message.trim();
    let mut chars = message.chars();
    let mut text: String = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => return "That didn't work.".into(),
    };
    if !text.ends_with(['.', '!', '?']) {
        text.push('.');
    }
    text
}

/// Auth's error codes in words for the website's forms; none for a failure.
pub fn auth_words(state: &AppState, error: &AuthError) -> Option<String> {
    let min = state.cfg.auth.password_min_length;
    Some(match error.code {
        "invalid_credentials" => "That email and password don't match an account.".into(),
        "email_not_confirmed" => "Confirm your email first, with the link we sent. For a new link, make the account \
                                  again."
            .into(),
        "user_banned" => CLOSED.into(),
        "over_request_rate_limit" | "over_email_send_rate_limit" => TOO_MANY.into(),
        "weak_password" => format!("Use at least {min} characters for the password."),
        "signup_disabled" => "That email can't make an account here.".into(),
        "user_already_exists" | "email_exists" => "That email already has an account.".into(),
        "same_password" => "That's the password you have now. Choose another.".into(),
        "otp_expired" => LINK_GONE.into(),
        "validation_failed" if error.message.contains("72") => "Use at most 72 characters for the password.".into(),
        "validation_failed" if error.message.contains("email") => "That doesn't look like an email address.".into(),
        "validation_failed" => sentence(&error.message),
        _ if error.status.is_client_error() => sentence(&error.message),
        _ => return None,
    })
}

pub fn fragment(body: Markup) -> Response {
    Html(body.into_string()).into_response()
}

/// A fragment for #main that is another page, with its title. HTMX 4 takes
/// the `<head>` out before swapping and sets the tab's title from it, so the
/// title follows HX-Push-Url. (The Worker's fragments had none, so the tab
/// kept the page's first title.)
pub fn page_fragment(title: &str, body: Markup) -> Response {
    fragment(page_markup(title, body))
}

fn page_markup(title: &str, body: Markup) -> Markup {
    html! { head { (title_tag(title)) } (body) }
}

/// A 302, as Hono's `c.redirect`. (axum's `Redirect::to` is a 303.)
pub fn found(location: &'static str) -> Response {
    (StatusCode::FOUND, [(header::LOCATION, location)]).into_response()
}

/// A 302 to a place made at run time, such as sign-in with where to go after.
pub fn found_at(location: &str) -> Response {
    match HeaderValue::try_from(location) {
        Ok(value) => (StatusCode::FOUND, [(header::LOCATION, value)]).into_response(),
        Err(_) => found("/"),
    }
}

/// Sign-in, coming back to `path_and_query` after.
pub fn sign_in_first(path_and_query: &str) -> Response {
    let next: String = form_urlencoded::byte_serialize(path_and_query.as_bytes()).collect();
    found_at(&format!("/signin?next={next}"))
}

pub fn set_header(response: &mut Response, name: &'static str, value: &'static str) {
    response.headers_mut().insert(name, HeaderValue::from_static(value));
}

/// A form's text fields, as Hono's `parseBody` reads them: the last of a
/// repeated field, and nothing from a body that isn't a form.
pub struct Form(Vec<(String, String)>);

impl<S: Send + Sync> FromRequest<S> for Form {
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Response> {
        let is_form = req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.to_ascii_lowercase().starts_with("application/x-www-form-urlencoded"));
        let body = Bytes::from_request(req, state).await.map_err(IntoResponse::into_response)?;
        if !is_form {
            return Ok(Self(Vec::new()));
        }
        Ok(Self::parse(&body))
    }
}

impl Form {
    /// A form body, as the extractor reads it.
    pub fn parse(body: &[u8]) -> Self {
        Self(form_urlencoded::parse(body).into_owned().collect())
    }

    /// Every value of a repeated field, such as a group of checkboxes.
    pub fn all(&self, name: &str) -> Vec<&str> {
        self.0.iter().filter(|(key, _)| key == name).map(|(_, value)| value.as_str()).collect()
    }
}

impl Fields for Form {
    fn field(&self, name: &str) -> &str {
        self.0.iter().rev().find(|(key, _)| key == name).map_or("", |(_, value)| value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn makes_sentences() {
        assert_eq!(sentence("invalid or expired TV login code"), "Invalid or expired TV login code.");
        assert_eq!(sentence("Done!"), "Done!");
        assert_eq!(sentence(" "), "That didn't work.");
    }

    #[test]
    fn a_page_fragment_carries_its_title_in_a_head() {
        let markup = page_markup("Your <account>", html! { section { h1 { "Your account" } } });
        assert_eq!(
            markup.into_string(),
            "<head><title>Your &lt;account&gt; · Velio</title></head><section><h1>Your account</h1></section>"
        );
    }
}
