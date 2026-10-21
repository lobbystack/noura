//! The chain verifier (§14).
//!
//! Each device keeps one chain of operations per domain it writes to: every
//! record carries the device's counter and the digest of its previous
//! record under the signature. The verifier tells a dropped, reordered,
//! replayed or forked record apart, and stays silent for a retried upload, a
//! page that repeats an applied position, consecutive re-seal attempts
//! served in either order, and counters a device skipped.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::records::{Operation, Ref, Signed};
use crate::types::{Digest, Id};

/// The chain fields of one record, with the sequence the server gave it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChainRecord {
    /// The signing device.
    pub device: Id,
    /// Its counter.
    pub counter: u64,
    /// The digest of the device's previous record in the domain.
    pub previous: Digest,
    /// The digest of the attempt this one re-seals, or zero.
    pub previous_attempt: Digest,
    /// The record's digest.
    pub digest: Digest,
    /// Its log sequence.
    pub seq: u64,
}

impl ChainRecord {
    /// The chain fields of an operation at sequence `seq`.
    pub fn from_operation(op: &Signed<Operation>, seq: u64) -> Self {
        Self {
            device: op.body.device,
            counter: op.body.counter,
            previous: op.body.previous,
            previous_attempt: op.body.previous_attempt,
            digest: op.digest(),
            seq,
        }
    }
}

/// A chain error (§14). The client applies nothing from the page, keeps
/// its cursor and reports it with the device and domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, thiserror::Error, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainError {
    /// A record the page should have carried is missing.
    #[error("sync_chain_gap")]
    Gap,
    /// The server placed a record after its successor.
    #[error("sync_chain_reordered")]
    Reordered,
    /// A record the client already holds came back at another sequence.
    #[error("sync_chain_replayed")]
    Replayed,
    /// Two records claim one position.
    #[error("sync_chain_forked")]
    Forked,
    /// The client's head fell below the retention floor: it bootstraps.
    #[error("pruned")]
    Pruned,
}

impl ChainError {
    /// The client error code.
    pub fn client_code(&self) -> &'static str {
        match self {
            ChainError::Gap => "sync_chain_gap",
            ChainError::Reordered => "sync_chain_reordered",
            ChainError::Replayed => "sync_chain_replayed",
            ChainError::Forked => "sync_chain_forked",
            ChainError::Pruned => "pruned",
        }
    }
}

/// What the client does with a record that passes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum ChainVerdict {
    /// Apply it and advance the device's head.
    Accept,
    /// It repeats a position the client applied, or is an earlier attempt
    /// of one it holds: skip it.
    Ignore,
    /// It is a later attempt of the device's head: switch to it and
    /// recompute the object's live set.
    Replace {
        /// The attempt it replaces.
        replaced: Digest,
    },
}

/// The server's answer to a chain-position fetch, `GET
/// .../chain/{device}/{counter}`: the record at that counter, or the
/// device's next record above it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fetched {
    /// The record.
    Record(ChainRecord),
    /// No record at or above the counter.
    NotFound,
    /// The position was pruned.
    Pruned,
}

/// Fetches chain positions from the server.
pub trait ChainFetcher {
    /// The device's record at `counter`, or its next record above it.
    fn fetch(&mut self, device: &Id, counter: u64) -> Fetched;
}

/// A fetcher for when no fetch is possible; every fetch finds nothing.
pub struct NoFetch;

impl ChainFetcher for NoFetch {
    fn fetch(&mut self, _: &Id, _: u64) -> Fetched {
        Fetched::NotFound
    }
}

/// A device's head in one domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Head {
    /// The counter, 0 before the device's first record.
    pub counter: u64,
    /// The digest, zero before the first record.
    pub digest: Digest,
    /// The sequence, 0 before the first record.
    pub seq: u64,
}

impl Head {
    /// The head before a device's first record.
    pub const NONE: Head = Head {
        counter: 0,
        digest: Digest::ZERO,
        seq: 0,
    };
}

#[derive(Clone, Copy, Debug)]
struct Attempt {
    previous_attempt: Digest,
    seq: u64,
}

#[derive(Clone, Debug)]
struct Position {
    applied: Digest,
    attempts: BTreeMap<Digest, Attempt>,
}

#[derive(Clone, Debug, Default)]
struct DeviceChain {
    head: Option<Head>,
    positions: BTreeMap<u64, Position>,
}

/// The verifier state of one domain: per device, the accepted head and the
/// positions above the latest accepted checkpoint.
#[derive(Clone, Debug, Default)]
pub struct ChainVerifier {
    devices: BTreeMap<Id, DeviceChain>,
}

impl ChainVerifier {
    /// A verifier that knows no record yet, as when pulling from sequence 0.
    pub fn new() -> Self {
        Self::default()
    }

    /// A verifier that starts at a checkpoint's heads, with their
    /// sequences.
    pub fn from_heads(heads: &[Ref]) -> Self {
        let mut verifier = Self::new();
        for head in heads {
            verifier.devices.insert(
                head.device,
                DeviceChain {
                    head: Some(Head {
                        counter: head.counter,
                        digest: head.op_digest,
                        seq: head.seq,
                    }),
                    positions: BTreeMap::new(),
                },
            );
        }
        verifier
    }

    /// The accepted head of `device`.
    pub fn head(&self, device: &Id) -> Head {
        self.devices
            .get(device)
            .and_then(|d| d.head)
            .unwrap_or(Head::NONE)
    }

    /// Forgets the positions at or below the heads, keeping only heads, as
    /// the client does once it accepts a checkpoint.
    pub fn forget_below_heads(&mut self) {
        for chain in self.devices.values_mut() {
            if let Some(head) = chain.head {
                chain.positions.retain(|&counter, _| counter > head.counter);
            }
        }
    }

    /// Judges a page of records in sequence order. On an error the caller
    /// applies nothing from the page; the verifier's state then reflects
    /// only the records before the failing one, so the caller should judge
    /// the page against a clone and keep it only on success.
    pub fn process_page(
        &mut self,
        page: &[ChainRecord],
        fetcher: &mut impl ChainFetcher,
    ) -> Result<Vec<ChainVerdict>, (usize, ChainError)> {
        let carried: BTreeMap<(Id, u64, Digest), Digest> = page
            .iter()
            .map(|r| ((r.device, r.counter, r.digest), r.previous_attempt))
            .collect();
        page.iter()
            .enumerate()
            .map(|(i, record)| self.process(record, &carried, fetcher).map_err(|e| (i, e)))
            .collect()
    }

    /// Judges one record.
    pub fn process_one(
        &mut self,
        record: &ChainRecord,
        fetcher: &mut impl ChainFetcher,
    ) -> Result<ChainVerdict, ChainError> {
        self.process(record, &BTreeMap::new(), fetcher)
    }

    fn process(
        &mut self,
        record: &ChainRecord,
        carried: &BTreeMap<(Id, u64, Digest), Digest>,
        fetcher: &mut impl ChainFetcher,
    ) -> Result<ChainVerdict, ChainError> {
        let head = self.head(&record.device);
        if record.counter <= head.counter {
            return self.rule_one(record, head, carried);
        }
        if record.previous == head.digest {
            self.accept(record);
            return Ok(ChainVerdict::Accept);
        }
        // Rule 3: first fetch the head's own position for a later attempt.
        // Like every fetched record, it must sit before the record.
        if head.counter > 0 {
            if let Fetched::Record(fetched) = fetcher.fetch(&record.device, head.counter) {
                if fetched.counter == head.counter && fetched.digest != head.digest {
                    let saved = self.devices.get(&record.device).cloned();
                    match self.rule_one(&fetched, head, carried) {
                        Ok(ChainVerdict::Replace { .. }) if fetched.seq < record.seq => {
                            return self.process(record, carried, fetcher);
                        }
                        Ok(ChainVerdict::Replace { .. }) => {
                            self.restore(record.device, saved);
                            return Err(ChainError::Reordered);
                        }
                        _ => self.restore(record.device, saved),
                    }
                }
            }
        }
        // Then the device's next record after its head. The record names a
        // predecessor the client lacks, so whatever comes back explains the
        // break: a predecessor placed before the record that the page left
        // out, one placed after it, another record at its position, or a
        // chain that never reaches it.
        match fetcher.fetch(&record.device, head.counter + 1) {
            Fetched::Pruned => Err(ChainError::Pruned),
            Fetched::NotFound => Err(ChainError::Gap),
            Fetched::Record(fetched) => {
                if fetched.counter <= head.counter || fetched.counter > record.counter {
                    Err(ChainError::Gap)
                } else if fetched.previous != head.digest || fetched.counter == record.counter {
                    Err(ChainError::Forked)
                } else if fetched.seq > record.seq {
                    Err(ChainError::Reordered)
                } else {
                    Err(ChainError::Gap)
                }
            }
        }
    }

    fn rule_one(
        &mut self,
        record: &ChainRecord,
        head: Head,
        carried: &BTreeMap<(Id, u64, Digest), Digest>,
    ) -> Result<ChainVerdict, ChainError> {
        let chain = self.devices.entry(record.device).or_default();
        let Some(position) = chain.positions.get_mut(&record.counter) else {
            // A position the client doesn't hold: below its checkpoint,
            // bootstrapped past, or skipped. Every legitimate record there
            // has a sequence at or below the head's.
            if record.seq > head.seq {
                return Err(ChainError::Replayed);
            }
            return Ok(ChainVerdict::Ignore);
        };
        if let Some(known) = position.attempts.get(&record.digest) {
            return if known.seq == record.seq {
                Ok(ChainVerdict::Ignore)
            } else {
                Err(ChainError::Replayed)
            };
        }
        // A different record at this position: attempts at one position form
        // a single line, each naming the one before.
        let lookup = |digest: &Digest| -> Option<Digest> {
            position
                .attempts
                .get(digest)
                .map(|a| a.previous_attempt)
                .or_else(|| {
                    carried
                        .get(&(record.device, record.counter, *digest))
                        .copied()
                })
        };
        let reaches = |from_previous: Digest, target: &Digest| -> bool {
            let mut current = from_previous;
            let mut seen = BTreeSet::new();
            while !current.is_zero() && seen.insert(current) {
                if current == *target {
                    return true;
                }
                match lookup(&current) {
                    Some(previous) => current = previous,
                    None => return false,
                }
            }
            false
        };
        let shares_previous = !record.previous_attempt.is_zero()
            && position
                .attempts
                .values()
                .any(|a| a.previous_attempt == record.previous_attempt);
        if shares_previous {
            return Err(ChainError::Forked);
        }
        let applied = position.applied;
        let applied_previous = position.attempts[&applied].previous_attempt;
        if reaches(record.previous_attempt, &applied) {
            // The new record is the later attempt.
            if record.counter != head.counter {
                return Err(ChainError::Forked);
            }
            position.attempts.insert(
                record.digest,
                Attempt {
                    previous_attempt: record.previous_attempt,
                    seq: record.seq,
                },
            );
            position.applied = record.digest;
            chain.head = Some(Head {
                counter: record.counter,
                digest: record.digest,
                seq: record.seq,
            });
            return Ok(ChainVerdict::Replace { replaced: applied });
        }
        if reaches(applied_previous, &record.digest) {
            // The client holds the later attempt; skip the earlier one.
            position.attempts.insert(
                record.digest,
                Attempt {
                    previous_attempt: record.previous_attempt,
                    seq: record.seq,
                },
            );
            return Ok(ChainVerdict::Ignore);
        }
        Err(ChainError::Forked)
    }

    fn restore(&mut self, device: Id, saved: Option<DeviceChain>) {
        match saved {
            Some(chain) => {
                self.devices.insert(device, chain);
            }
            None => {
                self.devices.remove(&device);
            }
        }
    }

    fn accept(&mut self, record: &ChainRecord) {
        let chain = self.devices.entry(record.device).or_default();
        let mut position = Position {
            applied: record.digest,
            attempts: BTreeMap::new(),
        };
        position.attempts.insert(
            record.digest,
            Attempt {
                previous_attempt: record.previous_attempt,
                seq: record.seq,
            },
        );
        chain.positions.insert(record.counter, position);
        chain.head = Some(Head {
            counter: record.counter,
            digest: record.digest,
            seq: record.seq,
        });
    }

    /// Compares a checkpoint's head for `device` with the client's own
    /// (§17): above the client's reports `sync_chain_gap`, below it
    /// `sync_manifest_mismatch`.
    pub fn compare_checkpoint_head(&self, head: &Ref) -> Result<(), &'static str> {
        let own = self.head(&head.device);
        match head.counter.cmp(&own.counter) {
            std::cmp::Ordering::Greater => Err("sync_chain_gap"),
            std::cmp::Ordering::Less => Err("sync_manifest_mismatch"),
            std::cmp::Ordering::Equal if head.op_digest != own.digest => {
                Err("sync_manifest_mismatch")
            }
            std::cmp::Ordering::Equal => Ok(()),
        }
    }
}
