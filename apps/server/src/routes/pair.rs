//! The web half of signing in a TV (the Worker's `routes/pair.tsx`; PLAN.md,
//! "Signing in a TV"):
//!
//!   GET  /pair        the pairing page, or sign-in first
//!   POST /pair/find   the TV with this id, for the right proof: its sealed details
//!   POST /pair/{id}   the signed change that adds the TV, and the payload sealed for it
//!
//! The TV's half is in tv.rs.

use anyhow::Result;
use axum::Router;
use axum::extract::Path;
use axum::http::{Method, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

use crate::AppState;
use crate::accounts::Fields;
use crate::config::TvLimits;
use crate::error::AppError;
use crate::limits::Limit;
use crate::routes::tv::{is_pairing_id, is_sealed};
use crate::routes::vault::{MAX_CHANGE_LENGTH, refusal_status, refusal_text, vault_props};
use crate::session::Session;
use crate::sync_core::{ChangeProblem, ProblemCode};
use crate::time::now_ms;
use crate::tokens::{new_token, same_digest, sha256_hex};
use crate::vault::{self, Changed, NO_KEYS};
use crate::views::accounts::signin_form;
use crate::views::pair::{PAIR_TITLE, PairConfirm, pair_confirm, pair_page};
use crate::web::{Form, TOO_MANY, Web, fragment, page_fragment};

type Answer = Result<Response, AppError>;

pub fn router() -> Router<AppState> {
    Router::new().route("/pair", get(show_pair)).route("/pair/find", post(find)).route("/pair/{id}", post(pair))
}

const MAX_PAYLOAD_LENGTH: usize = 2000;
const SIGNED_OUT: &str = "You were signed out. Sign in again.";
const GONE: &str =
    "No TV is waiting with this code. Codes last 10 minutes and work once: check the code, or start again on the TV.";

struct Waiting {
    device_id: String,
    public_key: String,
    sealed_device: String,
}

/// The pairing with this id, if it's still waiting and the proof is right
/// for its kind. Wrong ids and wrong proofs answer the same.
fn waiting_pairing(conn: &Connection, id: &str, kind: &str, proof: &str) -> Result<Option<Waiting>> {
    if !is_pairing_id(id) || !matches!(kind, "qr" | "code") || proof.is_empty() {
        return Ok(None);
    }
    type Row = (Waiting, String, String, i64, Option<i64>);
    let row: Option<Row> = conn
        .query_row(
            "SELECT device_id, public_key, sealed_device, proof_qr_hash, proof_code_hash, expires_at, completed_at
             FROM pairings WHERE id = ?",
            [id],
            |row| {
                let waiting = Waiting { device_id: row.get(0)?, public_key: row.get(1)?, sealed_device: row.get(2)? };
                Ok((waiting, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?))
            },
        )
        .optional()?;
    let Some((waiting, qr_hash, code_hash, expires_at, completed_at)) = row else { return Ok(None) };
    if completed_at.is_some() || expires_at <= now_ms() {
        return Ok(None);
    }
    let expected = if kind == "qr" { qr_hash } else { code_hash };
    Ok(same_digest(&expected, &sha256_hex(proof)).then_some(waiting))
}

/// The pairing page for a signed-in owner: the whole page for GET (boosted
/// links swap the body), or #main for a form's answer.
async fn pair_answer(web: &Web, method: &Method, session: &Session, signed_in: Option<&str>) -> Answer {
    let Some(props) = vault_props(web, &session.account_id, &session.email).await? else {
        return Ok(web.problem(NO_KEYS, StatusCode::INTERNAL_SERVER_ERROR));
    };
    let page = pair_page(&props, signed_in);
    Ok(if method == Method::POST && web.htmx {
        page_fragment(PAIR_TITLE, page)
    } else {
        web.page(StatusCode::OK, PAIR_TITLE, page)
    })
}

async fn show_pair(web: Web, method: Method) -> Answer {
    let Some(session) = web.session.clone() else {
        let note = "Sign in to your Velio account, then choose what the TV gets.";
        return Ok(web.page(StatusCode::OK, PAIR_TITLE, signin_form(None, Some(note), Some("/pair"))));
    };
    pair_answer(&web, &method, &session, None).await
}

async fn find(web: Web, form: Form) -> Answer {
    let Some(session) = web.session.clone() else {
        return Ok(web.problem(SIGNED_OUT, StatusCode::UNAUTHORIZED));
    };
    if !web.within(Limit::Auth, &format!("pair-find:{}", session.account_id)) {
        return Ok(web.problem(TOO_MANY, StatusCode::TOO_MANY_REQUESTS));
    }
    let (id, kind, proof) =
        (form.field("id").to_owned(), form.field("kind").to_owned(), form.field("proof").to_owned());
    let tv_limits = web.state.cfg.tv_limits;
    let found = web
        .state
        .db
        .call(move |conn| {
            let Some(waiting) = waiting_pairing(conn, &id, &kind, &proof)? else { return Ok(None) };
            let profiles = vault::get_state(conn, &session.account_id)?.profiles;
            let tv_limit = vault::tv_limit(conn, &session.account_id, tv_limits)?;
            Ok(Some((waiting, profiles.into_iter().map(|profile| profile.id).collect::<Vec<_>>(), tv_limit)))
        })
        .await?;
    let Some((waiting, profile_ids, tv_limit)) = found else {
        return Ok(web.problem(GONE, StatusCode::NOT_FOUND));
    };
    Ok(fragment(pair_confirm(&PairConfirm {
        pairing_id: form.field("id"),
        device_id: &waiting.device_id,
        kind: form.field("kind"),
        sealed_json: &waiting.sealed_device,
        profile_ids: profile_ids.iter().map(String::as_str).collect(),
        tv_limit,
    })))
}

enum Paired {
    Gone,
    Refused(ChangeProblem),
    /// The TV's id, and the change that added it.
    Done(String, Changed),
}

/// What the phone sent to sign in a TV.
struct SignIn {
    kind: String,
    proof: String,
    input: Value,
    payload: Value,
}

/// Claims the pairing for the account first, so two phones can't both sign
/// in the same TV; then the change, and the TV's token, which goes to the
/// TV once, from the pairing: only its hash stays.
fn sign_in_tv(conn: &mut Connection, account_id: &str, id: &str, sent: &SignIn, tv_limits: TvLimits) -> Result<Paired> {
    let SignIn { kind, proof, input, payload } = sent;
    let Some(waiting) = waiting_pairing(conn, id, kind, proof)? else { return Ok(Paired::Gone) };
    let now = now_ms();
    let claimed = conn.execute(
        "UPDATE pairings SET account_id = ?, completed_at = ? WHERE id = ? AND completed_at IS NULL AND expires_at > ?",
        params![account_id, now, id, now],
    )?;
    if claimed != 1 {
        return Ok(Paired::Gone);
    }
    let outcome = vault::change(conn, account_id, input, std::slice::from_ref(&waiting.device_id), tv_limits);
    if !matches!(outcome, Ok(Ok(_))) {
        conn.execute("UPDATE pairings SET account_id = NULL, completed_at = NULL WHERE id = ?", [id])?;
    }
    let changed = match outcome? {
        Ok(changed) => changed,
        Err(problem) => return Ok(Paired::Refused(problem)),
    };
    let device_token = new_token();
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO devices (id, account_id, public_key, token_hash, created_at) VALUES (?, ?, ?, ?, ?)",
        params![waiting.device_id, account_id, waiting.public_key, sha256_hex(&device_token), now],
    )?;
    tx.execute(
        "UPDATE pairings SET kind = ?, payload = ?, device_token = ? WHERE id = ?",
        params![kind, payload.to_string(), device_token, id],
    )?;
    tx.commit()?;
    Ok(Paired::Done(waiting.device_id, changed))
}

async fn pair(web: Web, method: Method, Path(id): Path<String>, form: Form) -> Answer {
    const TARGET: &str = "#pair-problem";
    let Some(session) = web.session.clone() else {
        return Ok(web.problem_at(SIGNED_OUT, StatusCode::UNAUTHORIZED, TARGET));
    };
    if !web.within(Limit::Auth, &format!("pair:{}", session.account_id)) {
        return Ok(web.problem_at(TOO_MANY, StatusCode::TOO_MANY_REQUESTS, TARGET));
    }
    let (kind, proof) = (form.field("kind").to_owned(), form.field("proof").to_owned());
    let waiting = web.state.db.call({
        let (id, kind, proof) = (id.clone(), kind.clone(), proof.clone());
        move |conn| waiting_pairing(conn, &id, &kind, &proof)
    });
    if waiting.await?.is_none() {
        return Ok(web.problem_at(GONE, StatusCode::NOT_FOUND, TARGET));
    }

    let (change_text, payload_text) = (form.field("change"), form.field("payload"));
    if change_text.encode_utf16().count() > MAX_CHANGE_LENGTH
        || payload_text.encode_utf16().count() > MAX_PAYLOAD_LENGTH
    {
        return Ok(web.problem_at("This sign-in is too big.", StatusCode::PAYLOAD_TOO_LARGE, TARGET));
    }
    let parsed = serde_json::from_str::<Value>(change_text)
        .and_then(|change| Ok((change, serde_json::from_str::<Value>(payload_text)?)));
    let Some((input, payload)) = parsed.ok().filter(|(_, payload)| is_sealed(payload)) else {
        let problem = "This browser didn't send a readable sign-in. Reload the page and try again.";
        return Ok(web.problem_at(problem, StatusCode::BAD_REQUEST, TARGET));
    };

    let account_id = session.account_id.clone();
    let tv_limits = web.state.cfg.tv_limits;
    let sent = SignIn { kind, proof, input, payload };
    let paired = web.state.db.call(move |conn| sign_in_tv(conn, &account_id, &id, &sent, tv_limits));
    match paired.await? {
        Paired::Gone => Ok(web.problem_at(GONE, StatusCode::NOT_FOUND, TARGET)),
        Paired::Refused(problem) => {
            if problem.code == ProblemCode::Refused {
                tracing::warn!("Refused a TV sign-in for {}: {}", session.account_id, problem.message);
            }
            Ok(web.problem_at(&refusal_text(&problem), refusal_status(&problem), TARGET))
        }
        Paired::Done(device_id, changed) => {
            web.state.sockets.notify(&session.account_id, changed.rev, &changed.devices);
            pair_answer(&web, &method, &session, Some(&device_id)).await
        }
    }
}
