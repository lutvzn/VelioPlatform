//! The server's side of changing an account (sync-core's `changes.ts`,
//! "The server's side"). The owner's device builds everything; the server
//! checks that a change is signed, starts from the account's revision,
//! stays within the limits, and leaves records that match the new manifest
//! exactly. It can't read any of it.
//!
//! Each check runs in the TypeScript's order, with its words, so the vectors
//! from `packages/sync-core/scripts/vectors.ts` compare exactly.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::crypto::{Signed, from_utf8, verify_json};
use super::read::{Read, ReadProblem, array, count, id, is_format_one, optional_array, problem, record, text};
use super::records::{MAX_DEVICES, MAX_LISTS, MAX_PROFILES, Manifest, empty_manifest, read_manifest};
use crate::tokens::decode;

// --- What the server holds: ciphertext, ids and numbers ---

/// A secretbox or box, as stored and sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sealed {
    pub nonce: String,
    #[serde(rename = "box")]
    pub sealed: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyFor {
    pub profile: String,
    pub key: Sealed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredProfile {
    pub id: String,
    pub rev: u64,
    /// The profile record, sealed under the profile key.
    pub sealed: Sealed,
    /// The profile key, wrapped under the account key.
    pub key: Sealed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredList {
    pub id: String,
    pub rev: u64,
    pub key_rev: u64,
    /// The list, sealed under the list key.
    pub sealed: Sealed,
    /// The list key, wrapped under the account key.
    pub key: Sealed,
    /// The list key, wrapped under the key of each profile that can use it.
    pub profile_keys: Vec<KeyFor>,
}

/// A TV signed in to the account: its profile keys, boxed from the owner's box key to the TV's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredDevice {
    pub id: String,
    pub keys: Vec<KeyFor>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredState {
    pub manifest: Option<Signed>,
    pub profiles: Vec<StoredProfile>,
    pub lists: Vec<StoredList>,
    pub devices: Vec<StoredDevice>,
}

/// One change, as POST /changes takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The manifest revision the change was built on; 409 if the account has moved on.
    pub base_rev: u64,
    pub manifest: Signed,
    pub put_profiles: Vec<StoredProfile>,
    pub delete_profiles: Vec<String>,
    pub put_lists: Vec<StoredList>,
    pub delete_lists: Vec<String>,
    pub put_devices: Vec<StoredDevice>,
    pub delete_devices: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProblemCode {
    /// Built on an older revision: 409.
    Behind,
    Refused,
    /// Past the limits per account.
    Limit,
}

impl ProblemCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ProblemCode::Behind => "behind",
            ProblemCode::Refused => "refused",
            ProblemCode::Limit => "limit",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeProblem {
    pub code: ProblemCode,
    pub message: String,
}

fn refused(message: impl Into<String>) -> ChangeProblem {
    ChangeProblem { code: ProblemCode::Refused, message: message.into() }
}

fn limit(message: impl Into<String>) -> ChangeProblem {
    ChangeProblem { code: ProblemCode::Limit, message: message.into() }
}

// --- Reading changes ---

/// Base64 of 24 bytes.
const NONCE_LENGTH: usize = 32;
/// A wrapped 32-byte key: 4-byte length + key + 16-byte tag, in base64.
const WRAPPED_KEY_LENGTH: usize = 72;
/// A profile record pads to 1 KB; a list to at most about 31 KB (10 long guide URLs).
const MAX_PROFILE_BOX: usize = 4096;
const MAX_LIST_BOX: usize = 48_000;

pub(super) fn read_sealed(value: Option<&Value>, what: &str, max_box: usize) -> Read<Sealed> {
    let item = record(value, what)?;
    let nonce = text(item.get("nonce"), &format!("{what}'s nonce"), NONCE_LENGTH)?;
    let sealed = text(item.get("box"), &format!("{what}'s box"), max_box)?;
    if super::read::js_length(nonce) != NONCE_LENGTH {
        return problem(format!("{what}'s nonce is the wrong size"));
    }
    Ok(Sealed { nonce: nonce.to_owned(), sealed: sealed.to_owned() })
}

fn read_wrapped_key(value: Option<&Value>, what: &str) -> Read<Sealed> {
    let sealed = read_sealed(value, what, WRAPPED_KEY_LENGTH)?;
    if super::read::js_length(&sealed.sealed) != WRAPPED_KEY_LENGTH {
        return problem(format!("{what} is the wrong size"));
    }
    Ok(sealed)
}

fn read_signed(value: Option<&Value>, what: &str) -> Read<Signed> {
    let item = record(value, what)?;
    Ok(Signed {
        body: text(item.get("body"), &format!("{what}'s body"), 64_000)?.to_owned(),
        signature: text(item.get("signature"), &format!("{what}'s signature"), 88)?.to_owned(),
    })
}

fn read_stored_profile(value: &Value) -> Read<StoredProfile> {
    let item = record(Some(value), "A profile")?;
    Ok(StoredProfile {
        id: id(item.get("id"), "A profile's id")?,
        rev: count(item.get("rev"), "A profile's rev")?,
        sealed: read_sealed(item.get("sealed"), "A profile", MAX_PROFILE_BOX)?,
        key: read_wrapped_key(item.get("key"), "A profile's key")?,
    })
}

fn read_keys(value: Option<&Value>, what: &str, each: &str) -> Read<Vec<KeyFor>> {
    array(value, what, MAX_PROFILES)?
        .iter()
        .map(|entry| {
            let key = record(Some(entry), each)?;
            Ok(KeyFor {
                profile: id(key.get("profile"), "A profile id")?,
                key: read_wrapped_key(key.get("key"), each)?,
            })
        })
        .collect()
}

fn read_stored_list(value: &Value) -> Read<StoredList> {
    let item = record(Some(value), "A list")?;
    Ok(StoredList {
        id: id(item.get("id"), "A list's id")?,
        rev: count(item.get("rev"), "A list's rev")?,
        key_rev: count(item.get("keyRev"), "A list's keyRev")?,
        sealed: read_sealed(item.get("sealed"), "A list", MAX_LIST_BOX)?,
        key: read_wrapped_key(item.get("key"), "A list's key")?,
        profile_keys: read_keys(item.get("profileKeys"), "A list's profile keys", "A list's profile key")?,
    })
}

fn read_stored_device(value: &Value) -> Read<StoredDevice> {
    let item = record(Some(value), "A TV")?;
    Ok(StoredDevice {
        id: id(item.get("id"), "A TV's id")?,
        keys: read_keys(item.get("keys"), "A TV's keys", "A TV's profile key")?,
    })
}

fn read_ids(items: &[Value], what: &str) -> Read<Vec<String>> {
    items.iter().map(|item| id(Some(item), what)).collect()
}

pub fn read_change(value: &Value) -> Read<Change> {
    let item = record(Some(value), "The change")?;
    if !is_format_one(item.get("format")) {
        return problem("The change has an unknown format");
    }
    Ok(Change {
        base_rev: count(item.get("baseRev"), "The change's baseRev")?,
        manifest: read_signed(item.get("manifest"), "The change's manifest")?,
        put_profiles: array(item.get("putProfiles"), "The changed profiles", MAX_PROFILES)?
            .iter()
            .map(read_stored_profile)
            .collect::<Read<_>>()?,
        delete_profiles: read_ids(
            array(item.get("deleteProfiles"), "The deleted profiles", MAX_PROFILES)?,
            "A deleted profile",
        )?,
        put_lists: array(item.get("putLists"), "The changed lists", MAX_LISTS)?
            .iter()
            .map(read_stored_list)
            .collect::<Read<_>>()?,
        delete_lists: read_ids(array(item.get("deleteLists"), "The deleted lists", MAX_LISTS)?, "A deleted list")?,
        put_devices: optional_array(item.get("putDevices"), "The changed TVs", MAX_DEVICES)?
            .iter()
            .map(read_stored_device)
            .collect::<Read<_>>()?,
        delete_devices: read_ids(
            optional_array(item.get("deleteDevices"), "The removed TVs", MAX_DEVICES)?,
            "A removed TV",
        )?,
    })
}

/// The manifest's body without checking its signature: the server reading what it stored itself.
pub fn stored_manifest(stored: &StoredState, account: &str) -> Result<Manifest, String> {
    let Some(signed) = &stored.manifest else { return Ok(empty_manifest(account)) };
    let body = decode(&signed.body).ok_or("The stored manifest isn't base64")?;
    let value: Value = serde_json::from_str(&from_utf8(&body)?).map_err(|error| error.to_string())?;
    read_manifest(&value).map_err(|ReadProblem(message)| message)
}

// --- Checking ---

fn same_set(a: &[&str], b: &[&str]) -> bool {
    let set: HashSet<&str> = a.iter().copied().collect();
    set.len() == a.len() && a.len() == b.len() && b.iter().all(|item| set.contains(item))
}

/// Records in the order they were made, by id, as a JavaScript Map keeps them.
struct Ordered<T> {
    items: Vec<T>,
}

impl<T: Clone> Ordered<T> {
    fn new(items: &[T]) -> Self {
        Self { items: items.to_vec() }
    }

    fn position(&self, id: &str, id_of: fn(&T) -> &str) -> Option<usize> {
        self.items.iter().position(|item| id_of(item) == id)
    }

    fn delete(&mut self, id: &str, id_of: fn(&T) -> &str) -> bool {
        self.position(id, id_of).map(|at| self.items.remove(at)).is_some()
    }

    fn set(&mut self, item: T, id_of: fn(&T) -> &str) {
        match self.position(id_of(&item), id_of) {
            Some(at) => self.items[at] = item,
            None => self.items.push(item),
        }
    }
}

fn profile_id(profile: &StoredProfile) -> &str {
    &profile.id
}

fn list_id(list: &StoredList) -> &str {
    &list.id
}

fn device_id(device: &StoredDevice) -> &str {
    &device.id
}

/// Checks that stored records are exactly the ones a manifest names, at its revisions.
pub fn check_matches(
    profiles: &[StoredProfile],
    lists: &[StoredList],
    devices: &[StoredDevice],
    manifest: &Manifest,
) -> Result<(), ChangeProblem> {
    let by_profile: HashMap<&str, &StoredProfile> = profiles.iter().map(|p| (p.id.as_str(), p)).collect();
    if by_profile.len() != profiles.len() {
        return Err(refused("A profile is stored twice"));
    }
    let profile_ids: Vec<&str> = by_profile.keys().copied().collect();
    let named: Vec<&str> = manifest.profiles.iter().map(|p| p.id.as_str()).collect();
    if !same_set(&profile_ids, &named) {
        return Err(refused("The profiles don't match the manifest"));
    }
    for entry in &manifest.profiles {
        if by_profile.get(entry.id.as_str()).map(|p| p.rev) != Some(entry.rev) {
            return Err(refused(format!("Profile {} isn't at the manifest's revision", entry.id)));
        }
    }

    let by_list: HashMap<&str, &StoredList> = lists.iter().map(|l| (l.id.as_str(), l)).collect();
    if by_list.len() != lists.len() {
        return Err(refused("A list is stored twice"));
    }
    let list_ids: Vec<&str> = by_list.keys().copied().collect();
    let named: Vec<&str> = manifest.lists.iter().map(|l| l.id.as_str()).collect();
    if !same_set(&list_ids, &named) {
        return Err(refused("The lists don't match the manifest"));
    }
    for entry in &manifest.lists {
        let list = by_list.get(entry.id.as_str()).filter(|l| l.rev == entry.rev && l.key_rev == entry.key_rev);
        let Some(list) = list else {
            return Err(refused(format!("List {} isn't at the manifest's revision", entry.id)));
        };
        let holders: Vec<&str> =
            manifest.profiles.iter().filter(|p| p.lists.contains(&entry.id)).map(|p| p.id.as_str()).collect();
        let keys: Vec<&str> = list.profile_keys.iter().map(|k| k.profile.as_str()).collect();
        if !same_set(&keys, &holders) {
            return Err(refused(format!("List {}'s keys aren't for the profiles the manifest gives it to", entry.id)));
        }
    }

    let by_device: HashMap<&str, &StoredDevice> = devices.iter().map(|d| (d.id.as_str(), d)).collect();
    if by_device.len() != devices.len() {
        return Err(refused("A TV is stored twice"));
    }
    let device_ids: Vec<&str> = by_device.keys().copied().collect();
    let named: Vec<&str> = manifest.devices.iter().map(|d| d.id.as_str()).collect();
    if !same_set(&device_ids, &named) {
        return Err(refused("The TVs don't match the manifest"));
    }
    for entry in &manifest.devices {
        if entry.profiles.iter().any(|p| !by_profile.contains_key(p.as_str())) {
            return Err(refused(format!("TV {} is given a profile the account doesn't have", entry.id)));
        }
        let keys: Vec<&str> = by_device
            .get(entry.id.as_str())
            .map(|d| d.keys.iter().map(|k| k.profile.as_str()).collect())
            .unwrap_or_default();
        let given: Vec<&str> = entry.profiles.iter().map(String::as_str).collect();
        if !same_set(&keys, &given) {
            return Err(refused(format!("TV {}'s keys aren't for the profiles the manifest gives it", entry.id)));
        }
    }
    Ok(())
}

/// The lists a set of profiles can use, in a manifest.
fn reachable<'a>(manifest: &'a Manifest, profile_ids: &[String]) -> Vec<&'a str> {
    let mut lists: Vec<&str> = Vec::new();
    for profile in &manifest.profiles {
        if profile_ids.contains(&profile.id) {
            for list in &profile.lists {
                if !lists.contains(&list.as_str()) {
                    lists.push(list);
                }
            }
        }
    }
    lists
}

/// A TV that loses a profile, or is removed, kept the keys it had, so the
/// owner's device must have made new ones (PLAN.md, "Changing who can use
/// what"). The server sees only that the records were replaced.
fn check_rekeyed(before: &Manifest, after: &Manifest, change: &Change) -> Result<(), ChangeProblem> {
    let put_profiles: HashSet<&str> = change.put_profiles.iter().map(|p| p.id.as_str()).collect();
    let put_devices: HashSet<&str> = change.put_devices.iter().map(|d| d.id.as_str()).collect();
    let profiles_after: HashSet<&str> = after.profiles.iter().map(|p| p.id.as_str()).collect();
    let lists_before: HashMap<&str, _> = before.lists.iter().map(|l| (l.id.as_str(), l)).collect();
    let lists_after: HashMap<&str, _> = after.lists.iter().map(|l| (l.id.as_str(), l)).collect();
    let no_profiles = Vec::new();
    for device in &before.devices {
        let kept = after.devices.iter().find(|d| d.id == device.id).map_or(&no_profiles, |d| &d.profiles);
        for profile in &device.profiles {
            if kept.contains(profile) || !profiles_after.contains(profile.as_str()) {
                continue;
            }
            if !put_profiles.contains(profile.as_str()) {
                return Err(refused(format!("Profile {profile} needs a new key, as TV {} lost it", device.id)));
            }
            for other in &after.devices {
                if other.profiles.contains(profile) && !put_devices.contains(other.id.as_str()) {
                    return Err(refused(format!("TV {} needs profile {profile}'s new key", other.id)));
                }
            }
        }
        let can_now = reachable(after, kept);
        for list in reachable(before, &device.profiles) {
            if let (Some(old), Some(now)) = (lists_before.get(list), lists_after.get(list))
                && !can_now.contains(&list)
                && now.key_rev <= old.key_rev
            {
                return Err(refused(format!("List {list} needs a new key, as TV {} can't use it any more", device.id)));
            }
        }
    }
    Ok(())
}

// --- The server's side ---

/// What a change leaves: the state the server will hold, the change, and its manifest.
#[derive(Debug, Clone)]
pub struct Applied {
    pub state: StoredState,
    pub change: Change,
    pub manifest: Manifest,
}

/// Checks a change against what the server holds, and returns what it will
/// hold after. `new_devices` are TVs this change may add: the one being
/// signed in, as the server knows from its pairing. `device_limit` is the
/// account's TV limit, held to MAX_DEVICES.
pub fn apply_change(
    stored: &StoredState,
    input: &Value,
    account: &str,
    sign_public_key: &[u8],
    new_devices: &[String],
    device_limit: usize,
) -> Result<Applied, ChangeProblem> {
    let change = read_change(input).map_err(|ReadProblem(message)| refused(message))?;
    let before = stored_manifest(stored, account).map_err(|error| refused(format!("The stored manifest: {error}")))?;
    let current_rev = before.rev;
    if change.base_rev != current_rev {
        return Err(ChangeProblem {
            code: ProblemCode::Behind,
            message: format!("The change was made on revision {}; the account is at {current_rev}", change.base_rev),
        });
    }

    let manifest = verify_json(sign_public_key, &change.manifest)
        .and_then(|value| read_manifest(&value).map_err(|ReadProblem(message)| format!("ReadProblem: {message}")))
        .map_err(|error| refused(format!("The manifest isn't signed by the owner, or isn't readable: {error}")))?;
    if manifest.account != account {
        return Err(refused("The manifest is for another account"));
    }
    if manifest.rev != current_rev + 1 {
        return Err(refused("The manifest's revision must go up by one"));
    }
    // Only a change that adds TVs is held to the limit, so an account the operator gave fewer TVs than it has still works.
    let device_limit = device_limit.min(MAX_DEVICES);
    if manifest.devices.len() > device_limit && manifest.devices.len() > before.devices.len() {
        let tvs = if device_limit == 1 { "TV" } else { "TVs" };
        return Err(limit(format!("At most {device_limit} {tvs}")));
    }
    if manifest.profiles.len() > MAX_PROFILES {
        return Err(limit(format!("At most {MAX_PROFILES} profiles")));
    }
    if manifest.lists.len() > MAX_LISTS {
        return Err(limit(format!("At most {MAX_LISTS} lists")));
    }

    let touched: Vec<String> = (change.put_profiles.iter().map(|p| format!("p:{}", p.id)))
        .chain(change.delete_profiles.iter().map(|id| format!("p:{id}")))
        .chain(change.put_lists.iter().map(|l| format!("l:{}", l.id)))
        .chain(change.delete_lists.iter().map(|id| format!("l:{id}")))
        .chain(change.put_devices.iter().map(|d| format!("d:{}", d.id)))
        .chain(change.delete_devices.iter().map(|id| format!("d:{id}")))
        .collect();
    if touched.iter().collect::<HashSet<_>>().len() != touched.len() {
        return Err(refused("The change names a record twice"));
    }

    let mut profiles = Ordered::new(&stored.profiles);
    for id in &change.delete_profiles {
        if !profiles.delete(id, profile_id) {
            return Err(refused(format!("Profile {id} doesn't exist")));
        }
    }
    for profile in &change.put_profiles {
        let old = profiles.position(&profile.id, profile_id).map(|at| &profiles.items[at]);
        if old.is_some_and(|old| profile.rev <= old.rev) {
            return Err(refused(format!("Profile {}'s revision must go up", profile.id)));
        }
        profiles.set(profile.clone(), profile_id);
    }

    let mut lists = Ordered::new(&stored.lists);
    for id in &change.delete_lists {
        if !lists.delete(id, list_id) {
            return Err(refused(format!("List {id} doesn't exist")));
        }
    }
    for list in &change.put_lists {
        let old = lists.position(&list.id, list_id).map(|at| &lists.items[at]);
        if old.is_some_and(|old| list.rev <= old.rev || list.key_rev < old.key_rev) {
            return Err(refused(format!("List {}'s revision must go up", list.id)));
        }
        lists.set(list.clone(), list_id);
    }

    let mut devices = Ordered::new(&stored.devices);
    for id in &change.delete_devices {
        if !devices.delete(id, device_id) {
            return Err(refused(format!("TV {id} doesn't exist")));
        }
    }
    for device in &change.put_devices {
        if devices.position(&device.id, device_id).is_none() && !new_devices.contains(&device.id) {
            return Err(refused(format!("TV {} can only be added by signing it in", device.id)));
        }
        devices.set(device.clone(), device_id);
    }
    check_rekeyed(&before, &manifest, &change)?;

    // A deleted profile's copies of list keys go with it; a TV keeps only the keys of the profiles it has.
    let has_profile = |id: &str| profiles.items.iter().any(|p| p.id == id);
    let given: HashMap<&str, &Vec<String>> = manifest.devices.iter().map(|d| (d.id.as_str(), &d.profiles)).collect();
    let next = StoredState {
        manifest: Some(change.manifest.clone()),
        profiles: profiles.items.clone(),
        lists: lists
            .items
            .iter()
            .map(|list| StoredList {
                profile_keys: list.profile_keys.iter().filter(|k| has_profile(&k.profile)).cloned().collect(),
                ..list.clone()
            })
            .collect(),
        devices: devices
            .items
            .iter()
            .map(|device| StoredDevice {
                id: device.id.clone(),
                keys: device
                    .keys
                    .iter()
                    .filter(|k| {
                        has_profile(&k.profile) && given.get(device.id.as_str()).is_some_and(|p| p.contains(&k.profile))
                    })
                    .cloned()
                    .collect(),
            })
            .collect(),
    };
    check_matches(&next.profiles, &next.lists, &next.devices, &manifest)?;
    Ok(Applied { state: next, change, manifest })
}
