//! Making, confirming, signing in to and resetting an account, and the
//! account page (the Worker's `views/pages.tsx`).
//!
//! Forms marked data-velio="…" have hx-confirm, so vault.js gets the request
//! before it is sent and puts keys in the body in place of the password.
//! Password inputs have no name, so HTMX never sends them.

use maud::{Markup, html};

use crate::accounts::{MIN_PASSWORD_LENGTH, Owner};
use crate::mail::links_in;
use crate::time::{day, iso};
use crate::vault::DeviceRow;
use crate::views::vault::{owner_bundle, remember_choice, unlock_form};

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
        section {
            h1 { "That didn't work" }
            p .problem { (message) }
        }
    }
}

pub fn signup_form(site_key: Option<&str>) -> Markup {
    html! {
        section {
            h1 { "Make an account" }
            form hx-post="/signup" hx-target="#main" hx-confirm="velio" data-velio="signup" {
                label {
                    "Email"
                    input type="email" name="email" autocomplete="username" required maxlength="254";
                }
                label {
                    "Password"
                    input type="password" data-password autocomplete="new-password" required
                        minlength=(MIN_PASSWORD_LENGTH);
                }
                label {
                    "Password again"
                    input type="password" data-password-again autocomplete="new-password" required;
                }
                p .note {
                    "Your password is also the key to your lists. Velio never sees it and can't recover it: if you \
                     forget it, resetting the account deletes the lists, unless you made a recovery key in your account."
                }
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
        section {
            h1 { "Check your email" }
            p { "We sent a link to " strong { (email) } ". Open it to finish. It works for 24 hours." }
        }
    }
}

pub fn signin_form(email: Option<&str>, note: Option<&str>, next: Option<&str>) -> Markup {
    html! {
        section {
            h1 { "Sign in" }
            @if let Some(note) = note { p .note { (note) } }
            form hx-post="/signin" hx-target="#main" hx-confirm="velio" data-velio="signin" {
                @if let Some(next) = next { input type="hidden" name="next" value=(next); }
                label {
                    "Email"
                    input type="email" name="email" autocomplete="username" required maxlength="254" value=[email];
                }
                label {
                    "Password"
                    input type="password" data-password autocomplete="current-password" required;
                }
                (remember_choice())
                (problem())
                button type="submit" { "Sign in" }
            }
            p {
                a href="/reset" hx-boost="false" { "Forgot the password?" }
                " · "
                a href="/signup" hx-boost="false" { "Make an account" }
            }
        }
    }
}

/// Links are confirmed with a button, so mail scanners that open links don't confirm them.
pub fn confirm_page(token: &str) -> Markup {
    let action = format!("/confirm/{token}");
    html! {
        section {
            h1 { "Confirm your email" }
            form method="post" action=(action) hx-post=(action) hx-target="#main" {
                button type="submit" { "Confirm" }
            }
        }
    }
}

pub fn link_problem() -> Markup {
    html! {
        section {
            h1 { "This link doesn't work" }
            p {
                "It was already used, or it's too old. " a href="/signin" { "Sign in" } ", or ask for a new one: "
                a href="/signup" hx-boost="false" { "make the account again" }
                " or "
                a href="/reset" hx-boost="false" { "reset it" }
                "."
            }
        }
    }
}

pub fn reset_request_form(site_key: Option<&str>) -> Markup {
    html! {
        section {
            h1 { "Reset your account" }
            p {
                "Velio can't read your lists, so it can't give them back without your password. A reset keeps your \
                 email and deletes your lists, profiles and signed-in TVs. You choose a new password and add them again."
            }
            form hx-post="/reset" hx-target="#main" {
                label {
                    "Email"
                    input type="email" name="email" autocomplete="username" required maxlength="254";
                }
                (turnstile(site_key))
                (problem())
                button type="submit" { "Send the reset link" }
            }
        }
    }
}

pub fn reset_sent(email: &str) -> Markup {
    html! {
        section {
            h1 { "Check your email" }
            p { "If " strong { (email) } " has an account, we sent it a reset link. It works for 1 hour." }
        }
    }
}

fn new_password() -> Markup {
    html! {
        label {
            "New password"
            input type="password" data-password autocomplete="new-password" required minlength=(MIN_PASSWORD_LENGTH);
        }
        label {
            "New password again"
            input type="password" data-password-again autocomplete="new-password" required;
        }
    }
}

fn current_password(email: &str, label: &str) -> Markup {
    html! {
        input type="email" hidden value=(email) autocomplete="username";
        label {
            (label)
            input type="password" data-password-current autocomplete="current-password" required;
        }
    }
}

/// The reset link's page: with the recovery key, a new password that keeps everything; without it, a reset.
pub fn reset_form(token: &str, recovery: bool) -> Markup {
    html! {
        section {
            h1 { "Choose a new password" }
            @if recovery {
                h2 { "With your recovery key" }
                p { "Keeps your lists, profiles and TVs." }
                form hx-post=(format!("/reset/{token}/recover")) hx-target="#main" hx-confirm="velio"
                    data-velio="recover" data-token=(token) {
                    label {
                        "Recovery key"
                        input type="text" data-recovery-key autocomplete="off" autocapitalize="characters"
                            spellcheck="false" placeholder="K7QM-2XHF-…" required;
                    }
                    (new_password())
                    p #recover-problem .problem role="alert" {}
                    button type="submit" { "Set the new password" }
                }
                h2 { "Without it" }
            }
            p .warning { "This deletes your lists, profiles and signed-in TVs. It can't be undone." }
            form hx-post=(format!("/reset/{token}")) hx-target="#main" hx-confirm="velio" data-velio="reset" {
                (new_password())
                (problem())
                button type="submit" .danger { "Reset the account" }
            }
        }
    }
}

// --- The account page ---

/// How many TVs: their names are sealed, and only the TVs page opens them.
fn devices(devices: &[DeviceRow]) -> Markup {
    let signed_out = devices.iter().filter(|device| device.signed_out_at.is_some()).count();
    html! {
        section {
            h2 { "TVs" }
            @match devices.len() {
                0 => p { "No TVs yet." },
                1 => p { "You have 1 TV." },
                count => p { "You have " (count) " TVs." },
            }
            @if signed_out > 0 {
                p .note { "Signed out: " (signed_out) ". Remove it on the TVs page to take it off the account." }
            }
            p {
                a href="/pair" .button { "Sign in a TV" }
                " "
                @if !devices.is_empty() { a href="/devices" { "Change or remove TVs" } }
            }
        }
    }
}

pub struct AccountView<'a> {
    pub email: &'a str,
    /// Straight after signing in: the owner bundle, which vault.js opens with
    /// the unlock key it kept from the password.
    pub owner: Option<&'a Owner>,
    pub devices: &'a [DeviceRow],
    /// When the recovery key was made; None when it's off.
    pub recovery_made_at: Option<i64>,
    /// What was just done, such as "Your password is changed."
    pub notice: Option<&'a str>,
    /// Just after a recovery key was made: the place vault.js shows it, once.
    pub show_recovery_key: bool,
}

pub fn account_view(view: &AccountView<'_>) -> Markup {
    let email = view.email;
    html! {
        section {
            h1 { "Your account" }
            @if let Some(notice) = view.notice { p .done role="status" { (notice) } }
            p { "Signed in as " strong { (email) } "." }
            @if let Some(owner) = view.owner { (owner_bundle(owner)) }
            p { a href="/lists" { "Your lists" } " · " a href="/profiles" { "Your profiles" } }
        }
        @if view.show_recovery_key { (recovery_key_shown()) }
        (devices(view.devices))
        (unlock_form(email))
        section data-needs-keys hidden {
            p data-vault-status { "Your keys are open in this browser." }
            button type="button" .link data-forget { "Lock this browser" }
        }
        (password_section(email))
        (recovery_section(email, view.recovery_made_at))
        (delete_section(email))
    }
}

fn recovery_key_shown() -> Markup {
    html! {
        section .recovery-shown data-recovery-shown {
            h2 { "Your recovery key" }
            p { code .recovery-key data-recovery-code {} }
            p {
                "Write it down or print this page now: it's shown only once. Keep it away from this device. With it \
                 and your email, a forgotten password keeps your lists; anyone with both can open them."
            }
            p { button type="button" data-print { "Print" } }
        }
    }
}

fn password_section(email: &str) -> Markup {
    html! {
        section {
            h2 { "Password" }
            p data-locked .note { "Unlock your lists above to change the password." }
            details data-needs-keys hidden {
                summary { "Change the password" }
                p .note { "Your lists and TVs stay as they are. Other browsers are signed out." }
                form hx-post="/password" hx-target="#main" hx-confirm="velio" data-velio="password" data-email=(email) {
                    (current_password(email, "Current password"))
                    (new_password())
                    p #password-problem .problem role="alert" {}
                    button type="submit" { "Change the password" }
                }
            }
        }
    }
}

fn recovery_section(email: &str, made_at: Option<i64>) -> Markup {
    html! {
        section {
            h2 { "Recovery key" }
            @match made_at {
                None => p {
                    "Off. Without one, a forgotten password means resetting the account, which deletes your lists and \
                     profiles."
                },
                Some(at) => p {
                    "On, made " (day(at)) ". With it and the reset email, a forgotten password keeps your lists."
                },
            }
            p data-locked .note { "Unlock your lists above to make one." }
            details data-needs-keys hidden {
                summary { (if made_at.is_none() { "Turn it on" } else { "Make a new one" }) }
                @if made_at.is_some() { p .note { "The old key stops working." } }
                form hx-post="/recovery" hx-target="#main" hx-confirm="velio" data-velio="recovery-on"
                    data-email=(email) {
                    input type="hidden" name="action" value="on";
                    (current_password(email, "Your password"))
                    p #recovery-problem .problem role="alert" {}
                    button type="submit" { "Make a recovery key" }
                }
            }
            @if made_at.is_some() {
                details {
                    summary { "Turn it off" }
                    form hx-post="/recovery" hx-target="#main" hx-confirm="velio" data-velio="recovery-off"
                        data-email=(email) {
                        input type="hidden" name="action" value="off";
                        (current_password(email, "Your password"))
                        p #recovery-off-problem .problem role="alert" {}
                        button type="submit" { "Turn the recovery key off" }
                    }
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
                    "This deletes your lists, profiles and keys from Velio, and signs out every TV. It can't be undone."
                }
                form hx-post="/account/delete" hx-target="#main" hx-confirm="velio" data-velio="delete-account"
                    data-email=(email) {
                    (current_password(email, "Your password"))
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
