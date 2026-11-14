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
use crate::http::ClientIp;
use crate::limits::Limit;
use crate::session::{CurrentSession, Session};
use crate::views::layout::{Page, title_tag};
use crate::views::{accounts, full_page};

pub const TOO_MANY: &str = "Too many tries. Wait a minute, then try again.";

/// What a web handler needs to know about the request.
pub struct Web {
    pub state: AppState,
    pub session: Option<Session>,
    /// Sent by HTMX, so the answer is a fragment.
    pub htmx: bool,
    pub ip: IpAddr,
}

impl FromRequestParts<AppState> for Web {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Infallible> {
        Ok(Self {
            state: state.clone(),
            session: parts.extensions.get::<CurrentSession>().and_then(|current| current.0.clone()),
            htmx: parts.headers.get("HX-Request").is_some_and(|value| value == "true"),
            ip: parts.extensions.get::<ClientIp>().map_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED), |client| client.ip),
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
        Ok(Self(form_urlencoded::parse(&body).into_owned().collect()))
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
    fn a_page_fragment_carries_its_title_in_a_head() {
        let markup = page_markup("Your <account>", html! { section { h1 { "Your account" } } });
        assert_eq!(
            markup.into_string(),
            "<head><title>Your &lt;account&gt; · Velio</title></head><section><h1>Your account</h1></section>"
        );
    }
}
