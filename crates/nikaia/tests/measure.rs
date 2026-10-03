//! The measurement loop.
//!
//! `docs/history/staging-candidates.md` §5 records that the benchmark *programs* exist
//! and the harness does not, and ADR-178 D2 is the rule this file serves: **a
//! staging decision enters the compiler only together with a measured
//! crossover.** This is where a crossover is measured.
//!
//! Instructions retired, under callgrind, on the same tree - which is how
//! ADR-010's hasher was measured (ADR-296) and is the only kind of number
//! this repository has ever accepted. Wall-clock on a shared machine is not a
//! measurement; an instruction count is deterministic and diffable.
//!
//! **Ignored by default**, because a `cargo test` that shells out to valgrind
//! is not a test suite. Run them on purpose:
//!
//! ```text
//! cargo test -p nikaia --test measure -- --ignored --nocapture
//! ```
//!
//! Each test prints a table and asserts only what it is *sure* of - usually
//! that the change did not make things worse. A number that has to be argued
//! about belongs in the output, not in an assertion.

mod common;

use std::path::PathBuf;
use std::process::Command;

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// One variant of a program, and what it cost.
struct Run {
    label: &'static str,
    instructions: u64,
}

/// Lower a `.nika` benchmark to Rust.
fn lower(file: &str) -> String {
    lower_with(file, Build::default())
}

/// [`lower`], at a setting of `user_parallelism` of the caller's choosing.
fn lower_with(file: &str, how: Build) -> String {
    let path = repo_root().join("benches").join(file);
    let source =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let parsed = parse_to_ast(&source).unwrap_or_else(|e| panic!("{file} does not parse:\n{e}"));
    emit_program(&parsed, how)
        .unwrap_or_else(|e| panic!("{file} does not lower:\n{e}"))
        .rust
}

/// Compile a Rust program and count the instructions one run of it retires.
///
/// `-O`, because an unoptimised binary measures the optimiser's absence rather
/// than the change: every one of these candidates is about work `rustc` is
/// already allowed to remove, and a number taken without it can only mislead.
fn instructions(label: &'static str, rust: &str, args: &[&str]) -> Run {
    let dir = common::scratch_dir(&format!("measure-{}", label.replace([' ', ':'], "-")));
    let source = dir.join("bench.rs");
    std::fs::write(&source, rust).expect("write the Rust");

    let binary = dir.join("bench");
    let compiled = common::compile(
        &source,
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
        "{label} did not compile:\n{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let run = counted(label, &binary, args);
    let _ = std::fs::remove_dir_all(&dir);
    run
}

/// **`std` as a build for throughput links it**: optimised, and carrying the
/// bitcode link-time optimisation reads (Part III 13.3's `opt-level = 3`,
/// `lto = true`).
///
/// The rlib the tests find beside themselves is the test profile's, at
/// `opt-level = 0`, and linking a program against it is not neutral even where
/// the program calls none of it: `rustc` then takes `std`'s unoptimised copies
/// of generic code both crates instantiate - `RawVec<i64>::grow_one` among
/// them - in place of its own, which cost the sparse-row kernel 88M
/// instructions that no build of a program pays. Built once per run, into a
/// target directory of its own, so the test build's rlibs are not touched.
fn optimized_std() -> &'static [String] {
    static STD: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    STD.get_or_init(|| {
        let target = repo_root().join("target/measure-std");
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
        let built = Command::new(cargo)
            .args(["build", "--release", "-p", "nikaia-std", "--target-dir"])
            .arg(&target)
            .env("RUSTFLAGS", "-C embed-bitcode=yes")
            .env("CARGO_INCREMENTAL", "0")
            .current_dir(repo_root())
            .output()
            .expect("run cargo");
        assert!(
            built.status.success(),
            "std did not build optimised:\n{}",
            String::from_utf8_lossy(&built.stderr)
        );
        let deps = target.join("release/deps");
        let newest = |name: &str| {
            let prefix = format!("lib{name}-");
            let mut found: Vec<PathBuf> = std::fs::read_dir(&deps)
                .expect("read the deps directory")
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    let file = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                    file.starts_with(&prefix) && file.ends_with(".rlib")
                })
                .collect();
            found.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
            found
                .pop()
                .unwrap_or_else(|| panic!("no {name} rlib in {}", deps.display()))
        };
        let mut args = vec!["-L".to_string(), format!("dependency={}", deps.display())];
        for name in ["nikaia_std", "winnow_grammar", "winnow"] {
            args.push("--extern".to_string());
            args.push(format!("{name}={}", newest(name).display()));
        }
        args
    })
}

/// [`instructions`], against [`optimized_std`] and with `flags` added to `-O`.
fn instructions_optimized(label: &'static str, rust: &str, args: &[&str], flags: &[&str]) -> Run {
    let dir = common::scratch_dir(&format!("measure-{}", label.replace([' ', ':', ','], "-")));
    let source = dir.join("bench.rs");
    std::fs::write(&source, rust).expect("write the Rust");
    let binary = dir.join("bench");
    let compiled = Command::new(common::rustc())
        .args(["--edition", "2024", "--crate-type", "bin", "-O"])
        .args(optimized_std())
        .args(flags)
        .arg("-o")
        .arg(&binary)
        .arg(&source)
        .output()
        .expect("run rustc");
    assert!(
        compiled.status.success(),
        "{label} did not compile:\n{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let run = counted(label, &binary, args);
    let _ = std::fs::remove_dir_all(&dir);
    run
}

/// Run `binary` under callgrind and read the instructions it retired.
fn counted(label: &'static str, binary: &std::path::Path, args: &[&str]) -> Run {
    let run = Command::new("valgrind")
        .args([
            "--tool=callgrind",
            "--callgrind-out-file=/dev/null",
            binary.to_str().expect("utf-8 path"),
        ])
        .args(args)
        .output()
        .expect("run under callgrind - is valgrind installed?");
    assert!(run.status.success(), "{label} failed under callgrind");

    // callgrind writes `==pid== I   refs:      1,234,567` to stderr.
    let stderr = String::from_utf8_lossy(&run.stderr);
    let instructions = stderr
        .lines()
        .find_map(|line| line.split("I   refs:").nth(1))
        .map(|n| n.trim().replace(',', ""))
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or_else(|| panic!("no instruction count in callgrind's output:\n{stderr}"));
    Run {
        label,
        instructions,
    }
}

/// Print what was measured, in the shape the ADRs quote.
fn report(what: &str, workload: &str, runs: &[Run]) {
    println!("\n{what} - {workload}");
    let baseline = runs.first().expect("at least one run").instructions;
    for run in runs {
        let delta = if run.instructions == baseline {
            "baseline".to_string()
        } else {
            let change = run.instructions as f64 / baseline as f64 - 1.0;
            format!("{:+.1}%", change * 100.0)
        };
        println!(
            "  {:<28} {:>14} {}",
            run.label,
            thousands(run.instructions),
            delta
        );
    }
}

fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The template's `String::new()` against a `String::with_capacity(…)`.
///
/// `docs/history/staging-candidates.md` §3 names this as the smallest candidate and
/// says the honest part out loud: it is a small win, and saying so afterwards
/// would look like an excuse. So it is said here, before the number.
///
/// The A/B is one tree and one emitter: the "before" variant is the emitted
/// Rust with the capacity taken back out, which is exactly what the emitter
/// used to write.
#[test]
#[ignore = "shells out to valgrind; run with --ignored"]
fn a_template_reserving_its_length_against_one_that_does_not() {
    let with_capacity = lower("template.nika");
    assert!(
        with_capacity.contains("String::with_capacity("),
        "the emitter does not reserve; there is nothing to compare:\n{with_capacity}"
    );

    // The emitter before the change: `String::new()`, and the same code after.
    let plain = strip_capacity(&with_capacity);

    for rows in ["200", "2000", "20000"] {
        let runs = [
            instructions("String::new()", &plain, &[rows]),
            instructions("String::with_capacity", &with_capacity, &[rows]),
        ];
        report("one growing table", &format!("{rows} rows"), &runs);
    }

    // The other shape, and the one the reservation was proposed for: mostly
    // static markup, rendered many times. Here the compiler knows almost the
    // whole answer rather than a few dozen bytes of it.
    let with_capacity = lower("page.nika");
    let plain = strip_capacity(&with_capacity);
    for pages in ["200", "2000", "20000"] {
        let runs = [
            instructions("String::new()", &plain, &[pages]),
            instructions("String::with_capacity", &with_capacity, &[pages]),
        ];
        report("a static page, rendered", &format!("{pages} times"), &runs);
    }
}

/// `String::with_capacity(N)` back to `String::new()`, and nothing else.
fn strip_capacity(rust: &str) -> String {
    let mut out = String::with_capacity(rust.len());
    let mut rest = rust;
    while let Some(at) = rest.find("String::with_capacity(") {
        out.push_str(&rest[..at]);
        out.push_str("String::new()");
        let after = &rest[at + "String::with_capacity(".len()..];
        let close = after.find(')').expect("a call has a closing paren");
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// What escaping costs, on the workload a template puts through it.
///
/// A `std` change cannot be A/B'd by rewriting the emitted Rust the way the
/// template's reservation can - the code being measured is behind a crate
/// boundary. So this prints one number and the comparison is made the way
/// ADR-010's hasher was: the same tree, built twice, once with the patch. The
/// numbers that comparison produced are in `docs/history/staging-candidates.md` §3.
///
/// `benches/template.nika` is the right workload for it: two holes per row,
/// one that needs escaping and one that does not, so both paths through
/// `html::escape` are on the build.
#[test]
#[ignore = "shells out to valgrind; run with --ignored"]
fn what_escaping_costs() {
    let rust = lower("template.nika");
    let runs = [instructions("html::escape", &rust, &["20000"])];
    report("escaping", "20000 rows, 40000 holes", &runs);
}

/// **What `break` and `continue` are worth**, measured against the shapes the
/// language forced before them.
///
/// `benches/jumps.nika` holds both halves of every A/B in one file, so the
/// lowering, the `rustc` invocation and the optimiser's settings are shared and
/// the only difference between a pair is the construct. The argument selects
/// which half runs; both print the same three numbers, which is what says they
/// are the same program.
///
/// **What is being asked.** Not *"is a jump fast"* - it is one instruction -
/// but *"what does a program pay for not having one"*, which is a different
/// question with three different answers, one per loop shape. They are printed
/// together rather than argued about: `docs/history/break-continue-cost.md` reads them.
#[test]
#[ignore = "shells out to valgrind; run with --ignored"]
fn what_a_jump_saves_against_the_shape_that_replaces_it() {
    let rust = lower("jumps.nika");

    // Three groups, each measured on its own. `stop` and `scan` are run at
    // three sizes because what they cost is a curve; `skip` is run at three
    // because a number that does not move is worth showing not moving.
    //
    // **The first group has three members and the reason is
    // [ADR-301](../../../docs/specification/adr/adr-301.md).** When these
    // numbers were first taken, the obvious `break`-less shape -
    // `while i < n && running` - did not parse, so the baseline had to nest an
    // `if` inside the loop. It parses now, and the honest thing is to measure
    // the shape that is writable **beside** the one the published numbers were
    // taken on rather than instead of it.
    let groups: [(&str, &[(&str, &str)]); 3] = [
        (
            "a `while` that stops mid-body",
            &[
                ("a flag, and a nested `if`", "stop-flag"),
                ("a flag, and `&&` in the head", "stop-and"),
                ("`break`", "stop-break"),
            ],
        ),
        (
            "a `for` that stops early",
            &[
                ("a flag, and every turn taken", "scan-flag"),
                ("`break`", "scan-break"),
            ],
        ),
        (
            "a `for` that skips turns",
            &[
                ("the body inside an `if`", "skip-nesting"),
                ("`continue`", "skip-continue"),
            ],
        ),
    ];

    for (what, members) in groups {
        for n in ["20000", "200000", "2000000"] {
            let runs: Vec<Run> = members
                .iter()
                .map(|(label, which)| instructions(label, &rust, &[n, which]))
                .collect();
            report(what, &format!("n = {n}"), &runs);
        }
    }
}

/// **What being `async` costs a program that never overlaps** (#106).
///
/// The emitted Rust is `async` ([ADR-055](../../../docs/specification/adr/adr-055.md))
/// at both settings of `user_parallelism`, and nothing had been measured
/// against the blocking lowering it replaced. `benches/awaits.nika` reads one
/// small file `n` times, one read after the other, each three `.await`s deep.
/// The A/B is one tree and one emitter: the "blocking" variant is the emitted
/// Rust with `async` and `.await` taken out, `block_on` around `main` taken
/// out, and the read made `std::fs::read_to_string` - what the lowering wrote
/// before the waiting moved into the executor. Both start the runtime, so what
/// differs is the state machines, the executor's polling and the hand-off of
/// each read to the I/O worker.
///
/// It asserts nothing: the number is the answer, and `docs/history/runtime-cost.md`
/// quotes it.
#[test]
#[ignore = "shells out to valgrind; run with --ignored"]
fn what_an_await_costs_a_program_that_never_overlaps() {
    let file = repo_root().join("benches/awaits.nika");
    let file = file.to_str().expect("utf-8 path");
    for (setting, how) in [("no", Build::default()), ("yes", Build::parallel())] {
        let awaited = lower_with("awaits.nika", how);
        let blocking = without_the_executor(&awaited);
        // **And the two halves apart**: `async` and the executor kept, only
        // the read made blocking - what the futures cost, without the hand-off
        // of the read to the runtime's I/O.
        let futures_only = with_a_blocking_read(&awaited);
        for reads in ["100", "1000"] {
            let runs = [
                instructions("blocking", &blocking, &[reads, file]),
                instructions("async, the read blocking", &futures_only, &[reads, file]),
                instructions("async (as lowered)", &awaited, &[reads, file]),
            ];
            report(
                "a program that never overlaps",
                &format!("{reads} reads, user_parallelism = {setting}"),
                &runs,
            );
            let n: u64 = reads.parse().expect("a count");
            let futures = runs[1].instructions.saturating_sub(runs[0].instructions) / n;
            let each = runs[2].instructions.saturating_sub(runs[0].instructions) / n;
            println!(
                "  per read, for being async: {} instructions, {} of them the futures",
                thousands(each),
                thousands(futures)
            );
        }
    }
}

/// The emitted Rust as the blocking lowering wrote it: no `async`, no
/// `.await`, no `block_on`, and the read a blocking one.
fn without_the_executor(rust: &str) -> String {
    with_a_blocking_read(rust)
        .replace("async fn ", "fn ")
        .replace(".await", "")
        .replace(
            "nikaia_std::rt::exec::block_on(__nikaia_main())",
            "__nikaia_main()",
        )
}

/// The emitted Rust with the one read made `std::fs::read_to_string`, as an
/// `async fn` that never pauses where it is still awaited.
fn with_a_blocking_read(rust: &str) -> String {
    let read = "fs::read_to_string(&path, &fs::Root::Anywhere).await";
    assert!(rust.contains(read), "the read is not where it was:\n{rust}");
    let mut out = rust.replace(read, "blocking_read(&path).await");
    out.push_str(
        "\nasync fn blocking_read(path: &str) -> Result<String, io::IoError> {\n    \
         std::fs::read_to_string(path).map_err(|e| io::IoError::of(e, path))\n}\n",
    );
    out
}

/// Link-time optimisation across every crate, as `lto = true` asks Cargo for.
const LTO: &[&str] = &["-C", "lto=fat"];

/// **The solver's inner loops, lowered from Nikaia against Rust by hand**
/// ([ADR-270](../../../docs/specification/adr/adr-270.md) D8 step 1).
///
/// `benches/solver-kernels.nika` and `benches/solver-workload/kernels.rs` hold the same
/// three kernels - combining sparse rows, scanning watch lists, multiplying big
/// integers - and print the same checksums, which this checks before it counts
/// anything. What it prints is how many more instructions the lowering's
/// version retires than the hand-written one; the cause of a gap is read off
/// the lowered Rust, and is the compiler's to close.
#[test]
#[ignore = "shells out to valgrind; run with --ignored"]
fn the_solver_kernels_lowered_against_rust_by_hand() {
    let nikaia = lower("solver-kernels.nika");
    // And with every index check the solver proves unnecessary dropped
    // ([ADR-306](../../../docs/specification/adr/adr-306.md) D4).
    let proved = lower_with(
        "solver-kernels.nika",
        Build {
            bounds: nikaia::bounds::BoundsChecks::Aggressive,
            overflow: nikaia::bounds::OverflowChecks::Aggressive,
            ..Build::default()
        },
    );
    let by_hand = std::fs::read_to_string(repo_root().join("benches/solver-workload/kernels.rs"))
        .expect("benches/solver-workload/kernels.rs");

    // The same program: the same checksums from both, at a small size.
    let outputs: Vec<String> = [("nikaia", &nikaia), ("rust", &by_hand), ("proved", &proved)]
        .iter()
        .map(|(label, source)| {
            let dir = common::scratch_dir(&format!("kernels-check-{label}"));
            let file = dir.join("kernels.rs");
            std::fs::write(&file, source).expect("write the Rust");
            let binary = dir.join("kernels");
            let compiled = common::compile(
                &file,
                &[
                    "--crate-type",
                    "bin",
                    "-O",
                    "-o",
                    binary.to_str().expect("utf-8"),
                ],
            );
            assert!(
                compiled.status.success(),
                "{label} did not compile:\n{}",
                String::from_utf8_lossy(&compiled.stderr)
            );
            let run = Command::new(&binary)
                .args(["2000", "all"])
                .output()
                .expect("run");
            let _ = std::fs::remove_dir_all(&dir);
            String::from_utf8_lossy(&run.stdout).to_string()
        })
        .collect();
    assert_eq!(
        outputs[0], outputs[1],
        "the two halves are not the same program"
    );
    assert_eq!(
        outputs[0], outputs[2],
        "dropping proved checks changed what the program prints"
    );

    // Both halves link the same optimised `std`, as a build of either would,
    // and both are counted at the two settings Part III 13.3 names for
    // throughput: `-O` alone, and `-O` with link-time optimisation.
    for (kernel, n) in [("rows", "200000"), ("watch", "2000"), ("bignum", "20000")] {
        let runs = [
            instructions_optimized("Rust, by hand", &by_hand, &[n, kernel], &[]),
            instructions_optimized("Nikaia, lowered", &nikaia, &[n, kernel], &[]),
            instructions_optimized("Nikaia, checks proved", &proved, &[n, kernel], &[]),
        ];
        report(kernel, &format!("n = {n}, -O"), &runs);
        let runs = [
            instructions_optimized("Rust, by hand, lto", &by_hand, &[n, kernel], LTO),
            instructions_optimized("Nikaia, lowered, lto", &nikaia, &[n, kernel], LTO),
            instructions_optimized("Nikaia, checks proved, lto", &proved, &[n, kernel], LTO),
        ];
        report(kernel, &format!("n = {n}, -O, lto = fat"), &runs);
    }
}
