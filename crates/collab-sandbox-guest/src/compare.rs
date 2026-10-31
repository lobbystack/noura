//! The snapshot check (sync-v2.md §22): compares a record with the other
//! records of its lineage that a device took in, element by element.
//!
//! An element is one clock position of a yrs item, with its content, its
//! left origin and its right origin. Two copies of one element agree when
//! their origins match and their content matches, a deleted copy matching
//! any content. U+FFFD matches a unit of a surrogate pair only where some
//! record splits that pair: an origin, a right origin or the edge of a
//! delete range falls between its two units. Item boundaries alone say
//! nothing, because an encoder may place them anywhere. A snapshot also
//! disagrees when it lacks an element of a record it covers, or shows as
//! not deleted an element that a record it covers deleted.

use std::collections::BTreeSet;

use crate::Unreadable;
use crate::lib0::{Item, Items, read_items};

const REPLACEMENT: u16 = 0xfffd;

/// The bytes of a [`crate::compare_records`] call.
pub struct Input<'a> {
    /// The record under comparison.
    pub subject: &'a [u8],
    /// The lineage's other records, each with whether the subject covers
    /// it.
    pub others: Vec<(&'a [u8], bool)>,
}

impl<'a> Input<'a> {
    /// Reads the call's input, or `None` when its lengths don't fit.
    pub fn parse(bytes: &'a [u8]) -> Option<Self> {
        let (subject, mut rest) = take_record(bytes)?;
        let mut others = Vec::new();
        while !rest.is_empty() {
            let (flag, tail) = rest.split_first()?;
            let (record, tail) = take_record(tail)?;
            others.push((record, *flag == 1));
            rest = tail;
        }
        Some(Self { subject, others })
    }
}

fn take_record(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let (word, rest) = bytes.split_first_chunk::<4>()?;
    rest.split_at_checked(u32::from_le_bytes(*word) as usize)
}

/// The positions that sit between two elements because a record names an
/// element next to them: each is the element on the right of the gap.
#[derive(Default)]
struct Gaps(BTreeSet<(u64, u64)>);

impl Gaps {
    fn note(&mut self, items: &Items<'_>) {
        for (client, list) in &items.clients {
            for item in list {
                // An item that follows on from its own client's last
                // element shows only where the encoder cut a run.
                let continues = item.origin == Some((*client, item.clock.wrapping_sub(1)));
                if let (Some((origin_client, origin_clock)), false) = (item.origin, continues) {
                    self.0.insert((origin_client, origin_clock + 1));
                }
                if let Some(right) = item.right {
                    self.0.insert(right);
                }
            }
        }
        for (client, ranges) in &items.deletes {
            for (clock, len) in ranges {
                self.0.insert((*client, *clock));
                self.0.insert((*client, clock + len));
            }
        }
    }

    /// Whether a gap falls between the pair whose high unit sits at
    /// `high_clock`.
    fn splits(&self, client: u64, high_clock: u64) -> bool {
        self.0.contains(&(client, high_clock + 1))
    }
}

fn units(item: &Item<'_>) -> Option<Vec<u16>> {
    item.text.map(|text| text.encode_utf16().collect())
}

fn is_high(unit: u16) -> bool {
    (0xd800..0xdc00).contains(&unit)
}

fn is_low(unit: u16) -> bool {
    (0xdc00..0xe000).contains(&unit)
}

/// Whether two units of one element agree, at `clock` of `client`.
fn units_agree(a: u16, b: u16, client: u64, clock: u64, gaps: &Gaps) -> bool {
    if a == b {
        return true;
    }
    let other = match (a, b) {
        (REPLACEMENT, other) | (other, REPLACEMENT) => other,
        _ => return false,
    };
    if is_high(other) {
        gaps.splits(client, clock)
    } else if is_low(other) {
        clock > 0 && gaps.splits(client, clock - 1)
    } else {
        false
    }
}

/// The left origin of the element at `clock` in `item`.
fn origin_at(item: &Item<'_>, client: u64, clock: u64) -> Option<(u64, u64)> {
    if clock == item.clock {
        item.origin
    } else {
        Some((client, clock - 1))
    }
}

/// The items of `list` that hold clocks from `start` on, by rising clock.
fn from_clock<'b, 'a>(list: &'b [Item<'a>], start: u64) -> &'b [Item<'a>] {
    &list[list.partition_point(|item| item.clock + item.len <= start)..]
}

/// Whether the copies of the elements that `record` and `subject` both hold
/// disagree, or, when `covered`, `subject` lacks one of `record`'s elements
/// or shows as not deleted one it deleted.
fn compare(subject: &Items<'_>, record: &Items<'_>, covered: bool, gaps: &Gaps) -> bool {
    for (client, list) in &record.clients {
        let theirs = subject.clients.get(client).map_or(&[][..], Vec::as_slice);
        for item in list {
            let end = item.clock + item.len;
            let mut at = item.clock;
            let mine = units(item);
            for other in from_clock(theirs, item.clock) {
                if other.clock >= end {
                    break;
                }
                let start = at.max(other.clock);
                if covered && start > at {
                    return true;
                }
                let stop = end.min(other.clock + other.len);
                if item.right != other.right
                    || origin_at(item, *client, start) != origin_at(other, *client, start)
                {
                    return true;
                }
                if let (Some(a), Some(b)) = (&mine, units(other)) {
                    for clock in start..stop {
                        let left = a[(clock - item.clock) as usize];
                        let right = b[(clock - other.clock) as usize];
                        if !units_agree(left, right, *client, clock, gaps) {
                            return true;
                        }
                    }
                }
                at = stop;
            }
            if covered && at < end {
                return true;
            }
        }
    }
    if covered {
        for (client, ranges) in &record.deletes {
            let theirs = subject.clients.get(client).map_or(&[][..], Vec::as_slice);
            let deleted = subject.deletes.get(client);
            for (clock, len) in ranges {
                if shows(theirs, deleted, *clock, clock + len) {
                    return true;
                }
            }
        }
    }
    false
}

/// Whether `list` holds a string element in `start..end` that no range of
/// `deleted` deletes.
fn shows(
    list: &[Item<'_>],
    deleted: Option<&Vec<(u64, u64)>>,
    start: u64,
    end: u64,
) -> bool {
    let mut ranges: Vec<(u64, u64)> = deleted.cloned().unwrap_or_default();
    ranges.sort_unstable();
    for item in from_clock(list, start) {
        if item.clock >= end {
            break;
        }
        if item.text.is_none() {
            continue;
        }
        let mut at = item.clock.max(start);
        let stop = (item.clock + item.len).min(end);
        for (clock, len) in &ranges {
            if at >= stop {
                break;
            }
            if *clock > at {
                return true;
            }
            at = at.max(clock + len);
        }
        if at < stop {
            return true;
        }
    }
    false
}

/// Whether `subject` disagrees with `others`, each with whether it covers
/// that record.
pub fn disagrees(subject: &[u8], others: &[(&[u8], bool)]) -> Result<bool, Unreadable> {
    let subject = read_items(subject)?;
    let mut records = Vec::with_capacity(others.len());
    for (bytes, covered) in others {
        records.push((read_items(bytes)?, *covered));
    }
    let mut gaps = Gaps::default();
    gaps.note(&subject);
    for (record, _) in &records {
        gaps.note(record);
    }
    Ok(records
        .iter()
        .any(|(record, covered)| compare(&subject, record, *covered, &gaps)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn var(mut value: u64, out: &mut Vec<u8>) {
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                out.push(byte);
                return;
            }
            out.push(byte | 0x80);
        }
    }

    fn string(text: &str, out: &mut Vec<u8>) {
        var(text.len() as u64, out);
        out.extend_from_slice(text.as_bytes());
    }

    /// One struct of `client` at `clock`, its text or deletion after the
    /// origins, followed by the delete ranges.
    fn update(
        client: u64,
        clock: u64,
        origin: Option<(u64, u64)>,
        right: Option<(u64, u64)>,
        text: Result<&str, u64>,
        deletes: &[(u64, u64, u64)],
    ) -> Vec<u8> {
        let mut out = vec![1, 1];
        var(client, &mut out);
        var(clock, &mut out);
        let content = if text.is_ok() { 4u8 } else { 1 };
        out.push(
            content
                | if origin.is_some() { 0x80 } else { 0 }
                | if right.is_some() { 0x40 } else { 0 },
        );
        for id in [origin, right].into_iter().flatten() {
            var(id.0, &mut out);
            var(id.1, &mut out);
        }
        if origin.is_none() && right.is_none() {
            out.push(1);
            string("content", &mut out);
        }
        match text {
            Ok(text) => string(text, &mut out),
            Err(len) => var(len, &mut out),
        }
        var(deletes.len() as u64, &mut out);
        for (client, clock, len) in deletes {
            var(*client, &mut out);
            out.push(1);
            var(*clock, &mut out);
            var(*len, &mut out);
        }
        out
    }

    const BASE: &str = "abcd";

    #[test]
    fn an_exact_copy_agrees() {
        let first = update(0, 0, None, None, Ok(BASE), &[]);
        let edit = update(7, 0, Some((0, 3)), None, Ok("xy"), &[]);
        assert_eq!(disagrees(&first, &[(&first, true)]), Ok(false));
        assert_eq!(disagrees(&edit, &[(&edit, true)]), Ok(false));
        assert_eq!(disagrees(&first, &[(&edit, false)]), Ok(false));
    }

    #[test]
    fn other_text_at_one_element_disagrees() {
        let a = update(7, 0, Some((0, 3)), None, Ok("xy"), &[]);
        let b = update(7, 0, Some((0, 3)), None, Ok("xz"), &[]);
        assert_eq!(disagrees(&b, &[(&a, false)]), Ok(true));
    }

    #[test]
    fn other_origins_disagree() {
        let a = update(7, 0, Some((0, 3)), None, Ok("xy"), &[]);
        let left = update(7, 0, Some((0, 2)), None, Ok("xy"), &[]);
        let right = update(7, 0, Some((0, 3)), Some((0, 1)), Ok("xy"), &[]);
        assert_eq!(disagrees(&left, &[(&a, false)]), Ok(true));
        assert_eq!(disagrees(&right, &[(&a, false)]), Ok(true));
    }

    #[test]
    fn a_deleted_copy_matches_any_content() {
        let a = update(7, 0, Some((0, 3)), None, Ok("xy"), &[]);
        let gone = update(7, 0, Some((0, 3)), None, Err(2), &[]);
        assert_eq!(disagrees(&gone, &[(&a, true)]), Ok(false));
        assert_eq!(disagrees(&a, &[(&gone, true)]), Ok(false));
    }

    #[test]
    fn a_snapshot_must_hold_what_it_covers() {
        let a = update(7, 0, Some((0, 3)), None, Ok("xy"), &[]);
        let short = update(7, 0, Some((0, 3)), None, Ok("x"), &[]);
        let other = update(0, 0, None, None, Ok(BASE), &[]);
        assert_eq!(disagrees(&short, &[(&a, true)]), Ok(true));
        assert_eq!(disagrees(&other, &[(&a, true)]), Ok(true));
        assert_eq!(disagrees(&short, &[(&a, false)]), Ok(false));
    }

    #[test]
    fn a_snapshot_must_show_a_covered_delete() {
        let delete = update(7, 0, Some((0, 3)), None, Ok("x"), &[(0, 1, 2)]);
        let shows_it = update(0, 0, None, None, Ok(BASE), &[]);
        let hides_it = update(0, 0, None, None, Ok(BASE), &[(0, 1, 2)]);
        let half = update(0, 0, None, None, Ok(BASE), &[(0, 1, 1)]);
        assert_eq!(disagrees(&shows_it, &[(&delete, false)]), Ok(false));
        let record = update(0, 0, None, None, Ok(BASE), &[(0, 1, 2)]);
        assert_eq!(disagrees(&shows_it, &[(&record, true)]), Ok(true));
        assert_eq!(disagrees(&hides_it, &[(&record, true)]), Ok(false));
        assert_eq!(disagrees(&half, &[(&record, true)]), Ok(true));
    }

    #[test]
    fn a_replacement_matches_a_surrogate_unit_only_at_a_named_split() {
        let pair = update(7, 0, Some((0, 3)), None, Ok("😀"), &[]);
        let split = update(7, 0, Some((0, 3)), None, Ok("\u{fffd}\u{fffd}"), &[]);
        // Nothing splits the pair.
        assert_eq!(disagrees(&split, &[(&pair, true)]), Ok(true));
        // An insert after the high unit names the gap.
        let insert = update(8, 0, Some((7, 0)), None, Ok("q"), &[]);
        assert_eq!(
            disagrees(&split, &[(&pair, true), (&insert, false)]),
            Ok(false)
        );
        // A right origin on the low unit names it too.
        let before = update(8, 0, Some((0, 3)), Some((7, 1)), Ok("q"), &[]);
        assert_eq!(
            disagrees(&split, &[(&pair, true), (&before, false)]),
            Ok(false)
        );
        // So does the edge of a delete range.
        let delete = update(8, 0, Some((0, 3)), None, Ok("q"), &[(7, 1, 1)]);
        assert_eq!(
            disagrees(&split, &[(&pair, true), (&delete, false)]),
            Ok(false)
        );
        // A range that holds both units doesn't.
        let whole = update(8, 0, Some((0, 3)), None, Ok("q"), &[(7, 0, 2)]);
        assert_eq!(
            disagrees(&split, &[(&pair, true), (&whole, false)]),
            Ok(true)
        );
    }

    #[test]
    fn an_item_cut_at_a_pair_by_the_encoder_alone_splits_nothing() {
        let pair = update(7, 0, Some((0, 3)), None, Ok("😀"), &[]);
        // Two structs whose second follows on from the first.
        let mut cut = vec![1, 2];
        var(7, &mut cut);
        var(0, &mut cut);
        for (clock, origin) in [(0u64, (0u64, 3u64)), (1, (7, 0))] {
            let _ = clock;
            cut.push(0x84);
            var(origin.0, &mut cut);
            var(origin.1, &mut cut);
            string("\u{fffd}", &mut cut);
        }
        cut.push(0);
        assert_eq!(disagrees(&cut, &[(&pair, true)]), Ok(true));
    }

    #[test]
    fn a_covered_record_with_skipped_elements_counts_as_lacking() {
        let a = update(7, 0, Some((0, 3)), None, Ok("xyz"), &[]);
        let later = update(7, 1, Some((7, 0)), None, Ok("yz"), &[]);
        assert_eq!(disagrees(&later, &[(&a, true)]), Ok(true));
        assert_eq!(disagrees(&later, &[(&a, false)]), Ok(false));
    }

    #[test]
    fn the_input_round_trips() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&[9, 9]);
        bytes.push(1);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.push(8);
        let input = Input::parse(&bytes).unwrap();
        assert_eq!(input.subject, [9, 9]);
        assert_eq!(input.others, [(&[8u8][..], true)]);
        bytes.pop();
        assert!(Input::parse(&bytes).is_none());
    }
}
