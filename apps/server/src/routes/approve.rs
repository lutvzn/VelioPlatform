//! Signing in a TV or app from the website, with Nuvio's own functions:
//!
//!   GET  /tv-login?code=   the page a TV's QR code opens (start_tv_login_session's web_url)
//!   POST /tv-login         approve_tv_login_session; the TV then gets its session from tv-logins-exchange
//!   GET  /link[?code=]     the page for a typed code (start_device_login_session's web_url)
//!   POST /link             approve_device_login_session
//!
//! Each needs the visitor signed in, and sends them to sign in first, coming back after.

use axum::Router;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use serde::Deserialize;

use crate::AppState;
use crate::accounts::Fields;
use crate::error::AppError;
use crate::limits::Limit;
use crate::routes::devices::tv_count;
use crate::views::approve::{self as views, DeviceLogin, TvLogin};
use crate::web::{Form, TOO_MANY, Web, sign_in_first};

type Answer = Result<Response, AppError>;

pub fn router() -> Router<AppState> {
    Router::new().route("/tv-login", get(tv_login_page).post(tv_login)).route("/link", get(link_page).post(link))
}

#[derive(Deserialize)]
struct CodeQuery {
    #[serde(default)]
    code: String,
}

/// A TV login code: letters and digits, as random_tv_login_code makes them.
fn plain_code(code: &str) -> Option<&str> {
    let code = code.trim();
    let plain = (4..=32).contains(&code.len()) && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    plain.then_some(code)
}

async fn tv_login_page(web: Web, Query(query): Query<CodeQuery>) -> Answer {
    let Some(session) = &web.session else {
        return Ok(sign_in_first(&format!("/tv-login?code={}", plain_code(&query.code).unwrap_or(""))));
    };
    let Some(code) = plain_code(&query.code) else {
        return Ok(web.page(StatusCode::NOT_FOUND, "TV sign-in", views::code_gone()));
    };
    let client = web.state.db.client().await?;
    let pending = client
        .query_opt(
            "SELECT device_name FROM public.tv_login_sessions
              WHERE code = $1 AND status = 'pending' AND expires_at > now()",
            &[&code],
        )
        .await?;
    let Some(pending) = pending else {
        return Ok(web.page(StatusCode::NOT_FOUND, "TV sign-in", views::code_gone()));
    };
    let device_name: Option<String> = pending.get(0);
    let (tvs, tv_limit) = tv_count(&web.state, &client, session.user_id).await?;
    let view = TvLogin { code, device_name: device_name.as_deref().filter(|name| !name.is_empty()), tvs, tv_limit };
    Ok(web.page(StatusCode::OK, "Sign in this TV?", views::tv_login(&view)))
}

async fn tv_login(web: Web, form: Form) -> Answer {
    if web.session.is_none() {
        return Ok(web.problem("Sign in first.", StatusCode::UNAUTHORIZED));
    }
    if !web.within(Limit::Auth, &format!("tv-login:{}", web.ip)) {
        return Ok(web.problem(TOO_MANY, StatusCode::TOO_MANY_REQUESTS));
    }
    let code = form.field("code").trim().to_owned();
    let approved = web
        .as_user(async |tx| {
            let row =
                tx.query_one("SELECT success, message FROM public.approve_tv_login_session($1)", &[&code]).await?;
            Ok(row.get::<_, bool>(0))
        })
        .await?;
    match approved {
        Ok(true) => Ok(web.answer("Done", views::approved("The TV"))),
        Ok(false) => Ok(web.answer("TV sign-in", views::code_gone())),
        Err(problem) => Ok(web.problem(&problem, StatusCode::BAD_REQUEST)),
    }
}

async fn link_page(web: Web, Query(query): Query<CodeQuery>) -> Answer {
    let code = query.code.trim();
    if web.session.is_none() {
        let back = if code.is_empty() { "/link".to_owned() } else { format!("/link?code={code}") };
        return Ok(sign_in_first(&back));
    }
    if code.is_empty() {
        return Ok(web.answer("Sign in a TV or app", views::link_form(None)));
    }
    // Nuvio's function counts wrong codes, and stops an account after too many.
    let code = code.to_owned();
    let found = web
        .as_user(async |tx| {
            let row = tx
                .query_one(
                    "SELECT success, message, user_code, device_name, device_type
                       FROM public.get_device_login_session($1)",
                    &[&code],
                )
                .await?;
            let success: bool = row.get(0);
            Ok(success
                .then(|| (row.get::<_, String>(2), row.get::<_, Option<String>>(3), row.get::<_, Option<String>>(4))))
        })
        .await?;
    match found {
        Ok(Some((user_code, device_name, device_type))) => {
            let view = DeviceLogin {
                code: &user_code,
                device_name: device_name.as_deref(),
                device_type: device_type.as_deref(),
            };
            Ok(web.answer("Sign it in?", views::link_confirm(&view)))
        }
        Ok(None) if web.htmx => {
            Ok(web.problem("That code doesn't work. Check it on the TV or app.", StatusCode::NOT_FOUND))
        }
        Ok(None) => Ok(web.page(StatusCode::NOT_FOUND, "Sign in a TV or app", views::link_form(Some(&code)))),
        Err(problem) if web.htmx => Ok(web.problem(&problem, StatusCode::BAD_REQUEST)),
        Err(problem) => {
            Ok(web.page(StatusCode::BAD_REQUEST, "Problem", crate::views::accounts::problem_page(&problem)))
        }
    }
}

async fn link(web: Web, form: Form) -> Answer {
    if web.session.is_none() {
        return Ok(web.problem("Sign in first.", StatusCode::UNAUTHORIZED));
    }
    let code = form.field("code").trim().to_owned();
    let approved = web
        .as_user(async |tx| {
            let row = tx.query_one("SELECT success FROM public.approve_device_login_session($1)", &[&code]).await?;
            Ok(row.get::<_, bool>(0))
        })
        .await?;
    match approved {
        Ok(true) => Ok(web.answer("Done", views::approved("It"))),
        Ok(false) => Ok(web.answer("Sign in a TV or app", views::code_gone())),
        Err(problem) => Ok(web.problem(&problem, StatusCode::BAD_REQUEST)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_only_plain_codes() {
        assert_eq!(plain_code(" AB12CD34 "), Some("AB12CD34"));
        assert_eq!(plain_code("ABCD-EFGH"), Some("ABCD-EFGH"));
        assert_eq!(plain_code("AB"), None);
        assert_eq!(plain_code("AB12&next=x"), None);
    }
}
