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

/// A record a reader applied on the branch that a settled fork dropped
/// (§14), which it takes out of its live set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Reverted {
    /// Its counter.
    pub counter: u64,
    /// Its digest.
    pub digest: Digest,
    /// The sequence it was applied at.
    pub seq: u64,
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

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct Attempt {
    previous_attempt: Digest,
    seq: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Position {
    applied: Digest,
    attempts: BTreeMap<Digest, Attempt>,
}

impl Position {
    /// The sequence of the applied attempt.
    fn seq(&self) -> u64 {
        self.attempts.get(&self.applied).map_or(0, |a| a.seq)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct DeviceChain {
    head: Option<Head>,
    positions: BTreeMap<u64, Position>,
    /// The records of a forked chain's other branch, which the client
    /// ignores once a checkpoint settled the fork (§14), with their
    /// sequences.
    #[serde(default)]
    ignored: BTreeMap<Digest, u64>,
}

/// The verifier state of one domain: per device, the accepted head and the
/// positions above the latest accepted checkpoint. It serializes, so a
/// client's journal can keep it (§15).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
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
                    ignored: BTreeMap::new(),
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

    /// Forgets the per-record state at or below `covered_seq`, as the
    /// client does once it accepts a checkpoint covering it (§14): each
    /// device keeps its positions above `covered_seq`, its last position at
    /// or below it, which is its head in that checkpoint, and the ignored
    /// records of a settled fork above it.
    pub fn forget_below(&mut self, covered_seq: u64) {
        for chain in self.devices.values_mut() {
            let below = chain
                .positions
                .iter()
                .rev()
                .find(|(_, p)| p.seq() <= covered_seq)
                .map(|(counter, _)| *counter);
            chain
                .positions
                .retain(|counter, p| p.seq() > covered_seq || Some(*counter) == below);
            chain.ignored.retain(|_, seq| *seq > covered_seq);
        }
    }

    /// How many records the verifier keeps state for: positions and
    /// ignored records, across devices.
    pub fn retained(&self) -> usize {
        self.devices
            .values()
            .map(|chain| chain.positions.len() + chain.ignored.len())
            .sum()
    }

    /// Whether the record is on the other branch of a settled fork, which
    /// the client ignores (§14).
    pub fn ignores(&self, device: &Id, digest: &Digest) -> bool {
        self.devices
            .get(device)
            .is_some_and(|chain| chain.ignored.contains_key(digest))
    }

    /// Settles a fork in `kept.device`'s chain (§14). The client holds
    /// `sync_chain_forked` for that device and accepted a policy change
    /// that ends its write access; `kept` is the device's head in the
    /// first checkpoint that covers that change's entry, and `records` are
    /// the log's records from the client's cursor through that checkpoint's
    /// `covered_seq`.
    ///
    /// The branch that ends at `kept` reaches back, through `previous`
    /// links or as a later attempt, either to the client's head or to a
    /// record the client applied below it: the fork point. Above a fork
    /// point the client applied the other branch, so those records are
    /// reverted: the head moves back to the fork point and the reverted
    /// records are returned, oldest first, for the caller to take out of
    /// its live set. The device's other records in `records`, and the
    /// reverted ones, are ignored from now on. A branch that reaches
    /// neither, as when the fork point lies below what the verifier keeps
    /// after a checkpoint, reports `sync_chain_forked`.
    pub fn resolve_fork(
        &mut self,
        kept: &Ref,
        records: &[ChainRecord],
    ) -> Result<Vec<Reverted>, ChainError> {
        let device = kept.device;
        let head = self.head(&device);
        let mine: BTreeMap<Digest, &ChainRecord> = records
            .iter()
            .filter(|r| r.device == device)
            .map(|r| (r.digest, r))
            .collect();
        // The records the client applied, by digest: candidate fork points.
        let applied: BTreeMap<Digest, (u64, u64)> = self
            .devices
            .get(&device)
            .map(|chain| {
                chain
                    .positions
                    .iter()
                    .map(|(counter, p)| (p.applied, (*counter, p.seq())))
                    .collect()
            })
            .unwrap_or_default();
        let at_head = kept.op_digest == head.digest;
        let holds = |counter: u64, seq: u64| counter == kept.counter && seq == kept.seq;
        let known = if at_head {
            holds(head.counter, head.seq)
        } else if let Some((counter, seq)) = applied.get(&kept.op_digest) {
            holds(*counter, *seq)
        } else {
            mine.get(&kept.op_digest)
                .is_some_and(|r| holds(r.counter, r.seq))
        };
        if !known {
            return Err(ChainError::Forked);
        }
        // Earlier attempts of a kept record are harmless: the verifier
        // ignores them, or replaces them with the later one.
        let attempts = |from: Digest, keep: &mut BTreeSet<Digest>| -> bool {
            let mut current = from;
            loop {
                if current == head.digest && !current.is_zero() {
                    return true;
                }
                let Some(record) = mine.get(&current) else {
                    return false;
                };
                if !keep.insert(current) {
                    return false;
                }
                current = record.previous_attempt;
            }
        };
        let mut keep = BTreeSet::new();
        let mut current = kept.op_digest;
        let mut steps = 0;
        let fork_point = loop {
            if current == head.digest {
                break None;
            }
            if let Some((counter, _)) = applied.get(&current) {
                break Some(*counter);
            }
            steps += 1;
            if steps > mine.len() {
                return Err(ChainError::Forked);
            }
            let record = mine.get(&current).ok_or(ChainError::Forked)?;
            if record.counter == head.counter && attempts(current, &mut keep) {
                // A later attempt of the head, whose line reaches it.
                break None;
            }
            attempts(current, &mut keep);
            current = record.previous;
        };
        let chain = self.devices.entry(device).or_default();
        let mut reverted = Vec::new();
        if let Some(base) = fork_point {
            let position = chain.positions.get(&base).ok_or(ChainError::Forked)?;
            let base_head = Head {
                counter: base,
                digest: position.applied,
                seq: position.seq(),
            };
            let above: Vec<u64> = chain.positions.range(base + 1..).map(|(c, _)| *c).collect();
            for counter in above {
                let Some(position) = chain.positions.remove(&counter) else {
                    continue;
                };
                reverted.push(Reverted {
                    counter,
                    digest: position.applied,
                    seq: position.seq(),
                });
                for (digest, attempt) in position.attempts {
                    chain.ignored.insert(digest, attempt.seq);
                }
            }
            chain.head = Some(base_head);
        }
        for (digest, record) in &mine {
            if !keep.contains(digest) {
                chain.ignored.insert(*digest, record.seq);
            }
        }
        Ok(reverted)
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
        if self.ignores(&record.device, &record.digest) {
            return Ok(ChainVerdict::Ignore);
        }
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
        let later_attempt = match head.counter {
            0 => None,
            counter => match fetcher.fetch(&record.device, counter) {
                Fetched::Record(fetched)
                    if fetched.counter == head.counter && fetched.digest != head.digest =>
                {
                    Some(fetched)
                }
                _ => None,
            },
        };
        if let Some(fetched) = later_attempt {
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
        // Then the device's next record after its head. The record names a
        // predecessor the client lacks, so whatever comes back explains the
        // break: a predecessor placed before the record that the page left
        // out, one placed after it, another record at its position, or a
        // chain that never reaches it.
        match fetcher.fetch(&record.device, head.counter + 1) {
            Fetched::Pruned => Err(ChainError::Pruned),
            Fetched::NotFound => Err(ChainError::Gap),
            Fetched::Record(fetched) => {
                // The record itself, or nothing up to it: the server holds no
                // predecessor, so the chain stops short of the record.
                if fetched.digest == record.digest
                    || fetched.counter <= head.counter
                    || fetched.counter > record.counter
                {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(name: &str) -> Digest {
        crate::crypto::hash(name.as_bytes())
    }

    fn record(device: Id, counter: u64, previous: Digest, name: &str, seq: u64) -> ChainRecord {
        ChainRecord {
            device,
            counter,
            previous,
            previous_attempt: Digest::ZERO,
            digest: digest(name),
            seq,
        }
    }

    #[test]
    fn an_accepted_checkpoint_leaves_only_heads_below_it() {
        let x = Id([1; 16]);
        let y = Id([2; 16]);
        let mut verifier = ChainVerifier::new();
        let mut page = Vec::new();
        let (mut px, mut py) = (Digest::ZERO, Digest::ZERO);
        for i in 1..=10u64 {
            let rx = record(x, i, px, &format!("x{i}"), 2 * i - 1);
            let ry = record(y, i, py, &format!("y{i}"), 2 * i);
            (px, py) = (rx.digest, ry.digest);
            page.extend([rx, ry]);
        }
        verifier.process_page(&page, &mut NoFetch).unwrap();
        assert_eq!(verifier.retained(), 20);
        // A checkpoint covering through sequence 14: each device keeps its
        // head there and the positions above it.
        verifier.forget_below(14);
        assert_eq!(verifier.retained(), 2 + 6);
        // A page that repeats a kept position is still ignored, and one
        // that replays a forgotten position at a later sequence is refused.
        assert_eq!(
            verifier.process_one(&page[19], &mut NoFetch),
            Ok(ChainVerdict::Ignore)
        );
        assert_eq!(
            verifier.process_one(&at(page[0], 30), &mut NoFetch),
            Err(ChainError::Replayed)
        );
        verifier.forget_below(20);
        assert_eq!(verifier.retained(), 2);
    }

    fn at(r: ChainRecord, seq: u64) -> ChainRecord {
        ChainRecord { seq, ..r }
    }

    #[test]
    fn a_settled_fork_keeps_ignoring_the_other_branch_above_an_accepted_checkpoint() {
        let x = Id([1; 16]);
        let a = record(x, 1, Digest::ZERO, "a", 1);
        let b = record(x, 2, a.digest, "b", 2);
        let b2 = record(x, 2, a.digest, "b2", 4);
        let c2 = record(x, 3, b2.digest, "c2", 6);
        let mut verifier = ChainVerifier::new();
        verifier.process_page(&[a, b], &mut NoFetch).unwrap();
        assert_eq!(
            verifier.clone().process_page(&[b2, c2], &mut NoFetch),
            Err((0, ChainError::Forked))
        );
        let kept = Ref {
            device: x,
            counter: c2.counter,
            seq: c2.seq,
            op_digest: c2.digest,
        };
        let reverted = verifier.resolve_fork(&kept, &[b2, c2]).unwrap();
        assert_eq!(
            reverted,
            vec![Reverted {
                counter: 2,
                digest: b.digest,
                seq: 2
            }]
        );
        assert!(verifier.ignores(&x, &b.digest));
        // A checkpoint below the ignored record leaves it ignored.
        verifier.forget_below(1);
        assert!(verifier.ignores(&x, &b.digest));
        let verdicts = verifier.process_page(&[b2, c2], &mut NoFetch).unwrap();
        assert_eq!(verdicts, vec![ChainVerdict::Accept, ChainVerdict::Accept]);
        // One covering it forgets it.
        verifier.forget_below(2);
        assert!(!verifier.ignores(&x, &b.digest));
    }
}
