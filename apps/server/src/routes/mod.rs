//! The routes, by area, as the Worker's `apps/sync/src/routes`.

pub mod accounts;
pub mod notices;
pub mod ops;
pub mod pair;
pub mod probe;
pub mod settings;
pub mod tv;
pub mod vault;
pub mod web;

use axum::http::{StatusCode, Uri};
use axum::response::Response;

use crate::http::{api_problem, is_api};
use crate::views::pages;
use crate::web::Web;

/// Anything no route matched.
pub async fn not_found(web: Web, uri: Uri) -> Response {
    if is_api(uri.path()) {
        return api_problem(StatusCode::NOT_FOUND, "Not found");
    }
    web.page(StatusCode::NOT_FOUND, "Not found", pages::not_found())
}
