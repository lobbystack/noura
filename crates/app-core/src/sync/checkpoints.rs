//! The checkpoints a client reaches in its log, checks and signs
//! (docs/workspace-format/sync-v2.md §17).
//!
//! A client checks each checkpoint it reaches once it has applied every
//! record through `covered_seq`: its heads, its link to the last checkpoint
//! it reached, its signer by position, its policy and epoch fields, and its
//! root, leaf count and heads against the client's own live set. One that
//! fails is reported and never becomes the latest accepted checkpoint, but
//! it doesn't hold the cursor. A writer that applied the log past the due
//! point the server published signs a checkpoint of its own.
//!
//! The state keeps per-record refs only above the latest accepted
//! checkpoint, which is where the next checkpoint's heads come from.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sync_protocol::codec::{Nested, Sorted};
use sync_protocol::keys::{EpochChain, LogJudge};
use sync_protocol::live::LiveSet;
use sync_protocol::manifest::{Manifest, heads_digest, skip_number};
use sync_protocol::policy::PolicyChain;
use sync_protocol::records::{Checkpoint, CheckpointBundle, Ref, Signed};
use sync_protocol::types::{Digest, Id};

/// A checkpoint the client reached in its log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reached {
    /// Its digest.
    pub digest: Digest,
    /// Its `covered_seq`.
    pub covered_seq: u64,
    /// Its `policy_revision`, which the next checkpoint's heads rule reads.
    pub policy_revision: u64,
    /// The sequence of its entry.
    pub seq: u64,
}

/// The latest checkpoint the client accepted, with its heads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Accepted {
    /// The record.
    pub record: Signed<Checkpoint>,
    /// Its heads.
    pub heads: Vec<Ref>,
}

/// A checkpoint in the log that failed a check (§17).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    /// Its number.
    pub number: u64,
    /// Its digest.
    pub digest: Digest,
    /// The client error code.
    pub code: String,
    /// For a root mismatch, the objects whose leaves differ, once the
    /// client walked the tree.
    #[serde(default)]
    pub differing: Vec<Id>,
}

/// What one domain's journal keeps of its checkpoint chain.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Checkpoints {
    /// Every checkpoint the client reached, by number.
    pub reached: BTreeMap<u64, Reached>,
    /// The latest accepted checkpoint.
    pub accepted: Option<Accepted>,
    /// The refs of the records the client accepted above the accepted
    /// checkpoint's `covered_seq`, or from the start before the first.
    pub since: Vec<Ref>,
    /// Policy entries as (sequence, revision), from the latest at or below
    /// the accepted checkpoint's `covered_seq` on.
    pub policy_entries: Vec<(u64, u64)>,
    /// Epoch entries as (sequence, epoch, record digest), likewise.
    pub epoch_entries: Vec<(u64, u64, Digest)>,
    /// The checkpoints that failed a check, reported.
    pub failures: Vec<Failure>,
    /// The `covered_seq` of the checkpoint the device bootstrapped from,
    /// 0 if it pulled from the start (§17, Checkpoint chain).
    #[serde(default)]
    pub bootstrap_seq: u64,
    /// The due point the server published last, 0 while none is due.
    pub due: u64,
    /// Unix seconds after which this device signs a checkpoint if one is
    /// still due (§17).
    pub sign_at: Option<u64>,
}

impl Checkpoints {
    /// The last checkpoint reached in the log, with its number.
    pub fn last_reached(&self) -> Option<(u64, &Reached)> {
        self.reached.iter().next_back().map(|(n, r)| (*n, r))
    }

    /// Whether the client reached the checkpoint `digest`.
    pub fn has_reached(&self, digest: &Digest) -> bool {
        self.reached.values().any(|r| r.digest == *digest)
    }

    /// What an operation names (§13): `covered_seq` and digest of the
    /// latest checkpoint reached, or 0 and zero before the first.
    pub fn named(&self) -> (u64, Digest) {
        self.last_reached()
            .map_or((0, Digest::ZERO), |(_, r)| (r.covered_seq, r.digest))
    }

    /// Notes a record the chain verifier accepted at `r`'s position. A
    /// later attempt at a position replaces the earlier one.
    pub fn note_record(&mut self, r: Ref) {
        self.since
            .retain(|s| !(s.device == r.device && s.counter == r.counter));
        self.since.push(r);
    }

    /// Notes a policy entry.
    pub fn note_policy_entry(&mut self, seq: u64, revision: u64) {
        self.policy_entries.push((seq, revision));
    }

    /// Notes an epoch entry.
    pub fn note_epoch_entry(&mut self, seq: u64, epoch: u64, record: Digest) {
        self.epoch_entries.push((seq, epoch, record));
    }

    /// The revision of the latest policy entry at or below `at`.
    fn policy_at(&self, at: u64) -> Option<u64> {
        self.policy_entries
            .iter()
            .rev()
            .find(|(seq, _)| *seq <= at)
            .map(|(_, r)| *r)
    }

    /// The latest epoch entry at or below `at`.
    fn epoch_at(&self, at: u64) -> Option<(u64, Digest)> {
        self.epoch_entries
            .iter()
            .rev()
            .find(|(seq, _, _)| *seq <= at)
            .map(|(_, e, d)| (*e, *d))
    }

    /// The chain heads at `covered_seq` (§17): the accepted checkpoint's
    /// heads moved on by the records since, less the devices whose card
    /// can't come back by the previous checkpoint's `policy_revision`.
    pub fn heads_at(&self, covered_seq: u64, policy: &PolicyChain) -> Vec<Ref> {
        let mut heads: BTreeMap<Id, Ref> = self
            .accepted
            .iter()
            .flat_map(|a| a.heads.iter())
            .map(|h| (h.device, h.clone()))
            .collect();
        for r in self.since.iter().filter(|r| r.seq <= covered_seq) {
            match heads.get(&r.device) {
                Some(head) if head.counter > r.counter => {}
                _ => {
                    heads.insert(r.device, r.clone());
                }
            }
        }
        if let Some((_, previous)) = self.last_reached() {
            heads.retain(|device, _| !policy.card_gone(device, previous.policy_revision));
        }
        heads.into_values().collect()
    }

    /// The live set at a checkpoint with `floor`: the client's own, less
    /// the lone deletes that the floor's rise drops (§17).
    fn live_with_floor(&self, live: &LiveSet, floor: u64, covered_seq: u64) -> LiveSet {
        let mut live = live.clone();
        let previous_floor = self.accepted.as_ref().map_or(0, |a| a.record.body.floor);
        if floor > previous_floor {
            live.apply_floor(previous_floor, floor, covered_seq);
        }
        live
    }

    /// Checks a checkpoint the client reached in its log, after applying
    /// every record through its `covered_seq` (§17, Checking a
    /// checkpoint). `judge` stands at the checkpoint's entry. Returns the
    /// client error code of the first check that fails.
    pub fn check(
        &self,
        bundle: &CheckpointBundle,
        live: &LiveSet,
        judge: &LogJudge,
        policy: &PolicyChain,
        epochs: &EpochChain,
    ) -> Result<(), &'static str> {
        let record = &bundle.checkpoint.0;
        let c = &record.body;
        let heads: &[Ref] = &bundle.heads;
        if heads_digest(heads) != c.heads_digest || heads.iter().any(|h| h.seq > c.covered_seq) {
            return Err("sync_manifest_mismatch");
        }
        // Its link to the last checkpoint reached.
        let (previous, number) = self
            .last_reached()
            .map_or((Digest::ZERO, 0), |(n, r)| (r.digest, n));
        let skip = skip_number(c.number);
        let skip_digest = if skip == 0 {
            Some(Digest::ZERO)
        } else {
            self.reached.get(&skip).map(|r| r.digest)
        };
        if c.previous != previous
            || c.number != number + 1
            || skip_digest.is_some_and(|s| s != c.skip)
        {
            return Err("sync_chain_gap");
        }
        // The signer, by position and at its `policy_revision`.
        let card = policy
            .at(c.policy_revision)
            .and_then(|s| s.devices.get(&c.signer))
            .ok_or("sync_writer_not_authorized")?;
        record
            .verify(&card.card.signing_public)
            .map_err(|_| "sync_writer_not_authorized")?;
        judge
            .check_writer(&c.signer, c.policy_revision, policy)
            .map_err(|e| e.client_code())?;
        // The policy and epoch fields.
        let accepted = self.accepted.as_ref().map(|a| &a.record.body);
        if self.policy_at(c.covered_seq) != Some(c.policy_revision)
            || accepted.is_some_and(|a| c.policy_revision < a.policy_revision)
        {
            return Err("sync_manifest_mismatch");
        }
        if policy.change(c.policy_revision).map(|h| h.digest()) != Some(c.policy_head) {
            return Err("sync_policy_forked");
        }
        let accepted_epoch = epochs.get(c.epoch).map(|e| e.digest);
        if self.epoch_at(c.covered_seq) != Some((c.epoch, c.epoch_record))
            || accepted_epoch != Some(c.epoch_record)
        {
            return Err("sync_key_mismatch");
        }
        // Nothing goes back.
        if let Some(a) = &self.accepted {
            let before = &a.record.body;
            if c.floor < before.floor || c.covered_seq <= before.covered_seq {
                return Err("sync_manifest_rollback");
            }
            let previous_revision = self.last_reached().map_or(0, |(_, r)| r.policy_revision);
            for old in &a.heads {
                match heads.iter().find(|h| h.device == old.device) {
                    Some(new) if new.counter < old.counter => return Err("sync_manifest_rollback"),
                    None if !policy.card_gone(&old.device, previous_revision) => {
                        return Err("sync_manifest_rollback");
                    }
                    _ => {}
                }
            }
        }
        // The client's own live set and heads.
        let manifest = self.own_manifest(bundle, live);
        if manifest.root() != c.manifest_root || manifest.live_objects() != c.live_objects {
            return Err("sync_manifest_mismatch");
        }
        let own = self.heads_at(c.covered_seq, policy);
        for head in heads {
            match own.iter().find(|o| o.device == head.device) {
                Some(o) if o == head => {}
                Some(o) if o.counter < head.counter => return Err("sync_chain_gap"),
                None => return Err("sync_chain_gap"),
                Some(_) => return Err("sync_manifest_mismatch"),
            }
        }
        if own.len() != heads.len() {
            return Err("sync_manifest_mismatch");
        }
        Ok(())
    }

    /// The client's own manifest at a checkpoint's `covered_seq`, with the
    /// lone deletes its `floor` drops left out.
    pub fn own_manifest(&self, bundle: &CheckpointBundle, live: &LiveSet) -> Manifest {
        let c = &bundle.checkpoint.0.body;
        Manifest::new(
            self.live_with_floor(live, c.floor, c.covered_seq)
                .leaves_at(c.covered_seq),
        )
    }

    /// Records the objects a root mismatch found differing.
    pub fn set_differing(&mut self, digest: &Digest, differing: Vec<Id>) {
        if let Some(failure) = self.failures.iter_mut().find(|f| f.digest == *digest) {
            failure.differing = differing;
        }
    }

    /// Records a checkpoint the client reached at entry `seq`.
    pub fn reach(&mut self, record: &Signed<Checkpoint>, seq: u64) {
        self.reached.insert(
            record.body.number,
            Reached {
                digest: record.digest(),
                covered_seq: record.body.covered_seq,
                policy_revision: record.body.policy_revision,
                seq,
            },
        );
    }

    /// Accepts a checked checkpoint: the floor's drops leave `live` for
    /// good, and the per-record state at or below `covered_seq` goes.
    pub fn accept(&mut self, bundle: &CheckpointBundle, live: &mut LiveSet) {
        let c = &bundle.checkpoint.0.body;
        *live = self.live_with_floor(live, c.floor, c.covered_seq);
        self.since.retain(|r| r.seq > c.covered_seq);
        let keep_policy = self
            .policy_entries
            .iter()
            .rposition(|(seq, _)| *seq <= c.covered_seq)
            .unwrap_or(0);
        self.policy_entries.drain(..keep_policy);
        let keep_epoch = self
            .epoch_entries
            .iter()
            .rposition(|(seq, _, _)| *seq <= c.covered_seq)
            .unwrap_or(0);
        self.epoch_entries.drain(..keep_epoch);
        self.accepted = Some(Accepted {
            record: bundle.checkpoint.0.clone(),
            heads: bundle.heads.to_vec(),
        });
    }

    /// Reports a checkpoint that failed `code`.
    pub fn fail(&mut self, record: &Signed<Checkpoint>, code: &str) {
        let digest = record.digest();
        if self.failures.iter().any(|f| f.digest == digest) {
            return;
        }
        self.failures.push(Failure {
            number: record.body.number,
            digest,
            code: code.to_owned(),
            differing: Vec::new(),
        });
    }

    /// The checkpoint this device signs over every record through
    /// `covered_seq`, which it has applied, with the retention floor it
    /// read (§17). `None` when the log holds no policy or epoch entry yet.
    #[allow(clippy::too_many_arguments)]
    pub fn sign(
        &self,
        workspace: Id,
        domain: Id,
        covered_seq: u64,
        retention_floor: u64,
        live: &LiveSet,
        policy: &PolicyChain,
        signer: Id,
        key: &sync_protocol::crypto::SigningKey,
    ) -> Option<CheckpointBundle> {
        let policy_revision = self.policy_at(covered_seq)?;
        let (epoch, epoch_record) = self.epoch_at(covered_seq)?;
        let policy_head = policy.change(policy_revision)?.digest();
        let previous_floor = self.accepted.as_ref().map_or(0, |a| a.record.body.floor);
        let floor = retention_floor.max(previous_floor).min(covered_seq);
        let (previous, number) = self
            .last_reached()
            .map_or((Digest::ZERO, 0), |(n, r)| (r.digest, n));
        let number = number + 1;
        let skip = skip_number(number);
        let skip = if skip == 0 {
            Digest::ZERO
        } else {
            self.reached.get(&skip)?.digest
        };
        let manifest = Manifest::new(
            self.live_with_floor(live, floor, covered_seq)
                .leaves_at(covered_seq),
        );
        let heads = self.heads_at(covered_seq, policy);
        let body = Checkpoint {
            workspace,
            domain,
            previous,
            number,
            skip,
            covered_seq,
            floor,
            policy_revision,
            policy_head,
            live_objects: manifest.live_objects(),
            manifest_root: manifest.root(),
            heads_digest: heads_digest(&heads),
            epoch,
            epoch_record,
            signer,
        };
        Some(CheckpointBundle {
            checkpoint: Nested(Signed::sign(body, key)),
            heads: Sorted::new(heads).ok()?,
        })
    }
}
