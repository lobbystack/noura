//! The chain verifier (§14) over hand-written and random schedules.

use std::collections::BTreeMap;

use proptest::prelude::*;
use sync_protocol::chain::{
    ChainError, ChainFetcher, ChainRecord, ChainVerdict, ChainVerifier, Fetched, NoFetch,
};
use sync_protocol::crypto::hash_parts;
use sync_protocol::records::Ref;
use sync_protocol::types::{Digest, Id};

const DEVICE: Id = Id([0xd1; 16]);

/// A device's chain as the device signs it.
struct Device {
    records: Vec<ChainRecord>,
}

impl Device {
    fn new() -> Self {
        Self {
            records: Vec::new(),
        }
    }

    fn head(&self) -> (u64, Digest) {
        self.records
            .last()
            .map_or((0, Digest::ZERO), |r| (r.counter, r.digest))
    }

    /// Signs the next record at `counter`, which may skip ahead.
    fn sign_at(&mut self, counter: u64) -> ChainRecord {
        let (_, previous) = self.head();
        let record = ChainRecord {
            device: DEVICE,
            counter,
            previous,
            previous_attempt: Digest::ZERO,
            digest: hash_parts(&[b"record", &counter.to_be_bytes(), &previous.0]),
            seq: 0,
        };
        self.records.push(record);
        record
    }

    fn sign(&mut self) -> ChainRecord {
        let next = self.head().0 + 1;
        self.sign_at(next)
    }

    /// Re-seals the latest record at the same counter, naming the attempt
    /// it replaces.
    fn reseal(&mut self) -> ChainRecord {
        let last = self.records.pop().unwrap();
        let record = ChainRecord {
            previous_attempt: last.digest,
            digest: hash_parts(&[b"attempt", &last.digest.0]),
            ..last
        };
        self.records.push(record);
        record
    }
}

fn at(record: ChainRecord, seq: u64) -> ChainRecord {
    ChainRecord { seq, ..record }
}

/// The server's store: the record it holds at each counter.
#[derive(Default)]
struct Server {
    records: BTreeMap<u64, ChainRecord>,
}

impl Server {
    fn hold(&mut self, record: ChainRecord) {
        self.records.insert(record.counter, record);
    }
}

impl ChainFetcher for Server {
    fn fetch(&mut self, _: &Id, counter: u64) -> Fetched {
        self.records
            .range(counter..)
            .next()
            .map_or(Fetched::NotFound, |(_, r)| Fetched::Record(*r))
    }
}

fn run(page: &[ChainRecord], server: &mut Server) -> Result<Vec<ChainVerdict>, ChainError> {
    let mut verifier = ChainVerifier::new();
    verifier.process_page(page, server).map_err(|(_, e)| e)
}

#[test]
fn an_honest_page_passes() {
    let mut d = Device::new();
    let page = vec![at(d.sign(), 1), at(d.sign(), 2), at(d.sign(), 5)];
    assert_eq!(
        run(&page, &mut Server::default()).unwrap(),
        vec![ChainVerdict::Accept; 3]
    );
}

#[test]
fn a_dropped_record_is_a_gap() {
    let mut d = Device::new();
    let (a, b, c) = (at(d.sign(), 1), at(d.sign(), 2), at(d.sign(), 3));
    let mut server = Server::default();
    for r in [a, b, c] {
        server.hold(r);
    }
    assert_eq!(run(&[a, c], &mut server), Err(ChainError::Gap));
    // A server that can't produce the predecessor at all.
    let mut empty = Server::default();
    assert_eq!(run(&[a, c], &mut empty), Err(ChainError::Gap));
}

#[test]
fn a_reordered_record_is_reported_as_such() {
    let mut d = Device::new();
    let (a, b, c) = (at(d.sign(), 1), at(d.sign(), 3), at(d.sign(), 2));
    // b sits after its successor c.
    let mut server = Server::default();
    for r in [a, b, c] {
        server.hold(r);
    }
    assert_eq!(run(&[a, c], &mut server), Err(ChainError::Reordered));
}

#[test]
fn a_replayed_record_is_reported_as_such() {
    let mut d = Device::new();
    let (a, b) = (at(d.sign(), 1), at(d.sign(), 2));
    assert_eq!(
        run(&[a, b, at(a, 3)], &mut Server::default()),
        Err(ChainError::Replayed)
    );
    // Right after a bootstrap, the client holds only heads.
    let mut verifier = ChainVerifier::from_heads(&[Ref {
        device: DEVICE,
        counter: b.counter,
        seq: b.seq,
        op_digest: b.digest,
    }]);
    assert_eq!(
        verifier.process_one(&at(a, 7), &mut NoFetch),
        Err(ChainError::Replayed)
    );
    // The same position at or below the head's sequence is just a repeat.
    assert_eq!(
        verifier.process_one(&a, &mut NoFetch),
        Ok(ChainVerdict::Ignore)
    );
}

#[test]
fn a_forked_record_is_reported_as_such() {
    let mut d = Device::new();
    let a = at(d.sign(), 1);
    let mut other = Device::new();
    let mut fork = other.sign();
    fork.digest = Digest([0xee; 32]);
    assert_eq!(
        run(&[a, at(fork, 2)], &mut Server::default()),
        Err(ChainError::Forked)
    );
}

#[test]
fn retries_repeats_and_skipped_counters_pass_silently() {
    let mut d = Device::new();
    let a = at(d.sign(), 1);
    let b = at(d.sign_at(1_025), 2);
    let mut verifier = ChainVerifier::new();
    assert_eq!(
        verifier.process_one(&a, &mut NoFetch),
        Ok(ChainVerdict::Accept)
    );
    // A retried upload served at the same sequence.
    assert_eq!(
        verifier.process_one(&a, &mut NoFetch),
        Ok(ChainVerdict::Ignore)
    );
    // A device that skipped counters 2 to 1,024.
    assert_eq!(
        verifier.process_one(&b, &mut NoFetch),
        Ok(ChainVerdict::Accept)
    );
    // A page that repeats applied positions.
    assert_eq!(
        verifier.process_page(&[a, b], &mut NoFetch).unwrap(),
        vec![ChainVerdict::Ignore, ChainVerdict::Ignore]
    );
}

#[test]
fn consecutive_reseal_attempts_pass_in_either_order() {
    let mut d = Device::new();
    let first = d.sign();
    let second = d.reseal();
    let third = d.reseal();
    // In signing order: each later attempt replaces the head.
    let mut verifier = ChainVerifier::new();
    let verdicts = verifier
        .process_page(&[at(first, 1), at(second, 2), at(third, 3)], &mut NoFetch)
        .unwrap();
    assert_eq!(
        verdicts,
        vec![
            ChainVerdict::Accept,
            ChainVerdict::Replace {
                replaced: first.digest
            },
            ChainVerdict::Replace {
                replaced: second.digest
            },
        ]
    );
    // Latest first: the earlier attempts are skipped.
    let mut verifier = ChainVerifier::new();
    let verdicts = verifier
        .process_page(&[at(third, 1), at(second, 2), at(first, 3)], &mut NoFetch)
        .unwrap();
    assert_eq!(
        verdicts,
        vec![
            ChainVerdict::Accept,
            ChainVerdict::Ignore,
            ChainVerdict::Ignore
        ]
    );
    // The middle attempt arrives last and links through the page.
    let mut verifier = ChainVerifier::new();
    let verdicts = verifier
        .process_page(&[at(first, 1), at(third, 2), at(second, 3)], &mut NoFetch)
        .unwrap();
    assert_eq!(
        verdicts[1],
        ChainVerdict::Replace {
            replaced: first.digest
        }
    );
    assert_eq!(verdicts[2], ChainVerdict::Ignore);
}

#[test]
fn sibling_attempts_and_late_attempts_fork() {
    let mut d = Device::new();
    let first = d.sign();
    let second = d.reseal();
    let sibling = ChainRecord {
        digest: Digest([0x5b; 32]),
        ..second
    };
    let mut verifier = ChainVerifier::new();
    assert_eq!(
        verifier
            .process_page(&[at(first, 1), at(second, 2), at(sibling, 3)], &mut NoFetch)
            .map_err(|(_, e)| e),
        Err(ChainError::Forked)
    );
    // An attempt that arrives after a successor of the other.
    let mut d = Device::new();
    let first = d.sign();
    let next = d.sign();
    let mut late = first;
    late.previous_attempt = first.digest;
    late.digest = Digest([0x1a; 32]);
    let mut verifier = ChainVerifier::new();
    assert_eq!(
        verifier
            .process_page(&[at(first, 1), at(next, 2), at(late, 3)], &mut NoFetch)
            .map_err(|(_, e)| e),
        Err(ChainError::Forked)
    );
}

#[test]
fn a_skipping_record_whose_predecessor_the_page_left_out_is_a_gap_or_reorder() {
    let mut d = Device::new();
    let a = at(d.sign(), 1);
    let b = at(d.sign_at(1_025), 4);
    let c = at(d.sign(), 9);
    let mut server = Server::default();
    for r in [a, b, c] {
        server.hold(r);
    }
    // b sits at 4 below c at 9: the page left it out.
    assert_eq!(run(&[a, c], &mut server), Err(ChainError::Gap));
    let mut late = Server::default();
    for r in [a, at(b.clone_with_seq(12), 12), c] {
        late.hold(r);
    }
    assert_eq!(run(&[a, c], &mut late), Err(ChainError::Reordered));
}

#[test]
fn a_later_attempt_of_the_head_is_fetched_before_judging() {
    // The client holds the first attempt as its head; the page carries the
    // successor of a later attempt. Fetching the head's position finds the
    // later attempt.
    let mut d = Device::new();
    let first = d.sign();
    let second = d.reseal();
    let next = d.sign();
    let mut server = Server::default();
    server.hold(at(second, 2));
    server.hold(at(next, 3));
    let mut verifier = ChainVerifier::new();
    verifier.process_one(&at(first, 1), &mut NoFetch).unwrap();
    assert_eq!(
        verifier.process_one(&at(next, 3), &mut server),
        Ok(ChainVerdict::Accept)
    );
    assert_eq!(verifier.head(&DEVICE).digest, next.digest);
}

#[test]
fn a_checkpoint_head_above_or_below_the_clients_reports() {
    let mut d = Device::new();
    let a = at(d.sign(), 1);
    let mut verifier = ChainVerifier::new();
    verifier.process_one(&a, &mut NoFetch).unwrap();
    let head = |counter, digest| Ref {
        device: DEVICE,
        counter,
        seq: 1,
        op_digest: digest,
    };
    assert_eq!(verifier.compare_checkpoint_head(&head(1, a.digest)), Ok(()));
    assert_eq!(
        verifier.compare_checkpoint_head(&head(2, a.digest)),
        Err("sync_chain_gap")
    );
    assert_eq!(
        verifier.compare_checkpoint_head(&head(1, Digest([1; 32]))),
        Err("sync_manifest_mismatch")
    );
}

trait WithSeq {
    fn clone_with_seq(&self, seq: u64) -> ChainRecord;
}

impl WithSeq for ChainRecord {
    fn clone_with_seq(&self, seq: u64) -> ChainRecord {
        at(*self, seq)
    }
}

/// A random honest schedule: records, skipped counters and re-seals, each
/// served once at increasing sequences, plus retries of the same bytes.
fn schedule() -> impl Strategy<Value = Vec<(u8, bool)>> {
    proptest::collection::vec((0u8..4, any::<bool>()), 1..40)
}

fn build(steps: &[(u8, bool)]) -> (Vec<ChainRecord>, Server) {
    let mut d = Device::new();
    let mut served = Vec::new();
    let mut seq = 0;
    let mut server = Server::default();
    for &(kind, retry) in steps {
        let record = match kind {
            0 | 1 => d.sign(),
            2 => {
                let skip = d.head().0 + 1 + 1_024;
                d.sign_at(skip)
            }
            _ if !d.records.is_empty() => d.reseal(),
            _ => d.sign(),
        };
        seq += 1;
        let placed = at(record, seq);
        served.push(placed);
        server.hold(placed);
        if retry {
            served.push(placed);
        }
    }
    (served, server)
}

proptest! {
    #[test]
    fn honest_schedules_report_nothing(steps in schedule()) {
        let (served, mut server) = build(&steps);
        let mut verifier = ChainVerifier::new();
        prop_assert!(verifier.process_page(&served, &mut server).is_ok());
    }

    #[test]
    fn a_dropped_record_never_passes(steps in schedule(), pick in any::<prop::sample::Index>()) {
        let (served, mut server) = build(&steps);
        let mut unique: Vec<ChainRecord> = Vec::new();
        for r in served {
            if unique.last() != Some(&r) {
                unique.push(r);
            }
        }
        // Drop one record that a later record of the chain builds on. A
        // later attempt of the record before it is exempt: rule 3 fetches
        // the head's own position and takes it, as §14 asks.
        let candidates: Vec<usize> = (0..unique.len().saturating_sub(1))
            .filter(|&i| unique[i + 1..].iter().any(|r| r.previous == unique[i].digest))
            .filter(|&i| unique[i].previous_attempt.is_zero())
            .collect();
        prop_assume!(!candidates.is_empty());
        let drop = candidates[pick.index(candidates.len())];
        let mut page = unique.clone();
        page.remove(drop);
        let mut verifier = ChainVerifier::new();
        let result = verifier.process_page(&page, &mut server).map_err(|(_, e)| e);
        prop_assert!(result.is_err(), "{:?}", result);
    }

    #[test]
    fn a_replayed_record_never_passes(steps in schedule(), pick in any::<prop::sample::Index>()) {
        let (served, mut server) = build(&steps);
        let last_seq = served.last().unwrap().seq;
        let replay = at(served[pick.index(served.len())], last_seq + 1);
        let mut page = served.clone();
        page.push(replay);
        let mut verifier = ChainVerifier::new();
        let result = verifier.process_page(&page, &mut server).map_err(|(_, e)| e);
        prop_assert_eq!(result, Err(ChainError::Replayed));
    }

    #[test]
    fn a_forked_record_never_passes(steps in schedule(), pick in any::<prop::sample::Index>()) {
        let (served, mut server) = build(&steps);
        let last_seq = served.last().unwrap().seq;
        let victim = served[pick.index(served.len())];
        let fork = ChainRecord {
            digest: hash_parts(&[b"fork", &victim.digest.0]),
            previous_attempt: Digest::ZERO,
            seq: last_seq + 1,
            ..victim
        };
        let mut page = served.clone();
        page.push(fork);
        let mut verifier = ChainVerifier::new();
        let result = verifier.process_page(&page, &mut server).map_err(|(_, e)| e);
        prop_assert_eq!(result, Err(ChainError::Forked));
    }

    #[test]
    fn swapping_two_records_of_a_chain_never_passes(steps in schedule(), pick in any::<prop::sample::Index>()) {
        let (served, mut server) = build(&steps);
        let mut unique: Vec<ChainRecord> = Vec::new();
        for r in served {
            if unique.last() != Some(&r) {
                unique.push(r);
            }
        }
        let candidates: Vec<usize> = (0..unique.len().saturating_sub(1))
            .filter(|&i| unique[i + 1].previous == unique[i].digest)
            .collect();
        prop_assume!(!candidates.is_empty());
        let i = candidates[pick.index(candidates.len())];
        // The server places the successor first.
        let (a, b) = (unique[i], unique[i + 1]);
        let mut page = unique.clone();
        page[i] = at(b, a.seq);
        page[i + 1] = at(a, b.seq);
        server.hold(at(a, b.seq));
        server.hold(at(b, a.seq));
        let mut verifier = ChainVerifier::new();
        let result = verifier.process_page(&page, &mut server).map_err(|(_, e)| e);
        prop_assert!(result.is_err());
    }
}
