//! The build cache, driven by the real emitter (ADR-021).
//!
//! `orchestrator` unit-tests the key's dimensions against synthetic
//! records. What it cannot check from there is the thing the ADR is actually
//! worried about: that the artifacts the cache hands back are the ones the
//! emitter would have produced. That needs a real `.nika` file whose lowering
//! *differs* between the builds, or the check passes without checking
//! anything - which is the failure mode D5 names by its own test,
//! `the_profiles_agree_on_every_example`.

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;
use orchestrator::cache::{Artifacts, Cache, Choices};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The tests store only the emitted Rust; what a real build also puts in the
/// envelope (ADR-020's ledger) is exercised through the binary further down.
fn rust(emitted: &str) -> Artifacts {
    Artifacts::new().with("rust", emitted)
}

/// Lowers `source` the way the `rust` backend does.
fn lower(source: &str, build: Build) -> String {
    let parsed = parse_to_ast(source).expect("parse");
    emit_program(&parsed, build).expect("emit").rust
}

fn cache_in(dir: &std::path::Path) -> Cache {
    Cache::open(
        dir.join("nikaia.lock"),
        dir.join("target/nikaia/cache"),
        "test-toolchain",
        "test-compiler",
    )
    .expect("open cache")
}

/// An example that lowers differently at either `user_parallelism`, so that a cache
/// which confused the two would be caught. `1brc.nika` is the one the
/// specification leans on for exactly this difference (ADR-009's `par_fold`).
fn a_switch_sensitive_example() -> String {
    let path = repo_root().join("examples/1brc.nika");
    let source = std::fs::read_to_string(&path).expect("1brc.nika is readable");
    assert_ne!(
        lower(&source, Build::default()),
        lower(&source, Build::parallel()),
        "this test is only meaningful while {} lowers differently per build; \
         if that changed, pick another example rather than deleting the check",
        path.display()
    );
    source
}

#[test]
fn an_unchanged_unit_comes_back_from_the_cache_unchanged() {
    let dir = common::scratch_dir("cache-roundtrip");
    let source = a_switch_sensitive_example();
    let choices = Choices::new("x86_64-linux/auto", "rust");
    let expected = lower(&source, Build::default());

    let mut cache = cache_in(&dir);
    assert!(
        cache.lookup("1brc.nika", &source, &choices, &dir).is_none(),
        "nothing is cached before the first build"
    );

    cache
        .record(
            "1brc.nika",
            &source,
            BTreeMap::new(),
            &choices,
            &rust(&expected),
        )
        .expect("record");

    assert_eq!(
        cache
            .lookup("1brc.nika", &source, &choices, &dir)
            .as_ref()
            .and_then(|artifacts| artifacts.get("rust")),
        Some(expected.as_str()),
        "the cached artifact is what the emitter produced"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// ADR-021 D5. The build is in the key, so each build gets its own entry
/// and neither is ever served the other's.
#[test]
fn two_settings_never_serve_each_others_artifacts() {
    let dir = common::scratch_dir("cache-builds");
    let source = a_switch_sensitive_example();

    let sequential = Choices::new("x86_64-linux/0", "rust");
    let parallel = Choices::new("x86_64-linux/auto", "rust");
    let lowered_sequential = lower(&source, Build::default());
    let lowered_parallel = lower(&source, Build::parallel());

    let mut cache = cache_in(&dir);
    cache
        .record(
            "1brc.nika",
            &source,
            BTreeMap::new(),
            &sequential,
            &rust(&lowered_sequential),
        )
        .expect("record sequential");
    cache
        .record(
            "1brc.nika",
            &source,
            BTreeMap::new(),
            &parallel,
            &rust(&lowered_parallel),
        )
        .expect("record parallel");

    // Same unit, same source, two builds - and each has to come back as
    // itself. Recording the second must not have displaced the first.
    assert_eq!(
        cache
            .lookup("1brc.nika", &source, &sequential, &dir)
            .as_ref()
            .and_then(|artifacts| artifacts.get("rust")),
        Some(lowered_sequential.as_str()),
        "the sequential build is served its own artifact"
    );
    assert_eq!(
        cache
            .lookup("1brc.nika", &source, &parallel, &dir)
            .as_ref()
            .and_then(|artifacts| artifacts.get("rust")),
        Some(lowered_parallel.as_str()),
        "the parallel build is served its own artifact"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// A changed source is a different build, and the previous artifact must not
/// answer for it.
#[test]
fn an_edited_source_misses() {
    let dir = common::scratch_dir("cache-edit");
    let source = a_switch_sensitive_example();
    let choices = Choices::new("x86_64-linux/auto", "rust");

    let mut cache = cache_in(&dir);
    cache
        .record(
            "1brc.nika",
            &source,
            BTreeMap::new(),
            &choices,
            &rust(&lower(&source, Build::default())),
        )
        .expect("record");

    let edited = format!("{source}\n// a comment is still a change\n");
    assert!(
        cache.lookup("1brc.nika", &edited, &choices, &dir).is_none(),
        "an edited source must not be answered by the old artifact"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// **A file the build read, changed** ([ADR-072](../../../docs/specification/adr/adr-072.md)
/// D7, [ADR-021](../../../docs/specification/adr/adr-021.md) D13).
///
/// The asset dimension travelled through the key for a year with nothing
/// producing it; `asset("…")` is what produces it now. This is the half the
/// unit tests over synthetic records could not reach — a recorded asset is
/// re-hashed **as it is on disk now**, so editing the file is a miss without
/// the source changing at all.
#[test]
fn an_edited_asset_misses_although_the_source_did_not_change() {
    let dir = common::scratch_dir("cache-asset");
    let source = a_switch_sensitive_example();
    let choices = Choices::new("x86_64-linux/auto", "rust").reading("digest-of-the-list");
    std::fs::write(dir.join("config.txt"), "one\n").expect("write the asset");

    let mut cache = cache_in(&dir);
    let assets = BTreeMap::from([(
        "config.txt".to_string(),
        orchestrator::cache::sha256_hex(b"one\n"),
    )]);
    cache
        .record(
            "1brc.nika",
            &source,
            assets,
            &choices,
            &rust(&lower(&source, Build::default())),
        )
        .expect("record");

    assert!(
        cache.lookup("1brc.nika", &source, &choices, &dir).is_some(),
        "the same bytes on disk are a hit"
    );
    std::fs::write(dir.join("config.txt"), "two\n").expect("edit the asset");
    assert!(
        cache.lookup("1brc.nika", &source, &choices, &dir).is_none(),
        "a build that read a file which has since changed must not keep the old answer"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// **The allowlist is a dimension of its own** (D7).
///
/// It is a file the build read, so it belongs in the key beside the files it
/// names — and it belongs with the *switches* rather than among the assets,
/// because it binds the whole build (D6). The empty digest is a build with **no
/// list**, which is a different build from one with a list: the first cannot
/// read at all, so the two must never serve each other's artifacts.
#[test]
fn two_allowlists_never_serve_each_others_artifacts() {
    let dir = common::scratch_dir("cache-allowlist");
    let source = a_switch_sensitive_example();
    let none = Choices::new("x86_64-linux/auto", "rust");
    let listed = Choices::new("x86_64-linux/auto", "rust").reading("one-digest");
    let other = Choices::new("x86_64-linux/auto", "rust").reading("another-digest");

    let mut cache = cache_in(&dir);
    cache
        .record(
            "1brc.nika",
            &source,
            BTreeMap::new(),
            &listed,
            &rust(&lower(&source, Build::default())),
        )
        .expect("record");

    assert!(cache.lookup("1brc.nika", &source, &listed, &dir).is_some());
    assert!(
        cache.lookup("1brc.nika", &source, &none, &dir).is_none(),
        "a build with no list is not the build that had one"
    );
    assert!(
        cache.lookup("1brc.nika", &source, &other, &dir).is_none(),
        "a changed line in the list must invalidate"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// The lockfile is the committed record (D2), and the build-time choices are
/// deliberately not in it (D5) - otherwise switching build would rewrite a
/// tracked file for a diff that means nothing.
#[test]
fn the_lockfile_survives_a_round_trip_and_holds_no_choices() {
    let dir = common::scratch_dir("cache-lockfile");
    let source = a_switch_sensitive_example();

    let mut cache = cache_in(&dir);
    cache
        .record(
            "1brc.nika",
            &source,
            BTreeMap::new(),
            &Choices::new("x86_64-linux/0", "rust"),
            &rust(&lower(&source, Build::default())),
        )
        .expect("record");
    cache.save().expect("save");

    let text = std::fs::read_to_string(dir.join("nikaia.lock")).expect("lockfile written");
    assert!(text.contains("1brc.nika"), "the unit is recorded:\n{text}");
    assert!(
        !text.contains("user_parallelism"),
        "the build is a choice and must not be recorded:\n{text}"
    );

    // Reopening sees the same units, so a second invocation can look up what
    // the first recorded.
    let reopened = cache_in(&dir);
    assert_eq!(
        reopened.lockfile().units.keys().collect::<Vec<_>>(),
        vec!["1brc.nika"]
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// The CLI's half of the bargain, and the reason the cache can be a default at
/// all: a cache that cannot be written slows the next build down and never
/// fails this one. Driven through the real binary, because the decision that
/// a cache error is not fatal lives in `main.rs` and nowhere else.
///
/// The store is aimed below a *file*, which no process can create a directory
/// inside - including root, so this holds in a container too.
#[test]
fn a_broken_cache_does_not_break_the_build() {
    let dir = common::scratch_dir("cache-degrades");
    let input = dir.join("hello.nika");
    std::fs::copy(repo_root().join("tests/samples/hello_world.nika"), &input).expect("sample");
    let blocker = dir.join("blocker");
    std::fs::write(&blocker, "not a directory").expect("blocker");
    let output = dir.join("out.rs");

    let run = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .args(["lower", input.to_str().unwrap()])
        .args(["--output", output.to_str().unwrap()])
        .env("NIKAIA_CACHE_DIR", blocker.join("cache"))
        .output()
        .expect("the nikaia binary runs");

    assert!(
        run.status.success(),
        "an unusable cache must not fail the build\nstderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(
        String::from_utf8_lossy(&run.stderr).contains("cache"),
        "and it should say so rather than failing silently: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let emitted = std::fs::read_to_string(&output).expect("the Rust is written anyway");
    assert!(!emitted.is_empty(), "and it is not empty");

    std::fs::remove_dir_all(&dir).ok();
}

/// Caching is on without asking, and leaves the source directory alone when
/// there is no project to own a lockfile.
#[test]
fn the_cache_is_on_by_default_and_writes_nothing_beside_the_source() {
    let dir = common::scratch_dir("cache-default-on");
    let src = dir.join("src");
    let home = dir.join("cache");
    std::fs::create_dir_all(&src).expect("dirs");
    let input = src.join("hello.nika");
    std::fs::copy(repo_root().join("tests/samples/hello_world.nika"), &input).expect("sample");

    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_nikaia"))
            .args(["lower", input.to_str().unwrap()])
            .args(args)
            .env("NIKAIA_CACHE_DIR", &home)
            .output()
            .expect("the nikaia binary runs");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    assert!(
        !run(&[]).contains("from cache"),
        "the first run has to lower"
    );
    assert!(
        run(&[]).contains("from cache"),
        "the second must be served without being asked to be"
    );
    assert!(
        !run(&["--no-cache"]).contains("from cache"),
        "and --no-cache is the way out"
    );

    // The whole point of the default: the build's own outputs are welcome
    // beside the source - the emitted Rust, and the ledger ADR-020 writes
    // where a build puts what it produced, with the record of what it was
    // derived from beside it (ADR-251 D1) - but nothing belonging to the
    // *cache* may appear there when no project owns a lockfile.
    let mut left: Vec<String> = std::fs::read_dir(&src)
        .expect("read src")
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(
        left,
        vec![
            "hello.nika",
            "hello.rs",
            "nikaia.contracts",
            "nikaia.derived"
        ],
        "only the source and what the build produced"
    );
    assert!(
        !src.join("nikaia.lock").exists() && !src.join("target").exists(),
        "no cache artifact may be written beside the source outside a project"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// A hit skips the parse, the `sync` check, the lowering and the inference -
/// but the ledger still lands, because it is in the envelope with the Rust
/// (ADR-021 D13), and `--locked` still runs, because "does the committed file
/// still match" is the user's question and not the compiler's.
#[test]
fn a_hit_still_writes_the_ledger_and_still_answers_locked() {
    let dir = common::scratch_dir("cache-hit-ledger");
    let home = dir.join("cache");
    let input = dir.join("hello.nika");
    std::fs::copy(repo_root().join("tests/samples/hello_world.nika"), &input).expect("sample");
    let output = dir.join("out.rs");
    let ledger = dir.join("nikaia.contracts");

    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_nikaia"))
            .args(["lower", input.to_str().unwrap()])
            .args(["--output", output.to_str().unwrap()])
            .args(args)
            .env("NIKAIA_CACHE_DIR", &home)
            .output()
            .expect("the nikaia binary runs")
    };

    let first = run(&[]);
    assert!(first.status.success());
    let fresh = std::fs::read_to_string(&ledger).expect("a fresh build writes the ledger");

    // Remove it, so the second run can only produce it from the envelope.
    std::fs::remove_file(&ledger).expect("remove");

    let second = run(&[]);
    assert!(second.status.success());
    assert!(
        String::from_utf8_lossy(&second.stdout).contains("from cache"),
        "the second run has to be a hit for this test to mean anything"
    );
    assert_eq!(
        std::fs::read_to_string(&ledger).expect("a cached build writes it too"),
        fresh,
        "the ledger comes back byte for byte, from the cache rather than from inference"
    );

    // And --locked is still a real check on a hit: tamper with the committed
    // file and the build has to refuse.
    std::fs::write(&ledger, format!("{fresh}\n# hand-edited\n")).expect("tamper");
    let locked = run(&["--locked"]);
    assert!(
        !locked.status.success(),
        "--locked must still fail on a cache hit\nstdout: {}",
        String::from_utf8_lossy(&locked.stdout)
    );

    // Restored, --locked passes again - so the failure was the tampering and
    // not the cache path being broken.
    std::fs::write(&ledger, &fresh).expect("restore");
    assert!(
        run(&["--locked"]).status.success(),
        "--locked passes once the committed ledger matches again"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// **What the output names is part of the key** (0.0.239). The abort table
/// names each file as it was handed to the compiler (ADR-044 D1), and the key
/// named it relative to its root - so `nikaia lower /abs/one.nika` followed
/// by `nikaia lower one.nika` served the first lowering to the second, and
/// an abort in it named a path the user never wrote. Each form lowers once and
/// names itself; the same form again is still a hit.
#[test]
fn a_file_named_another_way_is_not_served_the_other_name() {
    let dir = common::scratch_dir("cache-named");
    std::fs::write(
        dir.join("one.nika"),
        "fn main() {\n    let z = 1\n    println(f\"{z}\")\n}\n",
    )
    .expect("source");
    let cache = dir.join("cache");
    let lower = |input: &str| {
        let run = Command::new(env!("CARGO_BIN_EXE_nikaia"))
            .current_dir(&dir)
            .args(["lower", input, "--output", "one.rs"])
            .env("NIKAIA_CACHE_DIR", &cache)
            .output()
            .expect("the nikaia binary runs");
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        let said = String::from_utf8_lossy(&run.stdout).to_string();
        let rust = std::fs::read_to_string(dir.join("one.rs")).expect("the Rust");
        (said.contains("from cache"), rust)
    };
    let absolute = dir.join("one.nika").to_string_lossy().to_string();
    let (hit, rust) = lower(&absolute);
    assert!(!hit);
    assert!(rust.contains(&format!("{absolute:?}")), "{rust}");
    let (hit, rust) = lower("one.nika");
    assert!(
        !hit,
        "the relative name was served the absolute one's lowering"
    );
    assert!(
        rust.contains("\"one.nika\"") && !rust.contains(&absolute),
        "{rust}"
    );
    let (hit, _) = lower("one.nika");
    assert!(hit, "the same name again is a hit");
    std::fs::remove_dir_all(&dir).ok();
}
