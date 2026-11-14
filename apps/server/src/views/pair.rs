//! Signing in a TV (the Worker's `views/pair.tsx`; PLAN.md, "Signing in a
//! TV"). The QR code opens /pair#<id>.<secret>; vault.js keeps the part
//! after # (which never reaches the server), finds the TV with the pairing
//! id and a proof of the secret, opens its sealed name and key, and sends
//! the signed change that adds it.

use maud::{Markup, html};

use crate::views::vault::{VaultProps, device_name, profile_name, profile_pic, vault_page};

pub const PAIR_TITLE: &str = "Sign in a TV";

/// The pairing page: type the TV's code, or come from its QR code with the code already filled in.
/// `signed_in` is the TV just signed in, whose name vault.js fills in.
pub fn pair_page(props: &VaultProps, signed_in: Option<&str>) -> Markup {
    vault_page(
        props,
        PAIR_TITLE,
        html! {
            @if let Some(device_id) = signed_in {
                p .done role="status" {
                    strong { (device_name(device_id)) } " is signed in. It loads your lists in a few seconds."
                }
            }
            p .note {
                "On the TV, open VelioTV's Settings, then Account, and choose Sign in with a phone. Scan the QR code \
                 it shows with this phone's camera, or type the code below the QR code."
            }
            form hx-post="/pair/find" hx-target="#pair-step" hx-confirm="velio" data-velio="pair-find" {
                label {
                    "Code on the TV"
                    input type="text" data-field="code" required maxlength="120" placeholder="K7QM-2XHF-9WRT"
                        autocomplete="off" autocapitalize="characters" spellcheck="false";
                }
                p #form-problem .problem role="alert" {}
                button type="submit" { "Find the TV" }
            }
            div #pair-step {}
            p { a href="/account" { "Your account" } }
        },
    )
}

/// The TV that was found: its name (sealed, so vault.js shows it once it
/// checks out), and the profiles to give it, all ticked.
pub struct PairConfirm<'a> {
    pub pairing_id: &'a str,
    pub device_id: &'a str,
    pub kind: &'a str,
    /// The TV's details sealed under each secret, as JSON (sync-core SealedDeviceInfo).
    pub sealed_json: &'a str,
    pub profile_ids: Vec<&'a str>,
    /// The account's TV limit, which vault.js checks before it signs the change.
    pub tv_limit: usize,
}

pub fn pair_confirm(props: &PairConfirm<'_>) -> Markup {
    html! {
        section data-sealed-device=(props.sealed_json) data-kind=(props.kind) {
            h2 { "Sign in " span data-device-name { "this TV" } "?" }
            p .problem role="alert" data-pair-open-problem {}
            @if props.profile_ids.is_empty() {
                p {
                    "A TV shows the lists of the profiles you give it, and this account has none yet. "
                    a href="/profiles" { "Add a profile" } ", then scan the code again."
                }
            } @else {
                form hx-post=(format!("/pair/{}", props.pairing_id)) hx-target="#main" hx-confirm="velio"
                    data-velio="pair" data-pairing-id=(props.pairing_id) data-device-id=(props.device_id)
                    data-tv-limit=(props.tv_limit) hidden {
                    fieldset {
                        legend { "Profiles on this TV" }
                        @for id in &props.profile_ids {
                            label .check {
                                input type="checkbox" data-profile-choice=(id) checked;
                                (profile_pic(id))
                                (profile_name(id))
                            }
                        }
                    }
                    p .note {
                        "Only sign in a TV you're in front of: check that its name and code match what the TV shows."
                    }
                    p #pair-problem .problem role="alert" {}
                    button type="submit" { "Sign in the TV" }
                }
            }
        }
    }
}
