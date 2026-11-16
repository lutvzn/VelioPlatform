//! What every request goes through: the client's address, headers, body
//! caps, and the Origin check on web forms. Nuvio's API (`api`) has its own
//! CORS and key checks, in `api::gateway`.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use axum::Json;
use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http_body_util::BodyExt;
use serde_json::json;

use crate::AppState;
use crate::error::ServerError;

/// Nuvio's API for apps and TVs, and /ops for the operator: bearer tokens,
/// no cookies, no forms.
pub fn is_api(path: &str) -> bool {
    path.starts_with("/ops/") || crate::api::is_supabase(path)
}

/// A JSON problem for /ops: `{"problem": "…"}`.
pub fn api_problem(status: StatusCode, problem: &str) -> Response {
    (status, Json(json!({ "problem": problem }))).into_response()
}

/// A plain-text answer, as the Worker's `c.text`.
pub fn text(status: StatusCode, text: &'static str) -> Response {
    (status, text).into_response()
}

// --- The client's address ---

/// Who sent the request: from X-Forwarded-For when it came through the
/// trusted reverse proxy, else the socket's address.
#[derive(Clone, Copy, Debug)]
pub struct ClientIp {
    pub ip: IpAddr,
    /// Through the trusted proxy, so the X-Velio-* headers it sets can be believed.
    pub via_proxy: bool,
}

pub async fn client_ip(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let peer = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|info| info.0.ip());
    let via_proxy = peer.is_some_and(|peer| state.cfg.trusted_proxies.iter().any(|range| range.contains(peer)));
    let forwarded = via_proxy.then(|| forwarded_for(req.headers())).flatten();
    let ip = forwarded.or(peer).unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    req.extensions_mut().insert(ClientIp { ip, via_proxy });
    next.run(req).await
}

/// The rightmost address, the one the proxy added. Those to its left are
/// whatever the client sent.
fn forwarded_for(headers: &HeaderMap) -> Option<IpAddr> {
    let last = headers.get_all("x-forwarded-for").iter().next_back()?;
    last.to_str().ok()?.rsplit(',').next()?.trim().parse().ok()
}

/// One line per request. The query is left out, as it can carry a token.
pub async fn log_requests(req: Request, next: Next) -> Response {
    let started = Instant::now();
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let ip = req.extensions().get::<ClientIp>().map(|client| client.ip);
    let response = next.run(req).await;
    let ms = started.elapsed().as_millis();
    let status = response.status().as_u16();
    // The container's health check, every 30 seconds.
    if path == "/healthz" && status == 200 {
        return response;
    }
    match ip {
        Some(ip) => tracing::info!("{method} {path} {status} {ms}ms {ip}"),
        None => tracing::info!("{method} {path} {status} {ms}ms"),
    }
    response
}

// --- Headers ---

/// Turnstile, when it's on, needs its script and frame.
pub fn csp(turnstile: bool) -> String {
    let challenges = if turnstile { " https://challenges.cloudflare.com" } else { "" };
    [
        "default-src 'self'".to_owned(),
        format!("script-src 'self'{challenges}"),
        if turnstile { "frame-src https://challenges.cloudflare.com".into() } else { "frame-src 'none'".into() },
        "connect-src 'self'".into(),
        "img-src 'self' data:".into(),
        "style-src 'self'".into(),
        "base-uri 'none'".into(),
        "form-action 'self'".into(),
        "frame-ancestors 'none'".into(),
        "object-src 'none'".into(),
    ]
    .join("; ")
}

/// Browser features no page uses, turned off for the pages and anything they embed.
pub const PERMISSIONS: &str = "accelerometer=(), camera=(), display-capture=(), geolocation=(), \
    gyroscope=(), hid=(), magnetometer=(), microphone=(), midi=(), payment=(), serial=(), usb=()";

/// Marks a response from `assets`, which sets its own headers.
#[derive(Clone, Copy)]
pub struct StaticAsset;

pub async fn security_headers(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let api_only = crate::api::is_supabase(req.uri().path());
    let mut response = next.run(req).await;
    if response.extensions().get::<StaticAsset>().is_some() {
        return response;
    }
    let headers = response.headers_mut();
    let csp = HeaderValue::from_str(&state.csp).ok();
    let base = [
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::STRICT_TRANSPORT_SECURITY, "max-age=63072000; includeSubDomains"),
    ];
    let pages = [
        (header::X_FRAME_OPTIONS, "DENY"),
        (HeaderName::from_static("cross-origin-opener-policy"), "same-origin"),
        (HeaderName::from_static("cross-origin-resource-policy"), "same-origin"),
        (HeaderName::from_static("permissions-policy"), PERMISSIONS),
    ];
    for (name, value) in base {
        headers.insert(name, HeaderValue::from_static(value));
    }
    if !api_only {
        for (name, value) in pages {
            headers.insert(name, HeaderValue::from_static(value));
        }
        if let Some(csp) = csp {
            headers.insert(header::CONTENT_SECURITY_POLICY, csp);
        }
        // Pages and fragments carry the account's lists and session state, so nothing is cached.
        headers.entry(header::CACHE_CONTROL).or_insert(HeaderValue::from_static("no-store"));
    }
    response
}

/// Errors and panics: the words depend on who asked.
pub async fn server_errors(req: Request, next: Next) -> Response {
    let api = is_api(req.uri().path());
    let supabase = crate::api::is_supabase(req.uri().path());
    let response = next.run(req).await;
    if response.extensions().get::<ServerError>().is_none() {
        return response;
    }
    if supabase {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "message": "An unexpected error occurred" }))).into_response()
    } else if api {
        api_problem(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong on the server")
    } else {
        text(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong on the server. Try again in a minute.\n")
    }
}

// --- Bodies ---

/// Web forms are small (a list's address and login at most); Nuvio's API
/// takes a profile's settings or a 5 MB avatar image; /ops takes small JSON.
pub const WEB_BODY: usize = 1024 * 1024;
pub const API_BODY: usize = 8 * 1024 * 1024;
pub const OPS_BODY: usize = 64 * 1024;

/// Bodies are read whole, so their size is capped before any route reads one.
pub async fn limit_bodies(req: Request, next: Next) -> Response {
    let path = req.uri().path();
    let api = is_api(path);
    let max = if crate::api::is_supabase(path) {
        API_BODY
    } else if api {
        OPS_BODY
    } else {
        WEB_BODY
    };
    let too_big = || {
        if api {
            api_problem(StatusCode::PAYLOAD_TOO_LARGE, "This is too big to send")
        } else {
            text(StatusCode::PAYLOAD_TOO_LARGE, "This is too big to send.\n")
        }
    };
    let declared = req.headers().get(header::CONTENT_LENGTH).and_then(|value| value.to_str().ok()?.parse::<u64>().ok());
    let (parts, mut body) = req.into_parts();
    if declared.is_some_and(|length| length > max as u64) {
        linger(body).await;
        return too_big();
    }
    let mut bytes = Vec::new();
    while let Some(frame) = body.frame().await {
        // The client went away mid-body.
        let Ok(frame) = frame else { return StatusCode::BAD_REQUEST.into_response() };
        let Ok(data) = frame.into_data() else { continue };
        if bytes.len() + data.len() > max {
            linger(body).await;
            return too_big();
        }
        bytes.extend_from_slice(&data);
    }
    next.run(Request::from_parts(parts, Body::from(bytes))).await
}

/// How much of a body that's too big is read and dropped before the 413.
const LINGER_BYTES: usize = 16 * 1024 * 1024;
const LINGER_TIME: Duration = Duration::from_secs(10);

/// Reads what's left of a body that's too big, up to a point, and drops it,
/// so the client finishes sending and reads the 413, not a broken pipe (as
/// nginx's lingering close). Beyond that, the connection just closes.
async fn linger(mut body: Body) {
    let drain = async {
        let mut read = 0;
        while let Some(Ok(frame)) = body.frame().await {
            read += frame.data_ref().map_or(0, |data| data.len());
            if read > LINGER_BYTES {
                break;
            }
        }
    };
    let _ = tokio::time::timeout(LINGER_TIME, drain).await;
}

// --- Forms ---

/// Refuses web form posts from other sites. The session cookie is
/// SameSite=Strict already; this also covers the forms that need no session,
/// such as sign-up. Nuvio's API (apps and TVs) and /ops (the operator) take bearer
/// tokens and no cookies.
pub async fn same_origin_posts(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let method = req.method();
    let safe = method == Method::GET || method == Method::HEAD || method == Method::OPTIONS;
    if safe || is_api(req.uri().path()) {
        return next.run(req).await;
    }
    if !from_this_site(req.headers(), &state.cfg.site_url) {
        let message = format!("This form can only be sent from {}.\n", state.cfg.site_host);
        return (StatusCode::FORBIDDEN, message).into_response();
    }
    next.run(req).await
}

/// Browsers say where a request comes from in Sec-Fetch-Site, which pages
/// can't set. It counts first: with Referrer-Policy no-referrer a browser
/// may send `Origin: null` on a same-origin post, and the page may be open
/// on another name for this server, such as 127.0.0.1 for localhost. Older
/// browsers and other clients are judged by Origin.
pub fn from_this_site(headers: &HeaderMap, site_url: &str) -> bool {
    if let Some(site) = headers.get("sec-fetch-site") {
        return site == "same-origin";
    }
    headers.get(header::ORIGIN).is_some_and(|origin| origin == site_url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_posts_from_this_site_only() {
        let site = "https://velio.tv";
        let with = |pairs: &[(&'static str, &'static str)]| {
            let mut headers = HeaderMap::new();
            for (name, value) in pairs {
                headers.insert(*name, HeaderValue::from_static(value));
            }
            from_this_site(&headers, site)
        };
        assert!(with(&[("origin", "https://velio.tv")]));
        assert!(with(&[("sec-fetch-site", "same-origin"), ("origin", "null")]));
        assert!(!with(&[("sec-fetch-site", "same-site"), ("origin", "https://velio.tv")]));
        assert!(!with(&[("sec-fetch-site", "cross-site")]));
        assert!(!with(&[("origin", "https://elsewhere.example")]));
        assert!(!with(&[("origin", "null")]));
        assert!(!with(&[]));
    }

    #[test]
    fn reads_the_address_the_proxy_added() {
        let mut headers = HeaderMap::new();
        headers.append("x-forwarded-for", "1.2.3.4, 10.0.0.9".parse().unwrap());
        assert_eq!(forwarded_for(&headers), Some("10.0.0.9".parse().unwrap()));
        headers.append("x-forwarded-for", "2001:db8::1".parse().unwrap());
        assert_eq!(forwarded_for(&headers), Some("2001:db8::1".parse().unwrap()));
        assert_eq!(forwarded_for(&HeaderMap::new()), None);
    }

    #[test]
    fn csp_names_turnstile_only_when_it_is_on() {
        assert!(!csp(false).contains("challenges.cloudflare.com"));
        assert!(csp(true).contains("script-src 'self' https://challenges.cloudflare.com"));
        assert!(csp(false).contains("frame-ancestors 'none'"));
    }
}
