//! The nightly backup (PLAN.md, "Moving off Cloudflare", "Differences, on
//! purpose"): a copy of the database made with `VACUUM INTO`, kept
//! BACKUP_KEEP_DAYS in VELIO_BACKUP_DIR, and sent to S3-compatible storage when
//! VELIO_BACKUP_S3_URL is set. The bucket deletes its copies with a lifecycle
//! rule of the same length (README). The copies hold what the database holds:
//! ciphertext for every list, and the accounts' email addresses.
//!
//! Done by the server itself, as the container has no shell and no cron.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use rusqlite::Connection;

use crate::config::{Backup, S3};
use crate::db::Db;
use crate::time::{DAY_MS, day, iso};
use crate::tokens::{hmac_sha256, sha256_hex};

pub const BACKUP_KEEP_DAYS: i64 = 30;

/// What a backup did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackedUp {
    pub path: PathBuf,
    pub bytes: u64,
    /// Older copies deleted.
    pub pruned: usize,
    pub uploaded: bool,
}

fn name(now: i64) -> String {
    format!("velio-{}.db", day(now))
}

/// The day of a copy's name, such as 2026-11-14 for velio-2026-11-14.db.
fn day_of(name: &str) -> Option<&str> {
    let day = name.strip_prefix("velio-")?.strip_suffix(".db")?;
    let digits =
        |range: std::ops::Range<usize>| day.get(range).is_some_and(|part| part.bytes().all(|b| b.is_ascii_digit()));
    (day.len() == 10 && digits(0..4) && &day[4..5] == "-" && digits(5..7) && &day[7..8] == "-" && digits(8..10))
        .then_some(day)
}

/// Copies the database to `dir`, replacing today's copy if there is one.
pub fn copy_database(conn: &Connection, dir: &Path, now: i64) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("Making {}", dir.display()))?;
    let path = dir.join(name(now));
    // VACUUM INTO refuses a file that exists, and a half-made copy mustn't
    // take the name of a good one.
    let partial = dir.join(format!(".{}.partial", name(now)));
    match std::fs::remove_file(&partial) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    let target = partial.to_str().context("The backup directory's path must be UTF-8")?;
    conn.execute("VACUUM INTO ?", [target]).context("Copying the database")?;
    std::fs::rename(&partial, &path)?;
    Ok(path)
}

/// Deletes the copies in `dir` made BACKUP_KEEP_DAYS or more days before `now`.
pub fn prune(dir: &Path, now: i64) -> Result<usize> {
    let oldest_kept = day(now - (BACKUP_KEEP_DAYS - 1) * DAY_MS);
    let mut pruned = 0;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(day) = name.to_str().and_then(day_of) else { continue };
        if *day < *oldest_kept {
            std::fs::remove_file(entry.path())?;
            pruned += 1;
        }
    }
    Ok(pruned)
}

/// Makes tonight's copy, deletes old ones, and sends it to S3 when set.
pub async fn back_up(db: &Db, backup: &Backup, http: &reqwest::Client, now: i64) -> Result<BackedUp> {
    let dir = backup.dir.clone();
    let path = db.call(move |conn| copy_database(conn, &dir, now)).await?;
    let pruned = prune(&backup.dir, now)?;
    let bytes = std::fs::metadata(&path)?.len();
    let uploaded = match &backup.s3 {
        Some(s3) => {
            upload(http, s3, &path, &name(now), now).await?;
            true
        }
        None => false,
    };
    Ok(BackedUp { path, bytes, pruned, uploaded })
}

// --- S3 ---

/// `20130524T000000Z`.
fn amz_date(ms: i64) -> String {
    let iso = iso(ms);
    let stamp: String = iso[..19].chars().filter(|c| *c != '-' && *c != ':').collect();
    format!("{stamp}Z")
}

/// The Authorization header for a request signed with AWS Signature Version
/// 4. `headers` are lowercase, sorted, and include host; the query is empty.
fn authorization(
    s3: &S3,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    payload_sha256: &str,
    amz_date: &str,
) -> String {
    let date = &amz_date[..8];
    let canonical_headers: String = headers.iter().map(|(name, value)| format!("{name}:{}\n", value.trim())).collect();
    let signed_headers = headers.iter().map(|(name, _)| *name).collect::<Vec<_>>().join(";");
    let canonical_request = format!("{method}\n{path}\n\n{canonical_headers}\n{signed_headers}\n{payload_sha256}");
    let scope = format!("{date}/{}/s3/aws4_request", s3.region);
    let string_to_sign = format!("AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}", sha256_hex(&canonical_request));
    let key = [s3.region.as_str(), "s3", "aws4_request"]
        .iter()
        .fold(hmac_sha256(format!("AWS4{}", s3.secret_access_key), date), hmac_sha256);
    let signature = hex::encode(hmac_sha256(key, string_to_sign));
    format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
        s3.access_key_id
    )
}

async fn upload(http: &reqwest::Client, s3: &S3, file: &Path, name: &str, now: i64) -> Result<()> {
    let body = tokio::fs::read(file).await?;
    let payload = sha256_hex(&body);
    let path = format!("{}/{name}", s3.path);
    let date = amz_date(now);
    let headers = [("host", s3.host.as_str()), ("x-amz-content-sha256", &payload), ("x-amz-date", &date)];
    let response = http
        .put(format!("{}://{}{path}", s3.scheme, s3.host))
        .header("authorization", authorization(s3, "PUT", &path, &headers, &payload, &date))
        .header("x-amz-content-sha256", &payload)
        .header("x-amz-date", &date)
        .body(body)
        .timeout(Duration::from_secs(600))
        .send()
        .await
        .context("Sending the backup to S3")?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        bail!("S3 answered {status} to the backup: {}", text.chars().take(500).collect::<String>());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example_s3() -> S3 {
        S3 {
            scheme: "https".into(),
            host: "examplebucket.s3.amazonaws.com".into(),
            path: String::new(),
            region: "us-east-1".into(),
            access_key_id: "AKIAIOSFODNN7EXAMPLE".into(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into(),
        }
    }

    /// AWS's example "GET Object" request (Signature Version 4, header-based
    /// authentication, Amazon S3 API reference).
    #[test]
    fn signs_as_aws_documents() {
        let empty = sha256_hex(b"");
        let date = "20130524T000000Z";
        let headers = [
            ("host", "examplebucket.s3.amazonaws.com"),
            ("range", "bytes=0-9"),
            ("x-amz-content-sha256", empty.as_str()),
            ("x-amz-date", date),
        ];
        assert_eq!(
            authorization(&example_s3(), "GET", "/test.txt", &headers, &empty, date),
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, \
             SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, \
             Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
        // new Date(1369353600000).toISOString() is 2013-05-24T00:00:00.000Z.
        assert_eq!(amz_date(1_369_353_600_000), date);
    }

    #[test]
    fn copies_and_keeps_30_days() {
        let dir = std::env::temp_dir().join(format!("velio-backup-test-{}", crate::tokens::new_id()));
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (x); INSERT INTO t VALUES (42);").unwrap();
        let now = 1_794_560_703_042; // 2026-11-13
        let path = copy_database(&conn, &dir, now).unwrap();
        assert_eq!(path.file_name().unwrap(), "velio-2026-11-13.db");
        copy_database(&conn, &dir, now).unwrap(); // Today's copy is replaced.
        let copy = Connection::open(&path).unwrap();
        assert_eq!(copy.query_row("SELECT x FROM t", [], |row| row.get::<_, i64>(0)).unwrap(), 42);
        for name in ["velio-2026-10-14.db", "velio-2026-10-15.db", "velio-notes.db", "other-2020-01-01.db"] {
            std::fs::write(dir.join(name), b"").unwrap();
        }
        assert_eq!(prune(&dir, now).unwrap(), 1); // 2026-10-14 is 30 days old.
        let mut left: Vec<String> =
            std::fs::read_dir(&dir).unwrap().map(|entry| entry.unwrap().file_name().into_string().unwrap()).collect();
        left.sort();
        assert_eq!(left, ["other-2020-01-01.db", "velio-2026-10-15.db", "velio-2026-11-13.db", "velio-notes.db"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
