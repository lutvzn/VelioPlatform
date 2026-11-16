//! Nuvio's profiles: up to 6 an account, each with its own add-ons (or the
//! main profile's), and an optional 4-digit PIN.

use maud::{Markup, html};

use super::accounts::problem;

pub const MAX_PROFILES: i32 = 6;
pub const MAX_NAME_LENGTH: usize = 40;
pub const MAX_URL_LENGTH: usize = 2048;

/// Colours to choose from; Nuvio's apps may set others, which are shown as they are.
pub const PROFILE_COLORS: [(&str, &str); 8] = [
    ("Red", "#E53935"),
    ("Orange", "#FB8C00"),
    ("Gold", "#F9A825"),
    ("Green", "#43A047"),
    ("Teal", "#00897B"),
    ("Blue", "#1E88E5"),
    ("Purple", "#8E24AA"),
    ("Pink", "#D81B60"),
];

pub struct AddonRow {
    pub id: String,
    pub url: String,
    pub name: Option<String>,
    pub enabled: bool,
}

pub struct ProfileRow {
    /// Nuvio's profile_index, 1 for the main profile.
    pub index: i32,
    pub name: String,
    pub color: String,
    pub uses_primary_addons: bool,
    pub pin_enabled: bool,
    pub addons: Vec<AddonRow>,
}

/// A profile's picture: site.js colours it from data-color, as the CSP keeps inline styles out.
pub fn profile_pic(name: &str, color: &str) -> Markup {
    let letter: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    html! { span .pic data-color=(color) aria-hidden="true" { (letter) } }
}

fn swatches(chosen: &str) -> Markup {
    html! {
        fieldset .swatches {
            legend { "Colour" }
            @for (name, hex) in PROFILE_COLORS {
                label .swatch title=(name) {
                    input type="radio" name="color" value=(hex) checked[hex.eq_ignore_ascii_case(chosen)];
                    span data-swatch data-color=(hex) aria-hidden="true" {}
                    span .sr-only { (name) }
                }
            }
        }
    }
}

fn addon_label(addon: &AddonRow) -> &str {
    addon.name.as_deref().filter(|name| !name.is_empty()).unwrap_or(&addon.url)
}

fn addons(profile: &ProfileRow) -> Markup {
    let base = format!("/profiles/{}/addons", profile.index);
    html! {
        h3 { "Add-ons" }
        @if profile.uses_primary_addons {
            p .note { "Uses the main profile's add-ons." }
        } @else {
            @if profile.addons.is_empty() { p .note { "No add-ons yet." } }
            ul {
                @for addon in &profile.addons {
                    li {
                        (addon_label(addon))
                        @if !addon.enabled { " (turned off)" }
                        form method="post" action={ (base) "/" (addon.id) "/delete" }
                            hx-post={ (base) "/" (addon.id) "/delete" } hx-target="#main" {
                            button type="submit" .link { "Remove" }
                        }
                    }
                }
            }
            form method="post" action=(base) hx-post=(base) hx-target="#main" {
                label {
                    "Add-on address"
                    input type="url" name="url" required maxlength=(MAX_URL_LENGTH) autocomplete="off"
                        spellcheck="false" placeholder="https://…/manifest.json";
                }
                button type="submit" { "Add it" }
            }
        }
    }
}

fn profile_item(profile: &ProfileRow) -> Markup {
    let base = format!("/profiles/{}", profile.index);
    html! {
        li {
            div .row-head {
                (profile_pic(&profile.name, &profile.color))
                strong { (profile.name) }
                span .note {
                    @if profile.index == 1 { "Main profile" }
                    @if profile.pin_enabled { " · PIN" }
                }
            }
            details {
                summary { "Change" }
                form method="post" action=(base) hx-post=(base) hx-target="#main" {
                    label {
                        "Name"
                        input type="text" name="name" value=(profile.name) required maxlength=(MAX_NAME_LENGTH)
                            autocomplete="off";
                    }
                    (swatches(&profile.color))
                    @if profile.index > 1 {
                        label .check {
                            input type="checkbox" name="uses_primary_addons" value="yes"
                                checked[profile.uses_primary_addons];
                            span { "Use the main profile's add-ons" }
                        }
                    }
                    button type="submit" { "Save" }
                }
                (addons(profile))
                h3 { "PIN" }
                form method="post" action={ (base) "/pin" } hx-post={ (base) "/pin" } hx-target="#main" {
                    label {
                        @if profile.pin_enabled { "New PIN, or empty for none" } @else { "PIN" }
                        input type="text" name="pin" inputmode="numeric" pattern="[0-9]{4}" maxlength="4"
                            autocomplete="off" placeholder="None";
                    }
                    @if profile.pin_enabled {
                        label {
                            "Current PIN"
                            input type="text" name="current_pin" inputmode="numeric" pattern="[0-9]{4}" maxlength="4"
                                autocomplete="off" required;
                        }
                    }
                    p .note {
                        "4 digits the apps ask for before they open this profile. It keeps others at a shared TV \
                         out; it doesn't encrypt anything."
                    }
                    button type="submit" { "Save the PIN" }
                }
                @if profile.index > 1 {
                    form method="post" action={ (base) "/delete" } hx-post={ (base) "/delete" } hx-target="#main" {
                        p .note { "Deleting a profile deletes its add-ons, history, watch progress and settings." }
                        button type="submit" .danger { "Delete this profile" }
                    }
                }
            }
        }
    }
}

pub fn profiles_page(profiles: &[ProfileRow], notice: Option<&str>) -> Markup {
    html! {
        section {
            h1 { "Profiles" }
            @if let Some(notice) = notice { p .done role="status" { (notice) } }
            p .note {
                "A profile is a set of add-ons, history and settings, such as one for each person at home. \
                 Nuvio's apps and Velio's TV app show the same profiles."
            }
            (problem())
            @if profiles.is_empty() { p { "No profiles yet. Add the first one." } }
            ul .rows { @for profile in profiles { (profile_item(profile)) } }
            @if profiles.len() < MAX_PROFILES as usize {
                form method="post" action="/profiles" hx-post="/profiles" hx-target="#main" {
                    label {
                        "New profile"
                        input type="text" name="name" required maxlength=(MAX_NAME_LENGTH)
                            placeholder="Living room, Sam, Kids…" autocomplete="off";
                    }
                    button type="submit" { "Add the profile" }
                }
            } @else {
                p .note { "An account has at most " (MAX_PROFILES) " profiles." }
            }
            p { a href="/lists" { "Your IPTV lists" } }
        }
    }
}
