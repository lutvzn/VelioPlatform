//! A TV's pull, the server's side (sync-core's `device.ts`, `pullFor()`):
//! only what the TV's profiles may read.

use std::collections::HashSet;

use serde::Serialize;

use super::changes::{KeyFor, Sealed, StoredState};
use super::crypto::Signed;

/// What GET /v1/sync answers a TV.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyncPull {
    pub format: u8,
    pub manifest: Signed,
    /// The TV's own id in the manifest.
    pub device: String,
    /// This TV's profile keys, boxed from the owner's box key.
    pub keys: Vec<KeyFor>,
    pub profiles: Vec<PulledProfile>,
    /// Each list a profile of this TV has, with its key wrapped under those profiles' keys only.
    pub lists: Vec<PulledList>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PulledProfile {
    pub id: String,
    pub rev: u64,
    pub sealed: Sealed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PulledList {
    pub id: String,
    pub rev: u64,
    pub key_rev: u64,
    pub sealed: Sealed,
    pub keys: Vec<KeyFor>,
}

/// What one TV may read, or None when the account doesn't name it.
pub fn pull_for(stored: &StoredState, device_id: &str) -> Option<SyncPull> {
    let device = stored.devices.iter().find(|device| device.id == device_id)?;
    let manifest = stored.manifest.clone()?;
    let mine: HashSet<&str> = device.keys.iter().map(|key| key.profile.as_str()).collect();
    Some(SyncPull {
        format: 1,
        manifest,
        device: device_id.to_owned(),
        keys: device.keys.clone(),
        profiles: stored
            .profiles
            .iter()
            .filter(|profile| mine.contains(profile.id.as_str()))
            .map(|profile| PulledProfile { id: profile.id.clone(), rev: profile.rev, sealed: profile.sealed.clone() })
            .collect(),
        lists: stored
            .lists
            .iter()
            .filter(|list| list.profile_keys.iter().any(|key| mine.contains(key.profile.as_str())))
            .map(|list| PulledList {
                id: list.id.clone(),
                rev: list.rev,
                key_rev: list.key_rev,
                sealed: list.sealed.clone(),
                keys: list.profile_keys.iter().filter(|key| mine.contains(key.profile.as_str())).cloned().collect(),
            })
            .collect(),
    })
}
