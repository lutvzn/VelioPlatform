//! Making, confirming, signing in to and resetting an account, and the
//! account page. Plain forms: the password goes to the server, as it goes
//! to Supabase Auth from Nuvio's apps, and is kept only as a bcrypt hash.

use maud::{Markup, html};

use crate::mail::links_in;
use crate::time::iso;

pub(crate) fn problem() -> Markup {
    html! { p #form-problem .problem role="alert" {} }
}

pub(crate) fn turnstile(site_key: Option<&str>) -> Markup {
    html! {
        @if let Some(site_key) = site_key {
            div .cf-turnstile data-sitekey=(site_key) data-size="flexible" {}
        }
    }
}

/// A form's problem, as a page, for a form sent without HTMX.
pub fn problem_page(message: &str) -> Markup {
    html! {
        section .auth {
            h1 { "That didn't work" }
            p .problem { (message) }
        }
    }
}

fn new_password(min: usize, label: &str) -> Markup {
    html! {
        label {
            (label)
            input type="password" name="password" autocomplete="new-password" required minlength=(min) maxlength="72";
        }
        label {
            (label) " again"
            input type="password" name="password_again" autocomplete="new-password" required;
        }
    }
}

pub fn signup_form(site_key: Option<&str>, min: usize) -> Markup {
    html! {
        section .auth {
            h1 { "Make an account" }
            form method="post" action="/signup" hx-post="/signup" hx-target="#main" {
                label {
                    "Email"
                    input type="email" name="email" autocomplete="username" required maxlength="254";
                }
                (new_password(min, "Password"))
                p .note { "The same account signs in to Velio's TV app and to Nuvio's apps set to use this server." }
                label .check {
                    input type="checkbox" name="terms" value="yes" required;
                    span {
                        "I accept the " a href="/terms" { "terms" }
                        ", and will add only sources I have the right to use."
                    }
                }
                (turnstile(site_key))
                (problem())
                button type="submit" { "Make the account" }
            }
            p { "Already have one? " a href="/signin" { "Sign in" } }
        }
    }
}

pub fn check_email(email: &str) -> Markup {
    html! {
        section .auth {
            h1 { "Check your email" }
            p { "We sent a link to " strong { (email) } ". Open it to finish. It works for 24 hours." }
        }
    }
}

pub fn signin_form(email: Option<&str>, note: Option<&str>, next: Option<&str>) -> Markup {
    html! {
        section .auth {
            h1 { "Sign in" }
            @if let Some(note) = note { p .note { (note) } }
            form method="post" action="/signin" hx-post="/signin" hx-target="#main" {
                @if let Some(next) = next { input type="hidden" name="next" value=(next); }
                label {
                    "Email"
                    input type="email" name="email" autocomplete="username" required maxlength="254" value=[email];
                }
                label {
                    "Password"
                    input type="password" name="password" autocomplete="current-password" required;
                }
                (problem())
                button type="submit" { "Sign in" }
            }
            p {
                a href="/forgot" hx-boost="false" { "Forgot the password?" }
                " · "
                a href="/signup" hx-boost="false" { "Make an account" }
            }
        }
    }
}

/// What an email link does, for its page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkPage {
    Signup,
    EmailChange,
}

/// Links are used with a button, so mail scanners that open links don't use them up.
pub fn verify_page(kind: LinkPage, token: &str) -> Markup {
    let (title, button, name) = match kind {
        LinkPage::Signup => ("Confirm your email", "Confirm", "signup"),
        LinkPage::EmailChange => ("Confirm your new email", "Use this email", "email_change"),
    };
    html! {
        section .auth {
            h1 { (title) }
            form method="post" action="/verify" hx-post="/verify" hx-target="#main" {
                input type="hidden" name="type" value=(name);
                input type="hidden" name="token" value=(token);
                (problem())
                button type="submit" { (button) }
            }
        }
    }
}

pub fn link_problem() -> Markup {
    html! {
        section .auth {
            h1 { "This link doesn't work" }
            p {
                "It was already used, or it's too old. " a href="/signin" { "Sign in" } ", or ask for a new one: "
                a href="/signup" hx-boost="false" { "make the account again" }
                " or "
                a href="/forgot" hx-boost="false" { "reset the password" }
                "."
            }
        }
    }
}

pub fn forgot_form(site_key: Option<&str>) -> Markup {
    html! {
        section .auth {
            h1 { "Forgot the password?" }
            p { "We'll send a link to choose a new one. Your lists, profiles and add-ons stay as they are." }
            form method="post" action="/forgot" hx-post="/forgot" hx-target="#main" {
                label {
                    "Email"
                    input type="email" name="email" autocomplete="username" required maxlength="254";
                }
                (turnstile(site_key))
                (problem())
                button type="submit" { "Send the link" }
            }
        }
    }
}

pub fn reset_sent(email: &str) -> Markup {
    html! {
        section .auth {
            h1 { "Check your email" }
            p { "If " strong { (email) } " has an account, we sent it a link. It works for 1 hour." }
        }
    }
}

/// The reset link's page.
pub fn reset_form(token: &str, email: &str, min: usize) -> Markup {
    html! {
        section .auth {
            h1 { "Choose a new password" }
            p .note { "Your lists, profiles and add-ons stay. Everything signed in is signed out." }
            form method="post" action="/reset" hx-post="/reset" hx-target="#main" {
                input type="hidden" name="token" value=(token);
                input type="email" hidden value=(email) autocomplete="username";
                (new_password(min, "New password"))
                (problem())
                button type="submit" { "Set the new password" }
            }
        }
    }
}

// --- The account page ---

pub struct AccountView {
    pub email: String,
    /// A new address waiting for its link to be opened.
    pub new_email: Option<String>,
    /// The user's other sessions: TVs, apps and browsers.
    pub devices: usize,
    pub min_password: usize,
    /// What was just done, such as "Your password is changed."
    pub notice: Option<String>,
}

pub fn account_view(view: &AccountView) -> Markup {
    html! {
        section {
            h1 { "Your account" }
            @if let Some(notice) = &view.notice { p .done role="status" { (notice) } }
            p { "Signed in as " strong { (view.email) } "." }
            p { a href="/lists" { "Your lists" } " · " a href="/profiles" { "Your profiles" } }
        }
        section {
            h2 { "TVs and apps" }
            @match view.devices {
                0 => p { "Nothing else is signed in." },
                1 => p { "1 other TV, app or browser is signed in." },
                count => p { (count) " other TVs, apps and browsers are signed in." },
            }
            p { a href="/devices" .button { "See them" } }
        }
        (email_section(&view.email, view.new_email.as_deref()))
        (password_section(&view.email, view.min_password))
        (delete_section(&view.email))
    }
}

fn current_password(email: &str) -> Markup {
    html! {
        input type="email" hidden value=(email) autocomplete="username";
        label {
            "Current password"
            input type="password" name="current_password" autocomplete="current-password" required;
        }
    }
}

fn email_section(email: &str, new_email: Option<&str>) -> Markup {
    html! {
        section {
            h2 { "Email" }
            @if let Some(new_email) = new_email {
                p .note { "Waiting for " strong { (new_email) } " to be confirmed, with the link we sent it." }
            }
            details {
                summary { "Change the email" }
                form method="post" action="/account/email" hx-post="/account/email" hx-target="#main" {
                    label {
                        "New email"
                        input type="email" name="email" autocomplete="email" required maxlength="254";
                    }
                    (current_password(email))
                    p #email-problem .problem role="alert" {}
                    button type="submit" { "Send the link" }
                }
            }
        }
    }
}

fn password_section(email: &str, min: usize) -> Markup {
    html! {
        section {
            h2 { "Password" }
            details {
                summary { "Change the password" }
                p .note { "Everything else signed in is signed out." }
                form method="post" action="/account/password" hx-post="/account/password" hx-target="#main" {
                    (current_password(email))
                    (new_password(min, "New password"))
                    p #password-problem .problem role="alert" {}
                    button type="submit" { "Change the password" }
                }
            }
        }
    }
}

fn delete_section(email: &str) -> Markup {
    html! {
        section {
            h2 { "Delete your account" }
            details {
                summary { "Delete your account" }
                p .warning {
                    "This deletes your profiles, add-ons, lists, history and everything else of yours from this \
                     server, and signs out every TV and app. It can't be undone."
                }
                form method="post" action="/account/delete" hx-post="/account/delete" hx-target="#main" {
                    (current_password(email))
                    p #delete-problem .problem role="alert" {}
                    button type="submit" .danger { "Delete the account" }
                }
            }
        }
    }
}

// --- Development ---

pub struct OutboxRow {
    pub sent_at: i64,
    pub to_address: String,
    pub subject: String,
    pub text: String,
}

pub fn dev_outbox(mails: &[OutboxRow]) -> Markup {
    html! {
        section {
            h1 { "Outbox (DEV_MODE)" }
            @if mails.is_empty() { p { "No email yet." } }
            @for mail in mails {
                article .mail {
                    h2 { (mail.subject) }
                    p .note { "To " (mail.to_address) ", " (iso(mail.sent_at)) }
                    pre { (mail.text) }
                    @for link in links_in(&mail.text) {
                        p { a href=(link) { (link) } }
                    }
                }
            }
        }
    }
}
