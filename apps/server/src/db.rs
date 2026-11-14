//! The one SQLite file (PLAN.md, "Moving off Cloudflare", "Data").
//!
//! One connection, behind a mutex, used from blocking threads. Every write,
//! and so every change to an account, runs one at a time, which keeps them
//! strictly ordered as one Durable Object per account did. WAL lets other
//! processes (backups, the tests) read while the server writes.

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rusqlite::Connection;

/// Each migration, in order. `PRAGMA user_version` holds how many have run.
const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001_init.sql"),
    include_str!("../migrations/0002_sealed_device_names.sql"),
    include_str!("../migrations/0003_tv_limits.sql"),
];

#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("Opening {}", path.display()))?;
        Self::setup(conn)
    }

    #[cfg(test)]
    pub fn in_memory() -> Result<Self> {
        Self::setup(Connection::open_in_memory()?)
    }

    fn setup(mut conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        migrate(&mut conn)?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }

    /// Runs `work` with the connection on a blocking thread.
    pub async fn call<T, F>(&self, work: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            // A panic while holding the lock leaves SQLite consistent (an open
            // transaction rolls back when dropped), so a poisoned lock is usable.
            let mut conn = conn.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            work(&mut conn)
        })
        .await
        .context("A database task stopped")?
    }
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let done: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let done = usize::try_from(done).context("The database's user_version is negative")?;
    if done > MIGRATIONS.len() {
        anyhow::bail!(
            "The database has {done} migrations but this server knows {}: it's from a newer server",
            MIGRATIONS.len()
        );
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(done) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql).with_context(|| format!("Migration {}", index + 1))?;
        tx.pragma_update(None, "user_version", i64::try_from(index + 1)?)?;
        tx.commit()?;
        tracing::info!("Applied migration {}", index + 1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migrates_once_and_cascades_deletes() {
        let db = Db::in_memory().unwrap();
        db.call(|conn| {
            migrate(conn)?; // A second run does nothing.
            conn.execute_batch(
                "INSERT INTO accounts (id, email, salt, kdf_memory_kib, kdf_iterations, kdf_parallelism,
                   auth_hash, terms_accepted_at, created_at)
                 VALUES ('a1', 'owner@example.invalid', 's', 65536, 3, 1, 'h', 0, 0);
                 INSERT INTO vault_lists (account_id, id, rev, key_rev, sealed, key, position)
                 VALUES ('a1', 'l1', 1, 1, '{}', '{}', 0);
                 DELETE FROM accounts WHERE id = 'a1';",
            )?;
            let left: i64 = conn.query_row("SELECT count(*) FROM vault_lists", [], |row| row.get(0))?;
            assert_eq!(left, 0);
            Ok(())
        })
        .await
        .unwrap();
    }
}
