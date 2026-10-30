//! The high-water mark the credential store keeps per workspace
//! (docs/workspace-format/sync-v2.md §14, §15): the accepted policy
//! revision and head, the latest accepted epoch record and checkpoint of
//! each domain, this device's reserved counter per domain and its reserved
//! load number. A journal below it reports `sync_journal_rollback`.
//!
//! The device signs no counter above its reservation: it raises the
//! reservation by a block of 1,024 first, so the credential store takes one
//! write per 1,024 records, and each time the app starts it raises every
//! reservation by one more block and signs its next new record at the
//! start of that block. Whatever order a crash leaves the journal and the
//! credential store in, it never signs one counter twice.

use std::collections::BTreeMap;

use local_core::sync::SyncCredentials;
use serde::{Deserialize, Serialize};
use sync_protocol::types::{Digest, Id};

use super::SyncError;

/// How many counters one reservation takes.
pub const BLOCK: u64 = 1_024;

/// One domain's part of the mark.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainMark {
    /// The latest accepted epoch.
    pub epoch: u64,
    /// That epoch record's digest.
    pub epoch_record: Digest,
    /// `covered_seq` of the latest accepted checkpoint, 0 before the first.
    pub checkpoint_seq: u64,
    /// That checkpoint's digest.
    pub checkpoint: Digest,
    /// The restore generation the device accepted that checkpoint in, so
    /// an approval offer or a recovery kit anchors it with its generation
    /// (§10, §12).
    #[serde(default)]
    pub checkpoint_generation: u64,
    /// The highest counter this device may sign in the domain.
    pub reserved: u64,
}

/// The mark of one workspace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mark {
    /// The accepted policy revision.
    pub policy_revision: u64,
    /// That change's digest.
    pub policy_head: Digest,
    /// Per domain.
    pub domains: BTreeMap<Id, DomainMark>,
    /// The highest load number this device may use (§22).
    pub reserved_load: u64,
}

impl Default for DomainMark {
    fn default() -> Self {
        Self {
            epoch: 0,
            epoch_record: Digest::ZERO,
            checkpoint_seq: 0,
            checkpoint: Digest::ZERO,
            checkpoint_generation: 0,
            reserved: 0,
        }
    }
}

impl Default for Mark {
    fn default() -> Self {
        Self {
            policy_revision: 0,
            policy_head: Digest::ZERO,
            domains: BTreeMap::new(),
            reserved_load: 0,
        }
    }
}

fn name(device: &Id, workspace: &Id) -> String {
    format!(
        "noura.sync.v2.mark.{}.{}",
        device.to_hex(),
        workspace.to_hex()
    )
}

/// The credential store items a device's marks use for `workspaces`, which
/// signing out deletes.
pub fn items(device: &Id, workspaces: impl IntoIterator<Item = Id>) -> Vec<String> {
    workspaces.into_iter().map(|w| name(device, &w)).collect()
}

fn failed(error: impl std::fmt::Display) -> SyncError {
    SyncError::Invalid(format!("the credential store failed: {error}"))
}

impl Mark {
    /// The mark `device` keeps for `workspace`, or `None` before the first.
    pub fn read(
        credentials: &dyn SyncCredentials,
        device: &Id,
        workspace: &Id,
    ) -> Result<Option<Self>, SyncError> {
        let Some(text) = credentials
            .read_optional(&name(device, workspace))
            .map_err(failed)?
        else {
            return Ok(None);
        };
        // A store that can't delete leaves an empty item.
        if text.is_empty() {
            return Ok(None);
        }
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|_| SyncError::Invalid("the stored sync mark doesn't parse".into()))
    }

    /// Stores the mark.
    pub fn write(
        &self,
        credentials: &dyn SyncCredentials,
        device: &Id,
        workspace: &Id,
    ) -> Result<(), SyncError> {
        let text = serde_json::to_string(self).map_err(failed)?;
        credentials
            .write(&name(device, workspace), &text)
            .map_err(failed)
    }

    /// Raises every reservation by one block, as each start of the app
    /// does, and returns the counter each domain's next new record takes:
    /// the start of the new block.
    pub fn start(&mut self) -> BTreeMap<Id, u64> {
        self.domains
            .iter_mut()
            .map(|(domain, mark)| {
                let next = mark.reserved + 1;
                mark.reserved += BLOCK;
                (*domain, next)
            })
            .collect()
    }

    /// Whether `counter` in `domain` lies above the reservation, so the
    /// device raises it before it signs.
    pub fn needs_block(&self, domain: &Id, counter: u64) -> bool {
        self.domains
            .get(domain)
            .is_none_or(|mark| counter > mark.reserved)
    }

    /// Raises `domain`'s reservation by blocks until it covers `counter`.
    pub fn reserve(&mut self, domain: &Id, counter: u64) {
        let mark = self.domains.entry(*domain).or_default();
        while mark.reserved < counter {
            mark.reserved += BLOCK;
        }
    }

    /// Whether the journal's state lies below this mark: an older policy
    /// head, or an older epoch of a domain in `epochs` (§15). A domain the
    /// journal doesn't hold yet, as in a folder that starts fresh, loads its
    /// epochs when the device reaches it.
    pub fn above(&self, policy_revision: u64, epochs: &BTreeMap<Id, u64>) -> bool {
        policy_revision < self.policy_revision
            || self
                .domains
                .iter()
                .any(|(domain, mark)| epochs.get(domain).is_some_and(|epoch| *epoch < mark.epoch))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_start_takes_a_new_block_and_signing_past_it_takes_another() {
        let domain = Id([1; 16]);
        let mut mark = Mark::default();
        assert!(mark.needs_block(&domain, 1));
        mark.reserve(&domain, 1);
        assert_eq!(mark.domains[&domain].reserved, 1_024);
        assert!(!mark.needs_block(&domain, 1_024));
        assert!(mark.needs_block(&domain, 1_025));
        let next = mark.start();
        assert_eq!(next[&domain], 1_025, "the next record starts the new block");
        assert_eq!(mark.domains[&domain].reserved, 2_048);
        mark.reserve(&domain, 3_000);
        assert_eq!(mark.domains[&domain].reserved, 3_072);
    }

    #[test]
    fn a_journal_below_the_mark_is_a_rollback() {
        let domain = Id([1; 16]);
        let mut mark = Mark {
            policy_revision: 4,
            ..Mark::default()
        };
        mark.domains.insert(
            domain,
            DomainMark {
                epoch: 2,
                ..DomainMark::default()
            },
        );
        assert!(mark.above(3, &BTreeMap::from([(domain, 2)])));
        assert!(mark.above(4, &BTreeMap::from([(domain, 1)])));
        assert!(!mark.above(4, &BTreeMap::from([(domain, 2)])));
        assert!(!mark.above(5, &BTreeMap::from([(domain, 3)])));
        assert!(!mark.above(4, &BTreeMap::new()));
    }
}
