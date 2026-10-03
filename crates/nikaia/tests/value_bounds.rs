//! **A bound on a list's values, and `--optimization=remove-overflow-checks`**
//! ([ADR-306](../../../docs/specification/adr/adr-306.md)).
//!
//! Every program here is lowered with both options off and both at
//! `aggressive`, compiled with overflow checks on and without `-O`, and run:
//! it prints the same, or stops at the same place with the language's message,
//! at both (ADR-306 D2). The cases that stop are the ones a bound that says
//! more than the program does would get wrong.

mod common;

use std::process::Command;

use nikaia::bounds::{BoundsChecks, OverflowChecks};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

#[derive(Debug, Clone, Copy)]
enum Level {
    Off,
    Aggressive,
}

const LEVELS: [Level; 2] = [Level::Off, Level::Aggressive];

fn lowered(source: &str, level: Level) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    let how = match level {
        Level::Off => Build::default(),
        Level::Aggressive => Build {
            bounds: BoundsChecks::Aggressive,
            overflow: OverflowChecks::Aggressive,
            ..Build::default()
        },
    };
    emit_program(&parsed, how).expect("it lowers").rust
}

/// Indexes written without their check.
fn unchecked_indexes(rust: &str) -> usize {
    rust.matches("nikaia_std::proven::read(").count()
        + rust.matches("nikaia_std::proven::write(").count()
}

/// Operations written without their overflow check.
fn unchecked_arithmetic(rust: &str) -> usize {
    rust.matches(">::wrapping_").count()
}

fn run(purpose: &str, source: &str, level: Level) -> (bool, String, String) {
    let rust = lowered(source, level);
    let dir = common::scratch_dir(&format!("values-{purpose}-{level:?}"));
    let path = dir.join("program.rs");
    std::fs::write(&path, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let compiled = common::compile(
        &path,
        &[
            "--crate-type",
            "bin",
            "-C",
            "overflow-checks=on",
            "-o",
            &binary.to_string_lossy(),
        ],
    );
    assert!(
        compiled.status.success(),
        "{purpose} did not compile at {level:?}:\n{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let out = Command::new(&binary).output().expect("run it");
    std::fs::remove_dir_all(&dir).ok();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn prints(purpose: &str, source: &str, expected: &str) {
    for level in LEVELS {
        let (ok, out, err) = run(purpose, source, level);
        assert!(ok, "{purpose} failed at {level:?}:\n{err}");
        assert_eq!(out, expected, "{purpose} at {level:?}");
    }
}

/// Stops at both levels, with a message that contains `said` - never at a
/// debug assertion of a wrong proof.
fn stops(purpose: &str, source: &str, said: &str) {
    for level in LEVELS {
        let (ok, _, err) = run(purpose, source, level);
        assert!(!ok, "{purpose} ran to the end at {level:?}");
        assert!(
            !err.contains("a proved index is outside"),
            "{purpose}: a check was dropped that did not hold at {level:?}:\n{err}"
        );
        assert!(err.contains(said), "{purpose} at {level:?}:\n{err}");
    }
}

/// D1, D5: an operation the walk proves inside its type is written
/// `<T>::wrapping_*`, naming the type; nothing is unchecked at `off`.
#[test]
fn an_operation_proved_inside_its_type_is_written_without_its_check() {
    let source = "\
fn main() {
    let n: u32 = 2000
    let mut last: u32 = 0
    for v in 0..<n {
        last = 2 * v + 1
    }
    println(f\"{last}\")
}
";
    assert_eq!(unchecked_arithmetic(&lowered(source, Level::Off)), 0);
    let rust = lowered(source, Level::Aggressive);
    assert_eq!(unchecked_arithmetic(&rust), 2, "{rust}");
    assert!(rust.contains("<u32>::wrapping_add("), "{rust}");
    prints("proved-arithmetic", source, "3999\n");
}

/// An operation that does overflow keeps its check at every level.
#[test]
fn an_operation_that_overflows_stops_the_program_at_every_level() {
    let source = "\
fn double(x: u32) -> u32 {
    return x * 2
}

fn main() {
    println(f\"{double(3000000000)}\")
}
";
    assert_eq!(unchecked_arithmetic(&lowered(source, Level::Aggressive)), 0);
    stops("overflows", source, "overflow");
}

/// A guard is a fact: below it the operation fits.
#[test]
fn a_guard_proves_the_operation_under_it() {
    let source = "\
fn double(x: u32) -> u32 {
    if x < 1000 {
        return x * 2
    }
    return 0
}

fn main() {
    println(f\"{double(21)}\")
}
";
    assert_eq!(unchecked_arithmetic(&lowered(source, Level::Aggressive)), 1);
    prints("guarded", source, "42\n");
}

const TALLY: &str = "\
fn main() {
    let mut seen: Vec[i64] = []
    for i in 0..<100 {
        seen.push(i % 10)
    }
    let mut counts: Vec[i64] = []
    counts.resize(10, 0)
    let mut total = 0
    for k in 0..<seen.len() {
        total += counts[seen[k]] + seen[k]
    }
    println(f\"{total}\")
}
";

/// D2, D3: every value pushed is `i % 10`, so a value read back indexes a
/// list of ten.
#[test]
fn a_value_read_out_of_a_bounded_list_indexes_without_its_check() {
    let off = unchecked_indexes(&lowered(TALLY, Level::Off));
    let on = unchecked_indexes(&lowered(TALLY, Level::Aggressive));
    assert_eq!(off, 0);
    // `seen[k]` twice by its own length, `counts[seen[k]]` by `seen`'s values.
    assert_eq!(on, 3, "{}", lowered(TALLY, Level::Aggressive));
    prints("tally", TALLY, "450\n");
}

/// **Every write counts**: one value past the bound, written after the loop,
/// and the read is not proved - it stops at the index.
#[test]
fn a_later_write_past_the_bound_keeps_the_check() {
    let source = "\
fn main() {
    let mut seen: Vec[i64] = []
    for i in 0..<5 {
        seen.push(i % 10)
    }
    seen.push(10)
    let mut counts: Vec[i64] = []
    counts.resize(10, 0)
    let mut total = 0
    for k in 0..<seen.len() {
        total += counts[seen[k]]
    }
    println(f\"{total}\")
}
";
    let rust = lowered(source, Level::Aggressive);
    assert!(!rust.contains("proven::read(&counts"), "{rust}");
    stops("later-write", source, "index");
}

/// A write the walk does not see - in a function with a `mut` parameter -
/// leaves the list without a bound.
#[test]
fn a_list_handed_to_a_function_that_changes_it_has_no_bound() {
    let source = "\
fn spoil(mut xs: Vec[i64]) {
    xs.push(99)
}

fn main() {
    let mut seen: Vec[i64] = []
    for i in 0..<5 {
        seen.push(i % 3)
    }
    spoil(seen)
    let mut counts: Vec[i64] = []
    counts.resize(3, 0)
    let mut total = 0
    for k in 0..<seen.len() {
        total += counts[seen[k]]
    }
    println(f\"{total}\")
}
";
    let rust = lowered(source, Level::Aggressive);
    assert!(!rust.contains("proven::read(&counts"), "{rust}");
    stops("handed-on", source, "index");
}

/// **A bound may not hold by assuming itself**: each value is the last plus
/// one, which no bound found from an earlier pass covers.
#[test]
fn a_bound_that_only_holds_by_assuming_it_is_dropped() {
    let source = "\
fn main() {
    let mut xs: Vec[i64] = [0]
    for i in 0..<3 {
        xs.push(xs[i] + 1)
    }
    let small = [7, 8, 9]
    println(f\"{small[xs[3]]}\")
}
";
    let rust = lowered(source, Level::Aggressive);
    assert!(!rust.contains("proven::read(&small"), "{rust}");
    stops("self-assumed", source, "index");
}

/// D4: one `push` per turn of `0..<n` makes the list `n` long.
#[test]
fn a_loop_that_pushes_once_per_turn_sets_the_length() {
    let source = "\
fn main() {
    let mut xs: Vec[i64] = []
    for i in 0..<10 {
        xs.push(i)
    }
    let mut total = 0
    for i in 0..<10 {
        total += xs[i]
    }
    println(f\"{total}\")
}
";
    let rust = lowered(source, Level::Aggressive);
    assert!(rust.contains("proven::read(&xs"), "{rust}");
    prints("filled", source, "45\n");
}

/// D4 does not hold for a turn that can end before its `push`.
#[test]
fn a_loop_that_can_skip_its_push_says_nothing_of_the_length() {
    let source = "\
fn main() {
    let mut xs: Vec[i64] = []
    for i in 0..<10 {
        if i == 3 {
            continue
        }
        xs.push(i)
    }
    println(f\"{xs[9]}\")
}
";
    let rust = lowered(source, Level::Aggressive);
    assert!(!rust.contains("proven::read(&xs"), "{rust}");
    stops("skipped-push", source, "index");
}

/// A value read out of a list bounds an operation on it: `250 + 10` does not
/// fit a `u8`, so the check stays and the program stops.
#[test]
fn a_bounded_value_too_large_for_the_operation_keeps_its_check() {
    let source = "\
fn main() {
    let mut xs: Vec[u8] = []
    xs.push(250)
    let y = xs[0] + 10
    println(f\"{y}\")
}
";
    assert_eq!(unchecked_arithmetic(&lowered(source, Level::Aggressive)), 0);
    stops("too-large", source, "overflow");
}

/// A list a lambda writes has no bound: the lambda may run at any time.
#[test]
fn a_list_a_lambda_writes_has_no_bound() {
    let source = "\
fn main() {
    let mut xs: Vec[i64] = [1]
    let grow = fn (v) { xs.push(v) }
    grow(5)
    let small = [7, 8]
    println(f\"{small[xs[1]]}\")
}
";
    let rust = lowered(source, Level::Aggressive);
    assert!(!rust.contains("proven::read(&small"), "{rust}");
}

/// D1: the words.
#[test]
fn the_overflow_option_is_read_and_a_wrong_level_is_refused() {
    use nikaia::project::optimizations;
    let (bounds, overflow) =
        optimizations("remove-bounds-checks:basic,remove-overflow-checks:aggressive").unwrap();
    assert_eq!(bounds, BoundsChecks::Basic);
    assert_eq!(overflow, OverflowChecks::Aggressive);
    assert_eq!(optimizations("").unwrap().1, OverflowChecks::Kept);
    let basic = optimizations("remove-overflow-checks:basic").unwrap_err();
    assert!(
        basic.to_string().contains("`off` or `aggressive`"),
        "{basic}"
    );
}
