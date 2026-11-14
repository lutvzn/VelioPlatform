//! The operator's page at /ops (the Worker's `views/ops.tsx`): signing in,
//! and the shell that ops.js (`client/ops.ts`) fills from the
//! /ops API. No HTMX or vault.js here.

use maud::{Markup, PreEscaped, html};

/// Escapes as Hono's JSX does, apostrophes too, so the words read the same.
fn escaped(text: &str) -> PreEscaped<String> {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    PreEscaped(out)
}

fn ops_layout(title: &str, script: bool, body: Markup) -> Markup {
    html! {
        (PreEscaped("<!doctype html>"))
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="color-scheme" content="light dark";
                meta name="robots" content="noindex";
                // The site's no-referrer makes a browser send "Origin: null" with a plain form, which
                // same_origin_posts refuses; same-origin sends velio.tv's Origin and still nothing elsewhere.
                meta name="referrer" content="same-origin";
                title { (title) " · Velio operator" }
                link rel="stylesheet" href="/static/ops.css";
                @if script {
                    script src="/static/ops.js" defer {}
                }
            }
            body { (body) }
        }
    }
}

pub fn ops_sign_in(problem: Option<&str>) -> Markup {
    ops_layout(
        "Sign in",
        false,
        html! {
            main .narrow {
                h1 { "Velio operator" }
                p .note { "For handling notices and closing accounts. Sign in with the OPS_TOKEN secret." }
                form method="post" action="/ops" .stack {
                    // For password managers, which keep the token under this name.
                    input type="text" name="username" value="velio-operator" autocomplete="username" hidden;
                    label {
                        "Operator token"
                        input type="password" name="token" autocomplete="current-password" required autofocus;
                    }
                    @if let Some(problem) = problem {
                        p .problem role="alert" { (escaped(problem)) }
                    }
                    button type="submit" { "Sign in" }
                }
                p .note { "The session lasts 8 hours. Changing the secret ends every session." }
            }
        },
    )
}

fn actions(submit: Markup) -> Markup {
    html! {
        div .actions {
            button type="button" data-cancel autofocus { "Cancel" }
            (submit)
        }
    }
}

pub fn ops_app() -> Markup {
    ops_layout(
        "Notices",
        true,
        html! {
            header {
                strong { "Velio operator" }
                span .badge.live { "LIVE" }
                nav {
                    button type="button" data-tab="notices" aria-pressed="true" { "Notices" }
                    button type="button" data-tab="accounts" aria-pressed="false" { "Accounts" }
                    button type="button" #signout { "Sign out" }
                }
            }
            p #status role="status" hidden {}

            main {
                section #notices {
                    div .bar {
                        label {
                            "Show"
                            select #notice-status {
                                option value="open" { "Open" }
                                option value="actioned" { "Actioned" }
                                option value="rejected" { "Rejected" }
                                option value="all" { "All" }
                            }
                        }
                        button type="button" #notice-refresh { "Refresh" }
                    }
                    table {
                        thead {
                            tr {
                                th { "Reference" }
                                th { "Received" }
                                th { "Kind" }
                                th { "From" }
                                th { "Account named" }
                                th { "Status" }
                            }
                        }
                        tbody #notice-rows {}
                    }
                    p #notice-empty .note hidden { "No notices." }
                    article #notice .card hidden {}
                }

                section #accounts hidden {
                    form #account-find .bar {
                        label {
                            "Email or account id"
                            input #account-key type="text" autocomplete="off" spellcheck="false" required;
                        }
                        label {
                            "Show"
                            select #account-state {
                                option value="all" { "All" }
                                option value="open" { "Open" }
                                option value="unconfirmed" { "Email not confirmed" }
                                option value="closed" { "Closed" }
                            }
                        }
                        button type="submit" { "Look up" }
                    }
                    p .note { "Typing narrows the list; Look up opens the account with that email or id." }
                    table {
                        thead {
                            tr {
                                th { "Email" }
                                th { "Made" }
                                th { "State" }
                                th { "TVs" }
                                th { "Notices" }
                            }
                        }
                        tbody #account-rows {}
                    }
                    p #account-count .note {}
                    article #account .card hidden {}
                }
            }

            dialog #close-dialog {
                form method="dialog" #close-form {
                    h2 { "Close " span data-email {} "?" }
                    ul .facts {
                        li { "Every browser and TV signed in to it is signed out " strong { "now" } "." }
                        li { "The owner is emailed your reason, and where to contest it." }
                        li {
                            "Their lists, profiles and TVs are "
                            strong { "deleted on " span data-delete-on {} }
                            ", unless you open the account again before then. After that it can't be undone."
                        }
                        li { "The email address can't sign up again." }
                    }
                    p .note data-account-facts {}
                    label {
                        "Reason, which the owner reads"
                        textarea #close-reason rows="4" maxlength="2000" required {}
                    }
                    label {
                        "Notice acted on (optional)"
                        input #close-notice type="text" autocomplete="off" spellcheck="false" placeholder="N-XXXXXXXX";
                    }
                    label {
                        "Type the account's email to confirm"
                        input #close-confirm type="text" autocomplete="off" spellcheck="false";
                    }
                    p .problem role="alert" data-problem {}
                    (actions(html! { button type="submit" .danger #close-submit disabled { "Close this account" } }))
                }
            }

            dialog #reopen-dialog {
                form method="dialog" #reopen-form {
                    h2 { "Open " span data-email {} " again?" }
                    p {
                        "The owner can sign in again and is emailed. Their lists and profiles come back; TVs have to be \
                         signed in again."
                    }
                    p .problem role="alert" data-problem {}
                    (actions(html! { button type="submit" { "Open it again" } }))
                }
            }

            dialog #handle-dialog {
                form method="dialog" #handle-form {
                    h2 { "Mark notice " span data-notice {} " as " span data-handle-status {} "?" }
                    p .note { "This doesn't change any account. It records what you did." }
                    label {
                        "What was done"
                        textarea #handle-action rows="3" maxlength="2000" required {}
                    }
                    p .problem role="alert" data-problem {}
                    (actions(html! { button type="submit" { "Mark it" } }))
                }
            }
        },
    )
}
