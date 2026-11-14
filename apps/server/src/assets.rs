//! /static: what the browser loads (vault.js, ops.js, HTMX, the styles),
//! built by `apps/sync/scripts/build-client.mjs` and embedded in the binary.
//! The names have no hash, so browsers check for a new version each time (an
//! ETag makes that cheap): a page from a new server never runs with an old
//! vault.js.

use axum::Router;
use axum::extract::{Path, Request};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use rust_embed::RustEmbed;

use crate::AppState;
use crate::http::StaticAsset;
use crate::web::Web;

#[derive(RustEmbed)]
#[folder = "../sync/public/static/"]
struct Static;

pub fn router() -> Router<AppState> {
    Router::new().route("/static/{*file}", get(serve))
}

async fn serve(web: Web, Path(file): Path<String>, req: Request) -> Response {
    let Some(asset) = Static::get(&file) else {
        return crate::routes::not_found(web, req.uri().clone()).await;
    };
    let etag = format!("\"{}\"", hex::encode(asset.metadata.sha256_hash()));
    let fresh = req.headers().get(header::IF_NONE_MATCH).is_some_and(|sent| sent.as_bytes() == etag.as_bytes());
    let mut response = if fresh {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        ([(header::CONTENT_TYPE, content_type(&file))], asset.data.into_owned()).into_response()
    };
    let headers = response.headers_mut();
    if let Ok(etag) = HeaderValue::from_str(&etag) {
        headers.insert(header::ETAG, etag);
    }
    for (name, value) in [
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::STRICT_TRANSPORT_SECURITY, "max-age=63072000; includeSubDomains"),
        (header::CACHE_CONTROL, "no-cache"),
        (header::HeaderName::from_static("cross-origin-resource-policy"), "same-origin"),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }
    response.extensions_mut().insert(StaticAsset);
    response
}

fn content_type(file: &str) -> &'static str {
    match file.rsplit_once('.').map(|(_, extension)| extension) {
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("map" | "json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    }
}
