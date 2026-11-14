//! The page shell. Every page loads HTMX and vault.js; vault.js does
//! everything secret in the browser (PLAN.md, "HTMX and end-to-end encryption").

use maud::{Markup, html};

/// No inline styles from HTMX, so the CSP can keep style-src 'self'.
const HTMX_CONFIG: &str = r#"{"includeIndicatorCSS":false}"#;

pub struct Page<'a> {
    pub title: &'a str,
    /// Whose keys this tab may hold; none means signed out.
    pub account_id: Option<&'a str>,
    /// Loads Turnstile's script, for the sign-up, reset and report forms.
    pub turnstile: bool,
}

impl<'a> Page<'a> {
    pub fn new(title: &'a str, account_id: Option<&'a str>) -> Self {
        Self { title, account_id, turnstile: false }
    }
}

/// data-account-id tells vault.js whose keys this tab may hold; none means signed out.
pub fn nav(account_id: Option<&str>, oob: bool) -> Markup {
    html! {
        nav #nav hx-swap-oob=[oob.then_some("true")] data-account-id=[account_id] {
            @if account_id.is_some() {
                a href="/lists" { "Lists" }
                a href="/profiles" { "Profiles" }
                a href="/devices" { "TVs" }
                a href="/account" { "Account" }
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

// Links and forms are boosted, so moving between pages keeps vault.js, and the
// keys it holds, in memory. Pages with Turnstile are opened without boosting,
// as its script is in the head.
pub fn layout(page: Page<'_>, body: Markup) -> Markup {
    html! {
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="htmx-config" content=(HTMX_CONFIG);
                meta name="color-scheme" content="light dark";
                (title_tag(page.title))
                link rel="stylesheet" href="/static/app.css";
                script src="/static/htmx.min.js" defer {}
                script src="/static/vault.js" defer {}
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
