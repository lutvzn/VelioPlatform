//! The page shell. Every page loads HTMX and site.js, which only marks the
//! tab shown and shows a list form's fields for its kind.

use maud::{Markup, PreEscaped, html};

/// No inline styles from HTMX, so the CSP can keep style-src 'self'.
const HTMX_CONFIG: &str = r#"{"includeIndicatorCSS":false}"#;

pub struct Page<'a> {
    pub title: &'a str,
    /// Who is signed in; none means signed out.
    pub account_id: Option<&'a str>,
    /// Loads Turnstile's script, for the sign-up, reset and report forms.
    pub turnstile: bool,
}

impl<'a> Page<'a> {
    pub fn new(title: &'a str, account_id: Option<&'a str>) -> Self {
        Self { title, account_id, turnstile: false }
    }
}

/// A line icon, drawn in the text's colour (app.css `.icon`): shown in the
/// tabs at the bottom of a phone's screen, and on the home page.
pub fn icon(paths: &'static str) -> Markup {
    html! { svg .icon viewBox="0 0 24 24" aria-hidden="true" focusable="false" { (PreEscaped(paths)) } }
}

pub const ICON_LISTS: &str =
    r#"<path d="M9 6h11M9 12h11M9 18h11"/><path d="M4.5 6h.01M4.5 12h.01M4.5 18h.01" stroke-width="2.6"/>"#;
pub const ICON_PROFILES: &str = r#"<circle cx="9" cy="8" r="3.5"/><path d="M2.5 20a6.5 6.5 0 0 1 13 0"/><path d="M16 4.6a3.5 3.5 0 0 1 0 6.8M18.5 14.5a6.5 6.5 0 0 1 3 5.5"/>"#;
pub const ICON_TV: &str = r#"<rect x="2.5" y="6.5" width="19" height="13" rx="2.5"/><path d="m8 2.5 4 4 4-4"/>"#;
pub const ICON_ACCOUNT: &str = r#"<circle cx="12" cy="8" r="4"/><path d="M4 21a8 8 0 0 1 16 0"/>"#;
pub const ICON_LOCK: &str =
    r#"<rect x="4" y="10.5" width="16" height="10.5" rx="2.5"/><path d="M8 10.5V7a4 4 0 0 1 8 0v3.5"/>"#;
pub const ICON_SERVER: &str = r#"<rect x="3" y="3.5" width="18" height="7" rx="2"/><rect x="3" y="13.5" width="18" height="7" rx="2"/><path d="M7 7h.01M7 17h.01" stroke-width="2.6"/>"#;
pub const ICON_PHONE: &str = r#"<rect x="6" y="2.5" width="12" height="19" rx="2.5"/><path d="M11 18h2"/>"#;

pub fn nav(account_id: Option<&str>, oob: bool) -> Markup {
    html! {
        nav #nav hx-swap-oob=[oob.then_some("true")] {
            @if account_id.is_some() {
                a href="/lists" { (icon(ICON_LISTS)) "Lists" }
                a href="/profiles" { (icon(ICON_PROFILES)) "Profiles" }
                a href="/devices" { (icon(ICON_TV)) "TVs" }
                a href="/account" { (icon(ICON_ACCOUNT)) "Account" }
                form method="post" action="/signout" hx-post="/signout" {
                    button type="submit" class="link" { "Sign out" }
                }
            } @else {
                a href="/signin" { "Sign in" }
                a href="/signup" hx-boost="false" { "Make an account" }
            }
        }
    }
}

/// A page's `<title>`: in the layout's head, and in the head of a fragment
/// that is another page (`web::page_fragment`).
pub fn title_tag(title: &str) -> Markup {
    html! { title { (title) " · Velio" } }
}

// Links and forms are boosted, so moving between pages swaps only #main.
// Pages with Turnstile are opened without boosting, as its script is in the head.
pub fn layout(page: Page<'_>, body: Markup) -> Markup {
    html! {
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover";
                meta name="htmx-config" content=(HTMX_CONFIG);
                meta name="color-scheme" content="light dark";
                (title_tag(page.title))
                link rel="stylesheet" href="/static/app.css";
                script src="/static/htmx.min.js" defer {}
                script src="/static/site.js" defer {}
                @if page.turnstile {
                    script src="https://challenges.cloudflare.com/turnstile/v0/api.js" async defer {}
                }
            }
            body "hx-boost:inherited"="true" {
                header {
                    a href="/" class="brand" { "Velio" }
                    (nav(page.account_id, false))
                }
                main #main { (body) }
                footer {
                    a href="/terms" { "Terms" }
                    a href="/privacy" { "Privacy" }
                    a href="/security" { "Security" }
                    a href="/abuse" { "Report abuse" }
                }
            }
        }
    }
}
