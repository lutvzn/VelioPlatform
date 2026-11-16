//! The pages anyone can read. Accounts are in `accounts.rs`.

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use crate::AppState;
use crate::views::pages;
use crate::web::Web;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(home))
        .route("/terms", get(terms))
        .route("/privacy", get(privacy))
        .route("/security", get(security))
        .route("/healthz", get(healthz))
}

/// For the container's health check (`velio-server health`): the server
/// answers, and the database does too.
async fn healthz(State(state): State<AppState>) -> Response {
    let answered = async { anyhow::Ok(state.db.client().await?.query_one("SELECT 1", &[]).await?) };
    match answered.await {
        Ok(_) => (StatusCode::OK, "ok").into_response(),
        Err(error) => {
            tracing::error!("The health check failed: {error:#}");
            (StatusCode::SERVICE_UNAVAILABLE, "The database doesn't answer").into_response()
        }
    }
}

async fn home(web: Web) -> Response {
    web.page(StatusCode::OK, "Your lists, on every TV", pages::home(web.session.is_some()))
}

async fn terms(web: Web) -> Response {
    web.page(StatusCode::OK, "Terms", pages::terms(&web.state.cfg.abuse_email, web.state.cfg.tv_limits.default))
}

async fn privacy(web: Web) -> Response {
    web.page(StatusCode::OK, "Privacy", pages::privacy(&web.state.cfg))
}

async fn security(web: Web) -> Response {
    web.page(StatusCode::OK, "Security", pages::security(&web.state.cfg))
}
