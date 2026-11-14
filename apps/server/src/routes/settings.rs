//! Account settings (the Worker's `routes/settings.tsx`; PLAN.md, "Keys"):
//! changing the password, the recovery key, and deleting the account; and,
//! from the reset link, setting a new password with the recovery key.
//!
//! Each setting needs the current password, which vault.js sends as its
//! auth key, so a stolen session cookie alone can't change who opens the
//! lists or delete them. The server never sees a password, the owner's keys
//! or the recovery key: only auth keys' hashes and sealed owner bundles. It
//! does check that a new bundle comes with the account's own public keys.

use anyhow::Result;
use axum::extract::Path;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use maud::html;
use rusqlite::{Connection, params};
use serde_json::{Value, json};

use crate::AppState;
use crate::accounts::{
    Credentials, Fields, LinkKind, Owner, auth_hash_of, find_account_by_id, peek_email_link, read_credentials,
    read_owner_bundle, use_email_link,
};
use crate::error::AppError;
use crate::http::api_problem;
use crate::limits::Limit;
use crate::routes::accounts::{CLOSED, LINK_GONE, account_answer, signed_in_answer};
use crate::session::{Session, ended_cookie, session_cookie, start_session};
use crate::time::now_ms;
use crate::tokens::same_digest;
use crate::vault::{self, NO_KEYS};
use crate::views::{layout, pages};
use crate::web::{Form, TOO_MANY, Web, page_fragment, set_header};

type Answer = Result<Response, AppError>;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/password", post(change_password))
        .route("/recovery", post(recovery))
        .route("/reset/{token}/recovery-bundle", post(recovery_bundle))
        .route("/reset/{token}/recover", post(recover))
        .route("/account/delete", post(delete_account))
}

const NOT_OUR_KEYS: &str = "These aren't the account's keys. Reload the page and try again.";
const NOT_THE_RECOVERY_KEY: &str = "That isn't this account's recovery key.";

/// The signed-in account, if `current_auth_key` is its password's; otherwise the problem to answer.
async fn check_password(web: &Web, form: &Form, target: &'static str) -> Result<Result<Session, Response>, AppError> {
    let Some(session) = web.session.clone() else {
        return Ok(Err(web.problem_at("Sign in again.", StatusCode::UNAUTHORIZED, target)));
    };
    if !web.within(Limit::Auth, &format!("settings:{}", web.ip))
        || !web.within(Limit::Email, &format!("settings:{}", session.account_id))
    {
        return Ok(Err(web.problem_at(TOO_MANY, StatusCode::TOO_MANY_REQUESTS, target)));
    }
    let id = session.account_id.clone();
    let account = web.state.db.call(move |conn| find_account_by_id(conn, &id)).await?;
    let auth_hash = auth_hash_of(form.field("current_auth_key"));
    match (account, auth_hash) {
        (Some(account), Some(auth_hash)) if same_digest(&account.auth_hash, &auth_hash) => Ok(Ok(session)),
        _ => Ok(Err(web.problem_at("That isn't your password.", StatusCode::UNAUTHORIZED, target))),
    }
}

fn same_keys(a: &Owner, b: &Owner) -> bool {
    a.sign_public_key == b.sign_public_key && a.box_public_key == b.box_public_key
}

/// A new password for the same owner keys, all at once: the owner bundle,
/// the salt, settings and auth hash; every session and email link ends,
/// and a new session starts. Answers its token.
fn set_password(conn: &mut Connection, account_id: &str, credentials: &Credentials) -> Result<String> {
    let tx = conn.transaction()?;
    vault::set_owner(&tx, account_id, &credentials.owner)?;
    let Credentials { salt, kdf, auth_hash, .. } = credentials;
    // Opening a reset link proves the email, so it confirms an unconfirmed account, as a reset does.
    tx.execute(
        "UPDATE accounts SET salt = ?, kdf_memory_kib = ?, kdf_iterations = ?, kdf_parallelism = ?, auth_hash = ?,
           confirmed_at = COALESCE(confirmed_at, ?) WHERE id = ?",
        params![salt, kdf.memory_kib, kdf.iterations, kdf.parallelism, auth_hash, now_ms(), account_id],
    )?;
    tx.execute("DELETE FROM sessions WHERE account_id = ?", [account_id])?;
    // Reset links sent before stop working.
    tx.execute("DELETE FROM email_links WHERE account_id = ?", [account_id])?;
    let token = start_session(&tx, account_id)?;
    tx.commit()?;
    Ok(token)
}

// --- Changing the password ---

enum NewPassword {
    NoKeys,
    NotOurKeys,
    Set(String),
}

async fn change_password(web: Web, form: Form) -> Answer {
    const TARGET: &str = "#password-problem";
    let session = match check_password(&web, &form, TARGET).await? {
        Ok(session) => session,
        Err(problem) => return Ok(problem),
    };
    let credentials = match read_credentials(&form) {
        Ok(credentials) => credentials,
        Err(problem) => return Ok(web.problem_at(&problem, StatusCode::BAD_REQUEST, TARGET)),
    };
    let id = session.account_id.clone();
    let outcome = web.state.db.call(move |conn| {
        let Some(before) = vault::get_owner(conn, &id)? else { return Ok(NewPassword::NoKeys) };
        if !same_keys(&before, &credentials.owner) {
            return Ok(NewPassword::NotOurKeys);
        }
        Ok(NewPassword::Set(set_password(conn, &id, &credentials)?))
    });
    let token = match outcome.await? {
        NewPassword::NoKeys => return Ok(web.problem_at(NO_KEYS, StatusCode::INTERNAL_SERVER_ERROR, TARGET)),
        NewPassword::NotOurKeys => return Ok(web.problem_at(NOT_OUR_KEYS, StatusCode::BAD_REQUEST, TARGET)),
        NewPassword::Set(token) => token,
    };
    let notice = "Your password is changed. Other browsers are signed out; your TVs stay signed in.";
    let mut response = account_answer(&web, &session.account_id, &session.email, Some(notice), false).await?;
    response.headers_mut().append(header::SET_COOKIE, session_cookie(&token));
    Ok(response)
}

// --- The recovery key ---

async fn recovery(web: Web, form: Form) -> Answer {
    let on = form.field("action") == "on";
    let target = if on { "#recovery-problem" } else { "#recovery-off-problem" };
    let session = match check_password(&web, &form, target).await? {
        Ok(session) => session,
        Err(problem) => return Ok(problem),
    };
    let id = session.account_id.clone();
    if !on {
        web.state.db.call(move |conn| vault::set_recovery(conn, &id, None)).await?;
        return account_answer(&web, &session.account_id, &session.email, Some("The recovery key is off."), false)
            .await;
    }
    let (Some(auth_hash), Some(bundle)) =
        (auth_hash_of(form.field("recovery_auth_key")), read_owner_bundle(form.field("recovery_bundle")))
    else {
        let problem = "This browser didn't send the recovery key's bundle.";
        return Ok(web.problem_at(problem, StatusCode::BAD_REQUEST, target));
    };
    web.state.db.call(move |conn| vault::set_recovery(conn, &id, Some((&auth_hash, &bundle)))).await?;
    account_answer(&web, &session.account_id, &session.email, Some("Your recovery key is on."), true).await
}

/// From the reset link, with the recovery key's auth key: the recovery copy
/// of the owner bundle, for vault.js to open and seal again under a new
/// password. Doesn't use the link.
async fn recovery_bundle(web: Web, Path(token): Path<String>, form: Form) -> Answer {
    let mut response = recovery_bundle_answer(&web, token, &form).await?;
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

async fn recovery_bundle_answer(web: &Web, token: String, form: &Form) -> Answer {
    if !web.within(Limit::Auth, &format!("recover:{}", web.ip)) {
        return Ok(api_problem(StatusCode::TOO_MANY_REQUESTS, TOO_MANY));
    }
    let linked = web.state.db.call(move |conn| peek_email_link(conn, &token, LinkKind::Reset)).await?;
    let Some(account_id) = linked else { return Ok(api_problem(StatusCode::NOT_FOUND, LINK_GONE)) };
    if !web.within(Limit::Email, &format!("recover:{account_id}")) {
        return Ok(api_problem(StatusCode::TOO_MANY_REQUESTS, TOO_MANY));
    }
    let owner = match auth_hash_of(form.field("recovery_auth_key")) {
        Some(auth_hash) => web.state.db.call(move |conn| vault::recovery_bundle(conn, &account_id, &auth_hash)).await?,
        None => None,
    };
    let Some(owner) = owner else { return Ok(api_problem(StatusCode::UNAUTHORIZED, NOT_THE_RECOVERY_KEY)) };
    let bundle: Value = serde_json::from_str(&owner.bundle)?;
    let answer =
        json!({ "bundle": bundle, "signPublicKey": owner.sign_public_key, "boxPublicKey": owner.box_public_key });
    Ok(Json(answer).into_response())
}

enum Recover {
    LinkGone,
    NotTheRecoveryKey,
    NotOurKeys,
    Closed,
    NoKeys,
    /// The account, its email, and the new session's token.
    Done(String, String, String),
}

/// The checks in the Worker's order, then the link is used and the password set, in one go.
fn recover_account(
    conn: &mut Connection,
    token: &str,
    recovery_auth_hash: Option<&str>,
    credentials: &Credentials,
) -> Result<Recover> {
    let Some(linked) = peek_email_link(conn, token, LinkKind::Reset)? else { return Ok(Recover::LinkGone) };
    let recovery = match recovery_auth_hash {
        Some(auth_hash) => vault::recovery_bundle(conn, &linked, auth_hash)?,
        None => None,
    };
    let Some(recovery) = recovery else { return Ok(Recover::NotTheRecoveryKey) };
    if !same_keys(&recovery, &credentials.owner) {
        return Ok(Recover::NotOurKeys);
    }
    if use_email_link(conn, token, LinkKind::Reset)?.as_deref() != Some(linked.as_str()) {
        return Ok(Recover::LinkGone);
    }
    let Some(account) = find_account_by_id(conn, &linked)?.filter(|account| account.closed_at.is_none()) else {
        return Ok(Recover::Closed);
    };
    if vault::get_owner(conn, &linked)?.is_none() {
        return Ok(Recover::NoKeys);
    }
    let session = set_password(conn, &linked, credentials)?;
    Ok(Recover::Done(linked, account.email, session))
}

/// A new password with the recovery key: lists, profiles, TVs and the recovery key stay.
async fn recover(web: Web, Path(token): Path<String>, form: Form) -> Answer {
    const TARGET: &str = "#recover-problem";
    if !web.within(Limit::Auth, &format!("recover:{}", web.ip)) {
        return Ok(web.problem_at(TOO_MANY, StatusCode::TOO_MANY_REQUESTS, TARGET));
    }
    let credentials = match read_credentials(&form) {
        Ok(credentials) => credentials,
        Err(problem) => return Ok(web.problem_at(&problem, StatusCode::BAD_REQUEST, TARGET)),
    };
    let auth_hash = auth_hash_of(form.field("recovery_auth_key"));
    let outcome = web.state.db.call(move |conn| recover_account(conn, &token, auth_hash.as_deref(), &credentials));
    match outcome.await? {
        Recover::LinkGone => Ok(web.problem_at(LINK_GONE, StatusCode::NOT_FOUND, TARGET)),
        Recover::NotTheRecoveryKey => Ok(web.problem_at(NOT_THE_RECOVERY_KEY, StatusCode::UNAUTHORIZED, TARGET)),
        Recover::NotOurKeys => Ok(web.problem_at(NOT_OUR_KEYS, StatusCode::BAD_REQUEST, TARGET)),
        Recover::Closed => Ok(web.problem_at(CLOSED, StatusCode::FORBIDDEN, TARGET)),
        Recover::NoKeys => Ok(web.problem_at(NO_KEYS, StatusCode::INTERNAL_SERVER_ERROR, TARGET)),
        Recover::Done(account_id, email, session) => {
            let notice = "Your new password is set. Your lists, profiles and TVs are as they were, and your \
                          recovery key still works.";
            signed_in_answer(&web, &account_id, &email, &session, "", Some(notice)).await
        }
    }
}

// --- Deleting the account ---

async fn delete_account(web: Web, form: Form) -> Answer {
    let session = match check_password(&web, &form, "#delete-problem").await? {
        Ok(session) => session,
        Err(problem) => return Ok(problem),
    };
    let id = session.account_id.clone();
    web.state
        .db
        .call(move |conn| {
            let tx = conn.transaction()?;
            vault::reset(&tx, &id)?;
            for table in ["devices", "pairings", "sessions", "email_links"] {
                tx.execute(&format!("DELETE FROM {table} WHERE account_id = ?"), [&id])?;
            }
            tx.execute("DELETE FROM accounts WHERE id = ?", [&id])?;
            tx.commit()?;
            Ok(())
        })
        .await?;
    web.state.sockets.close_account(&session.account_id);
    let mut response = if web.htmx {
        let mut response =
            page_fragment("Account deleted", html! { (pages::account_deleted()) (layout::nav(None, true)) });
        set_header(&mut response, "HX-Push-Url", "/");
        response
    } else {
        (StatusCode::SEE_OTHER, [(header::LOCATION, "/")]).into_response()
    };
    response.headers_mut().append(header::SET_COOKIE, ended_cookie());
    Ok(response)
}
