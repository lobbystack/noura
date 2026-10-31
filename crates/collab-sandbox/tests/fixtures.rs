//! The sandbox against `sync-v2-collaboration.json`: the pinned module, its
//! budget and operator costs, and every raw-check, verdict, surrogate,
//! client ID and allowance case, on each backend this build runs.

mod support;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use collab_sandbox::{
    ALLOWANCE_FUEL, Event, FREE_OPERATORS, GUEST_MODULE, GUEST_MODULE_HASH, INTEGRATION_FUEL_LIMIT,
    REFILL_PERIOD, Record, RemoteRecord, Unreadable, VARIABLE_COSTS, VERDICT_BASE_FUEL,
    VERDICT_FUEL_PER_BYTE, VERDICT_MEMORY_BYTES, operator_costs,
};
use serde_json::{Value, json};
use support::{cases, client_ids, fixture, hex, note, sandbox};

fn code(outcome: Result<(), Unreadable>) -> Value {
    match outcome {
        Ok(()) => Value::Null,
        Err(reason) => json!(reason.code()),
    }
}

#[test]
fn the_sandbox_runs_the_module_the_fixture_pins() {
    let pinned = &fixture()["sandbox"]["module"];
    assert_eq!(pinned["blake3"], GUEST_MODULE_HASH);
    assert_eq!(
        blake3::hash(GUEST_MODULE).to_hex().as_str(),
        GUEST_MODULE_HASH
    );
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(pinned["path"].as_str().unwrap());
    assert_eq!(std::fs::read(path).unwrap(), GUEST_MODULE);
}

#[test]
fn the_budget_and_limits_are_the_fixtures() {
    let sandbox = &fixture()["sandbox"];
    let verdict = &sandbox["verdict"];
    assert_eq!(verdict["base_fuel"], VERDICT_BASE_FUEL);
    assert_eq!(verdict["fuel_per_byte"], VERDICT_FUEL_PER_BYTE);
    assert_eq!(verdict["memory_bytes"], VERDICT_MEMORY_BYTES as u64);
    assert_eq!(verdict["epoch_interruption"], false);
    let integration = &sandbox["integration"];
    assert_eq!(integration["allowance_fuel"], ALLOWANCE_FUEL);
    assert_eq!(integration["refill_seconds"], 60);
    assert_eq!(integration["record_fuel_limit"], INTEGRATION_FUEL_LIMIT);
}

/// `n` as the spec writes it, with a comma between each group of three
/// digits.
fn spelled(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// The section of `spec` under `heading`, up to the next section.
fn section<'a>(spec: &'a str, heading: &str) -> &'a str {
    let start = spec
        .find(heading)
        .unwrap_or_else(|| panic!("{heading} in the spec"));
    let rest = &spec[start + heading.len()..];
    &rest[..rest.find("\n## ").unwrap_or(rest.len())]
}

/// The budget and the integration limits as sync-v2.md's text states them,
/// in §22 and in the table of §27, so a change to the constants fails
/// until the spec's text changes with them, as well as the fixture.
#[test]
fn the_budget_and_limits_are_the_spec_text() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/workspace-format/sync-v2.md");
    let spec = std::fs::read_to_string(path).unwrap();
    let collaboration = section(&spec, "## 22. Live collaboration");
    let limits = section(&spec, "## 27. Limits");
    let base = spelled(VERDICT_BASE_FUEL);
    let per_byte = spelled(VERDICT_FUEL_PER_BYTE);
    let memory = VERDICT_MEMORY_BYTES >> 20;
    let allowance = spelled(ALLOWANCE_FUEL as u64);
    let record = spelled(INTEGRATION_FUEL_LIMIT);
    let integration = &fixture()["sandbox"]["integration"];
    let document = integration["document_bytes"].as_u64().unwrap() >> 20;
    let text = integration["text_bytes"].as_u64().unwrap() >> 20;
    assert_eq!(REFILL_PERIOD, Duration::from_secs(60));
    for (section, name, sentence) in [
        (
            collaboration,
            "§22",
            format!(
                "with a fuel budget of {base} units plus {per_byte} per byte of the \
                 record's decrypted payload"
            ),
        ),
        (
            collaboration,
            "§22",
            format!("and {memory} MiB of linear memory"),
        ),
        (
            collaboration,
            "§22",
            format!(
                "Each writer holds an allowance per note of {allowance} fuel units, \
                 refilled each minute up to that amount"
            ),
        ),
        (
            collaboration,
            "§22",
            format!(
                "When one record's integration alone passes {record} units, or would take \
                 the document past {document} MiB or its text past {text} MiB"
            ),
        ),
        (
            limits,
            "§27",
            format!(
                "| Collaboration sandbox verdict, per record | {base} fuel units plus \
                 {per_byte} per byte of the decrypted payload and blob file, {memory} MiB of \
                 memory, one pinned guest module |"
            ),
        ),
        (
            limits,
            "§27",
            format!(
                "| Collaboration sandbox allowance | {allowance} fuel units per writer and \
                 note, refilled each minute and overdrawn as debt; {record} for one record's \
                 integration |"
            ),
        ),
        (
            limits,
            "§27",
            format!(
                "a record that jumps past {text} MiB of text or {document} MiB of document \
                 stops the lineage on that device |"
            ),
        ),
    ] {
        assert!(
            section.contains(&sentence),
            "{name} of sync-v2.md doesn't say: {sentence}"
        );
    }
}

#[test]
fn the_operator_costs_are_the_fixtures() {
    let recorded = &fixture()["sandbox"]["operator_costs"];
    assert_eq!(recorded["default"], 1);
    assert_eq!(recorded["free"], json!(FREE_OPERATORS));
    let variable: BTreeMap<String, u64> =
        serde_json::from_value(recorded["variable"].clone()).unwrap();
    let ours: BTreeMap<String, u64> = VARIABLE_COSTS
        .iter()
        .map(|(name, cost)| ((*name).to_owned(), u64::from(*cost)))
        .collect();
    assert_eq!(variable, ours);

    // The table the engine runs with holds exactly those costs.
    let table = serde_json::to_value(operator_costs()).unwrap();
    for (name, cost) in table.as_object().unwrap() {
        if name == "variable" {
            let table: BTreeMap<String, u64> = serde_json::from_value(cost.clone()).unwrap();
            assert_eq!(table, ours);
        } else {
            let free = FREE_OPERATORS.contains(&name.as_str());
            assert_eq!(cost, &json!(u8::from(!free)), "{name}");
        }
    }
}

fn record(case: &Value) -> (Vec<u8>, Option<Vec<u8>>) {
    let blob = (!case["blob"].is_null()).then(|| hex(&case["blob"]));
    (hex(&case["payload"]), blob)
}

#[test]
fn every_verdict_case_reaches_the_fixtures_verdict_on_every_backend() {
    let all: Vec<Value> = cases("verdict")
        .into_iter()
        .chain(cases("client_round_trip"))
        .collect();
    assert!(all.len() > 80, "{} cases", all.len());
    for case in all {
        let (payload, blob) = record(&case);
        let record = Record {
            payload: &payload,
            blob: blob.as_deref(),
            card_index: case["card_index"].as_u64().unwrap() as u32,
            card_count: case["card_count"].as_u64().unwrap() as u32,
        };
        let mut fuel = None;
        for (backend, sandbox) in support::backends() {
            let verdict = sandbox.verdict(&record);
            assert_eq!(
                code(verdict.outcome),
                case["expect"],
                "{} on {backend}",
                case["name"]
            );
            // Fuel counts alike on every backend, so verdicts near the
            // budget agree too.
            match fuel {
                None => fuel = Some(verdict.fuel),
                Some(fuel) => assert_eq!(verdict.fuel, fuel, "{} fuel", case["name"]),
            }
        }
    }
}

#[test]
fn surrogate_splits_reach_one_text_arriving_or_from_a_snapshot() {
    let surrogates = cases("surrogate");
    assert_eq!(surrogates.len(), 5);
    for case in surrogates {
        let name = &case["name"];
        // A device that integrates the record as it arrives.
        let mut arriving = note(vec![hex(&case["base"])]);
        let effects = arriving.receive(
            RemoteRecord {
                id: 1,
                writer: 1,
                card_count: 1,
                payload: hex(&case["payload"]),
                blob: None,
                place: Default::default(),
            },
            Instant::now(),
        );
        assert!(
            effects.events.contains(&Event::Merged {
                id: 1,
                fuel: merged_fuel(&effects.events)
            }),
            "{name}: {:?}",
            effects.events
        );
        assert_eq!(arriving.text().unwrap(), case["text"], "{name}");

        // A device that boots from the first device's snapshot.
        let snapshot = arriving.state().unwrap();
        let mut booted = note(vec![snapshot]);
        assert_eq!(
            booted.text().unwrap(),
            case["text"],
            "{name} from a snapshot"
        );
        assert_eq!(booted.len(), arriving.len(), "{name}");
    }
}

fn merged_fuel(events: &[Event]) -> u64 {
    events
        .iter()
        .find_map(|event| match event {
            Event::Merged { fuel, .. } => Some(*fuel),
            _ => None,
        })
        .unwrap_or_default()
}

#[test]
fn client_ids_past_32_bits_round_trip_through_an_update() {
    let wide = cases("client_round_trip");
    assert_eq!(wide.len(), 2);
    for case in wide {
        let client = case["client"].as_u64().unwrap();
        let card = case["card_index"].as_u64().unwrap() as u32;
        let mut first = note(vec![]);
        first.receive(
            RemoteRecord {
                id: 1,
                writer: card,
                card_count: card,
                payload: hex(&case["payload"]),
                blob: None,
                place: Default::default(),
            },
            Instant::now(),
        );
        assert_eq!(first.text().unwrap(), case["text"]);
        // The client ID survives the document's own encoding: the state
        // names it, and a device booted from it holds the same text.
        let state = first.state().unwrap();
        let mut encoded_client = Vec::new();
        let mut value = client;
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                encoded_client.push(byte);
                break;
            }
            encoded_client.push(byte | 0x80);
        }
        assert!(
            state
                .windows(encoded_client.len())
                .any(|window| window == encoded_client),
            "the state names client {client}"
        );
        let mut booted = note(vec![state]);
        assert_eq!(booted.text().unwrap(), case["text"]);
    }
}

#[test]
fn a_document_writes_under_the_largest_client_id() {
    let largest = (1u64 << 53) - 1;
    let mut writer =
        collab_sandbox::Note::open(sandbox(), vec![], Box::new(move || Some(largest))).unwrap();
    let op = collab_sandbox::op::Op::from_splices(
        0,
        &[collab_sandbox::op::Splice {
            start: 0,
            removed: 0,
            inserted: "edge".into(),
        }],
    )
    .unwrap();
    writer.local(&op, true, 0).unwrap();
    let [update] = writer
        .take_updates()
        .unwrap()
        .try_into()
        .expect("one update");
    assert_eq!(update.load, 536_870_911);
    let payload = support::update_payload(update.load, &update.bytes);
    let verdict = sandbox().verdict(&Record {
        payload: &payload,
        blob: None,
        card_index: 16_777_215,
        card_count: 16_777_215,
    });
    assert_eq!(verdict.outcome, Ok(()));
    let mut reader = note(vec![]);
    reader.receive(
        RemoteRecord {
            id: 1,
            writer: 16_777_215,
            card_count: 16_777_215,
            payload,
            blob: None,
            place: Default::default(),
        },
        Instant::now(),
    );
    assert_eq!(reader.text().unwrap(), "edge");
}

#[test]
fn allowance_base_states_defer_an_overdrawn_writer_alone() {
    let allowance_cases = cases("allowance");
    assert_eq!(allowance_cases.len(), 2);
    for case in allowance_cases {
        let name = &case["name"];
        let start = Instant::now();
        let mut note =
            collab_sandbox::Note::open(sandbox(), vec![hex(&case["base"])], client_ids()).unwrap();
        for (writer, balance) in case["balances"].as_object().unwrap() {
            note.set_balance(writer.parse().unwrap(), balance.as_i64().unwrap(), start);
        }
        for step in case["steps"].as_array().unwrap() {
            if let Some(receive) = step.get("receive") {
                let id = receive["id"].as_u64().unwrap();
                let effects = note.receive(
                    RemoteRecord {
                        id,
                        writer: receive["card_index"].as_u64().unwrap() as u32,
                        card_count: receive["card_count"].as_u64().unwrap() as u32,
                        payload: hex(&receive["payload"]),
                        blob: None,
                        place: Default::default(),
                    },
                    start,
                );
                let outcome =
                    if effects.events.iter().any(
                        |event| matches!(event, Event::Merged { id: merged, .. } if *merged == id),
                    ) {
                        "merged"
                    } else if effects.events.contains(&Event::Deferred { id }) {
                        "deferred"
                    } else {
                        "other"
                    };
                assert_eq!(outcome, step["expect"], "{name} record {id}: {effects:?}");
            } else {
                let at = start + Duration::from_secs(step["after_seconds"].as_u64().unwrap());
                let effects = note.wake(at);
                let merged: Vec<u64> = effects
                    .events
                    .iter()
                    .filter_map(|event| match event {
                        Event::Merged { id, .. } => Some(*id),
                        _ => None,
                    })
                    .collect();
                assert_eq!(json!(merged), step["expect_merged"], "{name} at {step}");
            }
        }
        assert_eq!(note.text().unwrap(), case["text"], "{name}");
        assert_eq!(note.deferred(), 0, "{name}");
    }
}

#[test]
fn a_trap_during_integration_stops_the_lineage_and_rebuilds_the_last_good_state() {
    let trap_cases = cases("trap");
    assert_eq!(trap_cases.len(), 1);
    for case in trap_cases {
        let name = &case["name"];
        let mut note = note(vec![hex(&case["base"])]);
        for step in case["steps"].as_array().unwrap() {
            let receive = &step["receive"];
            let id = receive["id"].as_u64().unwrap();
            if step["trap"] == true {
                note.trap_next_integration();
            }
            let effects = note.receive(
                RemoteRecord {
                    id,
                    writer: receive["card_index"].as_u64().unwrap() as u32,
                    card_count: receive["card_count"].as_u64().unwrap() as u32,
                    payload: hex(&receive["payload"]),
                    blob: None,
                    place: Default::default(),
                },
                Instant::now(),
            );
            let events = &effects.events;
            let outcome = if events
                .iter()
                .any(|event| matches!(event, Event::Merged { id: merged, .. } if *merged == id))
            {
                "merged"
            } else if events.contains(&Event::Stopped {
                id,
                reason: collab_sandbox::StopReason::Trap,
            }) {
                // The device discards the instance and rebuilds it.
                assert!(events.contains(&Event::Rebuilt), "{name}: {events:?}");
                assert!(effects.changes.is_empty(), "{name}");
                "stopped"
            } else if events.contains(&Event::Skipped { id }) {
                "skipped"
            } else {
                "other"
            };
            assert_eq!(outcome, step["expect"], "{name} record {id}: {events:?}");
        }
        assert!(note.is_stopped());
        assert_eq!(note.text().unwrap(), case["text"], "{name}");
        // Nothing more of the lineage goes out.
        assert_eq!(note.take_updates().unwrap(), []);
        assert!(matches!(
            note.status(),
            collab_sandbox::LineageStatus::Stopped(collab_sandbox::StopReason::Trap)
        ));
    }
}

/// Bytes a limit case gives as a prefix, a unit repeated and a suffix.
fn expand(recipe: &Value) -> Vec<u8> {
    let unit = hex(&recipe["unit"]);
    let count = recipe["count"].as_u64().unwrap() as usize;
    let mut bytes = hex(&recipe["prefix"]);
    bytes.extend_from_slice(&unit.repeat(count));
    bytes.extend_from_slice(&hex(&recipe["suffix"]));
    bytes
}

/// Records at the verdict's fuel and memory limits reach the fixture's
/// verdict, with the same fuel, on every backend.
#[test]
fn records_at_the_verdicts_limits_reach_the_same_verdict_and_fuel_on_every_backend() {
    let limit_cases = cases("verdict_limit");
    assert_eq!(limit_cases.len(), 5);
    for case in limit_cases {
        let name = &case["name"];
        let payload = expand(&case["payload"]);
        let blob = (!case["blob"].is_null()).then(|| expand(&case["blob"]));
        let record = Record {
            payload: &payload,
            blob: blob.as_deref(),
            card_index: case["card_index"].as_u64().unwrap() as u32,
            card_count: case["card_count"].as_u64().unwrap() as u32,
        };
        for (backend, sandbox) in support::backends() {
            let started = Instant::now();
            let verdict = sandbox.verdict(&record);
            eprintln!("{name} on {backend}: {:?}", started.elapsed());
            assert_eq!(code(verdict.outcome), case["expect"], "{name} on {backend}");
            assert_eq!(verdict.fuel, case["fuel"], "{name} fuel on {backend}");
        }
    }
}
