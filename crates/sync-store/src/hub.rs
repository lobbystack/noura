//! The in-process event hub that wakes waiting pulls after a push commits
//! (§15) and carries presence to the connections on this instance (§23).
//! SQLite mode runs one instance, so the hub alone carries every change;
//! PostgreSQL mode feeds it from NOTIFY as well (ticket 45). Presence
//! passes through and nothing keeps it.

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex, PoisonError};

use sync_protocol::types::Id;
use tokio::sync::{broadcast, watch};

/// How many presence frames wait for a slow connection before it misses
/// the oldest, which the next record of that session replaces anyway.
const RELAY_BACKLOG: usize = 1_024;
/// The fixed bytes ahead of a relayed frame: domain, channel, device.
const RELAY_FIXED: usize = 48;

/// A presence frame on its way to the connections that may see it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relayed {
    /// The key domain of the record.
    pub domain: Id,
    /// The note's presence channel.
    pub channel: Id,
    /// The device that sent it, which doesn't get it back.
    pub device: Id,
    /// The encoded presence or `presence_gone` frame.
    pub frame: Vec<u8>,
}

impl Relayed {
    /// The bytes one instance hands the others.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(RELAY_FIXED + self.frame.len());
        bytes.extend_from_slice(&self.domain.0);
        bytes.extend_from_slice(&self.channel.0);
        bytes.extend_from_slice(&self.device.0);
        bytes.extend_from_slice(&self.frame);
        bytes
    }

    /// Reads [`Self::to_bytes`].
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let (fixed, frame) = bytes.split_at_checked(RELAY_FIXED)?;
        let id = |at: usize| fixed[at..at + 16].try_into().ok().map(Id);
        Some(Self {
            domain: id(0)?,
            channel: id(16)?,
            device: id(32)?,
            frame: frame.to_vec(),
        })
    }
}

/// A receiver that keeps its workspace's entry in the hub. Dropping the last
/// one of a workspace removes the entry, so the hub holds only workspaces
/// someone waits on, whatever IDs requests name.
pub struct Subscription<R> {
    receiver: Option<R>,
    hub: EventHub,
    workspace: Id,
    release: fn(&EventHub, Id),
}

impl<R> Deref for Subscription<R> {
    type Target = R;

    fn deref(&self) -> &R {
        self.receiver.as_ref().expect("held until drop")
    }
}

impl<R> DerefMut for Subscription<R> {
    fn deref_mut(&mut self) -> &mut R {
        self.receiver.as_mut().expect("held until drop")
    }
}

impl<R> Drop for Subscription<R> {
    fn drop(&mut self) {
        drop(self.receiver.take());
        (self.release)(&self.hub, self.workspace);
    }
}

/// Wakes the pulls waiting on a workspace and carries its presence.
#[derive(Clone, Default)]
pub struct EventHub {
    workspaces: Arc<Mutex<HashMap<Id, watch::Sender<u64>>>>,
    presence: Arc<Mutex<HashMap<Id, broadcast::Sender<Relayed>>>>,
}

impl EventHub {
    /// A receiver that sees every change to `workspace` notified after this
    /// call. Subscribe before reading, so no commit slips between the read
    /// and the wait.
    pub fn subscribe(&self, workspace: Id) -> Subscription<watch::Receiver<u64>> {
        let mut workspaces = self
            .workspaces
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let receiver = workspaces
            .entry(workspace)
            .or_insert_with(|| watch::channel(0).0)
            .subscribe();
        Subscription {
            receiver: Some(receiver),
            hub: self.clone(),
            workspace,
            release: Self::release_changes,
        }
    }

    fn release_changes(&self, workspace: Id) {
        let mut workspaces = self
            .workspaces
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if workspaces
            .get(&workspace)
            .is_some_and(|sender| sender.receiver_count() == 0)
        {
            workspaces.remove(&workspace);
        }
    }

    /// Wakes every pull waiting on `workspace`. Call it only after the
    /// change commits.
    pub fn notify(&self, workspace: Id) {
        let workspaces = self
            .workspaces
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(sender) = workspaces.get(&workspace) {
            sender.send_modify(|version| *version = version.wrapping_add(1));
        }
    }

    /// A receiver of the presence frames relayed in `workspace` after this
    /// call.
    pub fn subscribe_presence(&self, workspace: Id) -> Subscription<broadcast::Receiver<Relayed>> {
        let mut presence = self.presence.lock().unwrap_or_else(PoisonError::into_inner);
        let receiver = presence
            .entry(workspace)
            .or_insert_with(|| broadcast::channel(RELAY_BACKLOG).0)
            .subscribe();
        Subscription {
            receiver: Some(receiver),
            hub: self.clone(),
            workspace,
            release: Self::release_presence,
        }
    }

    fn release_presence(&self, workspace: Id) {
        let mut presence = self.presence.lock().unwrap_or_else(PoisonError::into_inner);
        if presence
            .get(&workspace)
            .is_some_and(|sender| sender.receiver_count() == 0)
        {
            presence.remove(&workspace);
        }
    }

    /// Hands a presence frame to `workspace`'s connections on this
    /// instance.
    pub fn relay(&self, workspace: Id, relayed: Relayed) {
        let presence = self.presence.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(sender) = presence.get(&workspace) {
            let _ = sender.send(relayed);
        }
    }

    /// How many workspaces the hub holds an entry for, as change channels
    /// and presence channels.
    pub fn entries(&self) -> (usize, usize) {
        let workspaces = self
            .workspaces
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len();
        let presence = self
            .presence
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len();
        (workspaces, presence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relayed_frame_round_trips_and_a_short_one_is_refused() {
        let relayed = Relayed {
            domain: Id([1; 16]),
            channel: Id([2; 16]),
            device: Id([3; 16]),
            frame: vec![2, 9],
        };
        assert_eq!(Relayed::from_bytes(&relayed.to_bytes()), Some(relayed));
        assert_eq!(Relayed::from_bytes(&[0; 47]), None);
    }
}
