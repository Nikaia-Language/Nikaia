//! A count a `std` entry takes, converted where it has to be.
//!
//! **The other direction of [ADR-048](../../../docs/specification/adr/adr-048.md)
//! D1.** That record made what a length *returns* an `i64` and left what a
//! parameter *takes* open, saying so; this is the answer
//! ([ADR-054](../../../docs/specification/adr/adr-054.md) D2). The ledger writes
//! such a parameter as the `i64` a program can hold, and the conversion is
//! emitted rather than written - exactly as `index::at` is at an index.
//!
//! **Why it is not `index::at`.** The conversion is the same and the failure is
//! not. A negative index *is* an access out of bounds and reports as one, which
//! is what let D1 emit it without inventing a failure mode. A negative count is
//! no such thing: `"ab".repeat(-1)` is not an access at all, so it needs a
//! message of its own and gets one.
//!
//! **Why a trait.** The emitter does not know types (ADR-011 D2): it sees a
//! method name and an argument. So this is chosen by the type of what is passed,
//! and is the **identity** for anything that is not a signed integer - which is
//! what makes it safe to write around every call of that name, including a
//! method of the program's own that happens to share it.

/// What a value passed as a count becomes.
pub trait Of {
    type Out;
    fn of(self) -> Self::Out;
}

/// The one message, so a negative count reads the same however it arrived.
///
/// `#[track_caller]` all the way out to [`of`], so the panic hook is handed the
/// **caller's** location - the line of the generated file that wrote the call -
/// and not a line of this file. Without it,
/// [ADR-044](../../../docs/specification/adr/adr-044.md) D1's table has nothing
/// to look up and the reader is told about `nikaia_std/src/count.rs`, which is
/// the Part III C.1 defect one crate over.
#[cold]
#[inline(never)]
#[track_caller]
fn negative(count: i64) -> ! {
    panic!("a count cannot be negative: the count is {count}")
}

macro_rules! signed {
    ($($t:ty),*) => {
        $(
            impl Of for $t {
                type Out = usize;
                // Inlined always, and a comparison rather than a conversion,
                // for `index::at`'s reason
                // ([ADR-270](../../../docs/specification/adr/adr-270.md) D8
                // step 1): the count of `vec![v; n]` is then a length the
                // optimiser can carry to every read of the list below.
                #[inline(always)]
                #[track_caller]
                fn of(self) -> usize {
                    if self < 0 {
                        negative(self as i64)
                    }
                    self as usize
                }
            }
        )*
    };
}

signed!(i8, i16, i32, i64, isize);

macro_rules! unsigned {
    ($($t:ty),*) => {
        $(
            impl Of for $t {
                type Out = usize;
                #[inline(always)]
                fn of(self) -> usize {
                    self as usize
                }
            }
        )*
    };
}

unsigned!(u8, u16, u32, u64, usize);

/// **Anything that is not a number goes through untouched**, so a method of the
/// program's own that shares the name is left alone - the same property that lets
/// `index::at` be written around every index.
impl<'a, T: ?Sized> Of for &'a T {
    type Out = &'a T;
    #[inline(always)]
    fn of(self) -> &'a T {
        self
    }
}

/// The emitted spelling: `text.repeat(nikaia_std::count::of(n))`.
///
/// `#[track_caller]`, so a negative count is reported at the line that wrote it -
/// see [`negative`].
#[inline(always)]
#[track_caller]
pub fn of<N: Of>(count: N) -> N::Out {
    count.of()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_count_becomes_a_usize() {
        assert_eq!(of(3_i64), 3_usize);
        assert_eq!(of(0_i32), 0_usize);
        assert_eq!("ab".repeat(of(2_i64)), "abab");
    }

    /// The failure D1 could not reuse: not an access, so not an access out of
    /// bounds.
    #[test]
    fn a_negative_count_says_what_it_is() {
        let panicked = std::panic::catch_unwind(|| of(-1_i64)).expect_err("a count aborts");
        let said = panicked
            .downcast_ref::<String>()
            .map(String::as_str)
            .unwrap_or("");
        assert!(said.contains("a count cannot be negative"), "{said}");
        assert!(said.contains("-1"), "{said}");
    }

    /// Which is what lets the emitter write it around a name it cannot type.
    #[test]
    fn anything_that_is_not_a_number_is_untouched() {
        assert_eq!(of("text"), "text");
    }
}
