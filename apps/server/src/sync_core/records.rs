//! The manifest (sync-core's `records.ts`): signed by the owner, not
//! encrypted, as it holds only ids and numbers, and each TV's name sealed
//! under the account key. Lists and profiles are only ever ciphertext here,
//! so the server reads nothing else of them.

use std::collections::HashSet;

use serde::Serialize;
use serde_json::Value;

use super::changes::{Sealed, read_sealed};
use super::read::{Read, array, count, id, is_format_one, problem, record, text};

/// Per account (PLAN.md, "Limits per account").
pub const MAX_PROFILES: usize = 8;
pub const MAX_LISTS: usize = 50;
/// The most TVs any manifest may name: what still fits the manifest's 64,000
/// characters with 8 profiles and 50 lists (PLAN.md, "TV limits"). Each
/// account has its own limit up to this: DEFAULT_DEVICE_LIMIT, or what the
/// operator gave it.
pub const MAX_DEVICES: usize = 50;
pub const DEFAULT_DEVICE_LIMIT: usize = 10;

/// A TV's sealed name pads to 256 bytes, or 512 for one full of escapes.
const MAX_DEVICE_NAME_BOX: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Manifest {
    pub format: u8,
    pub account: String,
    /// Goes up by one on any change in the account.
    pub rev: u64,
    pub profiles: Vec<ManifestProfile>,
    pub lists: Vec<ManifestList>,
    pub devices: Vec<ManifestDevice>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManifestProfile {
    pub id: String,
    pub rev: u64,
    pub lists: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestList {
    pub id: String,
    pub rev: u64,
    pub key_rev: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestDevice {
    pub id: String,
    pub profiles: Vec<String>,
    /// The TV's box public key; TVs signed in before Phase 6 have none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
    /// The TV's name, sealed under the account key; TVs signed in before names were sealed have none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<Sealed>,
}

pub fn empty_manifest(account: &str) -> Manifest {
    Manifest {
        format: 1,
        account: account.to_owned(),
        rev: 0,
        profiles: Vec::new(),
        lists: Vec::new(),
        devices: Vec::new(),
    }
}

fn no_duplicates<'a>(ids: impl IntoIterator<Item = &'a String>, what: &str) -> Read<()> {
    let mut seen = HashSet::new();
    if ids.into_iter().all(|id| seen.insert(id)) { Ok(()) } else { problem(format!("{what} has an id twice")) }
}

/// `^[A-Za-z0-9+/]{43}=$`: 32 bytes of base64.
fn is_public_key(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 44
        && bytes[43] == b'='
        && bytes[..43].iter().all(|b| b.is_ascii_alphanumeric() || *b == b'+' || *b == b'/')
}

pub fn read_manifest(value: &Value) -> Read<Manifest> {
    let manifest = record(Some(value), "The manifest")?;
    if !is_format_one(manifest.get("format")) {
        return problem("The manifest has an unknown format");
    }
    let account = text(manifest.get("account"), "The manifest's account", 64)?.to_owned();
    let rev = count(manifest.get("rev"), "The manifest's rev")?;
    let profiles = array(manifest.get("profiles"), "The manifest's profiles", MAX_PROFILES)?
        .iter()
        .map(|item| {
            let profile = record(Some(item), "A manifest profile")?;
            let id = id(profile.get("id"), "A manifest profile's id")?;
            let rev = count(profile.get("rev"), "A manifest profile's rev")?;
            let lists = array(profile.get("lists"), "A manifest profile's lists", MAX_LISTS)?
                .iter()
                .map(|list| self::id(Some(list), "A list id"))
                .collect::<Read<Vec<_>>>()?;
            no_duplicates(&lists, "A manifest profile's lists")?;
            Ok(ManifestProfile { id, rev, lists })
        })
        .collect::<Read<Vec<_>>>()?;
    let lists = array(manifest.get("lists"), "The manifest's lists", MAX_LISTS)?
        .iter()
        .map(|item| {
            let list = record(Some(item), "A manifest list")?;
            Ok(ManifestList {
                id: id(list.get("id"), "A manifest list's id")?,
                rev: count(list.get("rev"), "A manifest list's rev")?,
                key_rev: count(list.get("keyRev"), "A manifest list's keyRev")?,
            })
        })
        .collect::<Read<Vec<_>>>()?;
    let devices = array(manifest.get("devices"), "The manifest's devices", MAX_DEVICES)?
        .iter()
        .map(|item| {
            let device = record(Some(item), "A manifest device")?;
            let id = id(device.get("id"), "A manifest device's id")?;
            let profiles = array(device.get("profiles"), "A manifest device's profiles", MAX_PROFILES)?
                .iter()
                .map(|profile| self::id(Some(profile), "A profile id"))
                .collect::<Read<Vec<_>>>()?;
            let public_key = match device.get("publicKey") {
                None => None,
                Some(value) => {
                    let key = text(Some(value), "A manifest device's public key", 44)?;
                    if !is_public_key(key) {
                        return problem("A manifest device's public key isn't 32 bytes of base64");
                    }
                    Some(key.to_owned())
                }
            };
            let name = match device.get("name") {
                None => None,
                Some(value) => Some(read_sealed(Some(value), "A manifest device's name", MAX_DEVICE_NAME_BOX)?),
            };
            Ok(ManifestDevice { id, profiles, public_key, name })
        })
        .collect::<Read<Vec<_>>>()?;
    no_duplicates(profiles.iter().map(|profile| &profile.id), "The manifest's profiles")?;
    no_duplicates(lists.iter().map(|list| &list.id), "The manifest's lists")?;
    no_duplicates(devices.iter().map(|device| &device.id), "The manifest's devices")?;
    let list_ids: HashSet<&String> = lists.iter().map(|list| &list.id).collect();
    for profile in &profiles {
        for list in &profile.lists {
            if !list_ids.contains(list) {
                return problem(format!("The manifest gives a profile list {list}, which it doesn't have"));
            }
        }
    }
    Ok(Manifest { format: 1, account, rev, profiles, lists, devices })
}
