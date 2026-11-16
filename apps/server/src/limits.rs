//! Rate limits, in memory (PLAN.md, "Moving off Cloudflare", "Stack"): the
//! same limits and keys as the Worker's bindings. One process, so no shared
//! store is needed. A restart forgets the counts, which only errs toward a
//! fresh minute; bcrypt on the server is what makes each guess costly.
//!
//! Each key's window starts at its first request, so a burst can't straddle
//! two windows to get twice the limit.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Limit {
    /// Sign-in, /prelogin, settings, pairing and the operator's token: 20 a minute.
    Auth,
    /// Emails to one address: 5 a minute.
    Email,
    /// Sign-ups, reset requests and notices: 5 a minute.
    Signup,
}

impl Limit {
    const fn allowed(self) -> u32 {
        match self {
            Limit::Auth => 20,
            Limit::Email | Limit::Signup => 5,
        }
    }
}

const WINDOW: Duration = Duration::from_secs(60);
/// Expired windows are dropped once the map holds this many keys.
const PRUNE_AT: usize = 10_000;

#[derive(Default)]
pub struct Limits {
    windows: Mutex<HashMap<(Limit, String), (Instant, u32)>>,
}

impl Limits {
    /// True if this key may go ahead. Keys are prefixed by what they count,
    /// such as `signin:<ip>`, as one limit counts several things.
    pub fn within(&self, limit: Limit, key: &str) -> bool {
        self.within_at(limit, key, Instant::now())
    }

    fn within_at(&self, limit: Limit, key: &str, now: Instant) -> bool {
        let mut windows = self.windows.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if windows.len() >= PRUNE_AT {
            windows.retain(|_, (started, _)| now.duration_since(*started) < WINDOW);
        }
        let window = windows.entry((limit, key.to_owned())).or_insert((now, 0));
        if now.duration_since(window.0) >= WINDOW {
            *window = (now, 0);
        }
        if window.1 >= limit.allowed() {
            return false;
        }
        window.1 += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_each_key_for_a_minute() {
        let limits = Limits::default();
        let start = Instant::now();
        for _ in 0..5 {
            assert!(limits.within_at(Limit::Signup, "signup:10.0.0.1", start));
        }
        assert!(!limits.within_at(Limit::Signup, "signup:10.0.0.1", start + Duration::from_secs(59)));
        // Another key, or another limit with the same key, counts on its own.
        assert!(limits.within_at(Limit::Signup, "signup:10.0.0.2", start));
        assert!(limits.within_at(Limit::Email, "signup:10.0.0.1", start));
        // A minute after the first request, the key starts again.
        assert!(limits.within_at(Limit::Signup, "signup:10.0.0.1", start + WINDOW));
    }

    #[test]
    fn allows_twenty_auth_tries() {
        let limits = Limits::default();
        let now = Instant::now();
        let allowed = (0..21).filter(|_| limits.within_at(Limit::Auth, "signin:ip", now)).count();
        assert_eq!(allowed, 20);
    }

    #[test]
    fn drops_old_windows_when_full() {
        let limits = Limits::default();
        let start = Instant::now();
        for i in 0..PRUNE_AT {
            limits.within_at(Limit::Auth, &i.to_string(), start);
        }
        limits.within_at(Limit::Auth, "late", start + WINDOW);
        assert_eq!(limits.windows.lock().unwrap().len(), 1);
    }
}
