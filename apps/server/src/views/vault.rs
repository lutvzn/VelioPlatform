//! The pages for profiles, lists and TVs (the Worker's `views/vault.tsx`;
//! PLAN.md, "Routes"). The server can't read names, addresses or logins, so
//! it renders the shape it knows from the manifest (ids, which profile has
//! which list) with empty slots, and vault.js fills them from the ciphertext
//! in [data-vault-state].
//!
//! Every input that holds something secret has no name, so HTMX never sends
//! it: forms are marked data-velio="…", and vault.js sends a sealed, signed
//! change in its place.

use maud::{Markup, html};

use crate::accounts::Owner;
use crate::sync_core::{Manifest, StoredState};
use crate::vault::{DeviceRow, device_status};

// sync-core's records.ts.
const MAX_NAME_LENGTH: usize = 100;
const MAX_URL_LENGTH: usize = 2048;
const PROFILE_PIN_LENGTH: usize = 4;
const REFRESH_CHOICES: [u32; 5] = [6, 12, 24, 48, 168];
const DEFAULT_REFRESH_HOURS: u32 = 24;
/// The names of PROFILE_COLORS; vault.js has the colours.
const PROFILE_COLORS: [&str; 8] = ["Red", "Orange", "Gold", "Green", "Teal", "Blue", "Purple", "Pink"];

/// What every vault page shows.
pub struct VaultProps {
    pub account_id: String,
    pub email: String,
    /// The owner's public signing key, so vault.js can tell remembered keys from before a reset.
    pub sign_public_key: String,
    pub stored: StoredState,
    pub manifest: Manifest,
    /// The TVs' dates; their names are in the manifest, sealed.
    pub devices: Vec<DeviceRow>,
}

fn problem() -> Markup {
    html! { p #form-problem .problem role="alert" {} }
}

/// The ciphertext the page shows, for vault.js to open.
fn vault_state(props: &VaultProps) -> Markup {
    let stored = serde_json::to_string(&props.stored).expect("a stored state is JSON");
    html! {
        div hidden
            data-vault-state=(stored)
            data-account-id=(props.account_id)
            data-sign-public-key=(props.sign_public_key) {}
    }
}

/// The owner bundle, for vault.js to open with the unlock key it kept from the password.
pub fn owner_bundle(owner: &Owner) -> Markup {
    html! {
        div hidden
            data-owner-bundle=(owner.bundle)
            data-sign-public-key=(owner.sign_public_key)
            data-box-public-key=(owner.box_public_key) {}
    }
}

/// Shown until the owner's keys are open in this tab.
pub fn unlock_form(email: &str) -> Markup {
    html! {
        section data-locked {
            p data-vault-status { "Your lists are locked. Enter your password to open them in this browser." }
            form hx-post="/unlock" hx-target="#unlock-result" hx-confirm="velio" data-velio="unlock" {
                input type="hidden" name="email" value=(email);
                input type="email" hidden value=(email) autocomplete="username";
                label {
                    "Password"
                    input type="password" data-password autocomplete="current-password" required;
                }
                (remember_choice())
                p #unlock-problem .problem role="alert" {}
                button type="submit" { "Unlock" }
            }
            div #unlock-result hidden {}
        }
    }
}

pub fn remember_choice() -> Markup {
    html! {
        label .check {
            input type="checkbox" data-remember;
            span {
                "Remember this browser, so it opens your lists without the password. Only on your own computer."
            }
        }
    }
}

/// A page that needs the owner's keys: the unlock form, then the page once vault.js has opened it.
pub(crate) fn vault_page(props: &VaultProps, title: &str, children: Markup) -> Markup {
    html! {
        (vault_state(props))
        section {
            h1 { (title) }
            p .problem data-vault-problem hidden {}
        }
        (unlock_form(&props.email))
        div data-needs-keys hidden { (children) }
    }
}

pub(crate) fn profile_name(id: &str) -> Markup {
    html! { span data-profile-name=(id) { "…" } }
}

/// A TV's name, which only the owner can open: vault.js fills it in from the manifest.
pub(crate) fn device_name(id: &str) -> Markup {
    html! { span data-tv-name=(id) { "…" } }
}

fn list_name(id: &str) -> Markup {
    html! { span data-list-name=(id) { "…" } }
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

// --- Profiles ---

/// A profile's picture: vault.js draws the name's first letter on the profile's colour.
pub(crate) fn profile_pic(id: &str) -> Markup {
    html! { span .pic data-profile-pic=(id) aria-hidden="true" {} }
}

pub fn profiles_page(props: &VaultProps) -> Markup {
    let manifest = &props.manifest;
    vault_page(
        props,
        "Profiles",
        html! {
            p .note {
                "A profile is a set of lists, such as one for each person at home. Each TV shows the profiles you give it."
            }
            (problem())
            @if manifest.profiles.is_empty() { p { "No profiles yet. Add the first one." } }
            ul .rows {
                @for profile in &manifest.profiles {
                    li {
                        div .row-head {
                            (profile_pic(&profile.id))
                            strong { (profile_name(&profile.id)) }
                            span .note {
                                (plural(profile.lists.len(), "list", "lists"))
                                span data-profile-pin-note=(profile.id) {}
                            }
                        }
                        details {
                            summary { "Change" }
                            form hx-post="/changes" hx-target="#main" hx-confirm="velio"
                                data-velio="profile-edit" data-profile-id=(profile.id) {
                                input type="hidden" name="next" value="/profiles";
                                label {
                                    "Name"
                                    input type="text" data-field="name" required
                                        maxlength=(MAX_NAME_LENGTH) autocomplete="off";
                                }
                                fieldset .swatches {
                                    legend { "Colour" }
                                    @for (index, color) in PROFILE_COLORS.iter().enumerate() {
                                        label .swatch title=(color) {
                                            // Named only so the radios form a group; vault.js sends the change instead of the form.
                                            input type="radio" name={ "color-" (profile.id) } value=(index)
                                                data-color-choice;
                                            span data-swatch=(index) aria-hidden="true" {}
                                            span .sr-only { (color) }
                                        }
                                    }
                                }
                                label {
                                    "Profile PIN"
                                    input type="text" data-field="pin" inputmode="numeric"
                                        pattern={ "[0-9]{" (PROFILE_PIN_LENGTH) "}" }
                                        maxlength=(PROFILE_PIN_LENGTH) autocomplete="off" placeholder="None";
                                }
                                p .note {
                                    (PROFILE_PIN_LENGTH)
                                    " digits a TV asks for before it opens this profile, or empty for none. \
                                     Like VelioTV's parental PIN, it keeps others at a shared TV out of the profile; \
                                     it doesn't encrypt anything."
                                }
                                p .problem role="alert" {}
                                button type="submit" { "Save" }
                            }
                            form hx-post="/changes" hx-target="#main" hx-confirm="velio"
                                data-velio="profile-delete" data-profile-id=(profile.id) {
                                input type="hidden" name="next" value="/profiles";
                                p .note {
                                    "Deleting a profile keeps its lists. TVs stop showing it, and its lists get new keys."
                                }
                                button type="submit" .danger { "Delete this profile" }
                            }
                        }
                    }
                }
            }
            form hx-post="/changes" hx-target="#main" hx-confirm="velio" data-velio="profile-add" {
                input type="hidden" name="next" value="/profiles";
                label {
                    "New profile"
                    input type="text" data-field="name" required maxlength=(MAX_NAME_LENGTH)
                        placeholder="Living room, Sam, Kids…" autocomplete="off";
                }
                button type="submit" { "Add the profile" }
            }
            p { a href="/lists" { "Your lists" } }
        },
    )
}

// --- Lists ---

fn holders<'a>(manifest: &'a Manifest, list_id: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    manifest
        .profiles
        .iter()
        .filter(move |profile| profile.lists.iter().any(|id| id == list_id))
        .map(|profile| profile.id.as_str())
}

pub fn lists_page(props: &VaultProps) -> Markup {
    let manifest = &props.manifest;
    vault_page(
        props,
        "Lists",
        html! {
            p .note {
                "The M3U playlists and Xtream logins of your own subscriptions, and the Stremio add-ons whose \
                 films and series your TVs show. Give each list to the profiles that should have it."
            }
            @if manifest.lists.is_empty() { p { "No lists yet." } }
            ul .rows {
                @for list in &manifest.lists {
                    @let profiles: Vec<&str> = holders(manifest, &list.id).collect();
                    li {
                        div .row-head {
                            a href={ "/lists/" (list.id) } { strong { (list_name(&list.id)) } }
                            span .note data-list-summary=(list.id) {}
                        }
                        p .note {
                            @if profiles.is_empty() {
                                "No profile has it yet."
                            } @else {
                                @for (index, profile) in profiles.iter().enumerate() {
                                    (if index > 0 { ", " } else { "For " })
                                    (profile_name(profile))
                                }
                            }
                        }
                    }
                }
            }
            p { a href="/lists/new" .button { "Add a list" } }
            @if manifest.profiles.is_empty() {
                p .note {
                    "A list reaches a TV through a profile. "
                    a href="/profiles" { "Add a profile" }
                    " to give lists to."
                }
            } @else {
                p { a href="/profiles" { "Your profiles" } }
            }
            @if !manifest.lists.is_empty() {
                details {
                    summary { "Download a copy" }
                    p .note {
                        "A file with every list's address and login, and your profiles with their PINs, to keep \
                         or to use elsewhere. This browser makes it; it isn't encrypted, so keep it somewhere safe."
                    }
                    button type="button" data-export { "Download your lists" }
                }
            }
        },
    )
}

fn refresh_label(hours: u32) -> String {
    match hours {
        hours if hours < 24 => format!("Every {hours} hours"),
        24 => "Every day".into(),
        168 => "Every week".into(),
        hours => format!("Every {} days", hours / 24),
    }
}

fn refresh_choice() -> Markup {
    html! {
        label {
            "Refresh"
            select data-field="refreshHours" {
                @for hours in REFRESH_CHOICES {
                    option value=(hours) selected[hours == DEFAULT_REFRESH_HOURS] { (refresh_label(hours)) }
                }
            }
        }
    }
}

fn text_field(field: &str, label: &str, kind: &str, placeholder: Option<&str>, maxlength: usize) -> Markup {
    html! {
        label {
            (label)
            input type=(kind) data-field=(field) maxlength=(maxlength) placeholder=[placeholder]
                autocomplete="off" spellcheck="false";
        }
    }
}

/// Adding a list (no list_id) or changing one; vault.js fills the fields from the list.
pub fn list_page(props: &VaultProps, list_id: Option<&str>) -> Markup {
    let manifest = &props.manifest;
    let holders: Vec<&str> = list_id.map(|id| holders(manifest, id).collect()).unwrap_or_default();
    let title = if list_id.is_some() { "Change the list" } else { "Add a list" };
    vault_page(
        props,
        title,
        html! {
            form hx-post="/changes" hx-target="#main" hx-confirm="velio" data-velio="list-save"
                data-list-id=[list_id] data-list-form {
                input type="hidden" name="next" value="/lists";
                label {
                    "Kind"
                    select data-field="kind" {
                        option value="m3u" { "M3U playlist" }
                        option value="xtream" { "Xtream login" }
                        option value="stremio" { "Stremio add-on" }
                    }
                }
                fieldset data-kind-only="m3u" {
                    (text_field("url", "Playlist address", "url", Some("https://…"), MAX_URL_LENGTH))
                }
                fieldset data-kind-only="stremio" hidden {
                    (text_field("addonUrl", "Add-on address", "url",
                        Some("https://…/manifest.json or stremio://…"), MAX_URL_LENGTH))
                    p .note {
                        "Its catalogs show in Movies and Series on the TV, and its streams and subtitles on a \
                         title's page. Only streams with a web address play; torrents don't."
                    }
                }
                fieldset data-kind-only="xtream" hidden {
                    (text_field("server", "Server", "text", Some("https://…"), MAX_URL_LENGTH))
                    (text_field("username", "Username", "text", None, MAX_URL_LENGTH))
                    label {
                        "Password"
                        input type="password" data-field="password" maxlength=(MAX_URL_LENGTH) autocomplete="off";
                    }
                }
                (text_field("name", "Name (optional)", "text", Some("The server's name"), MAX_NAME_LENGTH))
                fieldset data-kind-not="stremio" {
                    label {
                        "Guide addresses (optional)"
                        textarea data-field="guideUrls" rows="2" spellcheck="false"
                            placeholder="XMLTV addresses, one per line" {}
                    }
                    (refresh_choice())
                }
                fieldset {
                    legend { "Profiles that have it" }
                    @if manifest.profiles.is_empty() {
                        p .note {
                            "No profiles yet. You can save the list now and "
                            a href="/profiles" { "add a profile" }
                            " after."
                        }
                    } @else {
                        @for profile in &manifest.profiles {
                            label .check {
                                input type="checkbox" data-profile-choice=(profile.id)
                                    checked[holders.contains(&profile.id.as_str())];
                                (profile_name(&profile.id))
                            }
                        }
                    }
                    @if list_id.is_some() {
                        p .note {
                            "Taking the list from a profile gives it a new key, so TVs with only that profile \
                             can't open it again."
                        }
                    }
                }
                label .check {
                    input type="checkbox" data-field="disabled";
                    span { "Turned off: TVs keep it but don't load it" }
                }
                (problem())
                button type="submit" { (if list_id.is_some() { "Save the list" } else { "Add the list" }) }
            }
            @if let Some(list_id) = list_id {
                form hx-post="/changes" hx-target="#main" hx-confirm="velio" data-velio="list-delete"
                    data-list-id=(list_id) {
                    input type="hidden" name="next" value="/lists";
                    button type="submit" .danger { "Delete this list" }
                }
            }
            p { a href="/lists" { "Back to your lists" } }
        },
    )
}

// --- TVs ---

/// The TVs signed in to the account: the profiles each has, changing them,
/// and removing a TV. Both give the profiles a TV loses new keys (vault.js,
/// with sync-core's withDeviceProfiles() and withoutDevice()).
pub fn devices_page(props: &VaultProps) -> Markup {
    let manifest = &props.manifest;
    vault_page(
        props,
        "TVs",
        html! {
            p .note {
                "Each TV shows the lists of the profiles you gave it. Taking a profile from a TV, or removing \
                 the TV, gives that profile and its lists new keys, so the TV can't open what changes after."
            }
            (problem())
            @if props.devices.is_empty() { p { "No TVs yet." } }
            ul .rows {
                @for device in &props.devices {
                    @let entry = manifest.devices.iter().find(|item| item.id == device.id);
                    @let signed_in = device.signed_out_at.is_none();
                    li {
                        div .row-head {
                            strong data-device-row=(device.id) { (device_name(&device.id)) }
                            span .note { (device_status(device)) }
                        }
                        @if let Some(entry) = entry {
                            p .note {
                                @for (index, profile_id) in entry.profiles.iter().enumerate() {
                                    (if index > 0 { ", " } else { "" })
                                    (profile_name(profile_id))
                                }
                            }
                            details {
                                summary { (if signed_in { "Change" } else { "Remove" }) }
                                @if signed_in {
                                    form hx-post="/changes" hx-target="#main" hx-confirm="velio"
                                        data-velio="device-profiles" data-device-id=(device.id) {
                                        input type="hidden" name="next" value="/devices";
                                        fieldset {
                                            legend { "Profiles on this TV" }
                                            @for profile in &manifest.profiles {
                                                label .check {
                                                    input type="checkbox" data-profile-choice=(profile.id)
                                                        checked[entry.profiles.contains(&profile.id)];
                                                    (profile_pic(&profile.id))
                                                    (profile_name(&profile.id))
                                                }
                                            }
                                        }
                                        p .problem role="alert" {}
                                        button type="submit" { "Save" }
                                    }
                                }
                                form hx-post="/changes" hx-target="#main" hx-confirm="velio"
                                    data-velio="device-remove" data-device-id=(device.id) {
                                    input type="hidden" name="next" value="/devices";
                                    p .note {
                                        "The TV signs out the next time it syncs: within seconds when it's on. \
                                         To use it again, sign it in again."
                                    }
                                    p .problem role="alert" {}
                                    button type="submit" .danger { "Remove this TV" }
                                }
                            }
                        }
                    }
                }
            }
            p { a href="/pair" .button { "Sign in a TV" } }
        },
    )
}
