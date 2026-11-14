//! Phase 0's test routes (PLAN.md, Phase 0), which VelioTV's `sync` probe
//! calls from the TV:
//!
//!   GET /v1/probe               how the request arrived: TLS version and cipher, HTTP version
//!   GET /v1/probe/test-account  a made-up account's pull, to time verifying and decrypting it
//!   GET /v1/probe/events        a WebSocket that sends "rev 1" to "rev 5", and echoes what it gets
//!
//! TLS ends at Caddy, so the TLS details are the headers it adds
//! (X-Velio-TLS-Version, X-Velio-TLS-Cipher, X-Velio-HTTP-Protocol), believed
//! only from the trusted proxy (PLAN.md, "Differences, on purpose").

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::Extension;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade, rejection::WebSocketUpgradeRejection};
use axum::http::{HeaderMap, StatusCode, Version, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::AppState;
use crate::http::{ClientIp, text};

/// How many "rev N" messages the test socket sends, and how far apart
/// (e2e/probe.test.ts counts them).
pub const EVENT_COUNT: u32 = 5;
pub const EVENT_GAP: Duration = Duration::from_millis(500);

/// Made by `pnpm --filter @velio/server test-account`.
const TEST_ACCOUNT: &str = include_str!("../../probe/test-account.json");

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/probe", get(probe))
        .route("/v1/probe/test-account", get(test_account))
        .route("/v1/probe/events", get(events))
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as u64)
}

async fn probe(Extension(client): Extension<ClientIp>, version: Version, headers: HeaderMap) -> Response {
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok()).map(str::to_owned);
    let proxied = |name: &str| if client.via_proxy { header(name) } else { None };
    let http_protocol = proxied("x-velio-http-protocol").or_else(|| Some(format!("{version:?}")));
    let body: Value = json!({
        "ok": true,
        "now": now_ms(),
        "tlsVersion": proxied("x-velio-tls-version"),
        "tlsCipher": proxied("x-velio-tls-cipher"),
        "httpProtocol": http_protocol,
        "colo": null,
        "userAgent": header("user-agent"),
    });
    ([(header::CACHE_CONTROL, "no-store")], Json(body)).into_response()
}

async fn test_account() -> Response {
    ([(header::CACHE_CONTROL, "no-store"), (header::CONTENT_TYPE, "application/json")], TEST_ACCOUNT).into_response()
}

async fn events(upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>) -> Response {
    match upgrade {
        Ok(upgrade) => upgrade.on_upgrade(send_events),
        Err(_) => text(StatusCode::UPGRADE_REQUIRED, "This address takes a WebSocket\n"),
    }
}

async fn send_events(mut socket: WebSocket) {
    let mut ticks = tokio::time::interval_at(tokio::time::Instant::now() + EVENT_GAP, EVENT_GAP);
    let mut sent = 0;
    loop {
        tokio::select! {
            _ = ticks.tick(), if sent < EVENT_COUNT => {
                sent += 1;
                if socket.send(Message::Text(format!("rev {sent}").into())).await.is_err() {
                    return;
                }
            }
            message = socket.recv() => {
                let echo = match message {
                    Some(Ok(Message::Text(text))) => format!("echo {}", text.as_str()),
                    Some(Ok(Message::Binary(_))) => "echo (binary)".to_owned(),
                    Some(Ok(_)) => continue,
                    Some(Err(_)) | None => return,
                };
                if socket.send(Message::Text(echo.into())).await.is_err() {
                    return;
                }
            }
        }
    }
}
