//! The website's routes, by area. Nuvio's API is in `api`.

pub mod accounts;
pub mod approve;
pub mod devices;
pub mod lists;
pub mod notices;
pub mod ops;
pub mod profiles;
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
