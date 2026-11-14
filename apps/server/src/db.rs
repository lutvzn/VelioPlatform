//! The one SQLite file (PLAN.md, "Moving off Cloudflare", "Data").
//!
//! One connection, behind a mutex, used from blocking threads. Every write,
//! and so every change to an account, runs one at a time, which keeps them
//! strictly ordered as one Durable Object per account did. WAL lets other
//! processes (backups, the tests) read while the server writes.
//!
//! Kept small: ciphertext and hashes are stored as bytes (`packed`, migration
//! 4), and pages freed by deletes go back to the disk after each clean-up
//! (incremental auto-vacuum).

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rusqlite::Connection;
use rusqlite::functions::FunctionFlags;
use rusqlite::types::Value;

use crate::packed::{pack_bundle, pack_sealed, pack_signed};
use crate::sync_core::{Sealed, Signed};

/// Each migration, in order. `PRAGMA user_version` holds how many have run.
const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001_init.sql"),
    include_str!("../migrations/0002_sealed_device_names.sql"),
    include_str!("../migrations/0003_tv_limits.sql"),
    include_str!("../migrations/0004_less_storage.sql"),
];

/// The most the write-ahead log keeps on disk between checkpoints.
const WAL_KEEP_BYTES: i64 = 4 * 1024 * 1024;

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
        // Takes effect at once on a new file; one made before needs the VACUUM below.
        conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "journal_size_limit", WAL_KEEP_BYTES)?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        migrate(&mut conn)?;
        let auto_vacuum: i64 = conn.pragma_query_value(None, "auto_vacuum", |row| row.get(0))?;
        if auto_vacuum != INCREMENTAL {
            // Once: rewrites the file compactly, with auto-vacuum on from then.
            conn.execute_batch("VACUUM").context("Compacting the database")?;
            tracing::info!("Compacted the database");
        }
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

/// `PRAGMA auto_vacuum`'s value for INCREMENTAL.
const INCREMENTAL: i64 = 2;

/// Hands the pages that deletes freed back to the disk.
pub fn give_back_free_pages(conn: &Connection) -> Result<()> {
    // It frees a page each step, so it runs until it is done.
    let mut statement = conn.prepare("PRAGMA incremental_vacuum")?;
    let mut rows = statement.query([])?;
    while rows.next()?.is_some() {}
    Ok(())
}

/// JSON text as `packed` stores it, for migration 4. Anything else is kept as it is.
fn packing<T: serde::de::DeserializeOwned + 'static>(
    conn: &Connection,
    name: &str,
    pack: fn(&T) -> Value,
) -> rusqlite::Result<()> {
    conn.create_scalar_function(name, 1, FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC, move |ctx| {
        Ok(match ctx.get::<Value>(0)? {
            Value::Text(text) => serde_json::from_str::<T>(&text).map_or(Value::Text(text), |value| pack(&value)),
            other => other,
        })
    })
}

fn migrate(conn: &mut Connection) -> Result<()> {
    packing::<Sealed>(conn, "velio_pack_sealed", pack_sealed)?;
    packing::<Signed>(conn, "velio_pack_signed", pack_signed)?;
    conn.create_scalar_function(
        "velio_pack_bundle",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            Ok(match ctx.get::<Value>(0)? {
                Value::Text(text) => pack_bundle(&text),
                other => other,
            })
        },
    )?;
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

    #[test]
    fn gives_freed_pages_back() {
        let dir = std::env::temp_dir().join(format!("velio-db-test-{}", crate::tokens::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("velio.db");
        let db = Db::open(&path).unwrap();
        let conn = db.conn.lock().unwrap();
        let pages = |name: &str| -> i64 { conn.pragma_query_value(None, name, |row| row.get(0)).unwrap() };
        assert_eq!(pages("auto_vacuum"), INCREMENTAL);
        conn.execute_batch(
            "CREATE TABLE filler (x BLOB);
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 200)
             INSERT INTO filler SELECT randomblob(2000) FROM n;
             DELETE FROM filler;",
        )
        .unwrap();
        let (before, free) = (pages("page_count"), pages("freelist_count"));
        assert!(free >= 100, "{free} free pages");
        give_back_free_pages(&conn).unwrap();
        assert_eq!(pages("freelist_count"), 0);
        assert!(pages("page_count") <= before - free);
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn migrates_once_and_cascades_deletes() {
        let db = Db::in_memory().unwrap();
        db.call(|conn| {
            migrate(conn)?; // A second run does nothing.
            conn.execute_batch(
                "INSERT INTO accounts (id, email, salt, kdf_memory_kib, kdf_iterations, kdf_parallelism,
                   auth_hash, terms_accepted_at, created_at)
                 VALUES ('a1', 'owner@example.invalid', 's', 65536, 3, 1, X'00', 0, 0);
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
