//! Errors a handler doesn't expect. They're logged, and the visitor gets a
//! plain "something went wrong" (`http::server_errors` picks the words by path).

use std::any::Any;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// Marks a 500 from an error or a panic, so its body is replaced.
#[derive(Clone, Copy)]
pub struct ServerError;

/// Any error, as a 500. Handlers return `Result<_, AppError>` and use `?`.
pub struct AppError(anyhow::Error);

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(error: E) -> Self {
        Self(error.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        tracing::error!("{:#}", self.0);
        server_error()
    }
}

pub fn server_error() -> Response {
    let mut response = StatusCode::INTERNAL_SERVER_ERROR.into_response();
    response.extensions_mut().insert(ServerError);
    response
}

/// For `CatchPanicLayer`: a panic in a handler answers like an error.
pub fn panicked(panic: Box<dyn Any + Send + 'static>) -> Response {
    let message = panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(no message)".into());
    tracing::error!("A handler panicked: {message}");
    server_error()
}
