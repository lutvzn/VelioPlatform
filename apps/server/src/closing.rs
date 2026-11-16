//! Closing an account (PLAN.md, "Legal and abuse"): for a valid notice or a
//! broken term, done by the operator through /ops. The user is banned, as
//! Supabase Auth bans one, so it can't sign in or refresh; every session ends,
//! which signs out the website, the apps and the TVs; the owner is told why;
//! and after 30 days the user is deleted, which deletes its profiles, add-ons,
//! lists and everything else by Nuvio's foreign keys. Until then it can be
//! opened again. The row in `velio_private.closed_accounts` stays, so the
//! email can't simply sign up again.

use anyhow::Result;
use axum::http::StatusCode;
use deadpool_postgres::Client;
use deadpool_postgres::GenericClient;
use serde_json::Value;
use tokio_postgres::types::ToSql;
use uuid::Uuid;

use crate::time::DAY_MS;

pub const CLOSED_KEEP_MS: i64 = 30 * DAY_MS;
pub const MAX_REASON_LENGTH: usize = 2000;

/// Why closing or opening was refused: its status and words.
pub type Refused = (StatusCode, &'static str);

const NO_ACCOUNT: Refused = (StatusCode::NOT_FOUND, "No such account");
const ALREADY_CLOSED: Refused = (StatusCode::CONFLICT, "The account is already closed");

/// Every account the operator can see, as one row each, times in
/// milliseconds: the users with an email, and the closed accounts whose user
/// is already deleted. Guests (Nuvio's anonymous users) aren't accounts.
pub const ACCOUNTS: &str = "(
    SELECT u.id, coalesce(u.email, c.email, '') AS email,
        (extract(epoch FROM u.created_at) * 1000)::bigint AS created_at,
        (extract(epoch FROM u.email_confirmed_at) * 1000)::bigint AS confirmed_at,
        c.closed_at, c.reason AS closed_reason, c.purged_at, l.tv_limit
    FROM auth.users u
    LEFT JOIN velio_private.closed_accounts c ON c.user_id = u.id
    LEFT JOIN velio_private.tv_limits l ON l.user_id = u.id
    WHERE NOT u.is_anonymous
    UNION ALL
    SELECT c.user_id, c.email, NULL, NULL, c.closed_at, c.reason, c.purged_at, NULL
    FROM velio_private.closed_accounts c
    WHERE NOT EXISTS (SELECT 1 FROM auth.users u WHERE u.id = c.user_id)
)";

/// The rows of a query as JSON objects, keyed by column.
pub async fn rows_json(db: &impl GenericClient, sql: &str, params: &[&(dyn ToSql + Sync)]) -> Result<Vec<Value>> {
    let row = db.query_one(&format!("SELECT coalesce(json_agg(r), '[]'::json) FROM ({sql}) r"), params).await?;
    let rows: Value = row.get(0);
    Ok(match rows {
        Value::Array(rows) => rows,
        _ => Vec::new(),
    })
}

/// Closes the account, and marks the notice acted on, if one is given.
/// Answers the owner's email, for the closed mail; the caller sends it.
pub async fn close_account(
    client: &mut Client,
    user_id: Uuid,
    reason: &str,
    notice_id: Option<&str>,
    now: i64,
) -> Result<Result<String, Refused>> {
    let tx = client.transaction().await?;
    let Some(user) = tx.query_opt("SELECT coalesce(email, '') FROM auth.users WHERE id = $1", &[&user_id]).await?
    else {
        return Ok(Err(NO_ACCOUNT));
    };
    let email: String = user.get(0);
    let closed = tx
        .execute(
            "INSERT INTO velio_private.closed_accounts (user_id, email, closed_at, reason, notice_id)
             VALUES ($1, $2, $3, $4, $5) ON CONFLICT (user_id) DO NOTHING",
            &[&user_id, &email, &now, &reason, &notice_id],
        )
        .await?;
    if closed != 1 {
        return Ok(Err(ALREADY_CLOSED));
    }
    // Banned as Supabase Auth bans: sign-in and refreshing refuse it.
    tx.execute(
        "UPDATE auth.users SET banned_until = now() + interval '100 years', updated_at = now() WHERE id = $1",
        &[&user_id],
    )
    .await?;
    // Every session: the website's, the apps' and the TVs' go with them.
    tx.execute("DELETE FROM auth.sessions WHERE user_id = $1", &[&user_id]).await?;
    if let Some(notice_id) = notice_id {
        // Added to what was already done with the notice, not put in its place.
        tx.execute(
            "UPDATE velio_private.notices SET status = 'actioned', handled_at = $1,
               action = CASE WHEN action = '' THEN $2 ELSE action || chr(10) || $2 END
             WHERE id = $3",
            &[&now, &format!("Account closed: {reason}"), &notice_id],
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Ok(email))
}

/// Opens a closed account again, while its data is still there. Answers the
/// owner's email, for the reopened mail.
pub async fn reopen_account(client: &mut Client, user_id: Uuid) -> Result<Result<String, Refused>> {
    let tx = client.transaction().await?;
    let closed = tx
        .query_opt(
            "SELECT email, purged_at IS NOT NULL FROM velio_private.closed_accounts WHERE user_id = $1 FOR UPDATE",
            &[&user_id],
        )
        .await?;
    let user = tx.query_opt("SELECT 1 FROM auth.users WHERE id = $1", &[&user_id]).await?;
    let Some(closed) = closed else {
        return Ok(Err(if user.is_some() { (StatusCode::CONFLICT, "The account isn't closed") } else { NO_ACCOUNT }));
    };
    let (email, purged): (String, bool) = (closed.get(0), closed.get(1));
    if purged || user.is_none() {
        return Ok(Err((StatusCode::CONFLICT, "The account's data is already deleted; it can't be opened again")));
    }
    tx.execute("DELETE FROM velio_private.closed_accounts WHERE user_id = $1", &[&user_id]).await?;
    tx.execute("UPDATE auth.users SET banned_until = NULL, updated_at = now() WHERE id = $1", &[&user_id]).await?;
    tx.commit().await?;
    Ok(Ok(email))
}

/// For the daily clean-up: deletes the users of accounts closed more than 30
/// days ago, up to 100 at a time, and with them their data. The email, the
/// dates and the reason stay.
pub async fn purge_closed_accounts(db: &impl GenericClient, now: i64) -> Result<usize> {
    let rows = db
        .query(
            "SELECT user_id FROM velio_private.closed_accounts
             WHERE closed_at < $1 AND purged_at IS NULL ORDER BY closed_at LIMIT 100",
            &[&(now - CLOSED_KEEP_MS)],
        )
        .await?;
    for row in &rows {
        let user_id: Uuid = row.get(0);
        db.execute("DELETE FROM auth.users WHERE id = $1", &[&user_id]).await?;
        db.execute("UPDATE velio_private.closed_accounts SET purged_at = $2 WHERE user_id = $1", &[&user_id, &now])
            .await?;
    }
    Ok(rows.len())
}
