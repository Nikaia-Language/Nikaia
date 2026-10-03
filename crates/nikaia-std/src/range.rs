//! A range that is a **value**
//! ([ADR-293](../../../docs/specification/adr/adr-293.md) D18).
//!
//! `0..<n` is two numbers. Walking it does not use it up, so a program may keep
//! one in a name and walk it twice - which the language below's `Range` does
//! not allow: it is the iterator itself, it moves into the first `for`, and the
//! second is *use of moved value* about a file nobody wrote.
//!
//! [`Span`] is the same two numbers, **`Copy`**, and its own iterator: every
//! walk takes a copy and steps that, so the name is where it was. That is the
//! whole of `replays` below (Part I's own word for it is that a range is a
//! value). It also knows its **length** for every integer type, where Rust's
//! `Range` does only up to 32 bits - which is what lets `(0..<n).step_by(2)`
//! and a `zip` over a range be walked from the back with `n` an `i64`, this
//! language's integer.
//!
//! A range written straight into a `for` or into brackets stays Rust's own: it
//! is walked once where it stands, or it is a slice, and neither needs this.

use std::iter::FusedIterator;
use std::ops::{Range, RangeInclusive};

/// `a..<b`, kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Span<T> {
    start: T,
    end: T,
}

/// What `a..<b` lowers to where it is not walked or sliced on the spot.
pub fn span<T>(start: T, end: T) -> Span<T> {
    Span { start, end }
}

/// `a..=b`, kept.
///
/// **A third field, and only here**: an inclusive range that has handed out its
/// last value cannot say so with its two ends - `3..=3` is one value, not none -
/// which is why the language below's `RangeInclusive` carries the same flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Through<T> {
    start: T,
    end: T,
    done: bool,
}

/// What `a..=b` lowers to where it is not walked or sliced on the spot.
pub fn through<T>(start: T, end: T) -> Through<T> {
    Through {
        start,
        end,
        done: false,
    }
}

impl<T: Copy> Iterator for Span<T>
where
    Range<T>: Iterator<Item = T>,
{
    type Item = T;

    fn next(&mut self) -> Option<T> {
        let mut rest = self.start..self.end;
        let next = rest.next();
        self.start = rest.start;
        next
    }

    fn nth(&mut self, n: usize) -> Option<T> {
        let mut rest = self.start..self.end;
        let nth = rest.nth(n);
        self.start = rest.start;
        nth
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.start..self.end).size_hint()
    }
}

impl<T: Copy> DoubleEndedIterator for Span<T>
where
    Range<T>: DoubleEndedIterator<Item = T>,
{
    fn next_back(&mut self) -> Option<T> {
        let mut rest = self.start..self.end;
        let last = rest.next_back();
        self.end = rest.end;
        last
    }
}

impl<T: Copy> FusedIterator for Span<T> where Range<T>: Iterator<Item = T> {}

impl<T: Copy + PartialOrd> Iterator for Through<T>
where
    RangeInclusive<T>: Iterator<Item = T>,
{
    type Item = T;

    fn next(&mut self) -> Option<T> {
        if self.done {
            return None;
        }
        let mut rest = self.start..=self.end;
        let next = rest.next();
        match rest.is_empty() {
            true => self.done = true,
            false => self.start = *rest.start(),
        }
        next
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self.done {
            true => (0, Some(0)),
            false => (self.start..=self.end).size_hint(),
        }
    }
}

impl<T: Copy + PartialOrd> DoubleEndedIterator for Through<T>
where
    RangeInclusive<T>: DoubleEndedIterator<Item = T>,
{
    fn next_back(&mut self) -> Option<T> {
        if self.done {
            return None;
        }
        let mut rest = self.start..=self.end;
        let last = rest.next_back();
        match rest.is_empty() {
            true => self.done = true,
            false => self.end = *rest.end(),
        }
        last
    }
}

impl<T: Copy + PartialOrd> FusedIterator for Through<T> where RangeInclusive<T>: Iterator<Item = T> {}

/// **The length, for every integer type** (ADR-293 D18).
///
/// Rust's `Range<i64>` has no `ExactSizeIterator`, because on a 32-bit target
/// its length may not fit a `usize`; its `size_hint` is still exact wherever
/// the length does fit, which is every range a program walks. The declaration
/// is what `step_by`, `zip`, `take` and `skip` ask for before they walk
/// from the back.
macro_rules! sized {
    ($($t:ty),*) => {
        $(
            impl ExactSizeIterator for Span<$t> {}
            impl ExactSizeIterator for Through<$t> {}
        )*
    };
}

sized!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

/// **A kept range slices as a written one does** (ADR-293 D18): `t[r]` with `r`
/// a name is the same run of `t` as `t[1..<3]`, and goes through the same
/// conversion to `usize` - so a negative end is the same abort, at the line
/// that wrote it.
macro_rules! slices {
    ($($t:ty),*) => {
        $(
            impl crate::index::At for Span<$t> {
                type Out = Range<usize>;
                #[track_caller]
                fn at(self) -> Range<usize> {
                    crate::index::at(self.start..self.end)
                }
            }
        )*
    };
}

slices!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

/// The inclusive half, for the signed types the written form converts.
macro_rules! slices_through {
    ($($t:ty),*) => {
        $(
            impl crate::index::At for Through<$t> {
                type Out = RangeInclusive<usize>;
                #[track_caller]
                fn at(self) -> RangeInclusive<usize> {
                    crate::index::at(self.start..=self.end)
                }
            }
        )*
    };
}

slices_through!(i8, i16, i32, i64, isize);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kept_range_is_walked_twice() {
        let r = span(0i64, 3);
        let first: Vec<i64> = r.collect();
        let second: Vec<i64> = r.collect();
        assert_eq!(first, vec![0, 1, 2]);
        assert_eq!(first, second);
    }

    #[test]
    fn from_the_back_and_with_a_step() {
        let r = span(0i64, 10);
        assert_eq!(
            r.rev().collect::<Vec<_>>(),
            (0..10).rev().collect::<Vec<i64>>()
        );
        assert_eq!(
            r.step_by(3).rev().collect::<Vec<_>>(),
            vec![9, 6, 3, 0],
            "`step_by` walked from the back needs the length, and an `i64` range has one"
        );
        assert_eq!(r.len(), 10);
        assert_eq!(r.zip(r.rev()).next_back(), Some((9, 0)));
    }

    #[test]
    fn an_inclusive_range_ends_on_its_last_value() {
        let r = through(1i64, 3);
        assert_eq!(r.collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(r.rev().collect::<Vec<_>>(), vec![3, 2, 1]);
        assert_eq!(through(3i64, 3).collect::<Vec<_>>(), vec![3]);
        assert_eq!(through(4i64, 3).count(), 0);
        let mut both = through(1i64, 4);
        assert_eq!(
            (
                both.next(),
                both.next_back(),
                both.next(),
                both.next_back(),
                both.next()
            ),
            (Some(1), Some(4), Some(2), Some(3), None)
        );
        assert_eq!(r.len(), 3);
    }

    #[test]
    fn a_kept_range_slices_as_a_written_one_does() {
        let text = "abcdef";
        assert_eq!(&text[crate::index::at(span(1i64, 3))], "bc");
        let xs = [1, 2, 3, 4];
        assert_eq!(&xs[crate::index::at(through(1i64, 2))], &[2, 3]);
    }

    #[test]
    fn an_empty_range_is_empty_from_both_ends() {
        let r = span(5i64, 2);
        assert_eq!(r.count(), 0);
        assert_eq!(r.rev().count(), 0);
        assert_eq!(r.len(), 0);
    }
}
