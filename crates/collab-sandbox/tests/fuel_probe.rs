mod support;

use std::time::Instant;

use collab_sandbox::{Event, RemoteRecord};
use support::{Lib0, insert};

const ME: u64 = (1 << 29) + 1;

fn structs(count: u64, alternate: bool, chain: bool) -> Vec<u8> {
    let mut w = Lib0::default().var(1).var(count).var(ME).var(0);
    let mut previous = (0u64, 0u64);
    for i in 0..count {
        let deleted = alternate && i % 2 == 1;
        w = w.raw(&[if deleted { 0x81 } else { 0x84 }]);
        let origin = if chain { previous } else { (0, 0) };
        w = w.var(origin.0).var(origin.1);
        w = if deleted { w.var(1) } else { w.string("a") };
        previous = (ME, i);
    }
    w.var(0).0
}

#[test]
#[ignore]
fn probe() {
    for (alternate, chain) in [(false, true), (true, true), (true, false)] {
        for count in [1_000u64, 4_000, 16_000] {
            let mut note = support::note(vec![insert(0, 0, "x")]);
            let yrs = structs(count, alternate, chain);
            let len = yrs.len();
            let started = Instant::now();
            let effects = note.receive(
                RemoteRecord { id: 1, writer: 1, card_count: 1, payload: support::update_payload(1, &yrs), blob: None },
                Instant::now(),
            );
            let mut verdict_fuel = 0;
            let mut merge_fuel = 0;
            for event in &effects.events {
                match event {
                    Event::Verdict { verdict, .. } => verdict_fuel = verdict.fuel,
                    Event::Merged { fuel, .. } => merge_fuel = *fuel,
                    _ => {}
                }
            }
            eprintln!(
                "alternate {alternate} chain {chain} count {count}: {len} bytes, verdict {} per byte, merge {} per struct, {:?}",
                verdict_fuel / len as u64,
                merge_fuel / count,
                started.elapsed()
            );
        }
    }
}
