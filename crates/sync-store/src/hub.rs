//! The in-process event hub that wakes waiting pulls after a push commits
//! (§15). SQLite mode runs one instance, so the hub alone carries every
//! change; PostgreSQL mode feeds it from NOTIFY as well (ticket 45).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use sync_protocol::types::Id;
use tokio::sync::watch;

/// Wakes the pulls waiting on a workspace.
#[derive(Clone, Default)]
pub struct EventHub {
    workspaces: Arc<Mutex<HashMap<Id, watch::Sender<u64>>>>,
}

impl EventHub {
    /// A receiver that sees every change to `workspace` notified after this
    /// call. Subscribe before reading, so no commit slips between the read
    /// and the wait.
    pub fn subscribe(&self, workspace: Id) -> watch::Receiver<u64> {
        let mut workspaces = self
            .workspaces
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        workspaces
            .entry(workspace)
            .or_insert_with(|| watch::channel(0).0)
            .subscribe()
    }

    /// Wakes every pull waiting on `workspace`. Call it only after the
    /// change commits.
    pub fn notify(&self, workspace: Id) {
        let mut workspaces = self
            .workspaces
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(sender) = workspaces.get(&workspace) {
            sender.send_modify(|version| *version = version.wrapping_add(1));
            if sender.receiver_count() == 0 {
                workspaces.remove(&workspace);
            }
        }
    }
}
