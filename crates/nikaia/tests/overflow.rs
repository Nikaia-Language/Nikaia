//! An arithmetic overflow aborts, at every build.
//!
//! [ADR-285](../../../docs/specification/adr/adr-285.md) D5. The specification
//! said nothing about overflow, and what the silence cost was measurable: the
//! generated project named one profile, `dev`, so a user's program aborted while
//! every benchmark in this repository runs `--release`, where it wraps. The
//! measurements were taken on wrapping arithmetic and the shipped program
//! aborted. Nobody chose that.
//!
//! **Why this file has three tests and not one.** The check lives in the
//! generated `Cargo.toml` (D6) rather than in the emitted code, so no single
//! test sees the whole of it:
//!
//! 1. the emitted Rust aborts when the check is on — the semantics;
//! 2. the manifest this compiler writes carries the check, and carries it
//!    *off* for every dependency — the setting;
//! 3. the two meet, end to end, through Cargo — the seam, and the only one
//!    that needs the network.
//!
//! Without the third the setting could be correct and unapplied; without the
//! second the third would not say why it passed. The first is what holds if
//! somebody ever reaches for a checked helper per operation, which D6 refuses.
//!
//! **And the conversions, which are a second mechanism** (D4). `as` truncates by
//! definition, so there is no check in the project file to switch on and the
//! answer is in the emitted code instead. The tests for those compile with `-O`
//! and **no** check flag at all - the shipping build, as far as this mechanism is
//! concerned - which is the whole claim: a conversion that does not fit aborts
//! there too.

mod common;

use std::path::Path;
use std::process::Command;

/// A sum that overflows an `i32`, across a call so that it is not constant.
///
/// Across a call on purpose: rustc refuses a *constant-evaluable* overflow at
/// compile time with "this arithmetic operation will overflow", which is a
/// message about the generated file and a different defect (ADR-285). What is
/// under test here is the run time, so the addition has to be one no lint can
/// see through.
const OVERFLOWS: &str = "\
fn add(a: i32, b: i32) -> i32 {
    return a + b
}

fn main() {
    println(f\"{add(2147483647, 1)}\")
}
";

fn lower(dir: &Path, source: &str) -> String {
    let input = dir.join("main.nika");
    std::fs::write(&input, source).expect("the source");
    let output = dir.join("main.rs");
    let run = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .args(["lower", input.to_str().expect("utf-8 path")])
        .args(["--output", output.to_str().expect("utf-8 path")])
        .args(["--no-cache"])
        .env("NIKAIA_CACHE_DIR", dir.join("cache"))
        .output()
        .expect("the nikaia binary runs");
    assert!(
        run.status.success(),
        "lowering failed:\n{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    std::fs::read_to_string(&output).expect("the emitted Rust")
}

/// **1. The semantics.** With the check on, the emitted program aborts.
///
/// And the emitted Rust says `a + b`, which is the other half of D6: the
/// arithmetic is identical whichever way the check is arranged, but anyone who
/// opens the generated Rust during a benchmark comparison and finds a helper
/// call where `+` should be will not believe the numbers.
#[test]
fn an_overflow_aborts_where_the_check_is_on() {
    let dir = common::scratch_dir("overflow-aborts");
    let rust = lower(&dir, OVERFLOWS);
    assert!(
        rust.contains("a + b"),
        "the emitted code must say `+` (ADR-285 D8):\n{rust}"
    );

    let binary = dir.join("program");
    let compiled = common::compile(
        &dir.join("main.rs"),
        &[
            "--crate-type",
            "bin",
            "-C",
            "overflow-checks=on",
            "-o",
            binary.to_str().expect("utf-8 path"),
        ],
    );
    assert!(
        compiled.status.success(),
        "the emitted Rust did not compile:\n{}",
        String::from_utf8_lossy(&compiled.stderr)
    );

    let ran = Command::new(&binary).output().expect("run the program");
    assert!(
        !ran.status.success(),
        "an overflow must abort, and this program exited cleanly printing {:?}",
        String::from_utf8_lossy(&ran.stdout)
    );
    let said = String::from_utf8_lossy(&ran.stderr);
    assert!(
        said.contains("attempt to add with overflow"),
        "it failed, but not of the overflow:\n{said}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// **2. The setting**, in the manifest this compiler writes for a real project.
///
/// Both halves, because one without the other is not the decision: on for the
/// program, and **off for every dependency**. A hash function in a Rust crate
/// wraps on purpose and is not our code to be right or wrong about, so the rule
/// reaches the program this compiler emits and stops at the crate boundary.
///
/// Which way round the two are written follows from
/// [ADR-286](../../../docs/specification/adr/adr-286.md) D22: a build emits a
/// workspace, and a default of "on" with an exception for `"*"` would reach the
/// members too. So the default is the foreign answer and each crate of this
/// language is named back onto the program's side.
#[test]
fn the_generated_manifest_checks_the_program_and_not_its_dependencies() {
    let dir = common::scratch_dir("overflow-manifest");
    let project = dir.join("src");
    std::fs::create_dir_all(&project).expect("the project directory");
    std::fs::write(dir.join("nikaia.toml"), "[package]\nname = \"ovf\"\n").expect("the manifest");
    std::fs::write(project.join("main.nika"), OVERFLOWS).expect("the source");

    // `--emit-manifest` is not a flag; the manifest is written as part of a
    // build. So this asks for the one thing a build does first and reads what it
    // left behind, which is why it tolerates the build itself failing.
    let _ = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .args(["build"])
        .current_dir(&dir)
        .env("CARGO_NET_OFFLINE", "true")
        .output()
        .expect("the nikaia binary runs");

    let written = find_manifest(&dir).unwrap_or_else(|| {
        panic!(
            "no generated Cargo.toml under {} - the build wrote none",
            dir.display()
        )
    });
    let text = std::fs::read_to_string(&written).expect("the generated manifest");
    let parsed: toml::Value = toml::from_str(&text).expect("it parses");
    let profile = &parsed["profile"];
    let (name, table) = profile
        .as_table()
        .expect("a profile table")
        .iter()
        .next()
        .expect("one profile");

    assert_eq!(
        table["overflow-checks"].as_bool(),
        Some(false),
        "`[profile.{name}]` must not check a foreign crate:\n{text}"
    );
    assert_eq!(
        table["package"]["ovf"]["overflow-checks"].as_bool(),
        Some(true),
        "`[profile.{name}.package.ovf]` must check the program:\n{text}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The generated workspace root's `Cargo.toml` - the one that carries the
/// profile ([ADR-286](../../../docs/specification/adr/adr-286.md) D19). A member's
/// is one directory further down and carries none, so the shallowest wins.
fn find_manifest(under: &Path) -> Option<std::path::PathBuf> {
    let mut stack = vec![(0usize, under.to_path_buf())];
    let mut best: Option<(usize, std::path::PathBuf)> = None;
    while let Some((depth, dir)) = stack.pop() {
        for entry in std::fs::read_dir(&dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push((depth + 1, path));
            } else if path.file_name().is_some_and(|n| n == "Cargo.toml")
                && best.as_ref().is_none_or(|(at, _)| depth < *at)
            {
                best = Some((depth, path));
            }
        }
    }
    best.map(|(_, path)| path)
}

/// **3. The seam.** A `nikaia build` of an overflowing program, run, aborts.
///
/// `#[ignore]`d for the reason `tests/foreign_runtime.rs` gives for the same
/// thing: it runs Cargo, which resolves and fetches `std`'s dependencies. The
/// two tests above are what hold in the fast suite, and between them they cover
/// both sides of this one — what is left is only that the setting is applied,
/// which is the thing a reviewer would most reasonably doubt.
#[test]
#[ignore = "runs cargo and resolves std's dependencies from crates.io"]
fn a_built_program_that_overflows_aborts() {
    let dir = common::scratch_dir("overflow-built");
    let project = dir.join("src");
    std::fs::create_dir_all(&project).expect("the project directory");
    std::fs::write(dir.join("nikaia.toml"), "[package]\nname = \"ovf\"\n").expect("the manifest");
    std::fs::write(project.join("main.nika"), OVERFLOWS).expect("the source");

    let ran = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .args(["run"])
        .current_dir(&dir)
        .env("NIKAIA_CACHE_DIR", dir.join("cache"))
        .output()
        .expect("the nikaia binary runs");

    let said = String::from_utf8_lossy(&ran.stderr);
    assert!(
        said.contains("attempt to add with overflow"),
        "a built program that overflows must abort:\nstdout: {}\nstderr: {said}",
        String::from_utf8_lossy(&ran.stdout)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// **Wrapping and saturating arithmetic, run.**
///
/// ADR-285 D6 and D7. These exist because D1 aborts: a hash function wraps on
/// purpose and has to be able to say so. Run rather than read, because what is
/// under test is a number and not a spelling - and the pair with the plain `*`
/// below is the whole point, since the same arithmetic two ways must give two
/// answers.
#[test]
fn wrapping_and_saturating_say_what_the_operator_would_refuse() {
    let dir = common::scratch_dir("overflow-named");
    let rust = lower(
        &dir,
        "fn hash(seed: i32, c: i32) -> i32 {\n    \
             return seed.wrapping_mul(31).wrapping_add(c)\n\
         }\n\
         \n\
         fn clamp(level: i32, rise: i32) -> i32 {\n    \
             return level.saturating_add(rise)\n\
         }\n\
         \n\
         fn shifted(n: i64) -> i64 {\n    \
             return n.wrapping_shl(2)\n\
         }\n\
         \n\
         fn main() {\n    \
             println(f\"{hash(2147483647, 7)} {clamp(2147483647, 5)} {shifted(3)}\")\n\
         }\n",
    );

    let binary = dir.join("program");
    let compiled = common::compile(
        &dir.join("main.rs"),
        &[
            "--crate-type",
            "bin",
            "-C",
            "overflow-checks=on",
            "-o",
            binary.to_str().expect("utf-8 path"),
        ],
    );
    assert!(
        compiled.status.success(),
        "the emitted Rust did not compile:\n{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&compiled.stderr)
    );

    let ran = Command::new(&binary).output().expect("run the program");
    assert!(
        ran.status.success(),
        "none of these may abort, and this one did:\n{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).trim(),
        "2147483624 2147483647 12",
        "wrapped, saturated, shifted"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// And the same multiplication with `*` aborts, which is what makes the names
/// above worth having rather than decoration.
#[test]
fn the_same_arithmetic_with_an_operator_aborts() {
    let dir = common::scratch_dir("overflow-contrast");
    lower(
        &dir,
        "fn hash(seed: i32, c: i32) -> i32 {\n    \
             return seed * 31 + c\n\
         }\n\
         \n\
         fn main() {\n    \
             println(f\"{hash(2147483647, 7)}\")\n\
         }\n",
    );
    let binary = dir.join("program");
    let compiled = common::compile(
        &dir.join("main.rs"),
        &[
            "--crate-type",
            "bin",
            "-C",
            "overflow-checks=on",
            "-o",
            binary.to_str().expect("utf-8 path"),
        ],
    );
    assert!(compiled.status.success());

    let ran = Command::new(&binary).output().expect("run the program");
    let said = String::from_utf8_lossy(&ran.stderr);
    assert!(
        said.contains("attempt to multiply with overflow"),
        "`*` must abort where `wrapping_mul` wraps:\n{said}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A `f64` that is no `i32`, `-O` and no check flag - and it aborts (D4).
///
/// **This is the conversion half of `a_built_program_that_overflows_aborts`, and
/// it needs no Cargo.** The arithmetic check is a setting, so nothing below the
/// full build proves it is applied; a conversion is emitted code, so the shipping
/// build is reproduced here by compiling the way a release build does - `-O`,
/// with `overflow-checks` left alone - and the abort has to come anyway.
const DOES_NOT_FIT: &str = "\
fn shrink(big: i64) -> i32 {
    return big as i32
}

fn main() {
    println(f\"{shrink(5000000000)}\")
}
";

/// Out of a floating-point number, where `try_from` does not exist and `std`
/// carries the test instead (D4, `nikaia_std::num`).
const NO_INTEGER: &str = "\
fn floored(x: f64) -> i32 {
    return x as i32
}

fn main() {
    println(f\"{floored(1e20)}\")
}
";

/// Compile in the shipping build and run. Hands back what the program said on
/// standard error.
fn run_optimized(dir: &Path, source: &str) -> (bool, String) {
    let rust = lower(dir, source);
    let binary = dir.join("program");
    let compiled = common::compile(
        &dir.join("main.rs"),
        &[
            "--crate-type",
            "bin",
            "-O",
            "-o",
            binary.to_str().expect("utf-8 path"),
        ],
    );
    assert!(
        compiled.status.success(),
        "the emitted Rust did not compile:\n{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let ran = Command::new(&binary).output().expect("run the program");
    (
        ran.status.success(),
        String::from_utf8_lossy(&ran.stderr).to_string(),
    )
}

/// **A narrowing conversion aborts, in the shipping build** (ADR-285 D12).
#[test]
fn a_number_that_does_not_fit_the_smaller_type_aborts() {
    let dir = common::scratch_dir("narrowing-integer");
    let (clean, said) = run_optimized(&dir, DOES_NOT_FIT);
    assert!(!clean, "`5000000000 as i32` must not come through");
    assert!(
        said.contains("the value does not fit in an `i32`"),
        "it failed, but not of the conversion:\n{said}"
    );
    // The abort must name the generated Nikaia line and not a file in `std`:
    // ADR-300's table has nothing to translate otherwise. `#[track_caller]` is
    // what makes this true for the float half below.
    assert!(
        !said.contains("num.rs"),
        "the abort must not point into `std`:\n{said}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// **And out of a floating-point number**, which `as` answered three different
/// ways in silence: it stopped at the limit in both directions and turned "not a
/// number" into zero. `1e20 as i32` was `2147483647`.
#[test]
fn a_float_that_is_no_integer_aborts() {
    let dir = common::scratch_dir("narrowing-float");
    let (clean, said) = run_optimized(&dir, NO_INTEGER);
    assert!(!clean, "`1e20 as i32` must not come through as 2147483647");
    assert!(
        said.contains("the value does not fit in an `i32`"),
        "it failed, but not of the conversion:\n{said}"
    );
    assert!(
        !said.contains("num.rs"),
        "`#[track_caller]` must put the Nikaia line here, not `std`:\n{said}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// **Truncation by name, and the two conversions that stay silent** (D7).
///
/// Three truncating sources - the larger integer, a floating-point number, and
/// the machine-width type `len` hands back - beside the two conversions D7
/// deliberately leaves alone: widening, which always fits, and an integer to an
/// `f64`, which loses digits at large values and is a written limit rather than a
/// check. None of the five may abort.
///
/// Run rather than read, because what is under test is five numbers. And read as
/// well, in one assertion: the name lowers to `as` and to nothing else, which is
/// what makes it the operation it claims to be rather than a helper with a
/// friendly name.
#[test]
fn truncating_says_by_name_what_a_conversion_no_longer_does_quietly() {
    let dir = common::scratch_dir("narrowing-named");
    let rust = lower(
        &dir,
        "fn shrink(big: i64) -> i32 {\n    \
             return big.truncating_i32()\n\
         }\n\
         \n\
         fn floored(x: f64) -> i32 {\n    \
             return x.truncating_i32()\n\
         }\n\
         \n\
         fn counted(text: ref String) -> i32 {\n    \
             return text.len().truncating_i32()\n\
         }\n\
         \n\
         fn widened(small: i32) -> i64 {\n    \
             return small as i64\n\
         }\n\
         \n\
         fn imprecise(n: i64) -> f64 {\n    \
             return n as f64\n\
         }\n\
         \n\
         fn main() {\n    \
             println(f\"{shrink(5000000000)} {floored(1e20)} {counted(\"hello\")} {widened(7)} {imprecise(9007199254740993)}\")\n\
         }\n",
    );
    assert!(
        rust.contains("big as i32") && !rust.contains("truncating"),
        "the name is Rust's `as` and nothing else (ADR-285 D13):\n{rust}"
    );
    // Bare where nothing needs it, parenthesised where something does: a length
    // is an `i64` (ADR-285 D1) and narrowing it is a second conversion, so
    // `text.len().truncating_i32()` is two of them and the inner one has to
    // happen first. A parenthesis nobody needs is a warning about a file nobody
    // wrote (Part III, C.1), which is why this is not simply always.
    assert!(rust.contains("(text.len() as i64) as i32"), "{rust}");
    assert!(
        !rust.contains("try_from") && !rust.contains("nikaia_std::num"),
        "nothing here is checked - that is what the name asked for:\n{rust}"
    );

    let binary = dir.join("program");
    let compiled = common::compile(
        &dir.join("main.rs"),
        &[
            "--crate-type",
            "bin",
            "-O",
            "-o",
            binary.to_str().expect("utf-8 path"),
        ],
    );
    assert!(
        compiled.status.success(),
        "the emitted Rust did not compile:\n{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&compiled.stderr)
    );

    let ran = Command::new(&binary).output().expect("run the program");
    assert!(
        ran.status.success(),
        "none of these may abort, and this one did:\n{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).trim(),
        "705032704 2147483647 5 7 9007199254740992",
        "truncated, clamped by the same truncation, counted, widened, and the \
         one digit an `f64` cannot keep"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// **A length is an `i64` and an index takes one, and neither conversion is
/// written** ([ADR-285](../../../docs/specification/adr/adr-285.md) D1).
///
/// The program below is the shape D1 is about: a loop over a length, an index by
/// the loop variable, a length in arithmetic and a length in a comparison. Before
/// this, every one of those lines carried an `as i64` the user wrote and that
/// could not fail.
#[test]
fn a_length_is_an_i64_and_the_conversions_are_emitted() {
    let dir = common::scratch_dir("length-i64");
    let rust = lower(
        &dir,
        "fn main() {\n    \
             let mut xs = Vec()\n    \
             xs.push(10)\n    \
             xs.push(20)\n    \
             xs.push(30)\n    \
             let mut sum = 0\n    \
             for i in 0..<xs.len() {\n        \
                 sum = sum + xs[i]\n    \
             }\n    \
             let last = xs[xs.len() - 1]\n    \
             let short = xs.len() < 2\n    \
             let fits = 1 + xs.len() < 5\n    \
             println(f\"{sum} {last} {short} {fits} {xs.len()}\")\n\
         }\n",
    );

    // Nothing the user wrote, and both directions emitted.
    assert!(rust.contains("0..xs.len() as i64"), "{rust}");
    // A counter that starts at zero and only grows is converted as it is
    // (0.0.373); a position that may be negative still goes through `at`.
    assert!(rust.contains("(i) as usize"), "{rust}");
    assert!(
        rust.contains("nikaia_std::index::at(xs.len() as i64 - 1)"),
        "{rust}"
    );
    // A length against a literal is compared as the length it is (0.0.373),
    // so there is no `as i64` for Rust to read as `i64<2>`.
    assert!(rust.contains("xs.len() < 2"), "{rust}");
    // And so is a sum that ends in one (#125).
    assert!(rust.contains("(1 + xs.len() as i64) < 5"), "{rust}");

    let binary = dir.join("program");
    let compiled = common::compile(
        &dir.join("main.rs"),
        &[
            "--crate-type",
            "bin",
            "-o",
            binary.to_str().expect("utf-8 path"),
        ],
    );
    assert!(
        compiled.status.success(),
        "{}\n{rust}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let ran = std::process::Command::new(&binary)
        .output()
        .expect("run it");
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).trim(),
        "60 30 false true 3"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// …and a **negative** index reports as an access out of bounds, which is D1's
/// one requirement: it is one (Part III, A.2), and a diagnostic about a failed
/// conversion would be about something the user never wrote.
#[test]
fn a_negative_index_aborts_as_an_access_out_of_bounds() {
    let dir = common::scratch_dir("negative-index");
    let _ = lower(
        &dir,
        "fn main() {\n    \
             let mut xs = Vec()\n    \
             xs.push(1)\n    \
             let pos = 0\n    \
             println(f\"{xs[pos - 1]}\")\n\
         }\n",
    );

    let binary = dir.join("program");
    let compiled = common::compile(
        &dir.join("main.rs"),
        &[
            "--crate-type",
            "bin",
            "-o",
            binary.to_str().expect("utf-8 path"),
        ],
    );
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let ran = std::process::Command::new(&binary)
        .output()
        .expect("run it");
    assert!(!ran.status.success(), "a negative index aborts");
    let said = String::from_utf8_lossy(&ran.stderr);
    assert!(
        said.contains("index out of bounds") && said.contains("-1"),
        "the message is about the index, not about a conversion: {said}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// **A count goes in as an `i64` and the conversion is emitted**
/// ([ADR-285](../../../docs/specification/adr/adr-285.md) D15) — the parameter
/// direction of [ADR-285](../../../docs/specification/adr/adr-285.md) D1, which
/// that record left open and named `"  ".repeat(indent as usize)` as the one
/// site it had.
///
/// Both halves in one program, because one without the other is not the answer:
/// the user writes **no** conversion, and a **literal** count is left alone
/// (`of(2)` has nothing to infer its type from, the same reason `index::at` skips
/// a literal index).
#[test]
fn a_count_is_written_as_an_i64_and_the_conversion_is_emitted() {
    let dir = common::scratch_dir("count-parameter");
    let rust = lower(
        &dir,
        "fn main() {\n    \
             let indent: i64 = 2\n    \
             let pad = \"  \".repeat(indent)\n    \
             let rule = \"-\".repeat(3)\n    \
             println(f\"[{pad}][{rule}]\")\n\
         }\n",
    );

    assert!(
        rust.contains("\"  \".repeat(nikaia_std::count::of(indent))"),
        "the conversion is written by the compiler:\n{rust}"
    );
    assert!(
        rust.contains("\"-\".repeat(3)"),
        "and a literal needs none and can take none:\n{rust}"
    );

    let binary = dir.join("program");
    let compiled = common::compile(
        &dir.join("main.rs"),
        &[
            "--crate-type",
            "bin",
            "-o",
            binary.to_str().expect("utf-8 path"),
        ],
    );
    assert!(
        compiled.status.success(),
        "{}\n{rust}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let ran = std::process::Command::new(&binary)
        .output()
        .expect("run it");
    assert_eq!(String::from_utf8_lossy(&ran.stdout).trim(), "[    ][---]");
    let _ = std::fs::remove_dir_all(dir);
}

/// …and a **negative** count says what it is, which is the one place this differs
/// from an index ([ADR-285](../../../docs/specification/adr/adr-285.md) D15).
///
/// A negative index *is* an access out of bounds and reports as one; a negative
/// count is not an access at all, so reusing that message would name something
/// the program does not do. What it must never be is what `as usize` was:
/// `-1` becoming 18,446,744,073,709,551,615, silently.
#[test]
fn a_negative_count_says_what_it_is_rather_than_wrapping() {
    let dir = common::scratch_dir("negative-count");
    let rust = lower(
        &dir,
        "fn main() {\n    \
             let n: i64 = 0 - 1\n    \
             println(\"x\".repeat(n))\n\
         }\n",
    );

    let binary = dir.join("program");
    let compiled = common::compile(
        &dir.join("main.rs"),
        &[
            "--crate-type",
            "bin",
            "-o",
            binary.to_str().expect("utf-8 path"),
        ],
    );
    assert!(
        compiled.status.success(),
        "{}\n{rust}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let ran = std::process::Command::new(&binary)
        .output()
        .expect("run it");
    assert!(!ran.status.success(), "a negative count aborts");
    let said = String::from_utf8_lossy(&ran.stderr);
    assert!(
        said.contains("a count cannot be negative") && said.contains("-1"),
        "the message is about the count, and not about an access: {said}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// **Asking whether it fits, run** ([ADR-315](../../../docs/specification/adr/adr-315.md)).
///
/// The fourth family beside aborting, wrapping and saturating: the answer, or
/// `null` where there is none - an overflow, a division by zero (D2), the most
/// negative number's sign (D1), and a conversion the destination does not hold
/// (D4). Run, because the claim is about numbers, and with the checks on,
/// because none of these may abort.
#[test]
fn checked_names_answer_null_where_the_operator_would_abort() {
    let dir = common::scratch_dir("overflow-checked");
    let rust = lower(
        &dir,
        "fn sum(a: i32, b: i32) -> i32? {\n    \
             return a.checked_add(b)\n\
         }\n\
         \n\
         fn half(a: u64, b: u64) -> u64? {\n    \
             let q = a.checked_div(b) ?? return null\n    \
             return q\n\
         }\n\
         \n\
         fn small(n: i64) -> i32? {\n    \
             return n.checked_i32()\n\
         }\n\
         \n\
         fn count(n: u64) -> i64 {\n    \
             return (n - 1).checked_i64() ?? -1\n\
         }\n\
         \n\
         fn main() {\n    \
             println(f\"{sum(2147483647, 1) ?? -1} {sum(1, 2) ?? -1} {half(7, 0) ?? 0} {half(7, 2) ?? 0}\")\n    \
             println(f\"{small(5000000000) ?? -1} {small(-5) ?? 0} {count(18446744073709551615)} {count(4)}\")\n    \
             let m: i64 = -9223372036854775807 - 1\n    \
             println(f\"{m.checked_neg() ?? 0} {m.checked_abs() ?? 0} {m.checked_rem(0) ?? 9} {m.checked_div(-1) ?? 8}\")\n\
         }\n",
    );
    // A conversion is a call around its argument, which needs no parentheses
    // of its own (`unused_parens` would be a warning about the generated file).
    assert!(
        rust.contains("i64::try_from(n - 1"),
        "a checked conversion is Rust's try_from:\n{rust}"
    );

    let binary = dir.join("program");
    let compiled = common::compile(
        &dir.join("main.rs"),
        &[
            "--crate-type",
            "bin",
            "-C",
            "overflow-checks=on",
            "-o",
            binary.to_str().expect("utf-8 path"),
        ],
    );
    assert!(
        compiled.status.success(),
        "the emitted Rust did not compile:\n{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&compiled.stderr).contains("warning"),
        "the emitted Rust compiled with a warning:\n{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let ran = Command::new(&binary).output().expect("run the program");
    assert!(
        ran.status.success(),
        "none of these may abort, and this one did:\n{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).trim(),
        "-1 3 0 3\n-1 -5 -1 3\n0 0 9 8",
        "overflow, sum, division by zero, quotient; conversions; sign of the minimum"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// **A named method on a number written as its receiver** (#417): the number
/// takes the integer type its use asks for - a `let`'s, a parameter's, a
/// `return`'s - as `7 + 1` would, and the first type that holds it where
/// nothing asks; a negative one keeps its sign. `(-7).wrapping_add(2)` came out
/// as `-7.wrapping_add(2)`, which is `-(7.wrapping_add(2))` below, and the
/// function was taken to pause on a method of nothing known.
#[test]
fn a_named_method_on_a_written_number_takes_the_type_its_use_asks() {
    let dir = common::scratch_dir("overflow-literal-receiver");
    let rust = lower(
        &dir,
        "fn f() -> i32 {\n    return (-7).wrapping_add(2)\n}\n\
         fn g() -> i64 {\n    return 7.wrapping_mul(3)\n}\n\
         fn h(n: u64) -> u64 {\n    return n + 1\n}\n\
         fn main() {\n    \
             let x: i64 = 7.wrapping_add(1)\n    \
             println(f\"{f()} {g()} {x} {h(5.saturating_add(1))} {7.checked_mul(3) ?? 0}\")\n\
         }\n",
    );
    assert!(rust.contains("(-7i32).wrapping_add(2)"), "{rust}");
    assert!(!rust.contains("async fn f"), "{rust}");
    let binary = dir.join("program");
    let compiled = common::compile(
        &dir.join("main.rs"),
        &[
            "--crate-type",
            "bin",
            "-C",
            "overflow-checks=on",
            "-o",
            binary.to_str().expect("utf-8 path"),
        ],
    );
    assert!(
        compiled.status.success(),
        "the emitted Rust did not compile:\n{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let ran = Command::new(&binary).output().expect("run the program");
    assert_eq!(String::from_utf8_lossy(&ran.stdout).trim(), "-5 21 8 7 21");
    let _ = std::fs::remove_dir_all(&dir);
}
