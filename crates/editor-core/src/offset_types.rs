//! Positions and counts in the editor's text, as distinct types so a byte
//! offset can't be passed where a grapheme offset belongs.
//!
//! [`Byte`] and [`Grapheme`] are positions; [`Bytes`] and [`Graphemes`] are
//! counts. A position plus a count is a position, and a position minus a
//! position is a count. Ranges are `(start, end)` tuples, extended by
//! [`RangeExt`].
//!
//! Adapted from Lockbook's `lb-rs` (`libs/lb/lb-rs/src/model/text/offset_types.rs`,
//! BSD-3-Clause); see `docs/provenance/lockbook-editor.md`.

use std::cmp::{max, min};
use std::fmt;
use std::ops::{Add, AddAssign, Sub, SubAssign};

/// A byte offset into UTF-8 text.
#[repr(transparent)]
#[derive(Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Byte(pub usize);

/// A number of bytes.
#[repr(transparent)]
#[derive(Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Bytes(pub usize);

/// An offset counted in extended grapheme clusters: the cursor positions in
/// the text.
#[repr(transparent)]
#[derive(Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Grapheme(pub usize);

/// A number of extended grapheme clusters.
#[repr(transparent)]
#[derive(Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Graphemes(pub usize);

/// Arithmetic and comparisons shared by every unit: plain `usize` interop
/// in both directions, and `Debug` that prints the bare number.
macro_rules! unit {
    ($unit:ident) => {
        impl From<usize> for $unit {
            fn from(value: usize) -> Self {
                Self(value)
            }
        }

        impl PartialEq<usize> for $unit {
            fn eq(&self, other: &usize) -> bool {
                self.0 == *other
            }
        }

        impl PartialOrd<usize> for $unit {
            fn partial_cmp(&self, other: &usize) -> Option<std::cmp::Ordering> {
                self.0.partial_cmp(other)
            }
        }

        impl PartialEq<$unit> for usize {
            fn eq(&self, other: &$unit) -> bool {
                *self == other.0
            }
        }

        impl PartialOrd<$unit> for usize {
            fn partial_cmp(&self, other: &$unit) -> Option<std::cmp::Ordering> {
                self.partial_cmp(&other.0)
            }
        }

        impl Add<usize> for $unit {
            type Output = Self;
            fn add(self, rhs: usize) -> Self {
                Self(self.0 + rhs)
            }
        }

        impl Sub<usize> for $unit {
            type Output = Self;
            fn sub(self, rhs: usize) -> Self {
                Self(self.0 - rhs)
            }
        }

        impl AddAssign<usize> for $unit {
            fn add_assign(&mut self, rhs: usize) {
                self.0 += rhs;
            }
        }

        impl SubAssign<usize> for $unit {
            fn sub_assign(&mut self, rhs: usize) {
                self.0 -= rhs;
            }
        }

        impl fmt::Debug for $unit {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Debug::fmt(&self.0, f)
            }
        }
    };
}

unit!(Byte);
unit!(Bytes);
unit!(Grapheme);
unit!(Graphemes);

/// Position and count arithmetic between a position unit and its count unit.
macro_rules! position {
    ($pos:ident, $count:ident) => {
        impl Add<$count> for $pos {
            type Output = $pos;
            fn add(self, rhs: $count) -> $pos {
                $pos(self.0 + rhs.0)
            }
        }

        impl Sub<$count> for $pos {
            type Output = $pos;
            fn sub(self, rhs: $count) -> $pos {
                $pos(self.0 - rhs.0)
            }
        }

        impl AddAssign<$count> for $pos {
            fn add_assign(&mut self, rhs: $count) {
                self.0 += rhs.0;
            }
        }

        impl SubAssign<$count> for $pos {
            fn sub_assign(&mut self, rhs: $count) {
                self.0 -= rhs.0;
            }
        }

        impl Sub<$pos> for $pos {
            type Output = $count;
            fn sub(self, rhs: $pos) -> $count {
                $count(self.0 - rhs.0)
            }
        }

        impl Add<$count> for $count {
            type Output = $count;
            fn add(self, rhs: $count) -> $count {
                $count(self.0 + rhs.0)
            }
        }

        impl Sub<$count> for $count {
            type Output = $count;
            fn sub(self, rhs: $count) -> $count {
                $count(self.0 - rhs.0)
            }
        }

        impl AddAssign<$count> for $count {
            fn add_assign(&mut self, rhs: $count) {
                self.0 += rhs.0;
            }
        }

        impl SubAssign<$count> for $count {
            fn sub_assign(&mut self, rhs: $count) {
                self.0 -= rhs.0;
            }
        }
    };
}

position!(Byte, Bytes);
position!(Grapheme, Graphemes);

/// Helpers for `(start, end)` ranges. The two ends may come in either order;
/// [`RangeExt::start`] and [`RangeExt::end`] sort them.
pub trait RangeExt: Sized {
    type Element: Copy + Sub<Self::Element> + Ord;

    /// Whether `value` lies in the range, with each end included or not.
    fn contains(&self, value: Self::Element, start_inclusive: bool, end_inclusive: bool) -> bool;

    /// Whether the ranges overlap. Ranges that only touch overlap when
    /// `allow_empty_intersection` is set.
    fn intersects(
        &self,
        other: &(Self::Element, Self::Element),
        allow_empty_intersection: bool,
    ) -> bool;

    fn start(&self) -> Self::Element;
    fn end(&self) -> Self::Element;
    fn len(&self) -> <Self::Element as Sub>::Output;
    fn is_empty(&self) -> bool;

    /// Whether both ends of `value` lie in the range.
    fn contains_range(
        &self,
        value: &(Self::Element, Self::Element),
        start_inclusive: bool,
        end_inclusive: bool,
    ) -> bool {
        self.contains(value.0, start_inclusive, end_inclusive)
            && self.contains(value.1, start_inclusive, end_inclusive)
    }

    fn contains_inclusive(&self, value: Self::Element) -> bool {
        self.contains(value, true, true)
    }

    fn intersects_allow_empty(&self, other: &(Self::Element, Self::Element)) -> bool {
        self.intersects(other, true)
    }

    /// This range clamped to `bounds`.
    fn trim(&self, bounds: &(Self::Element, Self::Element)) -> (Self::Element, Self::Element) {
        let clamp = |value: Self::Element| value.max(bounds.0).min(bounds.1);
        (clamp(self.start()), clamp(self.end()))
    }
}

impl<T> RangeExt for (T, T)
where
    T: Ord + Copy + Sub<T>,
{
    type Element = T;

    fn contains(&self, value: T, start_inclusive: bool, end_inclusive: bool) -> bool {
        let after_start = if start_inclusive {
            self.start() <= value
        } else {
            self.start() < value
        };
        let before_end = if end_inclusive {
            value <= self.end()
        } else {
            value < self.end()
        };
        after_start && before_end
    }

    fn intersects(&self, other: &(T, T), allow_empty_intersection: bool) -> bool {
        let starts_before_other_ends = if allow_empty_intersection {
            self.start() <= other.end()
        } else {
            self.start() < other.end()
        };
        let other_starts_before_end = if allow_empty_intersection {
            other.start() <= self.end()
        } else {
            other.start() < self.end()
        };
        starts_before_other_ends && other_starts_before_end
    }

    fn start(&self) -> T {
        min(self.0, self.1)
    }

    fn end(&self) -> T {
        max(self.0, self.1)
    }

    fn len(&self) -> <T as Sub>::Output {
        self.end() - self.start()
    }

    fn is_empty(&self) -> bool {
        self.0 == self.1
    }
}

/// An empty range at a position.
pub trait ToRangeExt: Sized {
    fn to_range(self) -> (Self, Self);
}

impl<T: Copy> ToRangeExt for T {
    fn to_range(self) -> (Self, Self) {
        (self, self)
    }
}

/// An empty range at a position, converting the position's type.
pub trait IntoRangeExt<I> {
    fn into_range(self) -> (I, I);
}

impl<T, I> IntoRangeExt<I> for T
where
    T: Copy + Into<I>,
{
    fn into_range(self) -> (I, I) {
        (self.into(), self.into())
    }
}

/// Iterates the positions in a `(start, end)` range, end excluded.
pub trait RangeIterExt {
    type Item;
    type Iter: DoubleEndedIterator<Item = Self::Item>;
    fn iter(self) -> Self::Iter;
}

impl<T> RangeIterExt for (T, T)
where
    T: Copy + PartialOrd + AddAssign<usize> + SubAssign<usize>,
{
    type Item = T;
    type Iter = RangeIter<T>;

    fn iter(self) -> RangeIter<T> {
        RangeIter {
            next: self.0,
            end: self.1,
        }
    }
}

pub struct RangeIter<T> {
    next: T,
    end: T,
}

impl<T> Iterator for RangeIter<T>
where
    T: Copy + PartialOrd + AddAssign<usize>,
{
    type Item = T;

    fn next(&mut self) -> Option<T> {
        (self.next < self.end).then(|| {
            let item = self.next;
            self.next += 1;
            item
        })
    }
}

impl<T> DoubleEndedIterator for RangeIter<T>
where
    T: Copy + PartialOrd + AddAssign<usize> + SubAssign<usize>,
{
    fn next_back(&mut self) -> Option<T> {
        (self.next < self.end).then(|| {
            self.end -= 1;
            self.end
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_sort_their_ends() {
        let range = (Grapheme(5), Grapheme(2));
        assert_eq!(range.start(), Grapheme(2));
        assert_eq!(range.end(), Grapheme(5));
        assert_eq!(range.len(), Graphemes(3));
    }

    #[test]
    fn touching_ranges_intersect_only_when_empty_intersections_count() {
        let a = (Grapheme(0), Grapheme(3));
        let b = (Grapheme(3), Grapheme(6));
        assert!(!a.intersects(&b, false));
        assert!(a.intersects(&b, true));
        assert!(a.intersects(&(Grapheme(2), Grapheme(4)), false));
    }

    #[test]
    fn contains_honors_each_end() {
        let range = (Byte(2), Byte(4));
        assert!(range.contains(Byte(2), true, false));
        assert!(!range.contains(Byte(2), false, false));
        assert!(range.contains(Byte(4), false, true));
        assert!(!range.contains(Byte(4), true, false));
    }

    // `contains`, `contains_empty` and `intersects_allow_empty_contained`
    // are ported from `lb-rs`'s offset tests.
    #[test]
    fn contains() {
        for (offset, expected) in [(0, false), (1, false), (2, true), (3, false), (4, false)] {
            assert_eq!((1, 3).contains(offset, false, false), expected, "{offset}");
        }
        for (offset, expected) in [(0, false), (1, true), (2, true), (3, false), (4, false)] {
            assert_eq!((1, 3).contains(offset, true, false), expected, "{offset}");
        }
        for (offset, expected) in [(0, false), (1, false), (2, true), (3, true), (4, false)] {
            assert_eq!((1, 3).contains(offset, false, true), expected, "{offset}");
        }
        for (offset, expected) in [(0, false), (1, true), (2, true), (3, true), (4, false)] {
            assert_eq!((1, 3).contains(offset, true, true), expected, "{offset}");
        }
    }

    #[test]
    fn contains_empty() {
        for (start_inclusive, end_inclusive) in [(false, false), (true, false), (false, true)] {
            for offset in 0..3 {
                assert!(!(1, 1).contains(offset, start_inclusive, end_inclusive));
            }
        }
        assert!(!(1, 1).contains(0, true, true));
        assert!((1, 1).contains(1, true, true));
        assert!(!(1, 1).contains(2, true, true));
    }

    #[test]
    fn intersects_allow_empty_contained() {
        assert!((1, 3).intersects(&(2, 2), false));
    }

    #[test]
    fn trim_clamps_to_bounds() {
        let range = (Grapheme(1), Grapheme(9));
        assert_eq!(
            range.trim(&(Grapheme(3), Grapheme(5))),
            (Grapheme(3), Grapheme(5))
        );
        assert_eq!(
            (Grapheme(7), Grapheme(9)).trim(&(Grapheme(3), Grapheme(5))),
            (Grapheme(5), Grapheme(5))
        );
    }

    #[test]
    fn range_iterates_both_ways() {
        let forward: Vec<_> = (Grapheme(1), Grapheme(4)).iter().collect();
        assert_eq!(forward, vec![Grapheme(1), Grapheme(2), Grapheme(3)]);
        let backward: Vec<_> = (Grapheme(1), Grapheme(4)).iter().rev().collect();
        assert_eq!(backward, vec![Grapheme(3), Grapheme(2), Grapheme(1)]);
    }
}
