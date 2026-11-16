//! The daily clean-up: accounts left unconfirmed after 7 days, closed
//! accounts' data after 30 days, the website's expired sessions, and the
//! refresh tokens and sessions GoTrue's own clean-up deletes. Run once
//! at start and then every day at 04:17 UTC, or by hand with
//! `velio-server clean-up`. Backups are Postgres's own (pg_dump, or the
//! host's), not the server's.

use std::time::Duration;

use anyhow::Result;

use crate::closing::purge_closed_accounts;
use crate::db::Db;
use crate::time::{DAY_MS, HOUR_MS, now_ms};

/// Unconfirmed accounts are deleted after this.
pub const UNCONFIRMED_MS: i64 = 7 * DAY_MS;

/// 04:17 UTC, a quiet time off the hour.
pub const DAILY_AT_MS: i64 = 4 * HOUR_MS + 17 * 60_000;

/// What a clean-up deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cleaned {
    pub unconfirmed: u64,
    /// Closed accounts whose data went.
    pub purged: usize,
    /// Refresh tokens used up a day ago or more.
    pub tokens: u64,
    /// Sessions 72 hours past their end, with their tokens.
    pub sessions: u64,
}

/// Deletes what's old at `now`, in milliseconds.
pub async fn clean_up(db: &Db, now: i64) -> Result<Cleaned> {
    let mut client = db.client().await?;
    let tx = client.transaction().await?;
    let at = now as f64 / 1000.0;
    // Their identities and sessions go with them (ON DELETE CASCADE). Guests
    // have no email to confirm, so they stay, as on Nuvio's server.
    let unconfirmed = tx
        .execute(
            "DELETE FROM auth.users WHERE email_confirmed_at IS NULL AND NOT is_anonymous
               AND created_at < to_timestamp($1)",
            &[&(at - (UNCONFIRMED_MS / 1000) as f64)],
        )
        .await?;
    let purged = purge_closed_accounts(&tx, now).await?;
    tx.execute("DELETE FROM velio_private.web_sessions WHERE expires_at <= to_timestamp($1)", &[&at]).await?;
    tx.execute("DELETE FROM velio_private.dev_outbox WHERE sent_at <= $1", &[&(now - 7 * DAY_MS)]).await?;
    // As GoTrue's clean-up (internal/models/cleanup.go), by the database's
    // clock: a token used up is kept a day for reuse detection, and a session
    // 72 hours after its end, so a late refresh is told it ended. Otherwise
    // every refresh leaves a row behind for good.
    let tokens = tx
        .execute("DELETE FROM auth.refresh_tokens WHERE revoked AND updated_at < now() - interval '24 hours'", &[])
        .await?;
    let sessions = tx.execute("DELETE FROM auth.sessions WHERE not_after < now() - interval '72 hours'", &[]).await?;
    tx.commit().await?;
    Ok(Cleaned { unconfirmed, purged, tokens, sessions })
}

fn log(cleaned: Cleaned) {
    if cleaned.unconfirmed > 0 {
        tracing::info!("Deleted {} accounts left unconfirmed", cleaned.unconfirmed);
    }
    if cleaned.purged > 0 {
        tracing::info!("Deleted the data of {} closed accounts", cleaned.purged);
    }
    if cleaned.tokens > 0 || cleaned.sessions > 0 {
        tracing::info!("Deleted {} used refresh tokens and {} ended sessions", cleaned.tokens, cleaned.sessions);
    }
}

/// How long from `now` until the next DAILY_AT_MS.
pub fn until_daily(now: i64) -> i64 {
    (now - DAILY_AT_MS).div_euclid(DAY_MS) * DAY_MS + DAY_MS + DAILY_AT_MS - now
}

async fn run_clean_up(db: &Db) {
    match clean_up(db, now_ms()).await {
        Ok(cleaned) => log(cleaned),
        Err(error) => tracing::error!("The clean-up failed: {error:#}"),
    }
}

/// Runs the clean-up now, and then every day, for as long as the server runs.
pub fn spawn_daily(db: Db) {
    tokio::spawn(async move {
        run_clean_up(&db).await;
        loop {
            let wait = u64::try_from(until_daily(now_ms())).unwrap_or(0);
            tokio::time::sleep(Duration::from_millis(wait)).await;
            run_clean_up(&db).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_at_04_17_utc() {
        assert_eq!(until_daily(0), DAILY_AT_MS);
        assert_eq!(until_daily(DAILY_AT_MS), DAY_MS);
        assert_eq!(until_daily(DAILY_AT_MS - 1), 1);
        assert_eq!(until_daily(DAILY_AT_MS + 1), DAY_MS - 1);
        assert_eq!(until_daily(10 * DAY_MS + 5 * HOUR_MS), DAY_MS - 43 * 60_000);
    }
}
