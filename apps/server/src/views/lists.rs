//! IPTV lists: M3U playlists and Xtream logins (public.velio_iptv_lists),
//! each given to every profile or to some. Stremio add-ons are Nuvio's own,
//! on the profiles page.

use maud::{Markup, html};

use super::accounts::problem;
use super::profiles::MAX_URL_LENGTH;

pub const MAX_NAME_LENGTH: usize = 100;
pub const REFRESH_CHOICES: [i32; 5] = [6, 12, 24, 48, 168];
pub const DEFAULT_REFRESH_HOURS: i32 = 24;

#[derive(Default)]
pub struct ListRow {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub url: Option<String>,
    pub server: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub guide_urls: Vec<String>,
    pub refresh_hours: i32,
    pub disabled: bool,
    pub profile_ids: Vec<i32>,
}

/// A profile to give lists to.
pub struct ProfileChoice {
    pub index: i32,
    pub name: String,
}

fn for_profiles(list: &ListRow, profiles: &[ProfileChoice]) -> String {
    if list.profile_ids.is_empty() {
        return "For every profile".into();
    }
    let names: Vec<&str> = profiles
        .iter()
        .filter(|profile| list.profile_ids.contains(&profile.index))
        .map(|profile| profile.name.as_str())
        .collect();
    format!("For {}", names.join(", "))
}

fn summary(list: &ListRow) -> String {
    let kind = if list.kind == "xtream" { "Xtream login" } else { "M3U playlist" };
    if list.disabled { format!("{kind}, turned off") } else { kind.to_owned() }
}

pub fn lists_page(lists: &[ListRow], profiles: &[ProfileChoice], notice: Option<&str>) -> Markup {
    html! {
        section {
            h1 { "Lists" }
            @if let Some(notice) = notice { p .done role="status" { (notice) } }
            p .note {
                "The M3U playlists and Xtream logins of your own subscriptions, for Velio's TV app. Stremio add-ons \
                 are on each " a href="/profiles" { "profile" } "."
            }
            @if lists.is_empty() { p { "No lists yet." } }
            ul .rows {
                @for list in lists {
                    li {
                        div .row-head {
                            a href={ "/lists/" (list.id) } { strong { (list.name) } }
                            span .note { (summary(list)) }
                        }
                        p .note { (for_profiles(list, profiles)) }
                    }
                }
            }
            p { a href="/lists/new" .button { "Add a list" } }
        }
    }
}

fn refresh_label(hours: i32) -> String {
    match hours {
        hours if hours < 24 => format!("Every {hours} hours"),
        24 => "Every day".into(),
        168 => "Every week".into(),
        hours => format!("Every {} days", hours / 24),
    }
}

fn text_field(name: &str, label: &str, kind: &str, value: Option<&str>, placeholder: Option<&str>) -> Markup {
    html! {
        label {
            (label)
            input type=(kind) name=(name) value=[value] maxlength=(MAX_URL_LENGTH) placeholder=[placeholder]
                autocomplete="off" spellcheck="false";
        }
    }
}

/// Adding a list (none) or changing one.
pub fn list_page(list: Option<&ListRow>, profiles: &[ProfileChoice]) -> Markup {
    let blank = ListRow { kind: "m3u".into(), refresh_hours: DEFAULT_REFRESH_HOURS, ..ListRow::default() };
    let shown = list.unwrap_or(&blank);
    let action = match list {
        Some(list) => format!("/lists/{}", list.id),
        None => "/lists".into(),
    };
    let xtream = shown.kind == "xtream";
    html! {
        section {
            h1 { (if list.is_some() { "Change the list" } else { "Add a list" }) }
            form method="post" action=(action) hx-post=(action) hx-target="#main" data-list-form {
                label {
                    "Kind"
                    select name="kind" {
                        option value="m3u" selected[!xtream] { "M3U playlist" }
                        option value="xtream" selected[xtream] { "Xtream login" }
                    }
                }
                fieldset data-kind-only="m3u" hidden[xtream] {
                    (text_field("url", "Playlist address", "url", shown.url.as_deref(), Some("https://…")))
                }
                fieldset data-kind-only="xtream" hidden[!xtream] {
                    (text_field("server", "Server", "text", shown.server.as_deref(), Some("https://…")))
                    (text_field("username", "Username", "text", shown.username.as_deref(), None))
                    (text_field("password", "Password", "password", shown.password.as_deref(), None))
                }
                label {
                    "Name (optional)"
                    input type="text" name="name" value=(shown.name) maxlength=(MAX_NAME_LENGTH)
                        placeholder="The server's name" autocomplete="off";
                }
                label {
                    "Guide addresses (optional)"
                    textarea name="guide_urls" rows="2" spellcheck="false" placeholder="XMLTV addresses, one per line" {
                        (shown.guide_urls.join("\n"))
                    }
                }
                label {
                    "Refresh"
                    select name="refresh_hours" {
                        @for hours in REFRESH_CHOICES {
                            option value=(hours) selected[hours == shown.refresh_hours] { (refresh_label(hours)) }
                        }
                    }
                }
                fieldset {
                    legend { "Profiles that have it" }
                    @if profiles.is_empty() {
                        p .note { "Every profile has it. " a href="/profiles" { "Add profiles" } " to choose." }
                    } @else {
                        p .note { "None ticked gives it to every profile." }
                        @for profile in profiles {
                            label .check {
                                input type="checkbox" name="profile" value=(profile.index)
                                    checked[shown.profile_ids.contains(&profile.index)];
                                span { (profile.name) }
                            }
                        }
                    }
                }
                label .check {
                    input type="checkbox" name="disabled" value="yes" checked[shown.disabled];
                    span { "Turned off: TVs keep it but don't load it" }
                }
                (problem())
                button type="submit" { (if list.is_some() { "Save the list" } else { "Add the list" }) }
            }
            @if let Some(list) = list {
                form method="post" action={ "/lists/" (list.id) "/delete" } hx-post={ "/lists/" (list.id) "/delete" }
                    hx-target="#main" {
                    button type="submit" .danger { "Delete this list" }
                }
            }
            p { a href="/lists" { "Back to your lists" } }
        }
    }
}
