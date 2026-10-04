//! A span of time, and the one call that waits out one.
//!
//! [ADR-150](../../../docs/specification/adr/adr-150.md): the type is
//! `std::time::Duration`, it is written `5.seconds()`, and the language learns
//! nothing — no keyword, no suffix literal, no type in the grammar. What is
//! here is a re-export, one extension trait, and `sleep`.

/// **A span of time** ([ADR-150](../../../docs/specification/adr/adr-150.md)
/// D1), which is the language below's own.
///
/// It is re-exported rather than wrapped, because a wrapper would be a second
/// type with the same meaning and nothing to say that the first one does not.
pub use std::time::Duration;

/// **`5.seconds()`** ([ADR-150](../../../docs/specification/adr/adr-150.md)
/// D2), as a method on an integer.
///
/// The spelling was chosen by whoever wrote Part II 12.4, and D3 is why it is
/// not `5s`: a literal has no suffix, and a rule with an exception in it is a
/// rule a reader has to remember rather than one they can apply.
///
/// **Two implementations and not one**, because Part I 2.2 offers two integer
/// types ([ADR-285](../../../docs/specification/adr/adr-285.md)) and `5` is an
/// `i32` where nothing asks otherwise: a program that writes `count.minutes()`
/// on an `i64` and `5.seconds()` on a literal would otherwise have one of the
/// two refused in the language below's words about a file nobody wrote
/// (Part III C.1).
///
/// **A negative count is no time at all.** A duration is unsigned below, and
/// the alternative to saturating at zero is aborting on a subtraction that came
/// out the wrong way — which is not what a program asking to wait wants.
pub trait DurationExt {
    /// This many hours.
    fn hours(self) -> Duration;
    /// This many minutes.
    fn minutes(self) -> Duration;
    /// This many seconds.
    fn seconds(self) -> Duration;
    /// This many thousandths of a second.
    fn millis(self) -> Duration;
    /// This many millionths of a second.
    fn micros(self) -> Duration;
}

macro_rules! spans {
    ($($number:ty),+) => {$(
        impl DurationExt for $number {
            fn hours(self) -> Duration {
                Duration::from_secs(unsigned(self).saturating_mul(3600))
            }
            fn minutes(self) -> Duration {
                Duration::from_secs(unsigned(self).saturating_mul(60))
            }
            fn seconds(self) -> Duration {
                Duration::from_secs(unsigned(self))
            }
            fn millis(self) -> Duration {
                Duration::from_millis(unsigned(self))
            }
            fn micros(self) -> Duration {
                Duration::from_micros(unsigned(self))
            }
        }
    )+};
}

spans!(i32, i64);

/// **`timeout.in_seconds()`** ([ADR-150](../../../docs/specification/adr/adr-150.md)
/// D5): a span read back as a number, in the five units it is made in.
///
/// **Whole units, cut off**: 90 seconds is one minute, as an integer division
/// is. The answer is an `i64`, a count's type
/// ([ADR-285](../../../docs/specification/adr/adr-285.md) D1). **Past
/// `i64::MAX` it stays at `i64::MAX`**, mirroring the constructors: only a
/// span that was itself saturated (`i64::MAX.hours()`) gets there, and *for
/// ever* read back as the largest number is what a timeout computation wants
/// rather than an abort. Rust's own `as_secs` and kin are not in the ledger.
pub trait DurationIn {
    /// How many whole hours.
    fn in_hours(self) -> i64;
    /// How many whole minutes.
    fn in_minutes(self) -> i64;
    /// How many whole seconds.
    fn in_seconds(self) -> i64;
    /// How many whole thousandths of a second.
    fn in_millis(self) -> i64;
    /// How many whole millionths of a second.
    fn in_micros(self) -> i64;
}

impl DurationIn for Duration {
    fn in_hours(self) -> i64 {
        counted(u128::from(self.as_secs() / 3600))
    }
    fn in_minutes(self) -> i64 {
        counted(u128::from(self.as_secs() / 60))
    }
    fn in_seconds(self) -> i64 {
        counted(u128::from(self.as_secs()))
    }
    fn in_millis(self) -> i64 {
        counted(self.as_millis())
    }
    fn in_micros(self) -> i64 {
        counted(self.as_micros())
    }
}

/// A count read back out of a span: past `i64::MAX` it stays there.
fn counted(count: u128) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

/// A count as the language below's durations take it: negative is none.
fn unsigned(count: impl Into<i64>) -> u64 {
    u64::try_from(count.into()).unwrap_or(0)
}

/// **Wait out a span** — Part II 12.4's own call, and a suspension point.
///
/// The thread is given up rather than held: what this arranges is to be polled
/// again at a time rather than at a completion, and the executor's park takes
/// the nearest such time as its bound
/// ([`crate::rt::timer`]). So a program that sleeps
/// runs its other tasks while it does, which is the whole difference between
/// this and `std::thread::sleep`.
pub async fn sleep(span: Duration) {
    if span.is_zero() {
        return;
    }
    Sleeping {
        until: std::time::Instant::now() + span,
    }
    .await
}

/// The future behind [`sleep`].
///
/// **It stores no waker**, for the reason `rt::io`'s own futures store none:
/// the executor is the only thing on this thread that parks, and what wakes
/// this one is the clock rather than anybody's call. What it does instead is
/// tell the executor when to look again.
struct Sleeping {
    until: std::time::Instant,
}

impl std::future::Future for Sleeping {
    type Output = ();

    fn poll(
        self: std::pin::Pin<&mut Self>,
        _context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<()> {
        if std::time::Instant::now() >= self.until {
            return std::task::Poll::Ready(());
        }
        crate::rt::timer::wake_at(self.until);
        std::task::Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_five_names_are_the_spans_they_read_as() {
        assert_eq!(1.seconds(), Duration::from_secs(1));
        assert_eq!(250.millis(), Duration::from_millis(250));
        assert_eq!(500.micros(), Duration::from_micros(500));
        assert_eq!(2.minutes(), Duration::from_secs(120));
        assert_eq!(3.hours(), Duration::from_secs(10_800));
    }

    #[test]
    fn either_integer_type_answers() {
        let wide: i64 = 90;
        assert_eq!(wide.minutes(), Duration::from_secs(5400));
        let narrow: i32 = 90;
        assert_eq!(narrow.minutes(), wide.minutes());
    }

    /// A duration is unsigned below, and the alternative to this is aborting on
    /// a subtraction that came out the wrong way.
    #[test]
    fn a_count_below_zero_is_no_time_at_all() {
        assert_eq!((-1).seconds(), Duration::ZERO);
        assert_eq!((-90i64).hours(), Duration::ZERO);
    }

    /// The largest count a program can write is an `i64`, and hours of it is
    /// more seconds than there are - so the multiplication **stops** rather
    /// than wrapping into a short wait.
    #[test]
    fn a_count_too_large_stops_rather_than_wrapping() {
        assert!(i64::MAX.hours() >= i64::MAX.seconds());
    }

    /// **D5: whole units, cut off**, as an integer division is.
    #[test]
    fn a_span_reads_back_in_whole_units() {
        let span = 90.seconds();
        assert_eq!(span.in_seconds(), 90);
        assert_eq!(span.in_minutes(), 1);
        assert_eq!(span.in_hours(), 0);
        assert_eq!(span.in_millis(), 90_000);
        assert_eq!(span.in_micros(), 90_000_000);
        assert_eq!(1500.micros().in_millis(), 1);
    }

    /// **Past `i64::MAX` it stays there**: only a span that was itself
    /// saturated gets there, and it reads back as the largest number.
    #[test]
    fn a_span_too_long_for_the_count_reads_as_the_largest() {
        let ever = i64::MAX.hours();
        assert_eq!(ever.in_micros(), i64::MAX);
        assert_eq!(ever.in_millis(), i64::MAX);
        assert_eq!(i64::MAX.seconds().in_seconds(), i64::MAX);
    }
}
