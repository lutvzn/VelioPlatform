//! What each account's Durable Object held, as rows in the vault_* tables
//! (PLAN.md, "Moving off Cloudflare", "Data"): the owner's keys, and the
//! manifest, profiles, lists and TVs, which are only ever ciphertext here.

use std::collections::HashMap;

use anyhow::{Context, Result, anyhow};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::Value;

use crate::accounts::Owner;
use crate::config::TvLimits;
use crate::sync_core::{
    ChangeProblem, KeyFor, ProblemCode, Sealed, Signed, StoredDevice, StoredList, StoredProfile, StoredState, SyncPull,
    apply_change, pull_for,
};
use crate::time::{day, now_ms};
use crate::tokens::{decode, same_digest};

pub const NO_KEYS: &str = "This account's keys are missing. Reset the account to start again.";

/// Every table that holds an account's vault, children first.
const VAULT_TABLES: &[&str] = &[
    "vault_device_keys",
    "vault_devices",
    "vault_list_keys",
    "vault_lists",
    "vault_profiles",
    "vault_manifest",
    "vault_recovery",
    "vault_owner",
];

/// Deletes everything the account's vault holds (a reset, or an account
/// deleted). The caller closes the account's sockets once this is committed
/// (`Sockets::close_account`).
pub fn reset(conn: &Connection, account_id: &str) -> Result<()> {
    for table in VAULT_TABLES {
        conn.execute(&format!("DELETE FROM {table} WHERE account_id = ?"), [account_id])?;
    }
    Ok(())
}

pub fn set_owner(conn: &Connection, account_id: &str, owner: &Owner) -> Result<()> {
    conn.execute(
        "INSERT INTO vault_owner (account_id, bundle, sign_public_key, box_public_key, updated_at)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT (account_id) DO UPDATE SET bundle = excluded.bundle,
           sign_public_key = excluded.sign_public_key, box_public_key = excluded.box_public_key,
           updated_at = excluded.updated_at",
        params![account_id, owner.bundle, owner.sign_public_key, owner.box_public_key, now_ms()],
    )?;
    Ok(())
}

pub fn get_owner(conn: &Connection, account_id: &str) -> Result<Option<Owner>> {
    Ok(conn
        .query_row(
            "SELECT bundle, sign_public_key, box_public_key FROM vault_owner WHERE account_id = ?",
            [account_id],
            |row| Ok(Owner { bundle: row.get(0)?, sign_public_key: row.get(1)?, box_public_key: row.get(2)? }),
        )
        .optional()?)
}

/// When the recovery key was made, or None when it is off.
pub fn recovery_made_at(conn: &Connection, account_id: &str) -> Result<Option<i64>> {
    Ok(conn
        .query_row("SELECT updated_at FROM vault_recovery WHERE account_id = ?", [account_id], |row| row.get(0))
        .optional()?)
}

/// The recovery key's copy of the owner bundle, with its auth key's hash; None turns it off.
pub fn set_recovery(conn: &Connection, account_id: &str, recovery: Option<(&str, &str)>) -> Result<()> {
    match recovery {
        None => conn.execute("DELETE FROM vault_recovery WHERE account_id = ?", [account_id])?,
        Some((auth_hash, bundle)) => conn.execute(
            "INSERT INTO vault_recovery (account_id, auth_hash, bundle, updated_at) VALUES (?, ?, ?, ?)
             ON CONFLICT (account_id) DO UPDATE SET auth_hash = excluded.auth_hash, bundle = excluded.bundle,
               updated_at = excluded.updated_at",
            params![account_id, auth_hash, bundle, now_ms()],
        )?,
    };
    Ok(())
}

/// The owner's public keys with the recovery copy of the bundle, if the recovery key's auth hash matches.
pub fn recovery_bundle(conn: &Connection, account_id: &str, auth_hash: &str) -> Result<Option<Owner>> {
    let row: Option<(String, String)> = conn
        .query_row("SELECT auth_hash, bundle FROM vault_recovery WHERE account_id = ?", [account_id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()?;
    let Some((_, bundle)) = row.filter(|(stored_hash, _)| same_digest(stored_hash, auth_hash)) else {
        return Ok(None);
    };
    Ok(get_owner(conn, account_id)?.map(|owner| Owner { bundle, ..owner }))
}

/// The manifest's revision, if the TV is in it: what its socket is told on connecting.
pub fn device_rev(conn: &Connection, account_id: &str, device_id: &str) -> Result<Option<u64>> {
    Ok(conn
        .query_row(
            "SELECT m.rev FROM vault_manifest m JOIN vault_devices d ON d.account_id = m.account_id
             WHERE m.account_id = ? AND d.id = ?",
            [account_id, device_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .map(|rev| rev as u64))
}

/// What GET /v1/sync answers a TV (the Durable Object's devicePull).
#[derive(Debug)]
pub enum DevicePull {
    /// Not in the manifest, or the account has none: sign in again.
    Removed,
    /// The TV already has this revision.
    Same(u64),
    Pull(u64, Box<SyncPull>),
}

pub fn device_pull(conn: &Connection, account_id: &str, device_id: &str, known_rev: Option<u64>) -> Result<DevicePull> {
    let Some(rev) = device_rev(conn, account_id, device_id)? else { return Ok(DevicePull::Removed) };
    if known_rev == Some(rev) {
        return Ok(DevicePull::Same(rev));
    }
    Ok(match pull_for(&get_state(conn, account_id)?, device_id) {
        Some(pull) => DevicePull::Pull(rev, Box::new(pull)),
        None => DevicePull::Removed,
    })
}

/// A TV signed in to the account, as the account page lists it.
#[derive(Debug, Clone)]
pub struct DeviceRow {
    pub id: String,
    pub created_at: i64,
    pub last_pulled_at: Option<i64>,
    /// Set when the TV signed itself out (POST /v1/signout); it stays in the manifest until removed.
    pub signed_out_at: Option<i64>,
}

pub fn devices_of(conn: &Connection, account_id: &str) -> Result<Vec<DeviceRow>> {
    let mut statement = conn.prepare(
        "SELECT id, created_at, last_pulled_at, signed_out_at FROM devices
         WHERE account_id = ? ORDER BY created_at",
    )?;
    let rows = statement.query_map([account_id], |row| {
        Ok(DeviceRow {
            id: row.get(0)?,
            created_at: row.get(1)?,
            last_pulled_at: row.get(2)?,
            signed_out_at: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The line under a TV's name on the account page and the TVs page.
pub fn device_status(device: &DeviceRow) -> String {
    match device.signed_out_at {
        None => {
            let synced = device.last_pulled_at.map(|at| format!(", last synced {}", day(at))).unwrap_or_default();
            format!("Signed in {}{synced}", day(device.created_at))
        }
        Some(at) => format!("Signed out {}. Remove it to take it off the account.", day(at)),
    }
}

fn sealed(json: &str) -> rusqlite::Result<Sealed> {
    serde_json::from_str(json)
        .map_err(|error| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error)))
}

fn json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("records are JSON")
}

/// The keys of each list or TV, in the order of their profiles.
fn keys_by(conn: &Connection, sql: &str, account_id: &str) -> Result<HashMap<String, Vec<KeyFor>>> {
    let mut keys: HashMap<String, Vec<KeyFor>> = HashMap::new();
    let mut statement = conn.prepare(sql)?;
    let rows = statement.query_map([account_id], |row| {
        Ok((row.get::<_, String>(0)?, KeyFor { profile: row.get(1)?, key: sealed(&row.get::<_, String>(2)?)? }))
    })?;
    for row in rows {
        let (owner, key) = row?;
        keys.entry(owner).or_default().push(key);
    }
    Ok(keys)
}

/// Everything the account's vault holds, as sync-core reads it.
pub fn get_state(conn: &Connection, account_id: &str) -> Result<StoredState> {
    let manifest = conn
        .query_row("SELECT signed FROM vault_manifest WHERE account_id = ?", [account_id], |row| {
            row.get::<_, String>(0)
        })
        .optional()?
        .map(|signed| serde_json::from_str::<Signed>(&signed).context("The stored manifest isn't readable"))
        .transpose()?;
    let profiles = conn
        .prepare("SELECT id, rev, sealed, key FROM vault_profiles WHERE account_id = ? ORDER BY position")?
        .query_map([account_id], |row| {
            Ok(StoredProfile {
                id: row.get(0)?,
                rev: row.get::<_, i64>(1)? as u64,
                sealed: sealed(&row.get::<_, String>(2)?)?,
                key: sealed(&row.get::<_, String>(3)?)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut list_keys = keys_by(
        conn,
        "SELECT k.list_id, k.profile_id, k.key FROM vault_list_keys k
         JOIN vault_profiles p ON p.account_id = k.account_id AND p.id = k.profile_id
         WHERE k.account_id = ?1 ORDER BY p.position",
        account_id,
    )?;
    let lists = conn
        .prepare("SELECT id, rev, key_rev, sealed, key FROM vault_lists WHERE account_id = ? ORDER BY position")?
        .query_map([account_id], |row| {
            Ok(StoredList {
                id: row.get(0)?,
                rev: row.get::<_, i64>(1)? as u64,
                key_rev: row.get::<_, i64>(2)? as u64,
                sealed: sealed(&row.get::<_, String>(3)?)?,
                key: sealed(&row.get::<_, String>(4)?)?,
                profile_keys: Vec::new(),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .map(|list| StoredList { profile_keys: list_keys.remove(&list.id).unwrap_or_default(), ..list })
        .collect();
    let mut device_keys = keys_by(
        conn,
        "SELECT k.device_id, k.profile_id, k.key FROM vault_device_keys k
         JOIN vault_profiles p ON p.account_id = k.account_id AND p.id = k.profile_id
         WHERE k.account_id = ?1 ORDER BY p.position",
        account_id,
    )?;
    let devices = conn
        .prepare("SELECT id FROM vault_devices WHERE account_id = ? ORDER BY position")?
        .query_map([account_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .map(|id| StoredDevice { keys: device_keys.remove(&id).unwrap_or_default(), id })
        .collect();
    Ok(StoredState { manifest, profiles, lists, devices })
}

/// How many TVs the account may have: what the operator gave it, or the default.
pub fn tv_limit(conn: &Connection, account_id: &str, tv_limits: TvLimits) -> Result<usize> {
    let granted = conn
        .query_row("SELECT tv_limit FROM accounts WHERE id = ?", [account_id], |row| row.get::<_, Option<i64>>(0))
        .optional()?
        .flatten();
    Ok(tv_limits.for_account(granted))
}

/// A change the vault took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changed {
    pub rev: u64,
    /// TVs the change removed, which are also gone from `devices`.
    pub removed_devices: Vec<String>,
    /// Every TV in the manifest after it: these are told the revision, the others are signed out.
    pub devices: Vec<String>,
}

/// Applies one change from the owner (sync-core's applyChange), all or
/// nothing. A ChangeProblem is "behind" when another tab or device changed
/// the account first. The caller tells the TVs once it's committed
/// (`Sockets::notify`).
pub fn change(
    conn: &mut Connection,
    account_id: &str,
    input: &Value,
    new_devices: &[String],
    tv_limits: TvLimits,
) -> Result<Result<Changed, ChangeProblem>> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let Some(owner) = get_owner(&tx, account_id)? else {
        return Ok(Err(ChangeProblem { code: ProblemCode::Refused, message: "The account has no owner keys".into() }));
    };
    let sign_public_key =
        decode(&owner.sign_public_key).ok_or_else(|| anyhow!("The owner's signing key isn't base64"))?;
    let device_limit = tv_limit(&tx, account_id, tv_limits)?;
    let applied = match apply_change(
        &get_state(&tx, account_id)?,
        input,
        account_id,
        &sign_public_key,
        new_devices,
        device_limit,
    ) {
        Ok(applied) => applied,
        Err(problem) => return Ok(Err(problem)),
    };
    let (change, manifest) = (&applied.change, &applied.manifest);
    let next_position = |table: &str| -> rusqlite::Result<i64> {
        tx.query_row(
            &format!("SELECT COALESCE(MAX(position), 0) + 1 FROM {table} WHERE account_id = ?"),
            [account_id],
            |row| row.get(0),
        )
    };
    for profile_id in &change.delete_profiles {
        tx.execute("DELETE FROM vault_profiles WHERE account_id = ? AND id = ?", [account_id, profile_id])?;
        tx.execute("DELETE FROM vault_list_keys WHERE account_id = ? AND profile_id = ?", [account_id, profile_id])?;
        tx.execute("DELETE FROM vault_device_keys WHERE account_id = ? AND profile_id = ?", [account_id, profile_id])?;
    }
    for profile in &change.put_profiles {
        tx.execute(
            "INSERT INTO vault_profiles (account_id, id, rev, sealed, key, position) VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT (account_id, id) DO UPDATE SET rev = excluded.rev, sealed = excluded.sealed, key = excluded.key",
            params![
                account_id,
                profile.id,
                profile.rev as i64,
                json(&profile.sealed),
                json(&profile.key),
                next_position("vault_profiles")?
            ],
        )?;
    }
    for list_id in &change.delete_lists {
        tx.execute("DELETE FROM vault_lists WHERE account_id = ? AND id = ?", [account_id, list_id])?;
        tx.execute("DELETE FROM vault_list_keys WHERE account_id = ? AND list_id = ?", [account_id, list_id])?;
    }
    for list in &change.put_lists {
        tx.execute(
            "INSERT INTO vault_lists (account_id, id, rev, key_rev, sealed, key, position) VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (account_id, id) DO UPDATE SET rev = excluded.rev, key_rev = excluded.key_rev,
               sealed = excluded.sealed, key = excluded.key",
            params![
                account_id,
                list.id,
                list.rev as i64,
                list.key_rev as i64,
                json(&list.sealed),
                json(&list.key),
                next_position("vault_lists")?
            ],
        )?;
        tx.execute("DELETE FROM vault_list_keys WHERE account_id = ? AND list_id = ?", [account_id, &list.id])?;
        for entry in &list.profile_keys {
            tx.execute(
                "INSERT INTO vault_list_keys (account_id, list_id, profile_id, key) VALUES (?, ?, ?, ?)",
                params![account_id, list.id, entry.profile, json(&entry.key)],
            )?;
        }
    }
    for device_id in &change.delete_devices {
        tx.execute("DELETE FROM vault_devices WHERE account_id = ? AND id = ?", [account_id, device_id])?;
        tx.execute("DELETE FROM vault_device_keys WHERE account_id = ? AND device_id = ?", [account_id, device_id])?;
        // forgetDevices(): the TV's sign-in goes with it.
        tx.execute("DELETE FROM devices WHERE account_id = ? AND id = ?", [account_id, device_id])?;
    }
    for device in &change.put_devices {
        tx.execute(
            "INSERT INTO vault_devices (account_id, id, position) VALUES (?, ?, ?) ON CONFLICT (account_id, id) DO NOTHING",
            params![account_id, device.id, next_position("vault_devices")?],
        )?;
        tx.execute("DELETE FROM vault_device_keys WHERE account_id = ? AND device_id = ?", [account_id, &device.id])?;
        for entry in &device.keys {
            tx.execute(
                "INSERT INTO vault_device_keys (account_id, device_id, profile_id, key) VALUES (?, ?, ?, ?)",
                params![account_id, device.id, entry.profile, json(&entry.key)],
            )?;
        }
    }
    // A TV a profile was taken from isn't always in putDevices: drop the keys the manifest no longer gives it.
    for entry in &manifest.devices {
        tx.execute(
            "DELETE FROM vault_device_keys WHERE account_id = ? AND device_id = ?
               AND profile_id NOT IN (SELECT value FROM json_each(?))",
            params![account_id, entry.id, json(&entry.profiles)],
        )?;
    }
    tx.execute(
        "INSERT INTO vault_manifest (account_id, rev, signed, updated_at) VALUES (?, ?, ?, ?)
         ON CONFLICT (account_id) DO UPDATE SET rev = excluded.rev, signed = excluded.signed,
           updated_at = excluded.updated_at",
        params![account_id, manifest.rev as i64, json(&change.manifest), now_ms()],
    )?;
    tx.commit()?;
    Ok(Ok(Changed {
        rev: manifest.rev,
        removed_devices: change.delete_devices.clone(),
        devices: manifest.devices.iter().map(|device| device.id.clone()).collect(),
    }))
}

#[cfg(test)]
mod tests {
    use rusqlite::types::ValueRef;

    use super::*;
    use crate::db::Db;

    /// The accepted changes of the vectors, one after another, through the tables.
    #[tokio::test]
    async fn keeps_what_sync_core_gives_and_reads_it_back() {
        let vectors: Value = serde_json::from_str(include_str!("../tests/vectors.json")).unwrap();
        let db = Db::in_memory().unwrap();
        let dump = db
            .call(move |conn| {
                let account = vectors["account"].as_str().unwrap();
                conn.execute(
                    "INSERT INTO accounts (id, email, salt, kdf_memory_kib, kdf_iterations, kdf_parallelism,
                       auth_hash, terms_accepted_at, created_at) VALUES (?, 'vectors@example.invalid', '', 0, 0, 0, '', 0, 0)",
                    [account],
                )?;
                let sign_public_key = vectors["signPublicKey"].as_str().unwrap().to_owned();
                set_owner(
                    conn,
                    account,
                    &Owner { bundle: "{}".into(), sign_public_key, box_public_key: String::new() },
                )?;
                let mut at = "empty".to_owned();
                let mut applied = 0;
                for case in vectors["changes"].as_array().unwrap() {
                    let Some(after) = case["expect"]["state"].as_str() else { continue };
                    if case["stored"] != at.as_str() || after == at {
                        continue;
                    }
                    let new_devices: Vec<String> =
                        serde_json::from_value(case.get("newDevices").cloned().unwrap_or_default()).unwrap_or_default();
                    let changed = change(conn, account, &case["input"], &new_devices, TvLimits::default())?;
                    assert!(changed.is_ok(), "{}: {changed:?}", case["name"]);
                    let state = serde_json::to_value(get_state(conn, account)?)?;
                    assert_eq!(state, vectors["states"][after], "{}", case["name"]);
                    at = after.to_owned();
                    applied += 1;
                }
                assert_eq!(at, "TV removed");
                assert!(applied >= 14, "only {applied} changes");
                // The same change again is behind.
                let first = &vectors["changes"][0]["input"];
                let again = change(conn, account, first, &[], TvLimits::default())?.unwrap_err();
                assert_eq!(again.code, ProblemCode::Behind);

                // Everything in the file, as text.
                let tables: Vec<String> = conn
                    .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")?
                    .query_map([], |row| row.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                let mut dump = String::new();
                for table in tables {
                    let mut statement = conn.prepare(&format!("SELECT * FROM {table}"))?;
                    let columns = statement.column_count();
                    let mut rows = statement.query([])?;
                    while let Some(row) = rows.next()? {
                        for column in 0..columns {
                            match row.get_ref(column)? {
                                ValueRef::Text(text) | ValueRef::Blob(text) => {
                                    dump.push_str(&String::from_utf8_lossy(text));
                                }
                                ValueRef::Integer(number) => dump.push_str(&number.to_string()),
                                _ => {}
                            }
                            dump.push('\n');
                        }
                    }
                }
                Ok(dump)
            })
            .await
            .unwrap();
        assert!(dump.contains("tv-1"), "the dump has the tables");
        // The names, hosts, usernames and passwords in scripts/vectors.ts.
        for secret in [
            "Living room",
            "Kids",
            "Home playlist",
            "example.invalid/",
            "secret-token",
            "xtream.",
            "someone",
            "hunter2",
            "Films",
        ] {
            assert!(!dump.contains(secret), "the database holds {secret}");
        }
    }
}
