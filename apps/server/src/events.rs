//! The TVs' open sockets (GET /v1/events; what each account's Durable Object
//! kept with its hibernating WebSockets). In memory, as the server is one
//! process: a socket only lives as long as the process anyway, and a TV
//! reconnects and catches up from the first "rev N".
//!
//!   on connecting      "rev N"
//!   after a change     "rev N", or "removed" and close 4401 for a TV the
//!                      change took off the account
//!   "ping"             "pong"
//!   a third socket     the oldest of the TV's closes 4409, "replaced"

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use tokio::sync::mpsc;

// sync-core's events.ts.
pub const EVENTS_PROTOCOL: &str = "velio-events.1";
const BEARER_PREFIX: &str = "bearer.";
const PING: &str = "ping";
const PONG: &str = "pong";
const REMOVED_EVENT: &str = "removed";
pub const CLOSE_REMOVED: u16 = 4401;
const CLOSE_REPLACED: u16 = 4409;

/// A TV keeps this many sockets at most; a new one closes the oldest.
const SOCKETS_PER_TV: usize = 2;
/// How long a closed socket waits for the TV's close frame before letting go.
const CLOSING_WAIT: Duration = Duration::from_secs(5);

/// The device token from a Sec-WebSocket-Protocol header, if it offers
/// EVENTS_PROTOCOL and a bearer token (sync-core's readEventsProtocols).
pub fn read_events_protocols(header: &str) -> Option<&str> {
    let offered: Vec<&str> = header.split(',').map(str::trim).collect();
    if !offered.contains(&EVENTS_PROTOCOL) {
        return None;
    }
    offered.iter().find_map(|part| part.strip_prefix(BEARER_PREFIX).filter(|token| !token.is_empty()))
}

pub fn rev_event(rev: u64) -> String {
    format!("rev {rev}")
}

/// What a socket's task is told to send.
#[derive(Debug)]
enum Outgoing {
    Text(String),
    /// "removed", then close 4401.
    Removed,
    Replaced,
}

struct Open {
    id: u64,
    device: String,
    send: mpsc::UnboundedSender<Outgoing>,
}

/// Every account's open sockets, oldest first.
#[derive(Default)]
pub struct Sockets {
    accounts: Mutex<HashMap<String, Vec<Open>>>,
    next_id: AtomicU64,
}

/// A socket in the registry; it leaves when dropped.
pub struct Registered {
    sockets: Arc<Sockets>,
    account: String,
    id: u64,
    receive: mpsc::UnboundedReceiver<Outgoing>,
}

impl Drop for Registered {
    fn drop(&mut self) {
        let mut accounts = self.sockets.lock();
        if let Some(open) = accounts.get_mut(&self.account) {
            open.retain(|socket| socket.id != self.id);
            if open.is_empty() {
                accounts.remove(&self.account);
            }
        }
    }
}

impl Sockets {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Vec<Open>>> {
        self.accounts.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Adds a TV's socket, closing its oldest beyond [`SOCKETS_PER_TV`].
    /// Before the upgrade is answered, as the Worker accepted its sockets,
    /// so a change or a signout right after the 101 reaches it.
    pub fn register(self: &Arc<Self>, account: &str, device: &str) -> Registered {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (send, receive) = mpsc::unbounded_channel();
        let mut accounts = self.lock();
        let open = accounts.entry(account.to_owned()).or_default();
        open.push(Open { id, device: device.to_owned(), send });
        let of_device: Vec<u64> =
            open.iter().filter(|socket| socket.device == device).map(|socket| socket.id).collect();
        let replaced = &of_device[..of_device.len().saturating_sub(SOCKETS_PER_TV)];
        open.retain(|socket| {
            if replaced.contains(&socket.id) {
                let _ = socket.send.send(Outgoing::Replaced);
                false
            } else {
                true
            }
        });
        Registered { sockets: Arc::clone(self), account: account.to_owned(), id, receive }
    }

    /// After a change: "rev N" to each TV still on the account, "removed" to the others.
    pub fn notify(&self, account: &str, rev: u64, devices: &[String]) {
        let mut accounts = self.lock();
        let Some(open) = accounts.get_mut(account) else { return };
        open.retain(|socket| {
            if devices.contains(&socket.device) {
                let _ = socket.send.send(Outgoing::Text(rev_event(rev)));
                true
            } else {
                let _ = socket.send.send(Outgoing::Removed);
                false
            }
        });
        if open.is_empty() {
            accounts.remove(account);
        }
    }

    /// One TV signed itself out.
    pub fn close_device(&self, account: &str, device: &str) {
        let mut accounts = self.lock();
        let Some(open) = accounts.get_mut(account) else { return };
        open.retain(|socket| {
            if socket.device == device {
                let _ = socket.send.send(Outgoing::Removed);
                false
            } else {
                true
            }
        });
        if open.is_empty() {
            accounts.remove(account);
        }
    }

    /// The account was reset, deleted or closed: every TV is signed out.
    pub fn close_account(&self, account: &str) {
        for socket in self.lock().remove(account).unwrap_or_default() {
            let _ = socket.send.send(Outgoing::Removed);
        }
    }

    pub fn count(&self, account: &str) -> usize {
        self.lock().get(account).map_or(0, Vec::len)
    }
}

impl Registered {
    /// Runs one TV's socket until it closes. `current` is read after the
    /// socket is registered, so no change falls between it and the
    /// notifications: Some(rev) while the TV is signed in and on the account.
    pub async fn serve<F>(mut self, mut socket: WebSocket, current: F)
    where
        F: Future<Output = Option<u64>>,
    {
        let first = match current.await {
            Some(rev) => Outgoing::Text(rev_event(rev)),
            None => Outgoing::Removed,
        };
        let mut next = Some(first);
        loop {
            let outgoing = match next.take() {
                Some(outgoing) => outgoing,
                None => tokio::select! {
                    outgoing = self.receive.recv() => match outgoing {
                        Some(outgoing) => outgoing,
                        None => return,
                    },
                    incoming = socket.recv() => match incoming {
                        Some(Ok(Message::Text(text))) if text.as_str() == PING => Outgoing::Text(PONG.into()),
                        Some(Ok(Message::Close(frame))) => {
                            // The TV closed: the same code back, as the Worker did.
                            let code = frame.map_or(1000, |frame| frame.code);
                            let code = if code == 1005 || code == 1006 { 1000 } else { code };
                            close(socket, code, "").await;
                            return;
                        }
                        Some(Ok(_)) => continue,
                        Some(Err(_)) | None => return,
                    },
                },
            };
            let sent = match outgoing {
                Outgoing::Text(text) => socket.send(Message::Text(text.into())).await,
                Outgoing::Removed => {
                    let _ = socket.send(Message::Text(REMOVED_EVENT.into())).await;
                    close(socket, CLOSE_REMOVED, "removed").await;
                    return;
                }
                Outgoing::Replaced => {
                    close(socket, CLOSE_REPLACED, "replaced").await;
                    return;
                }
            };
            if sent.is_err() {
                return;
            }
        }
    }
}

/// Sends a close frame, then waits a little for the TV's, so it reads the code.
async fn close(mut socket: WebSocket, code: u16, reason: &'static str) {
    let frame = CloseFrame { code, reason: reason.into() };
    if socket.send(Message::Close(Some(frame))).await.is_err() {
        return;
    }
    let _ = tokio::time::timeout(CLOSING_WAIT, async { while let Some(Ok(_)) = socket.recv().await {} }).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_token_beside_the_protocol() {
        assert_eq!(read_events_protocols("velio-events.1, bearer.abc"), Some("abc"));
        assert_eq!(read_events_protocols("bearer.abc, velio-events.1"), Some("abc"));
        assert_eq!(read_events_protocols("bearer.abc"), None);
        assert_eq!(read_events_protocols("velio-events.1, bearer."), None);
        assert_eq!(read_events_protocols(""), None);
    }

    #[test]
    fn keeps_two_sockets_per_tv_and_tells_each_what_changed() {
        let sockets = Arc::new(Sockets::default());
        let mut first = sockets.register("a", "tv-1");
        let mut second = sockets.register("a", "tv-1");
        let mut other = sockets.register("a", "tv-2");
        let mut third = sockets.register("a", "tv-1");
        assert!(matches!(first.receive.try_recv(), Ok(Outgoing::Replaced)));
        assert_eq!(sockets.count("a"), 3);

        sockets.notify("a", 7, &["tv-1".into()]);
        for socket in [&mut second, &mut third] {
            assert!(matches!(socket.receive.try_recv(), Ok(Outgoing::Text(text)) if text == "rev 7"));
        }
        assert!(matches!(other.receive.try_recv(), Ok(Outgoing::Removed)));
        assert_eq!(sockets.count("a"), 2);

        sockets.close_account("a");
        assert!(matches!(second.receive.try_recv(), Ok(Outgoing::Removed)));
        assert_eq!(sockets.count("a"), 0);
        drop((first, second, third, other));
        assert!(sockets.lock().is_empty());
    }
}
