//! Board order keys. A card's place in its column is its task's
//! `kanban_order`, compared byte by byte, or its stable ID when the file
//! holds none, so a task another app or the legacy app wrote sorts where
//! it always did. A move writes one key between the keys of the card's new
//! neighbors, and no other task changes.
//!
//! Keys are fractional, as in the legacy Tasks plugin: a key fits between
//! any two others by growing longer. New keys use the 62 digits and
//! letters, in their byte order, but a neighbor's key may hold anything,
//! such as the legacy board's `a0` or a stable ID.

/// The digits of new keys, in byte order.
const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// The smallest digit. No new key ends in it, so another key always fits
/// before each new one.
const LOWEST: u8 = b'0';

/// The largest digit.
const HIGHEST: u8 = b'z';

/// The digit a key takes when any would do.
const MIDDLE: u8 = b'V';

/// A key that sorts after `lower` and before `upper`, either of which may
/// be missing at a column's ends. When nothing fits between them, as when
/// they are equal or out of order, the key sorts just after `lower`.
pub(crate) fn between(lower: Option<&str>, upper: Option<&str>) -> String {
    let lower = lower.unwrap_or_default();
    let key = match upper {
        Some(upper) if lower < upper => midpoint(lower.as_bytes(), upper.as_bytes()),
        _ => None,
    };
    let key = key.unwrap_or_else(|| after(lower.as_bytes(), 0));
    // Each key keeps whole characters of its neighbors and adds ASCII, so
    // it is always text; a key that weren't would sort after `lower`.
    String::from_utf8(key).unwrap_or_else(|_| format!("{lower}{}", MIDDLE as char))
}

/// The digit in the middle of those between `low` and `high`, both left
/// out, leaving out the lowest digit too.
fn pick(low: Option<u8>, high: Option<u8>) -> Option<u8> {
    let fits: Vec<u8> = DIGITS
        .iter()
        .copied()
        .filter(|digit| *digit != LOWEST)
        .filter(|digit| low.is_none_or(|low| *digit > low))
        .filter(|digit| high.is_none_or(|high| *digit < high))
        .collect();
    fits.get(fits.len() / 2).copied()
}

/// A key after `key` that keeps its first `from` bytes: the first byte
/// from there that a digit can follow becomes that digit, or a digit goes
/// on the end.
fn after(key: &[u8], from: usize) -> Vec<u8> {
    for (at, byte) in key.iter().enumerate().skip(from) {
        if *byte < HIGHEST
            && let Some(digit) = pick(Some(*byte), None)
        {
            let mut next = key[..at].to_vec();
            next.push(digit);
            return next;
        }
    }
    let mut next = key.to_vec();
    next.push(MIDDLE);
    next
}

/// A key strictly between `lower` and `upper`, which sorts after it, or
/// `None` when none exists, as between `` and `\0`.
fn midpoint(lower: &[u8], upper: &[u8]) -> Option<Vec<u8>> {
    let shared = lower
        .iter()
        .zip(upper)
        .take_while(|(low, high)| low == high)
        .count();
    // `upper` sorts after `lower`, so it goes on past what they share.
    let high = *upper.get(shared)?;
    let low = lower.get(shared).copied();
    if let Some(digit) = pick(low, Some(high)) {
        let mut key = upper[..shared].to_vec();
        key.push(digit);
        return Some(key);
    }
    if low.is_some() {
        // Keep `lower`'s byte there, which sorts before `upper`'s, and
        // go past the rest of `lower`.
        return Some(after(lower, shared + 1));
    }
    // `lower` ends where they part, and no digit but the lowest sorts
    // before `upper`'s byte there: take the byte below it, or follow a NUL
    // with a key before the rest of `upper`.
    let mut key = upper[..shared].to_vec();
    if high > LOWEST {
        key.extend([LOWEST, MIDDLE]);
        return Some(key);
    }
    if high > 0 {
        key.extend([high - 1, MIDDLE]);
        return Some(key);
    }
    key.push(high);
    key.extend(midpoint(b"", &upper[shared + 1..])?);
    Some(key)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::between;

    fn fits(lower: Option<&str>, upper: Option<&str>) -> String {
        let key = between(lower, upper);
        if let Some(lower) = lower {
            assert!(lower < key.as_str(), "{key:?} isn't after {lower:?}");
        }
        if let Some(upper) = upper {
            assert!(key.as_str() < upper, "{key:?} isn't before {upper:?}");
        }
        key
    }

    #[test]
    fn a_key_fits_between_the_legacy_boards_keys() {
        // The legacy board's fractional-indexing keys.
        let key = fits(Some("a0"), Some("a1"));
        assert!(key.starts_with("a0"), "{key}");
        fits(Some("a1"), None);
        fits(None, Some("a0"));
        fits(Some("Zz"), Some("a0"));
    }

    #[test]
    fn a_key_fits_beside_a_task_that_sorts_by_its_stable_id() {
        let first = "task_01jbcdefghjkmnpqrstvwxyz0a";
        let second = "task_01jbcdefghjkmnpqrstvwxyz0b";
        fits(Some(first), Some(second));
        fits(Some("a0"), Some(first));
        fits(Some(second), None);
    }

    #[test]
    fn keys_stay_short_in_an_empty_column_and_at_its_ends() {
        assert_eq!(between(None, None), "V");
        let mut last = between(None, None);
        for _ in 0..20 {
            last = fits(Some(&last), None);
        }
        assert!(last.len() <= 6, "{last}");
        let mut first = between(None, None);
        for _ in 0..20 {
            first = fits(None, Some(&first));
        }
        assert!(first.len() <= 6, "{first}");
    }

    #[test]
    fn a_key_after_its_neighbor_when_nothing_fits_between() {
        assert!(between(Some("a0"), Some("a0")).as_str() > "a0");
        assert!(between(Some("b"), Some("a")).as_str() > "b");
        // Nothing sorts after the empty key and before a NUL.
        assert!(between(Some(""), Some("\0")).as_str() > "");
    }

    proptest! {
        #[test]
        fn a_key_fits_between_any_two_keys(mut keys in proptest::collection::vec(".{0,6}", 2)) {
            keys.sort();
            let (lower, upper) = (&keys[0], &keys[1]);
            let key = between(Some(lower), Some(upper));
            prop_assert!(lower.as_str() < key.as_str());
            // Nothing fits when `upper` is `lower` with NULs after it.
            let room = upper
                .strip_prefix(lower.as_str())
                .is_none_or(|rest| rest.bytes().any(|byte| byte != 0));
            if lower < upper && room {
                prop_assert!(key.as_str() < upper.as_str(), "{:?} < {:?} < {:?}", lower, key, upper);
            }
        }

        #[test]
        fn repeated_moves_between_two_cards_keep_fitting(steps in proptest::collection::vec(any::<bool>(), 1..60)) {
            let (mut lower, mut upper) = ("a0".to_owned(), "a1".to_owned());
            for toward_upper in steps {
                let key = between(Some(&lower), Some(&upper));
                prop_assert!(lower < key && key < upper);
                if toward_upper { lower = key } else { upper = key }
            }
        }
    }
}
