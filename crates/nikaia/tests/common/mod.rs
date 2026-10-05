//! Compiling emitted Rust from inside a test.
//!
//! A generated program links `nikaia_std`, `winnow_grammar` and `winnow`, and
//! all three are in cargo's deps directory - along with other copies of them:
//! the build-dependency graph builds the parser backend a second time under a
//! different Cargo profile, and an earlier build may have left a stale hash behind.
//! Two copies of `winnow` in one program is a type error at every `Stream`
//! bound, so guessing by modification time is not good enough.
//!
//! The copies that belong together are named in the crate metadata: an rlib
//! lists its dependencies as `name-hash`, and the hash is the one in the
//! file name. So one anchor - the newest `nikaia_std`, of which a build has
//! exactly one - and the rest is read, not guessed.
//!
//! **Read with nothing unstable.** A dependency's `extra-filename` is a plain
//! string *inside* the metadata, so the question is answered by asking it the
//! other way round rather than by asking the compiler to list anything: of the
//! candidate rlibs on disk, which one's filename hash appears in the anchor's
//! bytes. Exactly one may, and the helper refuses to guess if that is not so.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The `rustc` cargo built this test with. `build.rs` hands it over: the crates
/// in the deps directory were built by it, and any other toolchain on PATH
/// would only report that.
pub fn rustc() -> &'static str {
    env!("NIKAIA_RUSTC")
}

pub fn deps_dir() -> PathBuf {
    std::env::current_exe()
        .expect("test binary path")
        .parent()
        .expect("deps directory")
        .to_path_buf()
}

/// The newest rlib of a crate. Used for the anchor only.
fn newest_rlib(crate_name: &str) -> PathBuf {
    let mut found = candidates(crate_name);
    found.sort_by_key(|path| std::fs::metadata(path).and_then(|m| m.modified()).ok());
    found
        .pop()
        .unwrap_or_else(|| panic!("no {crate_name} rlib in {}", deps_dir().display()))
}

/// Every `lib<crate_name>-<hash>.rlib` in the deps directory.
fn candidates(crate_name: &str) -> Vec<PathBuf> {
    let prefix = format!("lib{crate_name}-");
    let mut found: Vec<_> = std::fs::read_dir(deps_dir())
        .expect("read deps directory")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            name.starts_with(&prefix) && name.ends_with(".rlib")
        })
        .collect();
    found.sort();
    found
}

/// The rlib `of` was built against for `crate_name`, from `of`'s own metadata.
///
/// An rlib's metadata records each direct dependency's `extra-filename` - the
/// `-<hash>` in its file name - as a plain string, so the one copy `of` links
/// is the one candidate whose hash occurs in `of`'s bytes. One match is the
/// answer; none or several is a deps directory this helper must not guess at.
fn dependency_of(of: &Path, crate_name: &str) -> PathBuf {
    let bytes = std::fs::read(of).unwrap_or_else(|e| panic!("read {}: {e}", of.display()));
    let candidates = candidates(crate_name);

    let mut matched: Vec<&PathBuf> = Vec::new();
    for candidate in &candidates {
        let name = candidate
            .file_name()
            .and_then(|n| n.to_str())
            .expect("rlib file name");
        let hash = name
            .trim_start_matches(&format!("lib{crate_name}"))
            .trim_end_matches(".rlib");
        if contains(&bytes, hash.as_bytes()) {
            matched.push(candidate);
        }
    }

    match matched.as_slice() {
        [one] => (*one).clone(),
        _ => panic!(
            "{} names {} of the {} `{crate_name}` rlibs in {}: {:?}",
            of.display(),
            matched.len(),
            candidates.len(),
            deps_dir().display(),
            candidates
                .iter()
                .map(|p| p.file_name().expect("file name"))
                .collect::<Vec<_>>()
        ),
    }
}

/// `haystack.contains(needle)` for bytes. An rlib is megabytes, so this looks
/// for the needle's first byte rather than comparing at every offset.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    let Some((first, rest)) = needle.split_first() else {
        return true;
    };
    let mut from = 0;
    while let Some(offset) = haystack[from..].iter().position(|byte| byte == first) {
        let at = from + offset + 1;
        match haystack.get(at..at + rest.len()) {
            Some(window) if window == rest => return true,
            Some(_) => from = at,
            None => return false,
        }
    }
    false
}

/// The `--extern` arguments for a generated program: one consistent set.
pub fn externs() -> Vec<String> {
    let std = newest_rlib("nikaia_std");
    let grammar = dependency_of(&std, "winnow_grammar");
    let winnow = dependency_of(&grammar, "winnow");

    vec![
        format!("nikaia_std={}", std.display()),
        format!("winnow_grammar={}", grammar.display()),
        format!("winnow={}", winnow.display()),
    ]
    .into_iter()
    .flat_map(|e| ["--extern".to_string(), e])
    .collect()
}

/// **The programs a corpus sweep reads in `dir`**: every `.nika` directly in
/// it, and the one file of each package there whose `src/` holds one
/// ([ADR-269](../../../../docs/specification/adr/adr-269.md) D13 moved most of
/// `examples/` into packages of a file each, with their output tests beside
/// them). A package of several files is not a standalone program - one of
/// its files names types another declares - and is left to the test that
/// builds it. Sorted, so what a sweep covers does not depend on `readdir`.
#[allow(dead_code)]
pub fn standalone_programs(dir: &Path) -> Vec<PathBuf> {
    let nika = |dir: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.extension().is_some_and(|e| e == "nika"))
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut found = nika(dir);
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if let [only] = nika(&entry.path().join("src")).as_slice() {
            found.push(only.clone());
        }
    }
    found.sort();
    found
}

/// A scratch directory of this test's own. The tests run in parallel threads
/// of one process, and two of them sharing a file name means one compiles the
/// other's code.
///
/// Inside [`run_root`], never directly in the temp directory: every test ends
/// by removing the directory it was handed, and it removes a path it *computed*
/// rather than one it was given a handle to. While the name was
/// `nikaia-<purpose>-<pid>-<n>` in the shared temp directory, the only thing
/// keeping one run off another run's path was the process id - and `pid_max` is
/// 32768 here, so a long-lived checkout gets the same id back. Under a root
/// that no other run can name, the computed path cannot belong to anybody else.
pub fn scratch_dir(purpose: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = run_root().join(format!("{purpose}-{id}"));
    std::fs::create_dir_all(&dir).expect("scratch directory");
    dir
}

/// The one directory this test binary owns, made once and named so that no
/// other run can arrive at the name.
///
/// The process id alone is not enough (it comes back around), so the name
/// carries the time the root was made as well. Two binaries of the same run
/// start in the same nanosecond about as often as they get the same pid, and
/// they cannot do both at once: the pair is what makes the name unique, not
/// either half.
///
/// **Under `target/`, not under the temp directory.** Something outside these
/// tests removes `/tmp/nikaia-*` wholesale when the disk fills: 340 directories
/// were listed and, seconds later, the same glob matched nothing. A run-scoped
/// name does not survive that - `nikaia-run-…` matches `nikaia-*` too - so the
/// tree lives beside the rlibs the tests already read instead. That directory
/// belongs to this checkout (a second worktree has its own), nothing sweeps it
/// from outside, and `cargo clean` collects it with everything else.
fn run_root() -> &'static Path {
    static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        let scratch = deps_dir()
            .parent()
            .expect("the deps directory is inside the profile directory")
            .join("nikaia-scratch");
        sweep_stale_scratch_dirs(&scratch);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or(0);
        let root = scratch.join(format!("{}-{stamp:x}", std::process::id()));
        std::fs::create_dir_all(&root).expect("scratch root");
        root
    })
}

/// Remove scratch directories a previous run left behind.
///
/// A directory is kept after the test that made it, deliberately: a failure is
/// much easier to look at with the emitted Rust and the binary still on disk.
/// Nothing removed them afterwards, so they accumulated - a run of the suite
/// leaves dozens, and they were found 891 deep and 2.1 GB heavy, with the
/// filesystem full.
///
/// **A root whose run has ended is collected at once, and the clock is only the
/// fallback.** Thirteen tests across four files create a scratch directory and
/// never remove it - `ordering`'s are 14 MB each, because they hold a compiled
/// binary - so "every test cleans up after itself" is a rule this suite has
/// already broken four times and will break again. What is reliable is that a
/// root belongs to a process, and a process either exists or does not: 578 roots
/// and 4.2 GB had built up inside one session before this asked.
///
/// So a root goes when **its process is gone**. That is exact rather than
/// cautious: a concurrent run's root is protected because its process is alive,
/// not because an hour has not passed. The clock stays for the one case liveness
/// cannot answer - a root whose process id has been handed to somebody else
/// since - and six hours rather than minutes because being wrong in that
/// direction deletes a running build's files.
///
/// Once per process, and before the root exists rather than on every
/// [`scratch_dir`] call, because a `readdir` is not free and the answer cannot
/// change usefully within one run.
///
/// `within` is the scratch tree and nothing else, so the sweep can only reach
/// roots of runs of this checkout. That is the difference from what this did
/// while the directories were loose in the temp directory: there, a prefix was
/// all that stood between the sweep and somebody else's files.
fn sweep_stale_scratch_dirs(within: &Path) {
    let Ok(entries) = std::fs::read_dir(within) else {
        // First run in a fresh `target/`: there is nothing to sweep.
        return;
    };
    let cutoff = std::time::Duration::from_secs(6 * 60 * 60);
    for entry in entries.flatten() {
        let name = entry.file_name();
        let stale = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .and_then(|at| at.elapsed().map_err(std::io::Error::other))
            .is_ok_and(|age| age > cutoff);
        if stale || run_has_ended(&name.to_string_lossy()) {
            // Best effort: another run may be removing the same directory, and
            // losing that race is not this run's problem.
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Whether the run that made a root named `<pid>-<stamp>` is over.
///
/// **Every uncertainty answers "still running"**, which is the direction that
/// keeps a live build's files: a name this does not recognise, a system with no
/// `/proc` to ask, and this run's own root all come back `false`. The cost of
/// being wrong here is deleting files a test is using; the cost of being wrong
/// the other way is a directory that waits six hours.
fn run_has_ended(root: &str) -> bool {
    if !Path::new("/proc").is_dir() {
        return false;
    }
    let Some(pid) = root.split('-').next().and_then(|p| p.parse::<u32>().ok()) else {
        return false;
    };
    if pid == std::process::id() {
        return false;
    }
    !Path::new(&format!("/proc/{pid}")).exists()
}

/// Compile emitted Rust. `args` are rustc's, after the edition and the externs
/// - `--crate-type`, `--emit`, `-O`, `--error-format`, `-o`.
pub fn compile(source: &Path, args: &[&str]) -> Output {
    Command::new(rustc())
        .args(["--edition", "2024"])
        .arg("-L")
        .arg(format!("dependency={}", deps_dir().display()))
        .args(externs())
        .args(args)
        .arg(source)
        .output()
        .expect("run rustc")
}

/// **The method this repository uses to mean "no ledger describes this"**, and
/// the one place to change it.
///
/// A test about an unresolved call needs a name nothing has written down — and
/// every real `std` name is a *candidate* for being written down, so such a test
/// breaks the day the ledger does its job. That has now happened four times
/// here: `String::push_str`, `cli::args().nth`, and `String::to_uppercase`
/// twice. Each time the fix was the same edit in a different file, found by a
/// red test rather than by looking.
///
/// So the name lives here. **When somebody writes this one down**, they will
/// break the tests that use it — which is correct, and the fix is one line:
/// point this at another method `std.contracts` does not carry, and check that
/// it still is one. What those tests measure is the *rule* that an unresolved
/// call says nothing; which name is unresolved is an accident of the day.
///
/// The better fixture would have been an absence the **language** decides rather
/// than one the ledger merely has not filled —
/// [ADR-024](../../../../docs/specification/adr/adr-024.md) D4's erased generic.
/// That road is closed rather than waiting:
/// [ADR-295](../../../../docs/specification/adr/adr-295.md) D5 made a member on
/// a parameter with no bound `NK1126`, so the erasure is now a **refusal** and
/// not an unknown. This constant stays, and the day somebody writes
/// `insert_str` down it moves rather than goes.
pub const UNDESCRIBED_METHOD: &str = "insert_str";

/// The same, as a call on a `String` receiver: `s.insert_str(0, "x")`.
pub fn undescribed_call(receiver: &str) -> String {
    format!("{receiver}.{UNDESCRIBED_METHOD}(0, \"x\")")
}

/// An undescribed method that **hands back a `String`**, for a test that needs
/// a *value* the checker cannot type rather than a statement it cannot type.
///
/// `insert_str` above yields nothing, so it cannot stand where a value is
/// wanted. `to_ascii_uppercase` is in neither `String`'s entries nor `str`'s,
/// so a call on a `String` receiver is `?` and still compiles, which is what a
/// test that lowers *and* runs needs.
///
/// **Here rather than written into a test**, for the reason the constant above
/// is: five tests have broken on a name somebody wrote down, and the fifth was
/// `.clone()` — `nullable.rs` used `self.name.clone()` as its example of a value
/// nothing could type, and
/// [ADR-083](../../../../docs/specification/adr/adr-083.md) put `String::clone`
/// in the ledger because `NK1131`'s advice needed it. That is the ledger getting
/// better and a test measuring the wrong thing, which is exactly the pair this
/// file exists to keep apart. The sixth was `repeat`: it was `str`'s only, until
/// [ADR-282](../../../../docs/specification/adr/adr-282.md) D18 let a `String`
/// receiver find `str`'s entries.
pub const UNDESCRIBED_VALUE_METHOD: &str = "to_ascii_uppercase";

/// `s.to_ascii_uppercase()` — an undescribed call that yields a `String`.
pub fn undescribed_value(receiver: &str) -> String {
    format!("{receiver}.{UNDESCRIBED_VALUE_METHOD}()")
}

// --- build-time code ----------------------------------------------------------

/// **One workshop for every test**, as a build keeps one beside its cache: a
/// `comptime` or a default that calls something is compiled and run there
/// ([ADR-321](../../../docs/specification/adr/adr-321.md) D1), and the library
/// it links against is built once.
pub fn workshop() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("comptime-compiled")
}

/// What a build may read, with the workshop it builds in.
pub fn reads() -> nikaia::assets::Reads {
    nikaia::assets::Reads::at(workshop()).building_in(workshop())
}

/// The ledger as a build infers it, its defaults compiled in the workshop.
pub fn infer(parsed: &nikaia::parser::Parsed) -> nikaia::contracts::Ledger {
    use nikaia::contracts::LedgerOps;
    nikaia::comptime_run::defaults_compiled_in(Some(&workshop()), || {
        nikaia::contracts::Ledger::infer(parsed)
    })
}

/// The check a build runs, with the workshop.
pub fn checked(
    parsed: &nikaia::parser::Parsed,
    own: &nikaia::contracts::Ledger,
    library: &nikaia::contracts::Ledger,
) -> nikaia::check::Checked {
    nikaia::check::check_against(
        parsed,
        &[],
        own,
        library,
        &std::collections::BTreeSet::new(),
        &nikaia::check::Newly::new(),
        &reads(),
    )
}
