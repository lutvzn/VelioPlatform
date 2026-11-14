//! Closing an account (the Worker's `lib/closing.ts`; PLAN.md, "Legal and
//! abuse"): for a valid notice or a broken term, done by the operator through
//! /ops. Velio can't read or remove one list, so it closes the account:
//! sessions, links and pairings end, TVs are signed out (their sockets are
//! closed as removed), the owner is told why, and the encrypted data is
//! deleted after 30 days. Until then it can be opened again. The row stays, so
//! the email can't simply sign up again.

use anyhow::Result;
use axum::http::StatusCode;
use rusqlite::{Connection, OptionalExtension, params};

use crate::time::DAY_MS;
use crate::vault;

pub const CLOSED_KEEP_MS: i64 = 30 * DAY_MS;
pub const MAX_REASON_LENGTH: usize = 2000;

/// Why closing or opening was refused: its status and words.
pub type Refused = (StatusCode, &'static str);

const NO_ACCOUNT: Refused = (StatusCode::NOT_FOUND, "No such account");
const ALREADY_CLOSED: Refused = (StatusCode::CONFLICT, "The account is already closed");

struct State {
    email: String,
    closed: bool,
    purged: bool,
}

fn state_of(conn: &Connection, account_id: &str) -> Result<Option<State>> {
    Ok(conn
        .query_row("SELECT email, closed_at, purged_at FROM accounts WHERE id = ?", [account_id], |row| {
            Ok(State {
                email: row.get(0)?,
                closed: row.get::<_, Option<i64>>(1)?.is_some(),
                purged: row.get::<_, Option<i64>>(2)?.is_some(),
            })
        })
        .optional()?)
}

/// Closes the account, and marks the notice acted on, if one is given.
/// Answers the owner's email, for the closed mail; the caller closes the
/// sockets and sends it.
pub fn close_account(
    conn: &mut Connection,
    account_id: &str,
    reason: &str,
    notice_id: Option<&str>,
    now: i64,
) -> Result<Result<String, Refused>> {
    let tx = conn.transaction()?;
    let Some(account) = state_of(&tx, account_id)? else { return Ok(Err(NO_ACCOUNT)) };
    let closed = tx.execute(
        "UPDATE accounts SET closed_at = ?, closed_reason = ? WHERE id = ? AND closed_at IS NULL",
        params![now, reason, account_id],
    )?;
    if account.closed || closed != 1 {
        return Ok(Err(ALREADY_CLOSED));
    }
    for table in ["sessions", "email_links", "pairings"] {
        tx.execute(&format!("DELETE FROM {table} WHERE account_id = ?"), [account_id])?;
    }
    // Signed out as a TV signs itself out (tv.rs): their tokens open nothing,
    // even if the account is opened again, and they stay in the signed
    // manifest until the owner removes them.
    tx.execute(
        "UPDATE devices SET token_hash = 'signed-out:' || id, signed_out_at = ?
         WHERE account_id = ? AND signed_out_at IS NULL",
        params![now, account_id],
    )?;
    if let Some(notice_id) = notice_id {
        // Added to what was already done with the notice, not put in its place.
        tx.execute(
            "UPDATE notices SET status = 'actioned', handled_at = ?1,
               action = CASE WHEN action = '' THEN ?2 ELSE action || char(10) || ?2 END
             WHERE id = ?3",
            params![now, format!("Account closed: {reason}"), notice_id],
        )?;
    }
    tx.commit()?;
    Ok(Ok(account.email))
}

/// Opens a closed account again, while its data is still there. Answers the
/// owner's email, for the reopened mail.
pub fn reopen_account(conn: &Connection, account_id: &str) -> Result<Result<String, Refused>> {
    let Some(account) = state_of(conn, account_id)? else { return Ok(Err(NO_ACCOUNT)) };
    if !account.closed {
        return Ok(Err((StatusCode::CONFLICT, "The account isn't closed")));
    }
    if account.purged {
        return Ok(Err((StatusCode::CONFLICT, "The account's data is already deleted; it can't be opened again")));
    }
    conn.execute(
        "UPDATE accounts SET closed_at = NULL, closed_reason = NULL WHERE id = ? AND purged_at IS NULL",
        [account_id],
    )?;
    Ok(Ok(account.email))
}

/// For the daily clean-up: deletes the data of accounts closed more than 30
/// days ago, up to 100 at a time. The email, the dates and the reason stay;
/// the password's salt and hash go.
pub fn purge_closed_accounts(conn: &Connection, now: i64) -> Result<usize> {
    let ids: Vec<String> = conn
        .prepare(
            "SELECT id FROM accounts WHERE closed_at IS NOT NULL AND closed_at < ? AND purged_at IS NULL LIMIT 100",
        )?
        .query_map([now - CLOSED_KEEP_MS], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    for id in &ids {
        vault::reset(conn, id)?;
        for table in ["devices", "pairings", "sessions", "email_links"] {
            conn.execute(&format!("DELETE FROM {table} WHERE account_id = ?"), [id])?;
        }
        conn.execute("UPDATE accounts SET purged_at = ?, salt = '', auth_hash = '' WHERE id = ?", params![now, id])?;
    }
    Ok(ids.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    #[tokio::test]
    async fn closes_reopens_and_purges() {
        let db = Db::in_memory().unwrap();
        db.call(|conn| {
            conn.execute_batch(
                "INSERT INTO accounts (id, email, confirmed_at, salt, kdf_memory_kib, kdf_iterations,
                   kdf_parallelism, auth_hash, terms_accepted_at, created_at)
                 VALUES ('a', 'a@example.invalid', 1, 's', 65536, 3, 1, 'h', 0, 0);
                 INSERT INTO vault_owner (account_id, bundle, sign_public_key, box_public_key, updated_at)
                 VALUES ('a', '{}', 'k', 'k', 0);",
            )?;
            assert_eq!(close_account(conn, "nobody", "r", None, 0)?, Err(NO_ACCOUNT));
            assert_eq!(close_account(conn, "a", "r", None, 0)?.as_deref(), Ok("a@example.invalid"));
            assert_eq!(close_account(conn, "a", "r", None, 0)?, Err(ALREADY_CLOSED));
            assert!(reopen_account(conn, "a")?.is_ok());
            assert_eq!(reopen_account(conn, "a")?, Err((StatusCode::CONFLICT, "The account isn't closed")));

            close_account(conn, "a", "r", None, DAY_MS)?.unwrap();
            assert_eq!(purge_closed_accounts(conn, DAY_MS + CLOSED_KEEP_MS)?, 0);
            assert_eq!(purge_closed_accounts(conn, DAY_MS + CLOSED_KEEP_MS + 1)?, 1);
            let owners: i64 = conn.query_row("SELECT count(*) FROM vault_owner", [], |row| row.get(0))?;
            assert_eq!(owners, 0);
            assert!(reopen_account(conn, "a")?.is_err());
            Ok(())
        })
        .await
        .unwrap();
    }
}
