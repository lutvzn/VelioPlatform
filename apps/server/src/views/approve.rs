//! Signing in a TV or app from this website: the page a TV's QR code opens
//! (/tv-login, Nuvio's TV login), and the page for a code typed in (/link,
//! Nuvio's device login).

use maud::{Markup, html};

use super::accounts::problem;

/// What the TV's code is waiting for.
pub struct TvLogin<'a> {
    pub code: &'a str,
    pub device_name: Option<&'a str>,
    /// The TVs signed in, and how many the account may have.
    pub tvs: usize,
    pub tv_limit: usize,
}

pub fn tv_login(view: &TvLogin<'_>) -> Markup {
    let full = view.tvs >= view.tv_limit;
    html! {
        section .auth {
            h1 { "Sign in this TV?" }
            p {
                @if let Some(name) = view.device_name { strong { (name) } " is" } @else { "A TV is" }
                " asking to sign in to your account. Only go on if it's yours, and in front of you."
            }
            @if full {
                p .warning {
                    "Your account has " (view.tvs) " TVs signed in, as many as it may have. "
                    a href="/devices" { "Sign one out" } " first, or this TV won't sign in."
                }
            }
            form method="post" action="/tv-login" hx-post="/tv-login" hx-target="#main" {
                input type="hidden" name="code" value=(view.code);
                (problem())
                button type="submit" { "Sign in the TV" }
            }
        }
    }
}

pub fn code_gone() -> Markup {
    html! {
        section .auth {
            h1 { "This code doesn't work" }
            p { "It was already used, or it's more than a few minutes old. Show a new one on the TV and scan it again." }
        }
    }
}

/// Done: the TV or app signs itself in on its next check, a few seconds on.
pub fn approved(what: &str) -> Markup {
    html! {
        section .auth {
            h1 { "Done" }
            p .done role="status" { (what) " signs in within a few seconds. You can close this page." }
            p { a href="/devices" { "See what's signed in" } }
        }
    }
}

pub fn link_form(code: Option<&str>) -> Markup {
    html! {
        section .auth {
            h1 { "Sign in a TV or app" }
            p { "Type the code it shows." }
            form method="get" action="/link" hx-get="/link" hx-target="#main" hx-push-url="true" {
                label {
                    "Code"
                    input type="text" name="code" value=[code] required maxlength="16" autocomplete="off"
                        autocapitalize="characters" spellcheck="false" placeholder="ABCD-EFGH";
                }
                (problem())
                button type="submit" { "Next" }
            }
        }
    }
}

pub struct DeviceLogin<'a> {
    pub code: &'a str,
    pub device_name: Option<&'a str>,
    pub device_type: Option<&'a str>,
}

pub fn link_confirm(view: &DeviceLogin<'_>) -> Markup {
    html! {
        section .auth {
            h1 { "Sign it in?" }
            p {
                @match (view.device_name, view.device_type) {
                    (Some(name), Some(kind)) => { strong { (name) } " (" (kind) ") is" }
                    (Some(name), None) => { strong { (name) } " is" }
                    (None, Some(kind)) => { "A " (kind) " is" }
                    (None, None) => { "A device is" }
                }
                " asking to sign in to your account with the code " strong { (view.code) } ". Only go on if it's yours."
            }
            form method="post" action="/link" hx-post="/link" hx-target="#main" {
                input type="hidden" name="code" value=(view.code);
                (problem())
                button type="submit" { "Sign it in" }
            }
            p { a href="/link" { "That's not the code" } }
        }
    }
}
