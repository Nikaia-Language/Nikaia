//! Converting a floating-point number to an integer, checked.
//!
//! [ADR-285](../../../docs/specification/adr/adr-285.md) D12 and D13. A narrowing
//! conversion aborts, and between two integers Rust gives the check:
//! `i32::try_from` hands back a failure value. Out of a floating-point number it
//! does not — `the trait bound i32: TryFrom<f64> is not satisfied` — so the test
//! lives here.
//!
//! **What Rust's `as` does instead, measured.** It is silent in three different
//! ways: `1e20 as i32` is `2147483647`, `-1e20 as i32` is `-2147483648`, and
//! `f64::NAN as i32` is `0`. A program that meant any of those said nothing about
//! it, which is what this removes.
//!
//! These are the one place ADR-285 puts a helper in the emitted code, and D4 says
//! why the objection that refused one for arithmetic does not reach here: a
//! conversion is rare and written down on purpose rather than standing in every
//! loop, and a checked conversion is what a Rust programmer would write at this
//! spot anyway.

use std::any::type_name;

/// A `f64` as an integer, or an abort. The one check, written once.
///
/// **The range test goes through `i128` rather than through the target's limits
/// as `f64`.** The obvious spelling — `value >= T::MIN as f64 && value <=
/// T::MAX as f64` — is wrong for every 64-bit target, because `i64::MAX as f64`
/// rounds **up**: it accepts a value one past what the type holds and then lets
/// `as` clamp it. Written against the target's limits it needs a hand-computed
/// power of two per type, and for a type whose width the machine decides
/// (D7's second neighbour) the constant would have to differ per machine.
///
/// Through `i128` there is nothing to compute. `f64 as i128` saturates at
/// ±2^127, which is outside every integer type this can target, so a saturated
/// value cannot land back inside the target's range and be let through - and
/// `try_from` then answers exactly, at whatever width the machine gives the type.
///
/// **"Not a number" is asked first**, because it is the one case the range test
/// cannot catch: `f64::NAN as i128` is `0`, which fits everything.
///
/// `.trunc()` is what `as` means - toward zero, not toward the nearest.
#[inline]
#[track_caller]
fn to_int<T: TryFrom<i128>>(value: f64) -> T {
    if value.is_nan() {
        panic!(
            "the value is not a number, so it is no `{}`",
            type_name::<T>()
        );
    }
    // `match` and not `unwrap_or_else`: a closure is not `#[track_caller]`, so
    // the abort inside one names this file - measured, `num.rs:48` - and
    // [ADR-300](../../../docs/specification/adr/adr-300.md)'s table has no Nikaia
    // line to translate that to. A `panic!` in the body of a `#[track_caller]`
    // function carries the caller's location instead.
    match T::try_from(value.trunc() as i128) {
        Ok(fits) => fits,
        Err(_) => panic!(
            "the value does not fit in an `{}`: {value}",
            type_name::<T>()
        ),
    }
}

/// A `f64` as an `i32`, or an abort.
///
/// `#[track_caller]`, so the abort names the line the conversion was written on
/// rather than this file. Without it [ADR-300](../../../docs/specification/adr/adr-300.md)'s
/// table would have nothing to translate: the location would be in `std`, where no
/// Nikaia line maps.
#[inline]
#[track_caller]
pub fn to_i32(value: f64) -> i32 {
    to_int(value)
}

/// A `f64` as an `i64`, or an abort. `#[track_caller]`, for the reason
/// [`to_i32`] gives.
#[inline]
#[track_caller]
pub fn to_i64(value: f64) -> i64 {
    to_int(value)
}

/// A `f64` as a `usize`, or an abort.
///
/// This is D7's second neighbour - the width is the machine's, and what fits on a
/// large machine does not on a small one. There is no constant here to get wrong
/// per machine, which is [`to_int`]'s reason for going through `i128`.
#[inline]
#[track_caller]
pub fn to_usize(value: f64) -> usize {
    to_int(value)
}

/// A `f64` as an `isize`, or an abort. [`to_usize`]'s reason, signed.
#[inline]
#[track_caller]
pub fn to_isize(value: f64) -> isize {
    to_int(value)
}

/// **What a cast's operand is, where it arrived as a view**
/// ([ADR-182](../../../docs/specification/adr/adr-182.md) D1).
///
/// A `for` lends ([ADR-094](../../../docs/specification/adr/adr-094.md) D4), so
/// `for n in NS` binds a **view** of each element — which is right, and is what
/// lets the loop read without copying. Rust's `as` does not see through one:
/// `casting &i32 as i64 is invalid`, about a noun the program does not contain,
/// which is [Part III C.1](../../../docs/specification/30-nikaia-tooling.md)'s
/// class.
///
/// **Why a trait and not a `*`.** The same reason [`crate::index::At`] is one:
/// a `*` is right for the binding and wrong for a name that shadows it one line
/// down, and this emitter has no types to tell the two apart with
/// ([ADR-288](../../../docs/specification/adr/adr-288.md)). Choosing on the
/// type is what a trait is, and for a number that is already a number this is
/// the **identity** — so the rule can be applied wherever the name could be a
/// lent binding without ever being applied to the wrong operand.
pub trait Value {
    /// The number itself, however many views it arrived behind.
    type Out;
    fn value(self) -> Self::Out;
}

macro_rules! itself {
    ($($t:ty),*) => {
        $(
            impl Value for $t {
                type Out = $t;
                #[inline]
                fn value(self) -> $t {
                    self
                }
            }
        )*
    };
}

itself!(
    i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize, f32, f64, bool, char
);

/// **A view of one answers what it points at**, through any number of them:
/// `&&i32` is what a lent binding over a container of views is.
///
/// No overlap with the impls above, because `&T` is not any of them - which is
/// the same shape [`crate::index::At`]'s own `&T` impl has.
impl<T: Value + Copy> Value for &T {
    type Out = T::Out;
    #[inline]
    fn value(self) -> T::Out {
        (*self).value()
    }
}

/// The emitted spelling: `nikaia_std::num::value(n) as i64`.
#[inline]
pub fn value<T: Value>(operand: T) -> T::Out {
    operand.value()
}

/// **A `u64` and a signed integer compared as numbers**
/// ([ADR-285](../../../docs/specification/adr/adr-285.md) D32): they have no
/// common type, so the sign is tested first and the compare is then made in
/// `u64`. The emitted spelling is `nikaia_std::num::unsigned_with_signed(a,
/// b).is_lt()`, the sides in the order the program wrote them.
#[inline]
pub fn unsigned_with_signed(unsigned: u64, signed: i64) -> std::cmp::Ordering {
    match u64::try_from(signed) {
        Ok(signed) => unsigned.cmp(&signed),
        Err(_) => std::cmp::Ordering::Greater,
    }
}

/// The same with the signed side written first.
#[inline]
pub fn signed_with_unsigned(signed: i64, unsigned: u64) -> std::cmp::Ordering {
    unsigned_with_signed(unsigned, signed).reverse()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Every `u64` is above every negative number**, and the two compare
    /// as numbers where both are in range.
    #[test]
    fn a_u64_and_an_i64_compare_as_numbers() {
        use std::cmp::Ordering::*;
        assert_eq!(unsigned_with_signed(0, -1), Greater);
        assert_eq!(unsigned_with_signed(u64::MAX, i64::MAX), Greater);
        assert_eq!(unsigned_with_signed(7, 7), Equal);
        assert_eq!(unsigned_with_signed(6, 7), Less);
        assert_eq!(signed_with_unsigned(-1, 0), Less);
        assert_eq!(signed_with_unsigned(i64::MIN, 0), Less);
        assert_eq!(signed_with_unsigned(i64::MAX, u64::MAX), Less);
        assert_eq!(signed_with_unsigned(7, 7), Equal);
    }

    /// **A cast's operand reaches the number through any number of views**,
    /// which is what `for n in NS { n as i64 }` needs: the binding is a `&i32`
    /// and `as` does not see through one.
    #[test]
    fn a_view_of_a_number_answers_the_number() {
        let n: i32 = 7;
        assert_eq!(value(n), 7i32);
        assert_eq!(value(&n), 7i32);
        assert_eq!(value(&&n), 7i32);
        assert_eq!(value(&1.5f64) as i64, 1);
        assert_eq!(value(&'a') as u32, 97);
        assert!(value(&true));
    }

    #[test]
    fn a_value_that_fits_comes_through() {
        assert_eq!(to_i32(42.9), 42);
        assert_eq!(to_i32(-42.9), -42);
        assert_eq!(to_i64(1e18), 1_000_000_000_000_000_000);
        assert_eq!(to_i32(i32::MAX as f64), i32::MAX);
        assert_eq!(to_i32(i32::MIN as f64), i32::MIN);
        assert_eq!(to_usize(7.5), 7);
        assert_eq!(to_isize(-7.5), -7);
    }

    /// The three silent cases, each one an abort now.
    #[test]
    fn the_three_that_were_silent_abort() {
        for value in [1e20_f64, -1e20_f64, f64::NAN] {
            assert!(
                std::panic::catch_unwind(|| to_i32(value)).is_err(),
                "{value} must not pass as an `i32`"
            );
        }
        for value in [1e30_f64, -1e30_f64, f64::NAN, f64::INFINITY] {
            assert!(
                std::panic::catch_unwind(|| to_i64(value)).is_err(),
                "{value} must not pass as an `i64`"
            );
        }
    }

    /// `i64::MAX as f64` rounds up, so the value exactly at the rounded limit is
    /// one an `i64` cannot hold and must be refused. This is the case the
    /// `i128` detour exists for.
    #[test]
    fn the_rounded_upper_limit_of_an_i64_is_refused() {
        assert!(std::panic::catch_unwind(|| to_i64(i64::MAX as f64)).is_err());
        assert_eq!(
            to_i64((i64::MAX as f64).next_down()),
            9_223_372_036_854_774_784
        );
    }

    /// A negative number is no `usize` - a bound the machine decides on the
    /// other side, asked here without a constant.
    ///
    /// `-0.5` is not among them: truncating toward zero *is* zero, and a `usize`
    /// holds that. What the check refuses is a value the type cannot hold, not a
    /// value that had a fraction.
    #[test]
    fn a_negative_number_is_no_usize() {
        assert!(std::panic::catch_unwind(|| to_usize(-1.0)).is_err());
        assert!(std::panic::catch_unwind(|| to_usize(1e30)).is_err());
        assert_eq!(to_usize(-0.5), 0);
        assert_eq!(to_usize(0.0), 0);
    }

    /// The message names the type in this language's words and carries the value,
    /// because the line alone does not say which of two conversions on it failed.
    #[test]
    fn the_message_names_the_type_and_the_value() {
        let failed = std::panic::catch_unwind(|| to_i32(1e20)).expect_err("it aborts");
        let said = failed
            .downcast_ref::<String>()
            .expect("the message is a String");
        assert!(
            said.contains("`i32`") && said.contains("100000000000000000000"),
            "the message must name the type and the value: {said}"
        );
    }
}
