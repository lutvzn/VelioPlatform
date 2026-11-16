//! Profiles and their add-ons, through Nuvio's own functions and tables, as
//! the signed-in user, so Nuvio's policies decide:
//!
//!   GET  /profiles                               sync_pull_profiles, and public.addons
//!   POST /profiles                               a new profile: sync_push_profiles, at the first free index
//!   POST /profiles/{index}                       sync_patch_profile
//!   POST /profiles/{index}/pin                   set_profile_pin or clear_profile_pin
//!   POST /profiles/{index}/delete                sync_delete_profile_data (not the main profile)
//!   POST /profiles/{index}/addons                adds a row to public.addons
//!   POST /profiles/{index}/addons/{id}/delete    deletes one

use axum::Router;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use serde_json::json;
use uuid::Uuid;

use crate::AppState;
use crate::accounts::Fields;
use crate::error::AppError;
use crate::views::profiles::{
    self as views, AddonRow, MAX_NAME_LENGTH, MAX_PROFILES, MAX_URL_LENGTH, PROFILE_COLORS, ProfileRow,
};
use crate::web::{Form, Web, sign_in_first};

type Answer = Result<Response, AppError>;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/profiles", get(profiles).post(add))
        .route("/profiles/{index}", post(change))
        .route("/profiles/{index}/pin", post(set_pin))
        .route("/profiles/{index}/delete", post(delete))
        .route("/profiles/{index}/addons", post(add_addon))
        .route("/profiles/{index}/addons/{id}/delete", post(delete_addon))
}

/// The user's profiles, with each one's add-ons.
async fn profile_rows(web: &Web) -> Result<Result<Vec<ProfileRow>, String>, AppError> {
    web.as_user(async |tx| {
        let rows = tx
            .query(
                "SELECT profile_index, name, avatar_color_hex, uses_primary_addons, pin_enabled
                   FROM public.sync_pull_profiles() ORDER BY profile_index",
                &[],
            )
            .await?;
        let addons = tx
            .query(
                "SELECT profile_id, id, url, name, enabled FROM public.addons
                  WHERE user_id = auth.uid() ORDER BY profile_id, sort_order, created_at",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|row| {
                let index: i32 = row.get(0);
                ProfileRow {
                    index,
                    name: row.get(1),
                    color: row.get(2),
                    uses_primary_addons: row.get(3),
                    pin_enabled: row.get(4),
                    addons: addons
                        .iter()
                        .filter(|addon| addon.get::<_, i32>(0) == index)
                        .map(|addon| AddonRow {
                            id: addon.get::<_, Uuid>(1).to_string(),
                            url: addon.get(2),
                            name: addon.get(3),
                            enabled: addon.get(4),
                        })
                        .collect(),
                }
            })
            .collect())
    })
    .await
}

/// The page, whole for GET, or for #main after a change.
async fn answer(web: &Web, notice: Option<&str>) -> Answer {
    if web.session.is_none() {
        return Ok(sign_in_first("/profiles"));
    }
    let profiles = match profile_rows(web).await? {
        Ok(profiles) => profiles,
        Err(problem) => return Ok(web.problem(&problem, StatusCode::BAD_REQUEST)),
    };
    let body = views::profiles_page(&profiles, notice);
    if notice.is_some() { Ok(web.answer("Profiles", body)) } else { Ok(web.page(StatusCode::OK, "Profiles", body)) }
}

/// After a change: the page with its notice, or the database's refusal in the form's problem.
async fn after(web: &Web, done: Result<(), String>, notice: &str) -> Answer {
    match done {
        Ok(()) => answer(web, Some(notice)).await,
        Err(problem) => Ok(web.problem(&problem, StatusCode::BAD_REQUEST)),
    }
}

async fn profiles(web: Web) -> Answer {
    answer(&web, None).await
}

fn profile_name(form: &Form) -> Result<String, &'static str> {
    let name = form.field("name").trim();
    if name.is_empty() {
        return Err("Give the profile a name.");
    }
    if name.chars().count() > MAX_NAME_LENGTH {
        return Err("Use a shorter name.");
    }
    Ok(name.to_owned())
}

/// A colour of the picker's; anything else keeps what the profile has.
fn chosen_color(form: &Form) -> Option<&'static str> {
    let color = form.field("color");
    PROFILE_COLORS.iter().map(|(_, hex)| *hex).find(|hex| hex.eq_ignore_ascii_case(color))
}

fn signed_in(web: &Web) -> Option<Response> {
    web.session.is_none().then(|| web.problem("Sign in first.", StatusCode::UNAUTHORIZED))
}

async fn add(web: Web, form: Form) -> Answer {
    if let Some(response) = signed_in(&web) {
        return Ok(response);
    }
    let name = match profile_name(&form) {
        Ok(name) => name,
        Err(problem) => return Ok(web.problem(problem, StatusCode::BAD_REQUEST)),
    };
    let added = web
        .as_user(async |tx| {
            let taken: Vec<i32> = tx
                .query("SELECT profile_index FROM public.sync_pull_profiles()", &[])
                .await?
                .iter()
                .map(|row| row.get(0))
                .collect();
            let Some(index) = (1..=MAX_PROFILES).find(|index| !taken.contains(index)) else { return Ok(false) };
            let color = PROFILE_COLORS[(index as usize - 1) % PROFILE_COLORS.len()].1;
            let profile = json!([{ "profile_index": index, "name": name, "avatar_color_hex": color }]);
            tx.execute("SELECT public.sync_push_profiles($1, $2)", &[&profile, &MAX_PROFILES]).await?;
            Ok(true)
        })
        .await?;
    match added {
        Ok(true) => answer(&web, Some("The profile is added.")).await,
        Ok(false) => Ok(web.problem("An account has at most 6 profiles.", StatusCode::CONFLICT)),
        Err(problem) => Ok(web.problem(&problem, StatusCode::BAD_REQUEST)),
    }
}

fn profile_index(index: &str) -> Option<i32> {
    index.parse().ok().filter(|index| (1..=MAX_PROFILES).contains(index))
}

async fn change(web: Web, Path(index): Path<String>, form: Form) -> Answer {
    if let Some(response) = signed_in(&web) {
        return Ok(response);
    }
    let Some(index) = profile_index(&index) else {
        return Ok(web.problem("There's no such profile.", StatusCode::NOT_FOUND));
    };
    let name = match profile_name(&form) {
        Ok(name) => name,
        Err(problem) => return Ok(web.problem(problem, StatusCode::BAD_REQUEST)),
    };
    let color = chosen_color(&form);
    let uses_primary = (index > 1).then(|| form.field("uses_primary_addons") == "yes");
    let done = web
        .as_user(async |tx| {
            tx.execute(
                "SELECT public.sync_patch_profile(p_profile_id => $1, p_name => $2, p_avatar_color_hex => $3,
                     p_uses_primary_addons => $4)",
                &[&index, &name, &color, &uses_primary],
            )
            .await?;
            Ok(())
        })
        .await?;
    after(&web, done, "The profile is saved.").await
}

async fn set_pin(web: Web, Path(index): Path<String>, form: Form) -> Answer {
    if let Some(response) = signed_in(&web) {
        return Ok(response);
    }
    let Some(index) = profile_index(&index) else {
        return Ok(web.problem("There's no such profile.", StatusCode::NOT_FOUND));
    };
    let pin = form.field("pin").trim().to_owned();
    let current = Some(form.field("current_pin").trim().to_owned()).filter(|pin| !pin.is_empty());
    if !pin.is_empty() && !(pin.len() == 4 && pin.chars().all(|c| c.is_ascii_digit())) {
        return Ok(web.problem("A PIN is 4 digits.", StatusCode::BAD_REQUEST));
    }
    let done = web
        .as_user(async |tx| {
            if pin.is_empty() {
                tx.execute("SELECT public.clear_profile_pin($1, $2)", &[&index, &current]).await?;
            } else {
                tx.execute("SELECT public.set_profile_pin($1, $2, $3)", &[&index, &pin, &current]).await?;
            }
            Ok(())
        })
        .await?;
    after(&web, done, if pin.is_empty() { "The profile has no PIN now." } else { "The PIN is saved." }).await
}

async fn delete(web: Web, Path(index): Path<String>) -> Answer {
    if let Some(response) = signed_in(&web) {
        return Ok(response);
    }
    let Some(index) = profile_index(&index).filter(|index| *index > 1) else {
        return Ok(web.problem("The main profile can't be deleted.", StatusCode::BAD_REQUEST));
    };
    let done = web
        .as_user(async |tx| {
            tx.execute("SELECT public.sync_delete_profile_data($1)", &[&index]).await?;
            // Velio's lists given to it alone go to every profile, as they would with no profiles left.
            tx.execute(
                "UPDATE public.velio_iptv_lists SET profile_ids = array_remove(profile_ids, $1)
                  WHERE user_id = auth.uid() AND $1 = ANY (profile_ids)",
                &[&index],
            )
            .await?;
            Ok(())
        })
        .await?;
    after(&web, done, "The profile is deleted.").await
}

/// An add-on's address as Nuvio keeps it: https, with stremio:// read as https://.
pub fn addon_url(url: &str) -> Option<String> {
    let url = url.trim();
    let url = match url.strip_prefix("stremio://") {
        Some(rest) => format!("https://{rest}"),
        None => url.to_owned(),
    };
    let parsed = url::Url::parse(&url).ok()?;
    let fits =
        matches!(parsed.scheme(), "https" | "http") && parsed.host_str().is_some() && url.len() <= MAX_URL_LENGTH;
    fits.then_some(url)
}

async fn add_addon(web: Web, Path(index): Path<String>, form: Form) -> Answer {
    if let Some(response) = signed_in(&web) {
        return Ok(response);
    }
    let Some(index) = profile_index(&index) else {
        return Ok(web.problem("There's no such profile.", StatusCode::NOT_FOUND));
    };
    let Some(url) = addon_url(form.field("url")) else {
        return Ok(web.problem("That isn't an add-on's address. It starts with https://.", StatusCode::BAD_REQUEST));
    };
    let done = web
        .as_user(async |tx| {
            tx.execute(
                "INSERT INTO public.addons (user_id, url, profile_id, sort_order)
                 SELECT auth.uid(), $1, $2, coalesce(max(sort_order) + 1, 0)
                   FROM public.addons WHERE user_id = auth.uid() AND profile_id = $2",
                &[&url, &index],
            )
            .await?;
            Ok(())
        })
        .await?;
    after(&web, done, "The add-on is added.").await
}

async fn delete_addon(web: Web, Path((index, id)): Path<(String, String)>) -> Answer {
    if let Some(response) = signed_in(&web) {
        return Ok(response);
    }
    let (Some(index), Ok(id)) = (profile_index(&index), Uuid::parse_str(&id)) else {
        return Ok(web.problem("There's no such add-on.", StatusCode::NOT_FOUND));
    };
    let done = web
        .as_user(async |tx| {
            tx.execute("DELETE FROM public.addons WHERE id = $1 AND profile_id = $2", &[&id, &index]).await?;
            Ok(())
        })
        .await?;
    after(&web, done, "The add-on is removed.").await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_addon_addresses() {
        assert_eq!(
            addon_url(" stremio://addon.example/manifest.json ").as_deref(),
            Some("https://addon.example/manifest.json")
        );
        assert_eq!(
            addon_url("https://addon.example/manifest.json").as_deref(),
            Some("https://addon.example/manifest.json")
        );
        assert_eq!(addon_url("javascript:alert(1)"), None);
        assert_eq!(addon_url("addon.example"), None);
    }

    #[test]
    fn takes_profile_indexes() {
        assert_eq!(profile_index("1"), Some(1));
        assert_eq!(profile_index("6"), Some(6));
        assert_eq!(profile_index("7"), None);
        assert_eq!(profile_index("x"), None);
    }
}
