//! Pages and the fragments HTMX swaps into #main, in maud.

pub mod accounts;
pub mod approve;
pub mod devices;
pub mod layout;
pub mod lists;
pub mod ops;
pub mod pages;
pub mod profiles;

use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use maud::{Markup, PreEscaped};

/// A whole page in the layout.
pub fn full_page(status: StatusCode, page: layout::Page<'_>, body: Markup) -> Response {
    let html = format!("{}{}", PreEscaped("<!doctype html>").0, layout::layout(page, body).0);
    (status, Html(html)).into_response()
}
