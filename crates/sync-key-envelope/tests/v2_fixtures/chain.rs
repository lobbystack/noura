//! `sync-v2-chain.json`: schedules for the chain verifier (§14).

use serde_json::{Value, json};
use sync_protocol::chain::ChainRecord;
use sync_protocol::crypto::hash_parts;
use sync_protocol::manifest::ChainLink;
use sync_protocol::types::{Digest, Id};

use super::common::{Fixture, id};

struct Device {
    id: Id,
    records: Vec<ChainRecord>,
}

impl Device {
    fn new(label: &str) -> Self {
        Self {
            id: id(label),
            records: Vec::new(),
        }
    }

    fn head(&self) -> (u64, Digest) {
        self.records
            .last()
            .map_or((0, Digest::ZERO), |r| (r.counter, r.digest))
    }

    fn sign_at(&mut self, counter: u64, label: &str) -> ChainRecord {
        let (_, previous) = self.head();
        let r = ChainRecord {
            device: self.id,
            counter,
            previous,
            previous_attempt: Digest::ZERO,
            digest: hash_parts(&[b"chain fixture", label.as_bytes()]),
            seq: 0,
        };
        self.records.push(r);
        r
    }

    fn sign(&mut self, label: &str) -> ChainRecord {
        let next = self.head().0 + 1;
        self.sign_at(next, label)
    }

    fn reseal(&mut self, label: &str) -> ChainRecord {
        let last = self.records.pop().unwrap();
        let r = ChainRecord {
            previous_attempt: last.digest,
            digest: hash_parts(&[b"chain fixture", label.as_bytes()]),
            ..last
        };
        self.records.push(r);
        r
    }
}

fn at(r: ChainRecord, seq: u64) -> ChainRecord {
    ChainRecord { seq, ..r }
}

fn rec(r: &ChainRecord) -> Value {
    serde_json::to_value(r).unwrap()
}

fn schedule(
    f: &mut Fixture,
    name: &str,
    server: &[ChainRecord],
    start: &[ChainRecord],
    pages: &[Vec<ChainRecord>],
    expect: Value,
) {
    f.case(json!({
        "kind": "chain_schedule",
        "name": name,
        "server": server.iter().map(rec).collect::<Vec<_>>(),
        "start_heads": start.iter().map(rec).collect::<Vec<_>>(),
        "pages": pages.iter().map(|p| p.iter().map(rec).collect::<Vec<_>>()).collect::<Vec<_>>(),
        "expect": expect,
    }));
}

pub fn fixture() -> Fixture {
    let mut f = Fixture::new(
        "sync-v2-chain.json",
        "Device chains (sync-v2.md §14): schedules of served records and what the chain verifier reports for each. `server` lists what `GET .../chain/{device}/{counter}` serves (the record at that counter or the device's next one above it), `start_heads` the heads a bootstrap checkpoint gave, and `expect` either the verdict of each record of each page or the error and the page and index where it occurs. A `checkpoint_walk` case (§17, Checkpoint chain) gives the checkpoints a client `reached`, its `floor` and restore `generation`, the checkpoints `GET .../checkpoints/{digest}` serves, and a page's `entries`, and `expect` lists the checkpoints the client fetches before each entry, oldest first, or the index of the entry where the walk reports `sync_chain_gap`. A `fork_resolution` case gives the pages a reader `applied`, the page that `forked`, the head the first checkpoint covering the change that ends the device's write access `kept`, and the log's `records` from the reader's cursor through that checkpoint's `covered_seq`, with `accepted_covered_seq` when the reader accepted a checkpoint after the applied pages; `expect` holds the verdicts of the forked page judged again once the fork settles, with the digests of the applied records settling `reverted` when the fork point lies below the reader's head, or the error settling reports.",
    );
    let accept = json!({"verdict": "accept"});
    let ignore = json!({"verdict": "ignore"});
    let replace = |r: &ChainRecord| json!({"verdict": "replace", "replaced": r.digest.to_hex()});

    let mut d = Device::new("chain device");
    let (a, b, c) = (
        at(d.sign("a"), 1),
        at(d.sign_at(1_025, "b"), 2),
        at(d.sign_at(2_049, "c"), 3),
    );
    schedule(
        &mut f,
        "counters_skipped_at_each_start",
        &[],
        &[],
        &[vec![a, b, c]],
        json!([[accept, accept, accept]]),
    );
    schedule(
        &mut f,
        "retried_upload_served_once",
        &[],
        &[],
        &[vec![a, a, b]],
        json!([[accept, ignore, accept]]),
    );
    schedule(
        &mut f,
        "page_repeating_applied_positions",
        &[],
        &[],
        &[vec![a, b], vec![a, b, c]],
        json!([[accept, accept], [ignore, ignore, accept]]),
    );

    let mut d = Device::new("chain device");
    let (a, b, c) = (at(d.sign("a"), 1), at(d.sign("b"), 2), at(d.sign("c"), 3));
    schedule(
        &mut f,
        "dropped_record",
        &[a, b, c],
        &[],
        &[vec![a, c]],
        json!({"error": "gap", "page": 0, "index": 1}),
    );
    schedule(
        &mut f,
        "dropped_record_the_server_cant_produce",
        &[a, c],
        &[],
        &[vec![a, c]],
        json!({"error": "gap", "page": 0, "index": 1}),
    );
    let b_late = at(b, 4);
    schedule(
        &mut f,
        "reordered_record",
        &[a, b_late, c],
        &[],
        &[vec![a, c]],
        json!({"error": "reordered", "page": 0, "index": 1}),
    );
    schedule(
        &mut f,
        "replayed_record",
        &[],
        &[],
        &[vec![a, b, at(a, 3)]],
        json!({"error": "replayed", "page": 0, "index": 2}),
    );
    schedule(
        &mut f,
        "replay_right_after_bootstrap",
        &[],
        &[b],
        &[vec![at(a, 7)]],
        json!({"error": "replayed", "page": 0, "index": 0}),
    );
    schedule(
        &mut f,
        "repeat_below_a_bootstrap_head",
        &[],
        &[b],
        &[vec![a]],
        json!([[ignore]]),
    );
    let mut forker = Device::new("chain device");
    forker.sign("other a");
    let fork = at(forker.sign("other b"), 2);
    let mut fork2 = fork;
    fork2.counter = 2;
    fork2.previous = a.digest;
    fork2.digest = hash_parts(&[b"chain fixture", b"fork"]);
    schedule(
        &mut f,
        "forked_record",
        &[],
        &[],
        &[vec![a, b, at(fork2, 3)]],
        json!({"error": "forked", "page": 0, "index": 2}),
    );
    let mut not_linked = fork;
    not_linked.counter = 1;
    not_linked.previous = Digest::ZERO;
    schedule(
        &mut f,
        "two_records_at_one_position",
        &[],
        &[],
        &[vec![a, at(not_linked, 2)]],
        json!({"error": "forked", "page": 0, "index": 1}),
    );

    // Re-seal attempts.
    let mut d = Device::new("chain device");
    let first = d.sign("first attempt");
    let second = d.reseal("second attempt");
    let third = d.reseal("third attempt");
    schedule(
        &mut f,
        "reseal_attempts_in_order",
        &[],
        &[],
        &[vec![at(first, 1), at(second, 2), at(third, 3)]],
        json!([[accept, replace(&first), replace(&second)]]),
    );
    schedule(
        &mut f,
        "reseal_attempts_latest_first",
        &[],
        &[],
        &[vec![at(third, 1), at(second, 2), at(first, 3)]],
        json!([[accept, ignore, ignore]]),
    );
    schedule(
        &mut f,
        "reseal_attempt_linked_through_the_page",
        &[],
        &[],
        &[vec![at(first, 1), at(third, 2), at(second, 3)]],
        json!([[accept, replace(&first), ignore]]),
    );
    let sibling = ChainRecord {
        digest: hash_parts(&[b"chain fixture", b"sibling"]),
        ..second
    };
    schedule(
        &mut f,
        "sibling_attempts",
        &[],
        &[],
        &[vec![at(first, 1), at(second, 2), at(sibling, 3)]],
        json!({"error": "forked", "page": 0, "index": 2}),
    );
    let mut d = Device::new("chain device");
    let first = d.sign("first attempt");
    let next = d.sign("successor");
    let late = ChainRecord {
        previous_attempt: first.digest,
        digest: hash_parts(&[b"chain fixture", b"late attempt"]),
        ..first
    };
    schedule(
        &mut f,
        "attempt_after_a_successor_of_the_other",
        &[],
        &[],
        &[vec![at(first, 1), at(next, 2), at(late, 3)]],
        json!({"error": "forked", "page": 0, "index": 2}),
    );
    let mut d = Device::new("chain device");
    let mut line = vec![at(d.sign("attempt 0"), 1)];
    let mut verdicts = vec![accept.clone()];
    for i in 1..=6 {
        let previous = *line.last().unwrap();
        let r = at(d.reseal(&format!("attempt {i}")), i as u64 + 1);
        verdicts.push(replace(&previous));
        line.push(r);
    }
    schedule(
        &mut f,
        "more_than_4_reseal_attempts_forced_by_rotations",
        &[],
        &[],
        &[line],
        json!([verdicts]),
    );
    // A record the server rejected with snapshot_required, replaced at its
    // position by a different record that names it.
    let mut d = Device::new("chain device");
    let a = at(d.sign("a"), 1);
    d.sign("rejected patch");
    let snapshot = at(d.reseal("snapshot instead"), 2);
    schedule(
        &mut f,
        "record_replaced_at_its_position_after_snapshot_required",
        &[],
        &[],
        &[vec![a, snapshot]],
        json!([[accept, accept]]),
    );
    // The head's later attempt is fetched before judging its successor.
    let mut d = Device::new("chain device");
    let first = d.sign("first attempt");
    let second = d.reseal("second attempt");
    let next = d.sign("successor");
    schedule(
        &mut f,
        "later_attempt_of_the_head_fetched",
        &[at(second, 2), at(next, 3)],
        &[],
        &[vec![at(first, 1)], vec![at(next, 3)]],
        json!([[accept], [accept]]),
    );
    schedule(
        &mut f,
        "later_attempt_of_the_head_placed_after_its_successor",
        &[at(second, 4), at(next, 3)],
        &[],
        &[vec![at(first, 1)], vec![at(next, 3)]],
        json!({"error": "reordered", "page": 1, "index": 0}),
    );
    // A skipping record whose predecessor the page left out.
    let mut d = Device::new("chain device");
    let (a, b, c) = (
        at(d.sign("a"), 1),
        d.sign_at(1_025, "b"),
        at(d.sign("c"), 9),
    );
    schedule(
        &mut f,
        "skipping_predecessor_left_out",
        &[a, at(b, 4), c],
        &[],
        &[vec![a, c]],
        json!({"error": "gap", "page": 0, "index": 1}),
    );
    schedule(
        &mut f,
        "skipping_predecessor_placed_after",
        &[a, at(b, 12), c],
        &[],
        &[vec![a, c]],
        json!({"error": "reordered", "page": 0, "index": 1}),
    );
    // A head below the retention floor.
    f.case(json!({
        "kind": "chain_schedule",
        "name": "head_below_the_retention_floor",
        "server": [],
        "server_pruned": true,
        "start_heads": [],
        "pages": [[rec(&a), rec(&c)]],
        "expect": {"error": "pruned", "page": 0, "index": 1},
    }));
    checkpoint_walks(&mut f);
    fork_resolutions(&mut f);
    f
}

/// A `fork_resolution` case (§14): the reader applies `applied`, then
/// `forked` reports `sync_chain_forked`. Once the reader accepts the
/// change that ends the device's write access, the first checkpoint
/// covering its entry lists `kept` as the device's head, and `records` are
/// the log's records from the reader's cursor through that checkpoint's
/// `covered_seq`. `expect` holds the verdicts of `forked` judged again
/// after the fork settles, or the error that settling reports.
fn fork_resolution(
    f: &mut Fixture,
    name: &str,
    applied: &[Vec<ChainRecord>],
    forked: &[ChainRecord],
    records: &[ChainRecord],
    kept: &ChainRecord,
    expect: Value,
) {
    f.case(json!({
        "kind": "fork_resolution",
        "name": name,
        "applied": applied.iter().map(|p| p.iter().map(rec).collect::<Vec<_>>()).collect::<Vec<_>>(),
        "forked": forked.iter().map(rec).collect::<Vec<_>>(),
        "records": records.iter().map(rec).collect::<Vec<_>>(),
        "kept": rec(kept),
        "expect": expect,
    }));
}

fn fork_resolutions(f: &mut Fixture) {
    let accept = json!({"verdict": "accept"});
    let ignore = json!({"verdict": "ignore"});
    let forked = json!({"error": "forked"});

    // Two branches after `a`: `b` then `c`, and `b2` then `c2`.
    let mut one = Device::new("forking device");
    let a = at(one.sign("fork a"), 1);
    let mut two = Device {
        id: one.id,
        records: one.records.clone(),
    };
    let b = at(one.sign("fork b"), 2);
    let b2 = at(two.sign("fork b2"), 3);
    let c = at(one.sign("fork c"), 4);
    let c2 = at(two.sign("fork c2"), 5);
    let page = vec![b, b2, c, c2];
    fork_resolution(
        f,
        "a_fork_settled_on_the_branch_served_second",
        &[vec![a]],
        &page,
        &page,
        &c2,
        json!([ignore, accept, ignore, accept]),
    );
    fork_resolution(
        f,
        "a_fork_settled_on_the_branch_served_first",
        &[vec![a]],
        &page,
        &page,
        &c,
        json!([accept, ignore, accept, ignore]),
    );
    fork_resolution(
        f,
        "a_fork_settled_at_the_readers_own_head",
        &[vec![a]],
        &page,
        &page,
        &a,
        json!([ignore, ignore, ignore, ignore]),
    );
    let stray = ChainRecord {
        digest: hash_parts(&[b"chain fixture", b"fork stray"]),
        ..c2
    };
    fork_resolution(
        f,
        "a_kept_head_the_records_dont_hold",
        &[vec![a]],
        &page,
        &page,
        &stray,
        forked.clone(),
    );
    fork_resolution(
        f,
        "a_kept_head_at_another_sequence",
        &[vec![a]],
        &page,
        &page,
        &at(c2, 6),
        forked.clone(),
    );
    // The reader applied one branch past the fork before the other
    // arrived: the kept branch doesn't extend its head, so the reader
    // reverts the records it applied above the fork point and takes the
    // kept branch.
    fork_resolution(
        f,
        "a_kept_branch_that_doesnt_extend_the_readers_head_reverts_the_other",
        &[vec![a, b, c]],
        &[b2, c2],
        &[b2, c2],
        &c2,
        json!({
            "reverted": [b.digest.to_hex(), c.digest.to_hex()],
            "verdicts": [accept, accept],
        }),
    );
    // The same with the kept branch's head at the fork point itself: the
    // reader reverts everything it applied above `a`.
    fork_resolution(
        f,
        "a_kept_head_below_the_readers_head_reverts_above_it",
        &[vec![a, b, c]],
        &[b2],
        &[b2],
        &a,
        json!({
            "reverted": [b.digest.to_hex(), c.digest.to_hex()],
            "verdicts": [ignore],
        }),
    );
    // Once the reader accepted a checkpoint covering `b`, it keeps no
    // position below `b`, so the fork point `a` is gone and the fork can't
    // settle.
    let b2_late = at(b2, 5);
    let c2_late = at(c2, 6);
    let mut case = json!({
        "kind": "fork_resolution",
        "name": "a_fork_point_below_the_accepted_checkpoint",
        "applied": [[rec(&a), rec(&b), rec(&c)]],
        "accepted_covered_seq": 2,
        "forked": [rec(&b2_late), rec(&c2_late)],
        "records": [rec(&b2_late), rec(&c2_late)],
        "kept": rec(&c2_late),
        "expect": forked,
    });
    f.case(std::mem::take(&mut case));

    // Sibling re-seal attempts of the reader's head.
    let mut d = Device::new("forking device");
    let first = at(d.sign("fork attempt 1"), 1);
    let second = at(d.reseal("fork attempt 2"), 2);
    let sibling = ChainRecord {
        digest: hash_parts(&[b"chain fixture", b"fork attempt 2 sibling"]),
        seq: 3,
        ..second
    };
    let next = ChainRecord {
        counter: 2,
        previous: sibling.digest,
        previous_attempt: Digest::ZERO,
        digest: hash_parts(&[b"chain fixture", b"fork after the sibling"]),
        seq: 4,
        ..sibling
    };
    let page = vec![second, sibling, next];
    fork_resolution(
        f,
        "sibling_attempts_settled_on_the_later_sibling",
        &[vec![first]],
        &page,
        &page,
        &next,
        json!([ignore, {"verdict": "replace", "replaced": first.digest.to_hex()}, accept]),
    );
}

fn link(r: &ChainLink) -> Value {
    json!({"number": r.number, "digest": r.digest.to_hex(), "previous": r.previous.to_hex()})
}

/// A checkpoint entry at `seq` that covers the log through `covered_seq`.
fn entry(r: &ChainLink, seq: u64, covered_seq: u64) -> Value {
    json!({"checkpoint": link(r), "seq": seq, "covered_seq": covered_seq})
}

/// An operation at `seq` that names the checkpoint `named` covering
/// `checkpoint_seq`, written in restore generation `generation`.
fn names(named: Option<&ChainLink>, checkpoint_seq: u64, generation: u64, seq: u64) -> Value {
    json!({"operation": {
        "seq": seq,
        "generation": generation,
        "checkpoint_seq": checkpoint_seq,
        "checkpoint": named.map_or(Digest::ZERO, |n| n.digest).to_hex(),
    }})
}

fn fetched(before: usize, walk: &[&ChainLink]) -> Value {
    json!({"before": before, "checkpoints": walk.iter().map(|l| l.digest.to_hex()).collect::<Vec<_>>()})
}

/// Schedules for the walk that fetches the checkpoints a client's log left
/// out (§17, Checkpoint chain).
fn checkpoint_walks(f: &mut Fixture) {
    // Checkpoints 1 to 5 of one domain, checkpoint n covering the log
    // through 10n and committing at 10n + 5.
    let mut chain: Vec<ChainLink> = Vec::new();
    for number in 1..=5u64 {
        let previous = chain.last().map_or(Digest::ZERO, |c| c.digest);
        chain.push(ChainLink {
            number,
            digest: hash_parts(&[b"checkpoint fixture", &number.to_be_bytes()]),
            previous,
        });
    }
    let c = |n: usize| &chain[n - 1];
    let walk = |name: &str,
                reached: &[&ChainLink],
                floor: u64,
                server: &[&ChainLink],
                entries: Vec<Value>,
                expect: Value| {
        json!({
            "kind": "checkpoint_walk",
            "name": name,
            "reached": reached.iter().map(|l| link(l)).collect::<Vec<_>>(),
            "floor": floor,
            "generation": 1,
            "server": server.iter().map(|l| link(l)).collect::<Vec<_>>(),
            "entries": entries,
            "expect": expect,
        })
    };
    let none = json!({"fetched": []});
    f.case(walk(
        "the_next_checkpoint_entry",
        &[c(1)],
        0,
        &[],
        vec![names(Some(c(1)), 10, 1, 16), entry(c(2), 25, 20)],
        none.clone(),
    ));
    // The client applied records past `covered_seq` before the entry
    // arrived; it still reaches the checkpoint without a fetch, and checks
    // it against its live set at `covered_seq`.
    f.case(walk(
        "a_checkpoint_that_arrives_after_the_client_passed_its_covered_seq",
        &[c(1)],
        0,
        &[],
        vec![
            names(Some(c(1)), 10, 1, 21),
            names(Some(c(1)), 10, 1, 22),
            entry(c(2), 25, 20),
            names(Some(c(2)), 20, 1, 26),
        ],
        none.clone(),
    ));
    // The server left checkpoint 2's entry out of this client's log, while
    // another writer's records, which name it, still cross.
    f.case(walk(
        "a_checkpoint_entry_left_out_while_records_still_cross",
        &[c(1)],
        0,
        &[c(2)],
        vec![names(Some(c(1)), 10, 1, 21), names(Some(c(2)), 20, 1, 26)],
        json!({"fetched": [fetched(1, &[c(2)])]}),
    ));
    f.case(walk(
        "two_checkpoints_left_out_walked_back_through_previous",
        &[c(1)],
        0,
        &[c(2), c(3)],
        vec![names(Some(c(3)), 30, 1, 36), entry(c(4), 45, 40)],
        json!({"fetched": [fetched(0, &[c(2), c(3)])]}),
    ));
    f.case(walk(
        "a_checkpoint_entry_whose_previous_the_log_left_out",
        &[c(1)],
        0,
        &[c(2)],
        vec![entry(c(3), 35, 30)],
        json!({"fetched": [fetched(0, &[c(2)])]}),
    ));
    f.case(walk(
        "a_left_out_checkpoint_the_server_cant_serve",
        &[c(1)],
        0,
        &[],
        vec![names(Some(c(2)), 20, 1, 26)],
        json!({"error": "gap", "index": 0}),
    ));
    f.case(walk(
        "a_walk_that_misses_a_link",
        &[c(1)],
        0,
        &[c(3)],
        vec![names(Some(c(3)), 30, 1, 36)],
        json!({"error": "gap", "index": 0}),
    ));
    // A served record whose `previous` names a checkpoint the walk passed.
    let forged = ChainLink {
        number: 3,
        digest: c(3).digest,
        previous: c(4).digest,
    };
    f.case(walk(
        "a_walk_that_loops_back_above_the_last_reached",
        &[c(1)],
        0,
        &[c(4), &forged],
        vec![entry(c(5), 55, 50)],
        json!({"error": "gap", "index": 0}),
    ));
    f.case(walk(
        "a_record_naming_a_checkpoint_below_the_floor",
        &[c(3)],
        25,
        &[],
        vec![names(Some(c(2)), 20, 1, 36)],
        none.clone(),
    ));
    f.case(walk(
        "a_record_of_an_earlier_generation",
        &[],
        0,
        &[],
        vec![names(Some(c(2)), 20, 0, 3)],
        none.clone(),
    ));
    f.case(walk(
        "a_record_before_the_first_checkpoint",
        &[],
        0,
        &[],
        vec![names(None, 0, 1, 3)],
        none,
    ));
}
