//! The server's half of `packages/sync-core` (PLAN.md, "Moving off
//! Cloudflare", "Checking it matches"): reading and checking a change from
//! the owner, and picking out a TV's pull. The TypeScript is the reference;
//! `tests/vectors.rs` replays the cases `scripts/vectors.ts` writes from it.

pub mod changes;
pub mod crypto;
pub mod device;
pub mod read;
pub mod records;

pub use changes::{
    Applied, Change, ChangeProblem, KeyFor, ProblemCode, Sealed, StoredDevice, StoredList, StoredProfile, StoredState,
    apply_change, stored_manifest,
};
pub use crypto::Signed;
pub use device::{SyncPull, pull_for};
pub use read::is_id;
pub use records::{DEFAULT_DEVICE_LIMIT, MAX_DEVICES, Manifest, empty_manifest};
