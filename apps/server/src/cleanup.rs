//! The daily clean-up (the Worker's cron, `lib/cleanup.ts`): unconfirmed
//! accounts after 7 days, closed accounts' data after 30 days, and expired
//! sessions, links and pairings. Run once at start and then every day at
//! 04:17 UTC, followed by the backup when it's set; or by hand with
//! `velio-server clean-up`.

use std::time::Duration;

use anyhow::Result;
use rusqlite::Connection;

use crate::accounts::UNCONFIRMED_MS;
use crate::backup::back_up;
use crate::closing::purge_closed_accounts;
use crate::config::Backup;
use crate::db::Db;
use crate::time::{DAY_MS, HOUR_MS, now_ms};

/// 04:17 UTC, a quiet time off the hour.
pub const DAILY_AT_MS: i64 = 4 * HOUR_MS + 17 * 60_000;

/// What a clean-up deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cleaned {
    pub unconfirmed: usize,
    /// Closed accounts whose data went.
    pub purged: usize,
}

/// Deletes what's old at `now`.
pub fn clean_up(conn: &mut Connection, now: i64) -> Result<Cleaned> {
    let tx = conn.transaction()?;
    // Their sessions, links and vault rows go with them (ON DELETE CASCADE).
    let unconfirmed =
        tx.execute("DELETE FROM accounts WHERE confirmed_at IS NULL AND created_at < ?", [now - UNCONFIRMED_MS])?;
    let purged = purge_closed_accounts(&tx, now)?;
    tx.execute("DELETE FROM sessions WHERE expires_at <= ?", [now])?;
    tx.execute("DELETE FROM email_links WHERE expires_at <= ?", [now])?;
    tx.execute("DELETE FROM pairings WHERE expires_at <= ?", [now])?;
    tx.execute("DELETE FROM dev_outbox WHERE sent_at <= ?", [now - 7 * DAY_MS])?;
    tx.commit()?;
    Ok(Cleaned { unconfirmed, purged })
}

fn log(cleaned: Cleaned) {
    if cleaned.unconfirmed > 0 {
        tracing::info!("Deleted {} accounts left unconfirmed", cleaned.unconfirmed);
    }
    if cleaned.purged > 0 {
        tracing::info!("Deleted the data of {} closed accounts", cleaned.purged);
    }
}

/// How long from `now` until the next DAILY_AT_MS.
pub fn until_daily(now: i64) -> i64 {
    (now - DAILY_AT_MS).div_euclid(DAY_MS) * DAY_MS + DAY_MS + DAILY_AT_MS - now
}

async fn run_clean_up(db: &Db) {
    match db.call(|conn| clean_up(conn, now_ms())).await {
        Ok(cleaned) => log(cleaned),
        Err(error) => tracing::error!("The clean-up failed: {error:#}"),
    }
}

pub async fn run_backup(db: &Db, backup: &Backup, http: &reqwest::Client) {
    match back_up(db, backup, http, now_ms()).await {
        Ok(done) => tracing::info!(
            "Backed up the database to {} ({} bytes{}), and deleted {} older copies",
            done.path.display(),
            done.bytes,
            if done.uploaded { ", also sent to S3" } else { "" },
            done.pruned
        ),
        Err(error) => tracing::error!("The backup failed: {error:#}"),
    }
}

/// Runs the clean-up now, and then it and the backup every day, for as long
/// as the server runs.
pub fn spawn_daily(db: Db, backup: Option<Backup>, http: reqwest::Client) {
    tokio::spawn(async move {
        run_clean_up(&db).await;
        loop {
            let wait = u64::try_from(until_daily(now_ms())).unwrap_or(0);
            tokio::time::sleep(Duration::from_millis(wait)).await;
            run_clean_up(&db).await;
            if let Some(backup) = &backup {
                run_backup(&db, backup, &http).await;
            }
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

    #[tokio::test]
    async fn deletes_unconfirmed_accounts_after_7_days() {
        let db = Db::in_memory().unwrap();
        db.call(|conn| {
            conn.execute_batch(
                "INSERT INTO accounts (id, email, confirmed_at, salt, kdf_memory_kib, kdf_iterations,
                   kdf_parallelism, auth_hash, terms_accepted_at, created_at)
                 VALUES ('new', 'new@example.invalid', NULL, 's', 65536, 3, 1, 'h', 0, 0),
                        ('kept', 'kept@example.invalid', 1, 's', 65536, 3, 1, 'h', 0, 0);
                 INSERT INTO vault_owner (account_id, bundle, sign_public_key, box_public_key, updated_at)
                 VALUES ('new', '{}', 'k', 'k', 0);",
            )?;
            assert_eq!(clean_up(conn, 6 * DAY_MS)?.unconfirmed, 0);
            assert_eq!(clean_up(conn, 8 * DAY_MS)?.unconfirmed, 1);
            let ids: Vec<String> =
                conn.prepare("SELECT id FROM accounts")?.query_map([], |row| row.get(0))?.collect::<Result<_, _>>()?;
            assert_eq!(ids, ["kept"]);
            let owners: i64 = conn.query_row("SELECT count(*) FROM vault_owner", [], |row| row.get(0))?;
            assert_eq!(owners, 0);
            Ok(())
        })
        .await
        .unwrap();
    }
}
