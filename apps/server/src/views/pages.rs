//! Pages anyone can read. Neutral wording only: "your own IPTV
//! subscriptions", never content or providers (PLAN.md, "Legal and abuse", 4).
//! The words are the Worker's (`src/views/pages.tsx`), except who
//! handles what (R6): that follows the settings, as the server can run
//! anywhere, behind any proxy.

use maud::{Markup, html};

use crate::config::{Config, Mail};
use crate::views::accounts::{problem, turnstile};
use crate::views::layout::{ICON_LOCK, ICON_PHONE, ICON_SERVER, icon};

pub fn home(signed_in: bool) -> Markup {
    html! {
        section .hero {
            p .eyebrow { "Works with Nuvio" }
            h1 { "Your lists, on every TV" }
            p .lead {
                "Velio keeps the M3U playlists and Xtream logins of your own IPTV subscriptions in one account, and every \
                 VelioTV you sign in gets them. Type a login once here instead of with a remote on each TV."
            }
            div .cta {
                @if signed_in {
                    a href="/lists" .button { "Go to your lists" }
                    a href="/account" .button.secondary { "Your account" }
                } @else {
                    a href="/signup" hx-boost="false" .button { "Make an account" }
                    a href="/signin" .button.secondary { "Sign in" }
                }
            }
        }
        ul .features {
            li {
                (icon(ICON_LOCK))
                h2 { "Profiles with PINs" }
                p { "Up to 6 profiles, each with its own add-ons, history and lists, and a PIN if you like." }
            }
            li {
                (icon(ICON_SERVER))
                h2 { "One account for Nuvio and VelioTV" }
                p {
                    "Nuvio's apps can use this server instead of Nuvio's own, and see the same profiles, add-ons and \
                     progress as VelioTV."
                }
            }
            li {
                (icon(ICON_PHONE))
                h2 { "Sign in a TV with your phone" }
                p { "Scan the QR code the TV shows, choose its profiles, and it loads your lists." }
            }
        }
    }
}

/// `tv_limit` is VELIO_TV_LIMIT, the TVs an account has unless it's given more.
pub fn terms(abuse_email: &str, tv_limit: usize) -> Markup {
    let mailto = format!("mailto:{abuse_email}");
    html! {
        section.prose {
            h1 { "Terms" }
            p.note { "Draft. To be reviewed before Velio is open to the public." }
            ol {
                li { "Velio stores and syncs the sources you add, for your own devices. It provides no content." }
                li { "Add only sources you have the right to use, such as your own subscriptions." }
                li {
                    "An account is for one household: up to " (tv_limit) " TVs, 6 profiles and 50 lists. Lists can't be \
                     shared with other accounts."
                }
                li {
                    "Accounts that break these terms can be closed. Accounts named in valid copyright notices more than \
                     once are closed."
                }
                li {
                    "When an account is closed, its owner is emailed the reason. Its TVs and apps are signed out, and its \
                     data is deleted 30 days later. To contest it, write to "
                    a href=(mailto) { (abuse_email) }
                    " before then."
                }
                li {
                    "You can delete your account at any time, from "
                    a href="/account" { "your account" }
                    ". Everything in it is deleted at once."
                }
                li {
                    "To report abuse or copyright infringement, use "
                    a href="/abuse" { "the form" }
                    " or write to "
                    a href=(mailto) { (abuse_email) }
                    "."
                }
            }
        }
    }
}

/// Who handles what, from the settings: the network in front of the server,
/// email and Turnstile.
fn handlers(cfg: &Config) -> Vec<String> {
    let mut lines =
        vec!["Velio runs on a server of its own: the service and its database are on one machine.".to_owned()];
    if let Some(provider) = &cfg.network_provider {
        lines.push(format!(
            "{provider} carries traffic between you and that server, and handles HTTPS on the way. Like Velio, it \
             can see what's sent, your lists included."
        ));
    }
    lines.push(match cfg.mail {
        Mail::Resend(_) => "Resend sends its email.".into(),
        Mail::Smtp(_) | Mail::DevOutbox => "An email provider sends its email.".into(),
    });
    if cfg.turnstile.is_some() {
        lines.push("Cloudflare runs the check that you're a person on the sign-up, reset and report forms.".into());
    }
    lines
}

pub fn privacy(cfg: &Config) -> Markup {
    html! {
        section.prose {
            h1 { "Privacy" }
            p.note { "Draft. To be reviewed before Velio is open to the public." }
            h2 { "What Velio holds" }
            ul {
                li { "Your email address, when you made the account, and a hash of your password (bcrypt)." }
                li {
                    "Your profiles, add-ons, IPTV lists, library, watch progress, history and settings, as your apps \
                     and this site save them. They're stored as they are, not encrypted: Velio can read them, and \
                     reads them only to run the service or deal with a notice about the account."
                }
                li { "Profile PINs, only as hashes." }
                li {
                    "Your sign-ins: which TVs, apps and browsers, their names and versions, and when they were last \
                     used."
                }
                li { "If you send a notice: what you write in the form, kept as a record of the notice and what was done." }
            }
            h2 { "How long" }
            ul {
                li { "An account never confirmed is deleted after 7 days." }
                li { "Deleting your account deletes all of it at once." }
                li {
                    "A closed account's data is deleted 30 days after it's closed. Its email address, the dates and the \
                     reason are kept, so it can't simply be made again."
                }
                li { "Request logs, which can include IP addresses, are kept for at most 7 days." }
            }
            h2 { "Who handles it" }
            p {
                @for line in handlers(cfg) { (line) " " }
                "Email is sent only for your account: confirming it, resetting it, and telling you if it's closed. \
                 Nothing is sold or used for ads."
            }
        }
    }
}

/// How Velio protects the account, for owners.
pub fn security(cfg: &Config) -> Markup {
    html! {
        section.prose {
            h1 { "How Velio protects your account" }
            p {
                "Your lists hold addresses and logins that are yours. Velio keeps them on its server so that your TVs \
                 and apps can have them. They aren't end-to-end encrypted: like Nuvio's own server, Velio's can read \
                 what it stores."
            }

            h2 { "Your password" }
            p {
                "Velio keeps only a hash of your password (bcrypt). Signing in gives a short-lived token and a refresh \
                 token, as Nuvio's server does; changing your password or resetting it signs out everything else."
            }

            h2 { "What each sign-in can reach" }
            p {
                "Every request runs in the database as you, under row-level security, so a token reaches only your \
                 own account's rows."
            }

            h2 { "Your TVs" }
            p {
                "A TV shows a QR code and a short code that last 10 minutes. Approving it here signs it in to your \
                 account. "
                a href="/devices" { "TVs and apps" }
                " lists everything signed in, and signs out what you don't recognise."
            }

            h2 { "The pages" }
            p {
                "Pages run only code from "
                (cfg.site_host)
                @if cfg.turnstile.is_some() { ", plus Cloudflare's Turnstile check on the sign-up and reset pages" }
                ", under a strict Content-Security-Policy, and only over HTTPS."
            }
            @if let Some(provider) = &cfg.network_provider {
                p {
                    "HTTPS ends at " (provider) ", which passes requests on to Velio's server, so it can see what's \
                     sent too."
                }
            }
            p { "See " a href="/privacy" { "Privacy" } " for what Velio holds, and for how long." }
        }
    }
}

pub fn not_found() -> Markup {
    html! {
        section {
            h1 { "Not found" }
            p { a href="/" { "Go to the start" } }
        }
    }
}

pub fn account_deleted() -> Markup {
    html! {
        section {
            h1 { "Your account is deleted" }
            p { "Its lists, profiles and add-ons are gone from Velio, and its TVs and apps are signed out." }
            p { a href="/" { "Go to the start" } }
        }
    }
}

// --- Notices (Phase 8) ---

pub fn abuse_form(site_key: Option<&str>, abuse_email: &str) -> Markup {
    html! {
        section .prose {
            h1 { "Report abuse or copyright infringement" }
            p {
                "Velio syncs the lists people add for their own TVs. It provides no content. It can close the \
                 account that holds a list: name it by its email address if you know it."
            }
            p {
                "You can also write to " a href={ "mailto:" (abuse_email) } { (abuse_email) } ". Authorities with a \
                 legal request: write there."
            }
            form hx-post="/abuse" hx-target="#main" {
                label {
                    "What it's about"
                    select name="kind" required {
                        option value="copyright" { "Copyright infringement" }
                        option value="abuse" { "Other abuse of the service" }
                        option value="other" { "Something else, such as contesting a closed account" }
                    }
                }
                label {
                    "Your name"
                    input type="text" name="name" autocomplete="name" required maxlength="200";
                }
                label {
                    "Your email"
                    input type="email" name="email" autocomplete="email" required maxlength="254";
                }
                label {
                    "Organization, if you write for one"
                    input type="text" name="organization" autocomplete="organization" maxlength="200";
                }
                label {
                    "The Velio account's email address, if you know it"
                    input type="email" name="account_email" maxlength="254";
                }
                label {
                    "What happened, and how you know"
                    textarea name="details" required minlength="20" maxlength="10000" rows="6" {}
                }
                fieldset {
                    legend { "For copyright notices" }
                    label {
                        "The copyrighted work, such as the channel or programme, and who owns it"
                        textarea name="work" maxlength="10000" rows="3" {}
                    }
                    label .check {
                        input type="checkbox" name="sworn" value="yes";
                        span {
                            "I believe in good faith that the use described isn't authorized by the copyright owner, \
                             its agent, or the law. The information in this notice is accurate and, under penalty of \
                             perjury, I am the owner of the right concerned or authorized to act for the owner."
                        }
                    }
                    label {
                        "Signature: your full name"
                        input type="text" name="signature" maxlength="200";
                    }
                }
                (turnstile(site_key))
                (problem())
                button type="submit" { "Send the notice" }
            }
        }
    }
}

pub fn notice_received(id: &str) -> Markup {
    html! {
        section {
            h1 { "Notice received" }
            p {
                "Thank you. Its reference is " strong { (id) } ". We'll answer by email, and let you know what we did."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(extra: &[(&str, &str)]) -> Config {
        let pairs: Vec<(&str, &str)> = [
            ("VELIO_SITE_URL", "https://velio.tv"),
            ("JWT_SECRET", "a-jwt-secret-of-at-least-32-characters!"),
            ("DATABASE_URL", "postgres://velio@127.0.0.1/velio"),
            ("EMAIL_FROM", "no-reply@velio.tv"),
            ("ABUSE_EMAIL", "abuse@velio.tv"),
            ("SMTP_URL", "smtps://user:pass@mail.example:465"),
        ]
        .into_iter()
        .chain(extra.iter().copied())
        .collect();
        Config::from_lookup(|name| {
            pairs.iter().rev().find(|(key, _)| *key == name).map(|(_, value)| (*value).to_owned())
        })
        .unwrap()
    }

    #[test]
    fn names_only_who_is_configured() {
        let plain = config(&[]);
        let (plain_privacy, plain_security) = (privacy(&plain).into_string(), security(&plain).into_string());
        assert!(!plain_privacy.contains("Cloudflare") && !plain_security.contains("Cloudflare"));
        assert!(plain_privacy.contains("An email provider sends its email."));

        let velio = config(&[
            ("VELIO_NETWORK_PROVIDER", "Cloudflare"),
            ("RESEND_API_KEY", "re_x"),
            ("SMTP_URL", ""),
            ("TURNSTILE_SITE_KEY", "0x4"),
            ("TURNSTILE_SECRET", "0x4s"),
        ]);
        let velio_privacy = privacy(&velio).into_string();
        for words in ["Cloudflare carries traffic", "Resend sends its email.", "Cloudflare runs the check"] {
            assert!(velio_privacy.contains(words), "{words}");
        }
        let velio_security = security(&velio).into_string();
        assert!(velio_security.contains("HTTPS ends at Cloudflare"), "{velio_security}");
        assert!(velio_security.contains("Turnstile check on the sign-up"));
    }
}
