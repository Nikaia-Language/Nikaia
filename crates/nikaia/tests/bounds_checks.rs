//! **`--optimization=remove-bounds-checks`** ([ADR-271](../../../docs/specification/adr/adr-271.md)):
//! an index's check is dropped where it is proved inside, and only there.
//!
//! Every program here is lowered at each level, compiled and run, and prints
//! the same at all of them (D2). Where the proof would be wrong, the program
//! still stops at the index with the language's own message - the cases that
//! guard against a proof that says more than the code does. The programs are
//! compiled without `-O`, so a wrong proof that slipped through would stop at
//! `proven-index`'s debug assertion rather than read past the end.

mod common;

use std::process::Command;

use nikaia::bounds::BoundsChecks;
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str, level: BoundsChecks) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    let how = Build {
        bounds: level,
        ..Build::default()
    };
    emit_program(&parsed, how).expect("it lowers").rust
}

/// How many reads and writes are written without their check.
fn unchecked(rust: &str) -> usize {
    rust.matches("nikaia_std::proven::read(").count()
        + rust.matches("nikaia_std::proven::write(").count()
}

/// The program at `level`: whether it ran to the end, what it printed, and
/// what it said on stderr.
fn run(purpose: &str, source: &str, level: BoundsChecks) -> (bool, String, String) {
    let rust = lowered(source, level);
    let dir = common::scratch_dir(&format!("bounds-{purpose}-{}", level.name()));
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

const LEVELS: [BoundsChecks; 3] = [
    BoundsChecks::Kept,
    BoundsChecks::Basic,
    BoundsChecks::Aggressive,
];

/// The same output at every level.
fn prints(purpose: &str, source: &str, expected: &str) {
    for level in LEVELS {
        let (ok, out, err) = run(purpose, source, level);
        assert!(ok, "{purpose} failed at {level:?}:\n{err}");
        assert_eq!(out, expected, "{purpose} at {level:?}");
    }
}

/// At every level, the program stops at an index out of bounds, with the
/// language's message and not a debug assertion of a wrong proof.
fn stops_at_the_index(purpose: &str, source: &str) {
    for level in LEVELS {
        let (ok, _, err) = run(purpose, source, level);
        assert!(!ok, "{purpose} ran to the end at {level:?}");
        assert!(
            !err.contains("a proved index is outside"),
            "{purpose}: a check was dropped that did not hold at {level:?}:\n{err}"
        );
        assert!(err.contains("index"), "{purpose} at {level:?}:\n{err}");
    }
}

const OVER_ITS_LENGTH: &str = "\
fn total(xs: Vec[i64]) -> i64 {
    let mut sum = 0
    for i in 0..<xs.len() {
        sum += xs[i]
    }
    return sum
}

fn main() {
    println(f\"{total([1, 2, 3])}\")
}
";

/// D3: the loop over a list's own length is `basic`'s one shape.
#[test]
fn a_loop_over_the_lists_own_length_is_proved_at_basic() {
    assert_eq!(unchecked(&lowered(OVER_ITS_LENGTH, BoundsChecks::Kept)), 0);
    assert_eq!(unchecked(&lowered(OVER_ITS_LENGTH, BoundsChecks::Basic)), 1);
    assert_eq!(
        unchecked(&lowered(OVER_ITS_LENGTH, BoundsChecks::Aggressive)),
        1
    );
    prints("over-its-length", OVER_ITS_LENGTH, "6\n");
}

/// A list that shrinks inside the loop: the range was read once, so the last
/// turns are outside. Neither level may drop the check.
#[test]
fn a_list_that_shrinks_in_the_loop_keeps_its_check() {
    let source = "\
fn main() {
    let mut xs = [1, 2, 3]
    let mut sum = 0
    for i in 0..<xs.len() {
        sum += xs[i]
        xs.pop()
    }
    println(f\"{sum}\")
}
";
    for level in LEVELS {
        assert_eq!(unchecked(&lowered(source, level)), 0, "{level:?}");
    }
    stops_at_the_index("shrinks", source);
}

/// D4: a guard that leaves is a fact for what follows it - `aggressive` only.
#[test]
fn a_guard_that_leaves_proves_the_index_after_it() {
    let source = "\
fn pick(xs: Vec[i64], i: i64) -> i64 {
    if i < 0 || i >= xs.len() {
        return 0
    }
    return xs[i]
}

fn main() {
    let xs = [10, 20, 30]
    println(f\"{pick(xs, 1)} {pick(xs, 3)} {pick(xs, 0 - 1)}\")
}
";
    assert_eq!(unchecked(&lowered(source, BoundsChecks::Basic)), 0);
    assert_eq!(unchecked(&lowered(source, BoundsChecks::Aggressive)), 1);
    prints("guard", source, "20 0 0\n");
}

/// **What `a || (b && c)` false says**: `a` false, and nothing of `b` alone.
/// Read as `b` it would prove `xs[i]` here, and the program would read past
/// the end; it stops instead.
#[test]
fn a_conjunction_denied_says_nothing_of_one_half() {
    let source = "\
fn pick(xs: Vec[i64], i: i64, j: i64, wide: bool) -> i64 {
    if j >= 1 || (i < xs.len() && wide) {
        return 0
    }
    return xs[i]
}

fn main() {
    let xs = [10, 20, 30]
    println(f\"{pick(xs, 5, 0, false)}\")
}
";
    for level in LEVELS {
        assert_eq!(unchecked(&lowered(source, level)), 0, "{level:?}");
    }
    stops_at_the_index("denied-conjunction", source);
}

/// The sparse-row merge of `benches/solver-kernels.nika`: two counters walk
/// two lists under a `while` whose condition, and the branch taken, say which
/// one is inside.
#[test]
fn a_merge_of_two_lists_is_proved_from_its_conditions() {
    let source = "\
fn merge(a: Vec[i64], b: Vec[i64], mut out: Vec[i64]) {
    let mut i = 0
    let mut j = 0
    while i < a.len() || j < b.len() {
        if j >= b.len() || (i < a.len() && a[i] < b[j]) {
            out.push(a[i])
            i += 1
        } else {
            out.push(b[j])
            j += 1
        }
    }
}

fn main() {
    let mut out: Vec[i64] = []
    merge([1, 4, 9], [2, 3, 10, 11], out)
    println(f\"{out.len()} {out[0]} {out[6]}\")
}
";
    let rust = lowered(source, BoundsChecks::Aggressive);
    // `a[i]` and `b[j]` in the condition, `a[i]` and `b[j]` in the branches.
    assert_eq!(unchecked(&rust), 4, "{rust}");
    prints("merge", source, "7 1 11\n");
}

/// The big-integer product: a list resized to the sum of two lengths, and
/// both loops over the factors' lengths, so `out[i + j]` is inside.
#[test]
fn a_list_resized_to_a_sum_of_lengths_is_proved_inside() {
    let source = "\
fn multiply(x: ref Vec[u32], y: ref Vec[u32], mut out: Vec[u32]) {
    out.resize(x.len() + y.len(), 0)
    for i in 0..<x.len() {
        let mut carry: u64 = 0
        let xi = x[i] as u64
        for j in 0..<y.len() {
            let t = xi * (y[j] as u64) + (out[i + j] as u64) + carry
            out[i + j] = t.truncating_u32()
            carry = t >> 32
        }
        out[i + y.len()] = carry.truncating_u32()
    }
}

fn main() {
    let x: Vec[u32] = [4294967295, 7]
    let y: Vec[u32] = [3, 1]
    let mut out: Vec[u32] = []
    multiply(x, y, out)
    println(f\"{out[0]} {out[1]} {out[2]} {out[3]}\")
}
";
    let rust = lowered(source, BoundsChecks::Aggressive);
    assert_eq!(unchecked(&rust), 5, "{rust}");
    prints("multiply", source, "4294967293 22 8 0\n");
}

/// A counter changed inside the loop loses what was known of it at the head,
/// so an index past a `+= 2` is not proved from the first turn's bound.
#[test]
fn a_counter_changed_in_the_body_is_not_trusted_past_the_change() {
    let source = "\
fn main() {
    let xs = [1, 2, 3]
    let mut i = 0
    let mut sum = 0
    while i < xs.len() {
        i += 2
        sum += xs[i]
    }
    println(f\"{sum}\")
}
";
    assert_eq!(unchecked(&lowered(source, BoundsChecks::Aggressive)), 0);
    stops_at_the_index("counter", source);
}

/// D1: the words, and what is refused.
#[test]
fn the_option_is_read_and_a_wrong_word_is_refused() {
    use nikaia::project::optimizations;
    assert_eq!(optimizations("").unwrap(), BoundsChecks::Kept);
    assert_eq!(
        optimizations("remove-bounds-checks:basic").unwrap(),
        BoundsChecks::Basic
    );
    assert_eq!(
        optimizations("remove-bounds-checks:basic, remove-bounds-checks:aggressive").unwrap(),
        BoundsChecks::Aggressive
    );
    let level = optimizations("remove-bounds-checks:all").unwrap_err();
    assert!(level.to_string().contains("`off`, `basic` or `aggressive`"));
    let name = optimizations("inline:aggressive").unwrap_err();
    assert!(name.to_string().contains("remove-bounds-checks"));
    let bare = optimizations("remove-bounds-checks").unwrap_err();
    assert!(bare.to_string().contains("needs a level"));
}

/// D1 on the command line: `--optimization` reaches the lowering, and a word
/// it does not know stops the build before anything is read.
#[test]
fn the_command_line_carries_the_option_to_the_lowering() {
    let dir = common::scratch_dir("bounds-cli");
    let file = dir.join("total.nika");
    std::fs::write(&file, OVER_ITS_LENGTH).expect("write the source");
    let lower = |flag: &str| {
        Command::new(env!("CARGO_BIN_EXE_nikaia"))
            .arg(flag)
            .arg("lower")
            .arg(&file)
            .current_dir(&dir)
            .output()
            .expect("run nikaia")
    };
    let done = lower("--optimization=remove-bounds-checks:basic");
    assert!(
        done.status.success(),
        "{}",
        String::from_utf8_lossy(&done.stderr)
    );
    let rust = std::fs::read_to_string(dir.join("total.rs")).expect("the lowered file");
    assert_eq!(unchecked(&rust), 1, "{rust}");

    let refused = lower("--optimization=remove-bounds-checks:everything");
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("`off`, `basic` or `aggressive`"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// **The value is written before the index is reached**: `xs[2] = …` with a
/// value that shrinks `xs` to two elements writes outside, though `xs` had
/// three when the statement began.
#[test]
fn a_value_that_shrinks_the_list_is_reached_before_the_write() {
    let source = "\
fn main() {
    let mut xs = [1, 2, 3]
    xs[2] = xs.pop() ?? 0
    println(f\"{xs.len()}\")
}
";
    for level in LEVELS {
        assert_eq!(unchecked(&lowered(source, level)), 0, "{level:?}");
    }
    stops_at_the_index("shrinking-value", source);
}

/// **A pattern hides an outer name**: what was known of the outer `i` says
/// nothing of the `i` an arm binds.
#[test]
fn a_name_a_pattern_binds_is_not_the_outer_one() {
    let source = "\
fn pick(xs: Vec[i64], k: i64) -> i64 {
    let i = 0
    if i < xs.len() {
        let picked = match k {
            0 => { 0 }
            i => { xs[i] }
        }
        return picked
    }
    return 0
}

fn main() {
    println(f\"{pick([1, 2], 7)}\")
}
";
    for level in LEVELS {
        assert_eq!(unchecked(&lowered(source, level)), 0, "{level:?}");
    }
    stops_at_the_index("pattern", source);
}
