//! Turnstile on sign-up, reset requests and notices, when it's on (PLAN.md,
//! "Moving off Cloudflare": optional).

use std::net::IpAddr;

use crate::AppState;

const VERIFY_URL: &str = "https://challenges.cloudflare.com/turnstile/v0/siteverify";

/// The form field the Turnstile widget adds.
pub const FIELD: &str = "cf-turnstile-response";

/// True if the visitor passed Turnstile. Always with Turnstile off or DEV_MODE.
pub async fn passed(state: &AppState, token: &str, ip: IpAddr) -> bool {
    let Some(turnstile) = &state.cfg.turnstile else { return true };
    if state.cfg.dev_mode {
        return true;
    }
    if token.is_empty() {
        return false;
    }
    let ip = ip.to_string();
    let form = [("secret", turnstile.secret.as_str()), ("response", token), ("remoteip", ip.as_str())];
    let answer = async {
        let response = state.http.post(VERIFY_URL).form(&form).send().await?.error_for_status()?;
        response.json::<serde_json::Value>().await
    };
    match answer.await {
        Ok(result) => result.get("success") == Some(&serde_json::Value::Bool(true)),
        Err(error) => {
            tracing::warn!("Turnstile couldn't be asked: {error}");
            false
        }
    }
}
