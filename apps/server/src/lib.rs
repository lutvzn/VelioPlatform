//! The Velio server: velio.tv's website, and Nuvio's backend (Supabase Auth,
//! PostgREST, Storage and Nuvio's edge functions) on Nuvio's own Postgres
//! schema, so Nuvio's apps and VelioTV can both use it.

pub mod accounts;
pub mod api;
pub mod assets;
pub mod cleanup;
pub mod closing;
pub mod config;
pub mod db;
pub mod error;
pub mod http;
pub mod jwt;
pub mod limits;
pub mod mail;
pub mod notices;
pub mod ops_session;
pub mod routes;
pub mod session;
pub mod time;
pub mod tokens;
pub mod turnstile;
pub mod views;
pub mod web;

use std::sync::Arc;

use axum::Router;
use tower::ServiceBuilder;
use tower_http::catch_panic::CatchPanicLayer;

use crate::config::Config;
use crate::db::Db;
use crate::limits::Limits;
use crate::mail::Mailer;

/// What every handler can reach.
#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub db: Db,
    pub limits: Arc<Limits>,
    /// Built once, as it depends on whether Turnstile is on.
    pub csp: Arc<str>,
    pub mailer: Arc<Mailer>,
    /// For Turnstile and Resend.
    pub http: reqwest::Client,
    /// PostgREST's schema cache.
    pub schema: Arc<api::rest::Schema>,
}

impl AppState {
    pub fn new(cfg: Config, db: Db) -> anyhow::Result<Self> {
        let csp = http::csp(cfg.turnstile.is_some()).into();
        let http = reqwest::Client::builder().timeout(std::time::Duration::from_secs(10)).build()?;
        let mailer = Arc::new(Mailer::new(&cfg, http.clone())?);
        Ok(Self { cfg: Arc::new(cfg), db, limits: Arc::default(), csp, mailer, http, schema: Arc::default() })
    }
}

/// The whole site. The layers run in the order listed for each request.
pub fn app(state: AppState) -> Router {
    use axum::middleware::{from_fn, from_fn_with_state};

    Router::new()
        .merge(api::router())
        .merge(routes::web::router())
        .merge(routes::accounts::router())
        .merge(routes::approve::router())
        .merge(routes::profiles::router())
        .merge(routes::lists::router())
        .merge(routes::devices::router())
        .merge(routes::notices::router())
        .merge(routes::ops::router())
        .merge(assets::router())
        .fallback(routes::not_found)
        // A path that exists, asked with another method, is not found, as on the Worker.
        .method_not_allowed_fallback(routes::not_found)
        .layer(
            ServiceBuilder::new()
                .layer(from_fn_with_state(state.clone(), http::client_ip))
                .layer(from_fn(http::log_requests))
                .layer(from_fn_with_state(state.clone(), http::security_headers))
                .layer(from_fn(http::server_errors))
                .layer(CatchPanicLayer::custom(error::panicked))
                .layer(from_fn(http::limit_bodies))
                // Kong's part: CORS, and the API key on Supabase's paths.
                .layer(from_fn_with_state(state.clone(), api::gateway))
                .layer(from_fn_with_state(state.clone(), http::same_origin_posts))
                .layer(from_fn_with_state(state.clone(), session::sessions)),
        )
        .with_state(state)
}
