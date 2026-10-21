//! `sync-v2-chain.json`: schedules for the chain verifier (§14).

use serde_json::{Value, json};
use sync_protocol::chain::ChainRecord;
use sync_protocol::crypto::hash_parts;
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
        "Device chains (sync-v2.md §14): schedules of served records and what the chain verifier reports for each. `server` lists what `GET .../chain/{device}/{counter}` serves (the record at that counter or the device's next one above it), `start_heads` the heads a bootstrap checkpoint gave, and `expect` either the verdict of each record of each page or the error and the page and index where it occurs.",
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
    f
}
