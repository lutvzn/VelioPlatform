//! Pages anyone can read. Neutral wording only: "your own IPTV
//! subscriptions", never content or providers (PLAN.md, "Legal and abuse", 4).
//! The words are the Worker's (`src/views/pages.tsx`), except who
//! handles what (R6): that follows the settings, as the server can run
//! anywhere, behind any proxy.

use maud::{Markup, html};

use crate::backup::BACKUP_KEEP_DAYS;
use crate::config::{Config, Mail};
use crate::views::accounts::{problem, turnstile};
use crate::views::layout::{ICON_LOCK, ICON_PHONE, ICON_SERVER, icon};

pub fn home(signed_in: bool) -> Markup {
    html! {
        section .hero {
            p .eyebrow { "End-to-end encrypted" }
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
                h2 { "Encrypted in your browser" }
                p { "Your lists are encrypted in this browser with your password before they're sent." }
            }
            li {
                (icon(ICON_SERVER))
                h2 { "Only ciphertext on the server" }
                p {
                    "Velio's servers can't read a playlist address, a server, a username or a password."
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
                    "Your lists are encrypted on your devices. Velio can't see what you store, and can't recover it if you \
                     lose your password and have no recovery key."
                }
                li {
                    "An account is for one household: up to " (tv_limit) " TVs, 8 profiles and 50 lists. Lists can't be \
                     shared with other accounts."
                }
                li {
                    "Accounts that break these terms can be closed. Accounts named in valid copyright notices more than \
                     once are closed."
                }
                li {
                    "When an account is closed, its owner is emailed the reason. Its TVs are signed out, and its encrypted \
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
/// email, Turnstile and off-site backups.
fn handlers(cfg: &Config) -> Vec<String> {
    let mut lines =
        vec!["Velio runs on a server of its own: the service and its database are on one machine.".to_owned()];
    if let Some(provider) = &cfg.network_provider {
        lines.push(format!(
            "{provider} carries traffic between you and that server, and handles HTTPS on the way. It sees what \
             Velio sees: your lists only encrypted."
        ));
    }
    lines.push(match cfg.mail {
        Mail::Resend(_) => "Resend sends its email.".into(),
        Mail::Smtp(_) | Mail::DevOutbox => "An email provider sends its email.".into(),
    });
    if cfg.turnstile.is_some() {
        lines.push("Cloudflare runs the check that you're a person on the sign-up, reset and report forms.".into());
    }
    if cfg.backup.as_ref().is_some_and(|backup| backup.s3.is_some()) {
        lines.push("A storage provider keeps a second copy of the nightly backups.".into());
    }
    lines
}

pub fn privacy(cfg: &Config) -> Markup {
    html! {
        section.prose {
            h1 { "Privacy" }
            p.note { "Draft. To be reviewed before Velio is open to the public." }
            h2 { "What Velio can't read" }
            p {
                "Your lists, profile and TV names, PINs and keys are encrypted on your devices before they're sent. Velio can't \
                 read them, and neither can anyone who gets a copy of its databases. See "
                a href="/security" { "how Velio protects your lists" }
                "."
            }
            h2 { "What Velio holds" }
            ul {
                li {
                    "Your email address, when you made the account and accepted the terms, the salt and settings your \
                     password is turned into keys with, and a hash of your sign-in key."
                }
                li { "Your sessions in browsers, and how many TVs you have and when they last synced." }
                li { "How many profiles and lists you have, and when they changed." }
                li { "If you made a recovery key: a copy of your keys sealed under it, and a hash of its sign-in key." }
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
                @if cfg.backup.is_some() {
                    li {
                        "The database is backed up each night, and each backup is kept " (BACKUP_KEEP_DAYS) " days, so \
                         deleted data can stay in one for up to " (BACKUP_KEEP_DAYS) " days more."
                    }
                }
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

/// How Velio protects the lists, for owners (Phase 7). PLAN.md has the details.
pub fn security(cfg: &Config) -> Markup {
    html! {
        section.prose {
            h1 { "How Velio protects your lists" }
            p {
                "Your lists hold addresses and logins that are yours. Velio keeps them so that your TVs can have them, \
                 without being able to read them."
            }

            h2 { "Your password" }
            p {
                "Your browser turns your password into two keys, with Argon2id (64 MiB, 3 passes) and a salt of your own. \
                 One key signs you in: Velio keeps only a hash of it. The other opens your account's keys, which Velio \
                 keeps sealed under it. Your password and that second key never leave your browser."
            }

            h2 { "Your lists and profiles" }
            p {
                "Each list and each profile is encrypted in your browser with a key of its own (XSalsa20-Poly1305), \
                 before it's sent. Your browser signs the account's list of what exists (Ed25519), so a TV or another \
                 browser can tell if anything was changed, added, dropped or swapped on the way."
            }
            p {
                "This browser also remembers the newest version of your account it has opened, and refuses an older one. \
                 That stops a server from quietly undoing a change, such as bringing back a TV you removed. A browser \
                 that has never opened your account can't tell, so make changes from one you use often."
            }

            h2 { "Your TVs" }
            p {
                "A TV makes its own keys, and gets only the keys of the profiles you give it. The QR code it shows \
                 carries a secret that goes from the TV to your phone without passing through Velio, so Velio can't slip \
                 in keys of its own. The code you can type instead is shorter: a hostile server could work it out in \
                 days, though not within the 10 minutes it lasts. Scan the QR code when you can."
            }
            p {
                "When you remove a TV or take a profile from it, your browser makes new keys for what it could read, so \
                 it can't read anything you change after that. It may still have what it had before."
            }

            h2 { "A forgotten password" }
            p {
                "Velio can't open your lists, so it can't give them back. Without a recovery key, resetting the password \
                 deletes your lists and profiles. With a "
                a href="/account" { "recovery key" }
                " and the reset email, you keep everything. Keep the recovery key away from the devices you use Velio on."
            }

            h2 { "What Velio can still do" }
            p {
                "Velio sends the page your browser runs. If Velio itself were taken over, the page could be changed to \
                 send your password elsewhere: that's true of any service you use in a browser. Pages run only code from "
                (cfg.site_host)
                @if cfg.turnstile.is_some() { ", plus Cloudflare's Turnstile check on the sign-up and reset pages" }
                ", under a strict Content-Security-Policy, and only over HTTPS."
            }
            @if let Some(provider) = &cfg.network_provider {
                p {
                    "HTTPS ends at " (provider) ", which passes requests on to Velio's server. Like Velio, it sees your \
                     lists only encrypted, and could change the page in the same way."
                }
            }
            p {
                "Velio knows your email address, how many TVs, profiles and lists you have, and when things \
                 happen. See "
                a href="/privacy" { "Privacy" }
                "."
            }

            h2 { "A copy for you" }
            p {
                "On "
                a href="/lists" { "Your lists" }
                ", you can download your lists and profiles as a file. Your browser makes it from what it has opened. It \
                 isn't encrypted, so keep it safe."
            }
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
            p { "Its lists, profiles and keys are gone from Velio, and its TVs are signed out." }
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
                "Velio syncs the lists people add for their own TVs. It provides no content, and it stores lists \
                 encrypted, so it can't see them, search them, or remove a single one. What it can do is close the \
                 account that holds them: name it by its email address if you know it."
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
            ("PRELOGIN_SECRET", "a-prelogin-secret-of-32-characters!"),
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
        assert!(plain_privacy.contains("An email provider sends its email.") && !plain_privacy.contains("backed up"));

        let velio = config(&[
            ("VELIO_NETWORK_PROVIDER", "Cloudflare"),
            ("RESEND_API_KEY", "re_x"),
            ("SMTP_URL", ""),
            ("TURNSTILE_SITE_KEY", "0x4"),
            ("TURNSTILE_SECRET", "0x4s"),
            ("VELIO_BACKUP_DIR", "/data/backups"),
            ("VELIO_BACKUP_S3_URL", "https://s3.example/b"),
            ("VELIO_BACKUP_S3_ACCESS_KEY_ID", "k"),
            ("VELIO_BACKUP_S3_SECRET_ACCESS_KEY", "s"),
        ]);
        let velio_privacy = privacy(&velio).into_string();
        for words in [
            "Cloudflare carries traffic",
            "Resend sends its email.",
            "Cloudflare runs the check",
            "a second copy of the nightly backups",
            "kept 30 days",
        ] {
            assert!(velio_privacy.contains(words), "{words}");
        }
        let velio_security = security(&velio).into_string();
        assert!(velio_security.contains("HTTPS ends at Cloudflare"), "{velio_security}");
        assert!(velio_security.contains("Turnstile check on the sign-up"));
    }
}
