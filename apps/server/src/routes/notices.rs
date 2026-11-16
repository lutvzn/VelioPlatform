//! The /abuse form (the Worker's `routes/web.tsx`, "Notices"; PLAN.md,
//! "Legal and abuse"):
//!
//!   GET  /abuse   the form
//!   POST /abuse   keeps the notice, mails it to the abuse address, and answers its reference
//!
//! The operator handles notices through /ops (ops.rs).

use axum::Router;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;

use crate::AppState;
use crate::accounts::Fields;
use crate::error::AppError;
use crate::limits::Limit;
use crate::mail::{self, NoticeSummary, notice_mail};
use crate::notices::{read_notice, save_notice};
use crate::time::now_ms;
use crate::turnstile;
use crate::views::pages::{abuse_form, notice_received};
use crate::web::{Form, TOO_MANY, Web};

pub fn router() -> Router<AppState> {
    Router::new().route("/abuse", get(show_form).post(send))
}

async fn show_form(web: Web) -> Response {
    web.turnstile_page("Report abuse", abuse_form(web.site_key(), &web.state.cfg.abuse_email))
}

async fn send(web: Web, form: Form) -> Result<Response, AppError> {
    if !web.within(Limit::Signup, &format!("abuse:{}", web.ip)) {
        return Ok(web.problem(TOO_MANY, StatusCode::TOO_MANY_REQUESTS));
    }
    let notice = match read_notice(&form) {
        Ok(notice) => notice,
        Err(problem) => return Ok(web.problem(&problem, StatusCode::BAD_REQUEST)),
    };
    if !turnstile::passed(&web.state, form.field(turnstile::FIELD), web.ip).await {
        let problem = "The check that you're a person didn't pass. Try again.";
        return Ok(web.problem(problem, StatusCode::BAD_REQUEST));
    }
    let client = web.state.db.client().await?;
    let (id, account_found) = save_notice(&client, &notice, now_ms()).await?;
    let summary = NoticeSummary { id: &id, notice: &notice, account_found };
    mail::send(&web.state, notice_mail(&web.state.cfg.abuse_email, &summary)).await?;
    Ok(web.answer("Notice received", notice_received(&id)))
}
