//! Pages and the fragments HTMX swaps into #main, in maud. The markup is the
//! Worker's (`apps/sync/src/views`), so vault.js and the styles work as they
//! are.

pub mod accounts;
pub mod layout;
pub mod ops;
pub mod pages;
pub mod pair;
pub mod vault;

use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use maud::{Markup, PreEscaped};

/// A whole page in the layout.
pub fn full_page(status: StatusCode, page: layout::Page<'_>, body: Markup) -> Response {
    let html = format!("{}{}", PreEscaped("<!doctype html>").0, layout::layout(page, body).0);
    (status, Html(html)).into_response()
}
