//! IPTV lists, in public.velio_iptv_lists as the signed-in user, so the
//! table's policy (the owner's rows only) and checks decide, as they do for
//! the TV app through /rest/v1:
//!
//!   GET  /lists              the lists
//!   GET  /lists/new          the form for a new one
//!   POST /lists              adds one
//!   GET  /lists/{id}         the form for one
//!   POST /lists/{id}         saves it
//!   POST /lists/{id}/delete  deletes it

use axum::Router;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use deadpool_postgres::Transaction;
use tokio_postgres::Row;

use crate::AppState;
use crate::accounts::Fields;
use crate::error::AppError;
use crate::tokens::new_id;
use crate::views::lists::{
    self as views, DEFAULT_REFRESH_HOURS, ListRow, MAX_NAME_LENGTH, ProfileChoice, REFRESH_CHOICES,
};
use crate::views::profiles::MAX_URL_LENGTH;
use crate::web::{Form, Web, sign_in_first};

type Answer = Result<Response, AppError>;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/lists", get(lists).post(add))
        .route("/lists/new", get(new_list))
        .route("/lists/{id}", get(show).post(save))
        .route("/lists/{id}/delete", post(delete))
}

const COLUMNS: &str =
    "id, name, kind, url, server, username, password, guide_urls, refresh_hours, disabled, profile_ids";

fn list_row(row: &Row) -> ListRow {
    ListRow {
        id: row.get(0),
        name: row.get(1),
        kind: row.get(2),
        url: row.get(3),
        server: row.get(4),
        username: row.get(5),
        password: row.get(6),
        guide_urls: row.get(7),
        refresh_hours: row.get(8),
        disabled: row.get(9),
        profile_ids: row.get(10),
    }
}

async fn profile_choices(tx: &Transaction<'_>) -> Result<Vec<ProfileChoice>, tokio_postgres::Error> {
    let rows =
        tx.query("SELECT profile_index, name FROM public.sync_pull_profiles() ORDER BY profile_index", &[]).await?;
    Ok(rows.iter().map(|row| ProfileChoice { index: row.get(0), name: row.get(1) }).collect())
}

/// The lists page, whole for GET, or for #main after a change.
async fn lists_answer(web: &Web, notice: Option<&str>) -> Answer {
    if web.session.is_none() {
        return Ok(sign_in_first("/lists"));
    }
    let found = web
        .as_user(async |tx| {
            let rows = tx
                .query(
                    &format!("SELECT {COLUMNS} FROM public.velio_iptv_lists ORDER BY sort_order, created_at, id"),
                    &[],
                )
                .await?;
            Ok((rows.iter().map(list_row).collect::<Vec<_>>(), profile_choices(tx).await?))
        })
        .await?;
    let (lists, profiles) = match found {
        Ok(found) => found,
        Err(problem) => return Ok(web.problem(&problem, StatusCode::BAD_REQUEST)),
    };
    let body = views::lists_page(&lists, &profiles, notice);
    if notice.is_some() { Ok(web.answer("Lists", body)) } else { Ok(web.page(StatusCode::OK, "Lists", body)) }
}

async fn lists(web: Web) -> Answer {
    lists_answer(&web, None).await
}

async fn new_list(web: Web) -> Answer {
    if web.session.is_none() {
        return Ok(sign_in_first("/lists/new"));
    }
    let profiles = match web.as_user(async |tx| profile_choices(tx).await).await? {
        Ok(profiles) => profiles,
        Err(problem) => return Ok(web.problem(&problem, StatusCode::BAD_REQUEST)),
    };
    Ok(web.page(StatusCode::OK, "Add a list", views::list_page(None, &profiles)))
}

async fn show(web: Web, Path(id): Path<String>) -> Answer {
    if web.session.is_none() {
        return Ok(sign_in_first("/lists"));
    }
    let found = web
        .as_user(async |tx| {
            let row =
                tx.query_opt(&format!("SELECT {COLUMNS} FROM public.velio_iptv_lists WHERE id = $1"), &[&id]).await?;
            Ok((row.as_ref().map(list_row), profile_choices(tx).await?))
        })
        .await?;
    match found {
        Ok((Some(list), profiles)) => {
            Ok(web.page(StatusCode::OK, "Change the list", views::list_page(Some(&list), &profiles)))
        }
        Ok((None, _)) => Ok(web.page(StatusCode::NOT_FOUND, "Not found", crate::views::pages::not_found())),
        Err(problem) => Ok(web.problem(&problem, StatusCode::BAD_REQUEST)),
    }
}

fn web_address(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
}

/// The host of an address, for a list with no name.
fn host_of(value: &str) -> Option<String> {
    url::Url::parse(value).ok()?.host_str().map(str::to_owned)
}

/// The list the form describes, checked; the id is set by the caller.
fn read_list(form: &Form) -> Result<ListRow, &'static str> {
    let kind = match form.field("kind") {
        "xtream" => "xtream",
        "m3u" => "m3u",
        _ => return Err("Choose the kind of list."),
    };
    let field = |name: &str| Some(form.field(name).trim().to_owned()).filter(|value| !value.is_empty());
    let mut list = ListRow { kind: kind.into(), ..ListRow::default() };
    if kind == "m3u" {
        let url = field("url").ok_or("Enter the playlist's address.")?;
        if !web_address(&url) || url.len() > MAX_URL_LENGTH {
            return Err("The playlist's address starts with http:// or https://.");
        }
        list.url = Some(url);
    } else {
        let server = field("server").ok_or("Enter the server.")?;
        let server = if server.contains("://") { server } else { format!("http://{server}") };
        if !web_address(&server) || server.len() > MAX_URL_LENGTH {
            return Err("The server is an address, such as http://example.com:8080.");
        }
        list.username = Some(field("username").ok_or("Enter the username.")?);
        // Xtream passwords can have spaces at the ends, so the password is kept as typed.
        let password = form.field("password");
        if password.is_empty() {
            return Err("Enter the password.");
        }
        list.password = Some(password.to_owned());
        if list.username.as_ref().is_some_and(|name| name.len() > 256) || password.len() > 256 {
            return Err("The username or password is too long.");
        }
        list.server = Some(server);
    }
    let address = list.url.as_deref().or(list.server.as_deref()).unwrap_or_default();
    list.name = field("name").or_else(|| host_of(address)).unwrap_or_else(|| "IPTV".into());
    if list.name.chars().count() > MAX_NAME_LENGTH {
        return Err("Use a shorter name.");
    }
    list.guide_urls =
        form.field("guide_urls").lines().map(str::trim).filter(|line| !line.is_empty()).map(str::to_owned).collect();
    if list.guide_urls.len() > 10 {
        return Err("A list has at most 10 guide addresses.");
    }
    if list.guide_urls.iter().any(|guide| !web_address(guide) || guide.len() > MAX_URL_LENGTH) {
        return Err("A guide address starts with http:// or https://.");
    }
    list.refresh_hours = form.field("refresh_hours").parse().unwrap_or(DEFAULT_REFRESH_HOURS);
    if !REFRESH_CHOICES.contains(&list.refresh_hours) {
        list.refresh_hours = DEFAULT_REFRESH_HOURS;
    }
    list.disabled = form.field("disabled") == "yes";
    list.profile_ids = form.all("profile").iter().filter_map(|index| index.parse().ok()).collect();
    list.profile_ids.sort_unstable();
    list.profile_ids.dedup();
    Ok(list)
}

async fn add(web: Web, form: Form) -> Answer {
    if web.session.is_none() {
        return Ok(web.problem("Sign in first.", StatusCode::UNAUTHORIZED));
    }
    let list = match read_list(&form) {
        Ok(list) => list,
        Err(problem) => return Ok(web.problem(problem, StatusCode::BAD_REQUEST)),
    };
    let id = new_id();
    let done = web
        .as_user(async |tx| {
            tx.execute(
                "INSERT INTO public.velio_iptv_lists
                     (user_id, id, name, kind, url, server, username, password, guide_urls, refresh_hours, disabled,
                      profile_ids, sort_order)
                 SELECT auth.uid(), $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11,
                        coalesce((SELECT max(sort_order) + 1 FROM public.velio_iptv_lists), 0)",
                &[
                    &id,
                    &list.name,
                    &list.kind,
                    &list.url,
                    &list.server,
                    &list.username,
                    &list.password,
                    &list.guide_urls,
                    &list.refresh_hours,
                    &list.disabled,
                    &list.profile_ids,
                ],
            )
            .await?;
            Ok(())
        })
        .await?;
    match done {
        Ok(()) => lists_answer(&web, Some("The list is added.")).await,
        Err(problem) => Ok(web.problem(&problem, StatusCode::BAD_REQUEST)),
    }
}

async fn save(web: Web, Path(id): Path<String>, form: Form) -> Answer {
    if web.session.is_none() {
        return Ok(web.problem("Sign in first.", StatusCode::UNAUTHORIZED));
    }
    let list = match read_list(&form) {
        Ok(list) => list,
        Err(problem) => return Ok(web.problem(problem, StatusCode::BAD_REQUEST)),
    };
    let done = web
        .as_user(async |tx| {
            tx.execute(
                "UPDATE public.velio_iptv_lists
                    SET name = $2, kind = $3, url = $4, server = $5, username = $6, password = $7, guide_urls = $8,
                        refresh_hours = $9, disabled = $10, profile_ids = $11
                  WHERE id = $1",
                &[
                    &id,
                    &list.name,
                    &list.kind,
                    &list.url,
                    &list.server,
                    &list.username,
                    &list.password,
                    &list.guide_urls,
                    &list.refresh_hours,
                    &list.disabled,
                    &list.profile_ids,
                ],
            )
            .await
        })
        .await?;
    match done {
        Ok(0) => Ok(web.problem("That list isn't there any more.", StatusCode::NOT_FOUND)),
        Ok(_) => lists_answer(&web, Some("The list is saved.")).await,
        Err(problem) => Ok(web.problem(&problem, StatusCode::BAD_REQUEST)),
    }
}

async fn delete(web: Web, Path(id): Path<String>) -> Answer {
    if web.session.is_none() {
        return Ok(web.problem("Sign in first.", StatusCode::UNAUTHORIZED));
    }
    let done =
        web.as_user(async |tx| tx.execute("DELETE FROM public.velio_iptv_lists WHERE id = $1", &[&id]).await).await?;
    match done {
        Ok(_) => {
            let mut response = lists_answer(&web, Some("The list is deleted.")).await?;
            if web.htmx {
                crate::web::set_header(&mut response, "HX-Push-Url", "/lists");
            }
            Ok(response)
        }
        Err(problem) => Ok(web.problem(&problem, StatusCode::BAD_REQUEST)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(fields: &[(&str, &str)]) -> Form {
        let body: String = form_urlencoded::Serializer::new(String::new()).extend_pairs(fields).finish();
        Form::parse(body.as_bytes())
    }

    #[test]
    fn reads_an_m3u_list() {
        let list = read_list(&form(&[
            ("kind", "m3u"),
            ("url", " https://iptv.example/list.m3u "),
            ("guide_urls", "https://iptv.example/guide.xml\n\n"),
            ("profile", "2"),
            ("profile", "1"),
            ("refresh_hours", "12"),
        ]))
        .unwrap();
        assert_eq!(list.url.as_deref(), Some("https://iptv.example/list.m3u"));
        assert_eq!(list.name, "iptv.example");
        assert_eq!(list.guide_urls, ["https://iptv.example/guide.xml"]);
        assert_eq!(list.profile_ids, [1, 2]);
        assert_eq!(list.refresh_hours, 12);
    }

    #[test]
    fn reads_an_xtream_login() {
        let list = read_list(&form(&[
            ("kind", "xtream"),
            ("server", "iptv.example:8080"),
            ("username", "someone"),
            ("password", " pass "),
            ("name", "Home"),
        ]))
        .unwrap();
        assert_eq!(list.server.as_deref(), Some("http://iptv.example:8080"));
        assert_eq!(list.password.as_deref(), Some(" pass "));
        assert_eq!(list.name, "Home");
        assert!(read_list(&form(&[("kind", "xtream"), ("server", "x.example")])).is_err());
        assert!(read_list(&form(&[("kind", "m3u"), ("url", "ftp://x.example/a")])).is_err());
    }
}
