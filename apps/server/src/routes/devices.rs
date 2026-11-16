//! What's signed in, through Nuvio's own functions, so this page and Nuvio's
//! apps list and sign out the same sessions:
//!
//!   GET  /devices                  list_my_sessions, and TVs that signed in with a code but haven't registered
//!   POST /devices/{id}/signout     revoke_my_session

use axum::Router;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use uuid::Uuid;

use crate::AppState;
use crate::db::Client;
use crate::error::AppError;
use crate::views::devices::{self as views, DeviceKind, DeviceRow, DevicesView};
use crate::web::{Web, sign_in_first};

type Answer = Result<Response, AppError>;

pub fn router() -> Router<AppState> {
    Router::new().route("/devices", get(devices)).route("/devices/{session_id}/signout", post(sign_out))
}

/// The TVs signed in with a code, and how many the account may have.
pub async fn tv_count(state: &AppState, client: &Client, user_id: Uuid) -> Result<(usize, usize), AppError> {
    let row = client
        .query_one(
            "SELECT (SELECT count(*) FROM velio_private.tv_sessions WHERE user_id = $1),
                    (SELECT tv_limit::int8 FROM velio_private.tv_limits WHERE user_id = $1)",
            &[&user_id],
        )
        .await?;
    let tvs = usize::try_from(row.get::<_, i64>(0)).unwrap_or(0);
    Ok((tvs, state.cfg.tv_limits.for_account(row.get(1))))
}

fn detail(client: Option<&str>, version: Option<&str>, platform: Option<&str>) -> Option<String> {
    let client = match (client, version) {
        (Some(client), Some(version)) if !version.is_empty() => Some(format!("{client} {version}")),
        (client, _) => client.map(str::to_owned),
    };
    let parts: Vec<String> = client.into_iter().chain(platform.filter(|p| !p.is_empty()).map(str::to_owned)).collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

async fn device_rows(web: &Web) -> Result<Result<Vec<DeviceRow>, String>, AppError> {
    let Some(session) = &web.session else { return Ok(Err("Sign in first.".into())) };
    let listed = web
        .as_user(async |tx| {
            tx.query(
                "SELECT session_id, (extract(epoch FROM created_at) * 1000)::int8,
                        (extract(epoch FROM last_active_at) * 1000)::int8,
                        client_name, client_version, platform, device_name, is_current
                   FROM public.list_my_sessions()",
                &[],
            )
            .await
        })
        .await?;
    let rows = match listed {
        Ok(rows) => rows,
        Err(problem) => return Ok(Err(problem)),
    };
    let mut devices: Vec<DeviceRow> = rows
        .iter()
        .map(|row| {
            let client: Option<String> = row.get(3);
            let kind = DeviceKind::of(client.as_deref());
            let name: Option<String> = row.get(6);
            let fallback = match kind {
                DeviceKind::Tv => "A TV",
                DeviceKind::Browser => "A browser",
                DeviceKind::App => "An app",
            };
            DeviceRow {
                session_id: row.get::<_, Uuid>(0).to_string(),
                name: name.filter(|name| !name.trim().is_empty()).unwrap_or_else(|| fallback.into()),
                detail: detail(client.as_deref(), row.get(4), row.get::<_, Option<&str>>(5)),
                kind,
                created_at: row.get(1),
                last_active_at: row.get(2),
                current: row.get(7),
            }
        })
        .collect();
    // A TV that signed in with a code but hasn't registered itself yet still counts, so it's shown.
    let client = web.state.db.client().await?;
    let unlisted = client
        .query(
            "SELECT t.session_id, (extract(epoch FROM s.created_at) * 1000)::int8,
                    (extract(epoch FROM greatest(s.updated_at, s.created_at)) * 1000)::int8
               FROM velio_private.tv_sessions t JOIN auth.sessions s ON s.id = t.session_id
              WHERE t.user_id = $1
                AND NOT EXISTS (SELECT 1 FROM public.user_session_devices d WHERE d.session_id = t.session_id)
              ORDER BY s.created_at DESC",
            &[&session.user_id],
        )
        .await?;
    devices.extend(unlisted.iter().map(|row| DeviceRow {
        session_id: row.get::<_, Uuid>(0).to_string(),
        name: "A TV".into(),
        detail: None,
        kind: DeviceKind::Tv,
        created_at: row.get(1),
        last_active_at: row.get(2),
        current: false,
    }));
    Ok(Ok(devices))
}

async fn page(web: &Web, notice: Option<&str>, status: StatusCode) -> Answer {
    let Some(session) = &web.session else { return Ok(sign_in_first("/devices")) };
    let devices = match device_rows(web).await? {
        Ok(devices) => devices,
        Err(problem) => return Ok(web.problem(&problem, StatusCode::BAD_REQUEST)),
    };
    let client = web.state.db.client().await?;
    let (tvs, tv_limit) = tv_count(&web.state, &client, session.user_id).await?;
    let view = DevicesView { devices: &devices, tvs, tv_limit, notice };
    if notice.is_some() {
        return Ok(web.answer("TVs and apps", views::devices_page(&view)));
    }
    Ok(web.page(status, "TVs and apps", views::devices_page(&view)))
}

async fn devices(web: Web) -> Answer {
    page(&web, None, StatusCode::OK).await
}

async fn sign_out(web: Web, Path(session_id): Path<String>) -> Answer {
    if web.session.is_none() {
        return Ok(web.problem("Sign in first.", StatusCode::UNAUTHORIZED));
    }
    let Ok(session_id) = Uuid::parse_str(&session_id) else {
        return Ok(web.problem("That isn't signed in any more.", StatusCode::NOT_FOUND));
    };
    let revoked = web
        .as_user(async |tx| {
            let row = tx.query_one("SELECT public.revoke_my_session($1)", &[&session_id]).await?;
            Ok(row.get::<_, bool>(0))
        })
        .await?;
    match revoked {
        Ok(true) => page(&web, Some("It's signed out."), StatusCode::OK).await,
        Ok(false) => page(&web, Some("That was already signed out."), StatusCode::OK).await,
        Err(problem) => Ok(web.problem(&problem, StatusCode::BAD_REQUEST)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_clients() {
        assert_eq!(
            detail(Some("Nuvio TV"), Some("1.4.0"), Some("android")).as_deref(),
            Some("Nuvio TV 1.4.0 · android")
        );
        assert_eq!(detail(Some("Nuvio Web"), None, Some("")).as_deref(), Some("Nuvio Web"));
        assert_eq!(detail(None, None, None), None);
    }
}
