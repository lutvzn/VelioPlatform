//! The routes for profiles, lists and TVs (the Worker's `routes/vault.tsx`;
//! PLAN.md, "Routes"). Pages carry the account's ciphertext; vault.js opens
//! it and sends changes, sealed and signed, to POST /changes. The answer is
//! the next page, built from what the account holds after the change.

use anyhow::anyhow;
use axum::Router;
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use maud::Markup;
use serde_json::Value;

use crate::AppState;
use crate::accounts::{Fields, auth_hash_of, find_account_by_id};
use crate::error::AppError;
use crate::limits::Limit;
use crate::sync_core::{ChangeProblem, ProblemCode, is_id, stored_manifest};
use crate::tokens::same_digest;
use crate::vault::{self, NO_KEYS};
use crate::views::{pages, vault as views};
use crate::web::{Form, TOO_MANY, Web, found, fragment, page_fragment};

type Answer = Result<Response, AppError>;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/profiles", get(show_page))
        .route("/devices", get(show_page))
        .route("/lists", get(show_page))
        .route("/lists/new", get(show_page))
        .route("/lists/{id}", get(show_page))
        .route("/changes", post(change))
        .route("/unlock", post(unlock))
}

/// 50 lists of at most 48 KB each, plus keys and the manifest.
pub(crate) const MAX_CHANGE_LENGTH: usize = 3_000_000;

/// The pages a change can go on to.
enum Page {
    Profiles,
    Devices,
    Lists,
    NewList,
    /// A list's own page, only while the list exists.
    List(String),
}

fn page_for(path: &str, props: &views::VaultProps) -> Option<Page> {
    match path {
        "/profiles" => return Some(Page::Profiles),
        "/devices" => return Some(Page::Devices),
        "/lists" => return Some(Page::Lists),
        "/lists/new" => return Some(Page::NewList),
        _ => {}
    }
    let list_id = path.strip_prefix("/lists/").filter(|id| !id.is_empty() && !id.contains('/'))?;
    (is_id(list_id) && props.manifest.lists.iter().any(|list| list.id == list_id))
        .then(|| Page::List(list_id.to_owned()))
}

impl Page {
    fn title(&self) -> &'static str {
        match self {
            Page::Profiles => "Profiles",
            Page::Devices => "TVs",
            Page::Lists => "Lists",
            Page::NewList => "Add a list",
            Page::List(_) => "Change the list",
        }
    }

    fn render(&self, props: &views::VaultProps) -> Markup {
        match self {
            Page::Profiles => views::profiles_page(props),
            Page::Devices => views::devices_page(props),
            Page::Lists => views::lists_page(props),
            Page::NewList => views::list_page(props, None),
            Page::List(id) => views::list_page(props, Some(id)),
        }
    }
}

/// What the pages need from an account, or None when it has no owner keys.
pub(crate) async fn vault_props(
    web: &Web,
    account_id: &str,
    email: &str,
) -> Result<Option<views::VaultProps>, AppError> {
    let id = account_id.to_owned();
    let (owner, stored, devices) = web
        .state
        .db
        .call(move |conn| {
            Ok((vault::get_owner(conn, &id)?, vault::get_state(conn, &id)?, vault::devices_of(conn, &id)?))
        })
        .await?;
    let Some(owner) = owner else { return Ok(None) };
    let manifest = stored_manifest(&stored, account_id).map_err(|problem| anyhow!(problem))?;
    Ok(Some(views::VaultProps {
        account_id: account_id.to_owned(),
        email: email.to_owned(),
        sign_public_key: owner.sign_public_key,
        stored,
        manifest,
        devices,
    }))
}

async fn show_page(web: Web, uri: Uri) -> Answer {
    let Some(session) = web.session.clone() else { return Ok(found("/signin")) };
    let Some(props) = vault_props(&web, &session.account_id, &session.email).await? else {
        return Ok(web.problem(NO_KEYS, StatusCode::INTERNAL_SERVER_ERROR));
    };
    let Some(page) = page_for(uri.path(), &props) else {
        return Ok(web.page(StatusCode::NOT_FOUND, "Not found", pages::not_found()));
    };
    Ok(web.page(StatusCode::OK, page.title(), page.render(&props)))
}

/// Why a change was refused, in words for the owner.
pub(crate) fn refusal_text(problem: &ChangeProblem) -> String {
    match problem.code {
        ProblemCode::Behind => "Your lists were changed in another tab or on another device. Reload the page to \
                                see the changes, then make yours again."
            .into(),
        ProblemCode::Limit => format!("That's past the limits for an account: {}.", problem.message),
        ProblemCode::Refused => "Velio couldn't take this change. Reload the page and try again.".into(),
    }
}

/// 409 when another tab or device changed the account first, else 400.
pub(crate) fn refusal_status(problem: &ChangeProblem) -> StatusCode {
    if problem.code == ProblemCode::Behind { StatusCode::CONFLICT } else { StatusCode::BAD_REQUEST }
}

async fn change(web: Web, form: Form) -> Answer {
    let Some(session) = web.session.clone() else {
        let problem = "You were signed out. Sign in again, then make the change again.";
        return Ok(web.problem(problem, StatusCode::UNAUTHORIZED));
    };
    let text = form.field("change");
    // As JavaScript counts a string's length.
    if text.encode_utf16().count() > MAX_CHANGE_LENGTH {
        return Ok(web.problem("This change is too big.", StatusCode::PAYLOAD_TOO_LARGE));
    }
    let Ok(input) = serde_json::from_str::<Value>(text) else {
        let problem = "This browser didn't send a readable change. Reload the page and try again.";
        return Ok(web.problem(problem, StatusCode::BAD_REQUEST));
    };

    let (id, tv_limits) = (session.account_id.clone(), web.state.cfg.tv_limits);
    let result = web.state.db.call(move |conn| vault::change(conn, &id, &input, &[], tv_limits)).await?;
    let changed = match result {
        Ok(changed) => changed,
        Err(problem) => {
            if problem.code == ProblemCode::Refused {
                tracing::warn!("Refused a change for {}: {}", session.account_id, problem.message);
            }
            return Ok(web.problem(&refusal_text(&problem), refusal_status(&problem)));
        }
    };
    web.state.sockets.notify(&session.account_id, changed.rev, &changed.devices);

    let Some(props) = vault_props(&web, &session.account_id, &session.email).await? else {
        return Ok(web.problem(NO_KEYS, StatusCode::INTERNAL_SERVER_ERROR));
    };
    // The page the form asked for, or the lists, such as after deleting the list being changed.
    let asked = form.field("next");
    let page = page_for(asked, &props).unwrap_or(Page::Lists);
    let path = if matches!(page, Page::Lists) { "/lists" } else { asked };
    let location = HeaderValue::from_str(path).map_err(|error| anyhow!(error))?;
    if !web.htmx {
        return Ok((StatusCode::SEE_OTHER, [(header::LOCATION, location)]).into_response());
    }
    let mut response = page_fragment(page.title(), page.render(&props));
    response.headers_mut().insert("HX-Push-Url", location);
    Ok(response)
}

/// Opens a signed-in tab's keys again: the same check as signing in, without
/// a new session. Answers the owner bundle, for vault.js to open.
async fn unlock(web: Web, form: Form) -> Answer {
    const TARGET: &str = "#unlock-problem";
    let Some(session) = web.session.clone() else {
        return Ok(web.problem_at("You were signed out. Sign in again.", StatusCode::UNAUTHORIZED, TARGET));
    };
    if !web.within(Limit::Auth, &format!("unlock:{}", web.ip))
        || !web.within(Limit::Email, &format!("signin:{}", session.email))
    {
        return Ok(web.problem_at(TOO_MANY, StatusCode::TOO_MANY_REQUESTS, TARGET));
    }
    let Some(auth_hash) = auth_hash_of(form.field("auth_key")) else {
        let problem = "This browser didn't send a key. Reload the page and try again.";
        return Ok(web.problem_at(problem, StatusCode::BAD_REQUEST, TARGET));
    };
    let id = session.account_id.clone();
    let (account, owner) =
        web.state.db.call(move |conn| Ok((find_account_by_id(conn, &id)?, vault::get_owner(conn, &id)?))).await?;
    if !account.is_some_and(|account| same_digest(&account.auth_hash, &auth_hash)) {
        return Ok(web.problem_at("That isn't the password for this account.", StatusCode::UNAUTHORIZED, TARGET));
    }
    let Some(owner) = owner else {
        return Ok(web.problem_at(NO_KEYS, StatusCode::INTERNAL_SERVER_ERROR, TARGET));
    };
    Ok(fragment(views::owner_bundle(&owner)))
}
