//! A project build, end to end (ADR-002 D1).
//!
//! `nikaia.toml` becomes a `Cargo.toml`, `cargo` builds it, and what comes out
//! runs. The parts that can be checked without compiling anything - the
//! translation itself - are checked through the library; the parts that cannot
//! are driven through the real binary, because "produces a running binary" is
//! not a claim a unit test can make.
//!
//! The cargo-driven tests share one `NIKAIA_CACHE_DIR` under the workspace's
//! own `target/`. That is the **compiled-`std` cache** of ADR-002 D4 and not a
//! test convenience: each project would otherwise build `std` and everything
//! under it again from nothing, and the sharing is the decision being tested.
//! `CARGO_TARGET_DIR` is deliberately *not* set, because setting it is the one
//! way to switch that cache off.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use nikaia::project::Project;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// One cache, shared by every test here that actually runs `cargo` - and not the
/// developer's real one, which a test has no business writing into.
fn shared_cache_dir() -> PathBuf {
    repo_root().join("target").join("nikaia-project-tests")
}

/// A project directory with a manifest and an entry point (Part III 13.1).
fn a_project(purpose: &str, manifest: &str, entry: &str) -> PathBuf {
    let dir = common::scratch_dir(purpose);
    std::fs::create_dir_all(dir.join("src")).expect("src");
    std::fs::write(dir.join("nikaia.toml"), manifest).expect("manifest");
    std::fs::write(dir.join("src/main.nika"), entry).expect("entry");
    dir
}

const HELLO: &str = "fn main() {\n    println(\"Hallo Welt, Nikaia!\")\n}\n";

fn nikaia(args: &[&str], dir: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .args(args)
        .arg("--project")
        .arg(dir)
        .env("NIKAIA_CACHE_DIR", shared_cache_dir())
        // **The cache these tests check is the one a user gets**, so the
        // variable that overrides it is not handed on from whoever runs the
        // suite. `scripts/check-floor.sh` sets it for its own build, and a
        // project build honours it, as it must (`project.rs`, *`CARGO_TARGET_DIR`
        // still wins*): the compiled `std` went there, and
        // `a_second_project_links_the_std_the_first_one_built` failed on
        // every CI run since the floor job was added.
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("the nikaia binary runs")
}

fn said(output: &std::process::Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The whole of D1 in one test: a manifest is translated, `cargo` builds what
/// comes out, and the binary runs and prints what the program says.
///
/// Then the source is edited and the *new* output has to appear. That second
/// half is not a formality - Cargo decides freshness from the dependency file
/// `rustc` wrote, and `rustc` only ever saw the generated Rust, so without the
/// sources being put back into it an edited program would leave the package
/// fresh and the binary stale.
#[test]
fn a_project_builds_runs_and_notices_an_edit() {
    let dir = a_project(
        "project-run",
        "[package]\nname = \"greeter\"\nversion = \"0.2.0\"\n",
        HELLO,
    );

    let run = nikaia(&["run"], &dir);
    assert!(run.status.success(), "{}", said(&run));
    assert!(
        String::from_utf8_lossy(&run.stdout).contains("Hallo Welt, Nikaia!"),
        "the built binary runs and prints what the program says: {}",
        said(&run)
    );

    // The manifests Cargo was handed, and the ledger a build owes (13.5). The
    // root is the workspace and the package is a member of it, even where it is
    // the only one (ADR-053 D1).
    let root = std::fs::read_to_string(dir.join("target/nikaia/build/Cargo.toml"))
        .expect("the workspace root is written");
    assert!(root.contains("members = [\"greeter\"]"), "{root}");

    let generated = std::fs::read_to_string(dir.join("target/nikaia/build/greeter/Cargo.toml"))
        .expect("the translated manifest is written");
    assert!(generated.contains("name = \"greeter\""), "{generated}");
    assert!(
        generated.contains("src/main.nika"),
        "Cargo is told the `.nika` file is the crate root, which is what gives \
         the wrapper something to intercept:\n{generated}"
    );
    assert!(
        dir.join("nikaia.contracts").is_file(),
        "the ledger is written in the project root"
    );

    std::fs::write(
        dir.join("src/main.nika"),
        "fn main() {\n    println(\"zweiter Versuch\")\n}\n",
    )
    .expect("edit");

    let again = nikaia(&["run"], &dir);
    assert!(again.status.success(), "{}", said(&again));
    assert!(
        String::from_utf8_lossy(&again.stdout).contains("zweiter Versuch"),
        "an edited source reaches the binary: {}",
        said(&again)
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// A program is every file of its package (Part I 9.1), and all of them have to
/// reach Cargo's dependency info - not just the one Cargo was told about.
/// Editing a file that is **not** the entry is the case that separates "the
/// sources go back in" from "the entry does".
#[test]
fn editing_a_second_file_rather_than_the_entry_reaches_the_binary() {
    let dir = a_project(
        "project-modules",
        "[package]\nname = \"multi\"\nversion = \"0.1.0\"\n",
        "fn main() {\n    println(doubled(21))\n}\n",
    );
    std::fs::write(
        dir.join("src/utils.nika"),
        "pub fn doubled(n: i32) -> i32 {\n    return n * 2\n}\n",
    )
    .expect("module");

    let run = nikaia(&["run"], &dir);
    assert!(run.status.success(), "{}", said(&run));
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "42");

    std::fs::write(
        dir.join("src/utils.nika"),
        "pub fn doubled(n: i32) -> i32 {\n    return n * 3\n}\n",
    )
    .expect("edit the second file");

    let again = nikaia(&["run"], &dir);
    assert!(again.status.success(), "{}", said(&again));
    assert_eq!(
        String::from_utf8_lossy(&again.stdout).trim(),
        "63",
        "a file Cargo was never told about still invalidates the build: {}",
        said(&again)
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// A backend diagnostic reaches the user against the `.nika` line that caused
/// it, and not against the generated Rust (ADR-005 D7, Part III C.1).
///
/// **The project path had no interception at all.** `nikaia build` shelled out
/// to `cargo` and handed its stderr to the terminal, so everything `rustc` said
/// arrived as Rust about `target/nikaia/gen/….rs` - a file the author has never
/// read - which C.1's Iron Rule calls a bug in this compiler. The machinery to
/// place it existed and was only reachable through `--explain`; what was missing
/// was the channel, and `cargo --message-format=json` is it.
///
/// The program here is one the frontend says nothing about and `rustc` refuses:
/// `Ty::Unknown` means the checker does not guess at what a method on an integer
/// is, so this is exactly the shape that used to escape to the terminal.
#[test]
fn a_backend_error_is_reported_against_the_nika_line() {
    let dir = a_project(
        "project-backend-error",
        "[package]\nname = \"refused\"\nversion = \"0.1.0\"\n",
        "fn main() {\n    let n = 1\n    println(f\"{n.frobnicate()}\")\n}\n",
    );

    let built = nikaia(&["build"], &dir);
    assert!(!built.status.success(), "{}", said(&built));

    let complaint = said(&built);
    assert!(
        complaint.contains("src/main.nika:3:"),
        "the message names the `.nika` line: {complaint}"
    );
    assert!(
        complaint.contains("n.frobnicate()"),
        "and shows it, with a caret under it: {complaint}"
    );
    assert!(
        !complaint.contains("target/nikaia/gen/refused.rs"),
        "and no longer names the generated Rust: {complaint}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// A machine the toolchain cannot build for is refused before `cargo` is
/// started, with what is missing (ADR-037 D1). It never emits code for a
/// different machine than the one named.
#[test]
fn a_machine_the_toolchain_cannot_build_for_is_refused() {
    let dir = a_project(
        "project-wasm",
        "[package]\nname = \"wasmish\"\nversion = \"0.1.0\"\n\n\
         [build]\ntarget = \"wasm32-unknown\"\n",
        HELLO,
    );

    let error = Project::open(&dir, None, None).expect_err("wasm is not buildable yet");
    let text = format!("{error:#}");
    assert!(text.contains("wasm32-unknown"), "{text}");
    assert!(
        dir.join("target").try_exists().is_ok() && !dir.join("target/nikaia/build").exists(),
        "and nothing was generated on the way to failing"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// **The machine named is the machine built for** (ADR-037 D1): the triple
/// reaches Cargo as `--target`, which is what puts Cargo's output under a
/// directory named for it. Until 0.0.218 it was printed and never passed, so a
/// build was for whatever machine it ran on - ARM code under the name
/// `x86_64-linux` on an ARM machine - and this directory never appeared.
///
/// `CARGO_TARGET_DIR` is set here, where the other builds leave it alone,
/// because it is the one way to know where Cargo wrote without asking it.
#[test]
fn the_triple_of_the_target_is_handed_to_cargo() {
    use nikaia::emit::Target;

    let host = Target::host().expect("the suite runs on a machine that is a target");
    let dir = a_project(
        "project-triple",
        "[package]\nname = \"triple\"\nversion = \"0.1.0\"\n",
        HELLO,
    );
    let out = dir.join("cargo-target");

    let built = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .args(["build", "--target", host.name(), "--project"])
        .arg(&dir)
        .env("NIKAIA_CACHE_DIR", shared_cache_dir())
        .env("CARGO_TARGET_DIR", &out)
        .output()
        .expect("the nikaia binary runs");
    assert!(built.status.success(), "{}", said(&built));
    assert!(
        out.join(host.triple()).is_dir(),
        "Cargo built for `{}`, so its output is under that name in {}",
        host.triple(),
        out.display()
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Every target a manifest can name, and the default: the machine the compiler
/// runs on, so that a build that names nothing needs no toolchain but its own.
#[test]
fn aarch64_linux_is_a_target_and_the_default_is_the_host() {
    use nikaia::emit::Target;

    let arm = Target::parse("aarch64-linux").expect("a target that exists");
    assert_eq!(arm.triple(), "aarch64-unknown-linux-gnu");
    assert!(arm.has_threads() && arm.has_runtime() && arm.unbuildable().is_none());

    let error = Target::parse("riscv64-linux").expect_err("not a target");
    assert!(format!("{error:#}").contains("aarch64-linux"), "{error:#}");

    let dir = a_project(
        "project-default-target",
        "[package]\nname = \"host\"\nversion = \"0.1.0\"\n",
        HELLO,
    );
    let project = Project::open(&dir, None, None).expect("the project opens");
    assert_eq!(Some(project.settings.build.target), Target::host());

    std::fs::remove_dir_all(&dir).ok();
}

/// `RUSTC_WORKSPACE_WRAPPER` is applied to workspace members only, and that is
/// the reason it was chosen over `RUSTC_WRAPPER`: a crate from crates.io is
/// compiled by the real `rustc` with nothing in between.
///
/// The claim is invisible in the output - a dependency passed *through* the
/// wrapper and one that never reached it produce the same rlib - so the wrapper
/// is asked to write down what Cargo handed it, and the trace is the evidence.
///
/// Needs the network the first time, like any Cargo build of a project with a
/// dependency.
#[test]
fn a_crates_io_dependency_never_reaches_the_wrapper() {
    let dir = a_project(
        "project-crates-io",
        "[package]\nname = \"fetcher\"\nversion = \"0.1.0\"\n\n\
         [dependencies]\nregex = { type = \"rust\", version = \"1.5\" }\n",
        HELLO,
    );
    let trace = dir.join("wrapper.log");

    let built = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .args(["build", "--project"])
        .arg(&dir)
        .env("NIKAIA_CACHE_DIR", shared_cache_dir())
        .env("NIKAIA_WRAPPER_TRACE", &trace)
        .output()
        .expect("the nikaia binary runs");
    assert!(built.status.success(), "{}", said(&built));

    // Cargo resolved the dependency from crates.io, which is the half of D1
    // this toolchain deliberately does not implement itself.
    let lock = std::fs::read_to_string(dir.join("target/nikaia/build/Cargo.lock"))
        .expect("Cargo wrote a lockfile for the generated package");
    assert!(
        lock.contains("name = \"regex\""),
        "the dependency was resolved by Cargo:\n{lock}"
    );

    let trace = std::fs::read_to_string(&trace).expect("the wrapper wrote a trace");
    assert!(
        trace.lines().any(|line| line == "fetcher lowered"),
        "the project's own crate went through the Nikaia pipeline:\n{trace}"
    );
    assert!(
        !trace.contains("regex"),
        "a dependency from crates.io is never handed to this compiler at all - \
         that is what `RUSTC_WORKSPACE_WRAPPER` buys:\n{trace}"
    );
    assert_eq!(
        trace.lines().filter(|l| l.ends_with(" lowered")).count(),
        1,
        "exactly one crate in the build is Nikaia's:\n{trace}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// `nikaia.lock` records the versions Cargo resolved (ADR-021 D2), and the same
/// project resolved twice is the same bytes.
///
/// D2 has asked for this since it was written and it was not there. Since
/// ADR-002 D1 the versions exist - a project build generates a `Cargo.toml` and
/// Cargo resolves a real `Cargo.lock` beside it - so the record was the only
/// missing part, and without it the file whose job is to answer *does this build
/// the same thing for you as for me?* answered only about the Nikaia half.
///
/// Three claims, and the middle one is the reason it is worth a test over a real
/// dependency rather than a fixture:
///
/// 1. the **resolved** version is recorded, not the constraint the author wrote
///    - a measurement, not a claim (D4);
/// 2. the **whole graph**, including a crate nobody wrote down: `regex-syntax`
///    is not in `nikaia.toml` and a transitive crate that resolved differently
///    on two machines is a different build;
/// 3. the file is **byte-identical** for two independent resolutions in two
///    processes, with nothing absolute in it (ADR-005 D8). Two processes is the
///    load-bearing half: a hash map's iteration order is randomised per
///    process, so a same-process comparison would pass over exactly the slip
///    that is easiest to make.
///
/// Needs the network the first time, like the wrapper-trace test above.
#[test]
fn the_lockfile_records_what_cargo_resolved_and_does_so_identically_twice() {
    const MANIFEST: &str = "[package]\nname = \"locker\"\nversion = \"0.1.0\"\n\n\
                            [dependencies]\nregex = { type = \"rust\", version = \"1.5\" }\n";

    let mut locks = Vec::new();
    let mut dirs = Vec::new();
    for _ in 0..2 {
        let dir = a_project("project-lockfile", MANIFEST, HELLO);
        let built = nikaia(&["build"], &dir);
        assert!(built.status.success(), "{}", said(&built));

        let text = std::fs::read_to_string(dir.join("nikaia.lock"))
            .expect("a project build writes nikaia.lock in the root");
        locks.push(text);
        dirs.push(dir);
    }

    let lock = &locks[0];
    let parsed: toml::Value = toml::from_str(lock).expect("the lockfile is valid TOML");
    let dependencies = parsed
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .unwrap_or_else(|| panic!("nikaia.lock records no resolved dependencies:\n{lock}"));

    let regex = dependencies["regex"]
        .as_array()
        .expect("a list of versions, because one name can resolve twice");
    let resolved = regex[0].as_str().expect("a version string");
    assert_eq!(regex.len(), 1, "one `regex` in this graph: {regex:?}");
    assert_ne!(
        resolved, "1.5",
        "`1.5` is what the manifest asked for; the lock records what that \
         turned into (ADR-021 D4)"
    );
    assert_eq!(
        resolved.split('.').count(),
        3,
        "an exact version, not a requirement: {resolved}"
    );
    assert!(
        dependencies.contains_key("regex-syntax"),
        "the whole resolved graph, not the two lines the author wrote - a \
         transitive crate is as much a part of what was built:\n{lock}"
    );

    // D8: nothing absolute, and nothing that moves with the checkout.
    for dir in &dirs {
        assert!(
            !lock.contains(&dir.display().to_string()),
            "an absolute path reached the committed record:\n{lock}"
        );
    }

    assert_eq!(
        locks[0], locks[1],
        "two independent resolutions of one project, in two processes, must be \
         the same bytes (ADR-005 D8)"
    );

    for dir in dirs {
        std::fs::remove_dir_all(dir).ok();
    }
}

/// The translation, without building anything: what `[dependencies]` and
/// `[build.<target>]` become.
#[test]
fn the_manifest_becomes_a_cargo_manifest() {
    let dir = a_project(
        "project-translate",
        "[package]\nname = \"hyper-core\"\nversion = \"0.1.0\"\n\n\
         [dependencies]\nregex = { type = \"rust\", version = \"1.5\" }\n\n\
         [build.x86_64-linux]\nopt-level = 3\nlto = true\n\n\
         [build.wasm32-unknown]\nopt-level = \"z\"\n",
        HELLO,
    );

    // Named, because the default is the machine this runs on and the table
    // asserted below is `x86_64-linux`'s.
    let project = Project::open(&dir, Some("x86_64-linux"), None).expect("the project opens");
    let members = project.members().expect("the packages resolve");
    let workspace = project
        .cargo_workspace(&members, &["fn main() {}".to_string()])
        .expect("the manifest translates");

    // The program's own crate: the package, and what it depends on.
    let rendered = workspace.members[0].1.render();
    let parsed: toml::Value = toml::from_str(&rendered).expect("and is valid TOML");

    assert_eq!(parsed["package"]["name"].as_str(), Some("hyper-core"));
    assert_eq!(
        parsed["dependencies"]["regex"]["version"].as_str(),
        Some("1.5"),
        "a native Rust dependency passes through unchanged:\n{rendered}"
    );
    assert!(
        parsed["dependencies"]["regex"].get("type").is_none(),
        "`type = \"rust\"` is Nikaia's word and Cargo would refuse it:\n{rendered}"
    );

    // The codegen table of the *chosen* machine, and only that one - on the
    // workspace root, which is where a profile Cargo reads has to be
    // (ADR-053 D1).
    let root = workspace.render();
    let parsed: toml::Value = toml::from_str(&root).expect("the root is valid TOML");
    assert_eq!(parsed["profile"]["dev"]["opt-level"].as_integer(), Some(3));
    assert_eq!(parsed["profile"]["dev"]["lto"].as_bool(), Some(true));
    assert_eq!(
        parsed["profile"]["dev"]["panic"].as_str(),
        Some("unwind"),
        "x86-64 unwinds; the machine decides, not the author (ADR-037 D1)"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Part III 13.3 shows `http-server = "1.2"` beside the Rust crate, and nothing
/// anywhere decides what resolving it would mean. Refused with the reason,
/// rather than silently handed to Cargo - which would look for it on crates.io
/// and fail somewhere that explains nothing.
///
/// **A version, specifically.** A path dependency is decided and built
/// ([ADR-047](../../../docs/specification/adr/adr-047.md) D2); what a version
/// would need is a registry, a version grammar and a distribution format, which
/// are the three things [ADR-002](../../../docs/specification/adr/adr-002.md)
/// D1 §5 refuses to guess at. So the message names the form that works.
#[test]
fn a_nikaia_package_is_refused_and_says_why() {
    let dir = a_project(
        "project-nikaia-dep",
        "[package]\nname = \"consumer\"\nversion = \"0.1.0\"\n\n\
         [dependencies]\nhttp-server = \"1.2\"\n",
        HELLO,
    );

    let project = Project::open(&dir, None, None).expect("the project opens");
    let members = project.members().expect("the packages resolve");
    let error = project
        .cargo_workspace(&members, &["fn main() {}".to_string()])
        .expect_err("a Nikaia package has nowhere to come from");
    let text = format!("{error:#}");
    assert!(text.contains("http-server"), "{text}");
    // A **version** is what is not decided: no registry, no version grammar, no
    // distribution format (ADR-002 D1 §5). A path is decided and built
    // (ADR-047 D2), so the message names both ways that work.
    assert!(
        text.contains("given by version, but there's no package registry yet"),
        "{text}"
    );
    assert!(text.contains("path = \"../http-server\""), "{text}");
    assert!(
        text.contains("type = \"rust\""),
        "and the crates.io form too: {text}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// A directory with no manifest is not a project, and saying so beats whatever
/// Cargo would have said about a file it was never given.
#[test]
fn a_directory_without_a_manifest_is_not_a_project() {
    let dir = common::scratch_dir("project-none");
    let error = Project::open(&dir, None, None).expect_err("no manifest, no project");
    let text = format!("{error:#}");
    assert!(text.contains("nikaia.toml"), "{text}");
    assert!(
        text.contains("nikaia lower"),
        "and it points at the thing that does work for a single file: {text}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// **The single-file verbs** ([ADR-260](../../../docs/specification/adr/adr-260.md)
/// D3), asserted next to the project commands they stand beside: `lower`
/// writes Rust and `interpret` runs the file.
#[test]
fn the_single_file_verbs_lower_and_interpret() {
    let dir = common::scratch_dir("project-single-file");
    let input = dir.join("hello.nika");
    std::fs::write(&input, HELLO).expect("source");
    let output = dir.join("hello.rs");

    let run = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .args(["lower", input.to_str().unwrap()])
        .args(["--output", output.to_str().unwrap()])
        .env("NIKAIA_CACHE_DIR", dir.join("cache"))
        .output()
        .expect("the nikaia binary runs");
    assert!(run.status.success(), "{}", said(&run));
    assert!(
        std::fs::read_to_string(&output)
            .expect("the Rust is written")
            .contains("fn main()"),
        "`lower` writes the Rust"
    );

    let interpreted = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .args(["interpret", input.to_str().unwrap()])
        .env("NIKAIA_CACHE_DIR", dir.join("cache"))
        .output()
        .expect("the nikaia binary runs");
    assert!(interpreted.status.success(), "{}", said(&interpreted));
    assert!(
        String::from_utf8_lossy(&interpreted.stdout).contains("Hallo Welt, Nikaia!"),
        "and the interpreter still runs: {}",
        said(&interpreted)
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// **The compiled `std` is built once per machine, not once per project**
/// (ADR-002 D4).
///
/// The old shape was worse than "slow": `nikaia-std` had the compiler as a build
/// dependency, so Cargo compiled the compiler library again inside *every*
/// project's `target/` - while the installed compiler was running and doing that
/// very lowering. Two projects paid for it twice.
///
/// The claim is checked by the rlib's own modification time, because that is the
/// only thing that can carry it. A "Compiling nikaia-std" line missing from a log
/// says Cargo was quiet; an unmoved mtime says the file was not written.
#[test]
fn a_second_project_links_the_std_the_first_one_built() {
    let first = a_project(
        "project-std-cache-first",
        "[package]\nname = \"first\"\nversion = \"0.1.0\"\n",
        HELLO,
    );
    let built = nikaia(&["build"], &first);
    assert!(built.status.success(), "{}", said(&built));

    // Not "this build created it" - the tests in this file run in parallel and
    // share the cache on purpose, so by now any of them may have been the one to
    // compile it. That is the decision working, and what is left to check is that
    // the *next* build does not write over it.
    let after_first = compiled_stds(&shared_cache_dir());
    assert!(
        !after_first.is_empty(),
        "a project build leaves a compiled std in the cache, under {}",
        shared_cache_dir().display()
    );

    let second = a_project(
        "project-std-cache-second",
        "[package]\nname = \"second\"\nversion = \"0.1.0\"\n",
        HELLO,
    );
    let built = nikaia(&["build"], &second);
    assert!(built.status.success(), "{}", said(&built));

    assert_eq!(
        compiled_stds(&shared_cache_dir()),
        after_first,
        "the second project linked the compiled std the first one left and wrote \
         no new one - neither a second copy nor a fresh write of this one"
    );

    // The defect this decision removes, stated as an absence: nothing of the
    // compiler is in a project's build graph any more.
    let lock = std::fs::read_to_string(second.join("target/nikaia/build/Cargo.lock"))
        .expect("Cargo resolved the generated package");
    for gone in ["name = \"nikaia\"", "name = \"clap\"", "name = \"sha2\""] {
        assert!(
            !lock.contains(gone),
            "`{gone}` was one of the 58 packages that existed only to build std:\n{lock}"
        );
    }

    std::fs::remove_dir_all(&first).ok();
    std::fs::remove_dir_all(&second).ok();
}

/// Every compiled `std` under a cache directory, and when it was written.
///
/// Found on disk rather than derived from the key, so the test asserts what is
/// there instead of repeating the code that put it there. A map and not one path
/// because a cache carried over from an earlier toolchain holds an entry under an
/// older key, and the question is whether *this* build wrote anything.
fn compiled_stds(cache: &Path) -> BTreeMap<PathBuf, Option<std::time::SystemTime>> {
    fn walk(dir: &Path, found: &mut BTreeMap<PathBuf, Option<std::time::SystemTime>>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, found);
            } else if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("libnikaia_std-") && n.ends_with(".rlib"))
            {
                let when = std::fs::metadata(&path)
                    .ok()
                    .and_then(|m| m.modified().ok());
                found.insert(path, when);
            }
        }
    }

    let mut found = BTreeMap::new();
    walk(cache, &mut found);
    found
}

/// **The explain modes reach a project build** (issue #210, since closed).
///
/// `--sharing`'s own help says why they exist: there is no way to *ask* for the
/// cheaper reference count, every fallback is enumerated instead, and *"that is
/// only fair if the fallbacks can be asked about. This is the asking."* They were
/// on the single-file path only — `nikaia build --sharing` answered `unexpected
/// argument` — so the asking was unavailable exactly where a person with a real
/// program would do it.
///
/// Over **every file** of the package, and against the package's own ledger
/// rather than each file's: `sync`, the touch sets and the sharing classes are
/// whole-program facts, so a report built from one file's inferences would answer
/// a different question from the one the build answers.
#[test]
fn the_explain_modes_reach_a_project_build() {
    let dir = a_project(
        "project-explain",
        "[package]\nname = \"explained\"\nversion = \"0.1.0\"\n",
        "fn main() {\n    \
             let c = Shared(Conn { id: 1 })\n    \
             println(f\"{hold(c)}\")\n\
         }\n",
    );
    std::fs::write(
        dir.join("src/pool.nika"),
        "pub struct Conn { pub id: i64 }\n\
         \n\
         pub fn hold(c: Shared[Conn]) -> i64 {\n    \
             return c.id\n\
         }\n",
    )
    .expect("the second file");

    let ran = nikaia(&["build", "--sharing"], &dir);
    let out = String::from_utf8_lossy(&ran.stdout);
    assert!(
        out.contains("`c` (Shared[Conn])"),
        "the value is named: {}",
        said(&ran)
    );
    // Both files are reported, each under its own name - a slot key does not say
    // which file it is in.
    assert!(
        out.contains("main.nika") && out.contains("pool.nika"),
        "{out}"
    );

    // And the other two are accepted on `build` as well.
    for flag in ["--trust", "--overlaps"] {
        let ran = nikaia(&["build", flag], &dir);
        assert!(
            ran.status.success(),
            "`nikaia build {flag}`:\n{}",
            said(&ran)
        );
    }

    std::fs::remove_dir_all(&dir).ok();
}

/// **A package reached through two parents is one package**, and so its types
/// are one type ([ADR-053](../../../docs/specification/adr/adr-053.md) D2).
///
/// The diamond: the program depends on `deep` directly, and on a library that
/// depends on **the same directory** under its own key `c`. Cargo has always
/// made this one crate — D2 rests on that — but the checker in front of it named
/// a type by the manifest key it was reached *through*, so the program was told
/// its `deep::Id` was not the `c::Id` the library takes. One type, refused for
/// having two spellings, with a help line asking for what was already written.
///
/// The library's key is deliberately a word the program never writes, because
/// that is the case that has to work: whose word wins cannot be "whoever was
/// read first". It is the program's, here, and the package's `[package] name`
/// where the program does not name it either.
///
/// Run and not only built: the value is made in one package and read in another,
/// so if the two types were ever really two, the generated Rust would say so.
#[test]
fn a_package_reached_through_two_parents_is_one_package() {
    let dir = common::scratch_dir("project-diamond");
    for at in ["app/src", "lib/src", "deep/src"] {
        std::fs::create_dir_all(dir.join(at)).expect("the packages");
    }
    std::fs::write(
        dir.join("app/nikaia.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n\
         [dependencies]\ndeep = { path = \"../deep\" }\nlib = { path = \"../lib\" }\n",
    )
    .expect("the program's manifest");
    std::fs::write(
        dir.join("lib/nikaia.toml"),
        "[package]\nname = \"lib\"\nversion = \"0.1.0\"\n\n\
         [dependencies]\nc = { path = \"../deep\" }\n",
    )
    .expect("the library's manifest, with its own word for the same directory");
    std::fs::write(
        dir.join("deep/nikaia.toml"),
        "[package]\nname = \"deep\"\nversion = \"0.1.0\"\n",
    )
    .expect("the shared package's manifest");

    std::fs::write(
        dir.join("deep/src/main.nika"),
        "pub struct Id {\n    pub value: i64\n}\n\n\
         pub fn make(v: i64) -> Id {\n    return Id { value: v }\n}\n",
    )
    .expect("the shared package");
    std::fs::write(
        dir.join("lib/src/main.nika"),
        "use c\n\npub fn show(id: c::Id) -> i64 {\n    return id.value\n}\n",
    )
    .expect("the library");
    std::fs::write(
        dir.join("app/src/main.nika"),
        "use deep\nuse lib\n\nfn main() {\n\
         \x20   let id = deep::make(7)\n\
         \x20   println(f\"{lib::show(id)}\")\n}\n",
    )
    .expect("the program");

    let ran = nikaia(&["run"], &dir.join("app"));
    assert!(
        ran.status.success(),
        "a package reached through two parents is one package (ADR-053 D2): {}",
        said(&ran)
    );
    assert_eq!(String::from_utf8_lossy(&ran.stdout).trim(), "7");

    std::fs::remove_dir_all(&dir).ok();
}

/// A project and a package it depends on by path.
fn a_program_and_a_package(purpose: &str, dependency: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = common::scratch_dir(purpose);
    std::fs::create_dir_all(dir.join("app/src")).expect("the program");
    std::fs::create_dir_all(dir.join(format!("{dependency}/src"))).expect("the package");
    std::fs::write(
        dir.join("app/nikaia.toml"),
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n\
             [dependencies]\n{dependency} = {{ path = \"../{dependency}\" }}\n"
        ),
    )
    .expect("the program's manifest");
    std::fs::write(
        dir.join(format!("{dependency}/nikaia.toml")),
        format!("[package]\nname = \"{dependency}\"\nversion = \"0.1.0\"\n"),
    )
    .expect("the package's manifest");
    for (file, contents) in files {
        std::fs::write(dir.join(file), contents).expect("a source");
    }
    dir
}

/// **A Nikaia package is depended on by path, and its public surface is what a
/// program may write** ([ADR-047](../../../docs/specification/adr/adr-047.md)
/// D2).
///
/// The first thing in this repository that is two packages. Three claims in one
/// program, because each is a different piece of machinery: a **function** of the
/// package resolves and runs, a **type** it declares can be named and built, and
/// a name it declares in *another of its own files* is visible to it — one
/// namespace per package, one level up from D1.
#[test]
fn a_package_is_depended_on_by_path() {
    let dir = a_program_and_a_package(
        "package-path",
        "http",
        &[
            (
                "http/src/main.nika",
                "pub struct Request { pub path: ref String }\n\
                 \n\
                 pub fn ok(body: ref String) -> String {\n    \
                     return f\"200 {body}\"\n\
                 }\n",
            ),
            (
                "http/src/routes.nika",
                "pub fn route(r: Request) -> String {\n    \
                     return ok(r.path)\n\
                 }\n",
            ),
            (
                "app/src/main.nika",
                "use http\n\
                 \n\
                 fn main() {\n    \
                     let r = http::Request { path: \"/index\" }\n    \
                     println(http::route(r))\n\
                 }\n",
            ),
        ],
    );

    let ran = nikaia(&["run"], &dir.join("app"));
    assert!(ran.status.success(), "{}", said(&ran));
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).trim(),
        "200 /index",
        "{}",
        said(&ran)
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// **A package's own type is constructed through the package that declares it**
/// (Part I 4.2, [ADR-140](../../../docs/specification/adr/adr-140.md) D2), and
/// the whole of [ADR-018](../../../docs/specification/adr/adr-018.md)'s shape
/// runs on top of it.
///
/// **Found by writing the shape that record prints.** `tiny::Server()` lowered
/// **verbatim** — the constructor rule asked the *library*'s ledger about a
/// qualified name and never the package's — and `rustc` answered *use struct
/// literal syntax instead* and *type annotations needed* about a file nobody
/// wrote ([Part III C.1](../../../docs/specification/30-nikaia-tooling.md)). It
/// is also the form `NK1149`'s own help hands over, so the way out could not be
/// taken either ([C.2](../../../docs/specification/30-nikaia-tooling.md)).
///
/// Four things at once, because the point is that they compose: a **type**
/// constructed through its package, a **method chain** over it, a
/// **function-typed parameter** crossing the package boundary
/// ([ADR-102](../../../docs/specification/adr/adr-102.md) D1), and the
/// **`async` closure** [ADR-192](../../../docs/specification/adr/adr-192.md) D1
/// writes for a run parameter — handed to a callee in another package.
#[test]
fn a_packages_type_is_constructed_and_its_handler_chain_runs() {
    let dir = a_program_and_a_package(
        "package-constructor",
        "tiny",
        &[
            (
                "tiny/src/main.nika",
                "pub struct Request { pub path: String }\n\
                 pub struct Response { pub status: i32, pub body: String }\n\
                 pub struct Server { pub routes: i64 }\n\
                 \n\
                 impl Server {\n    \
                     pub fn new() -> Server { return Server { routes: 0 } }\n\
                 \n    \
                     pub fn route(ref self, path: ref String, handler: fn(Request) -> Response) -> Server {\n        \
                         let answered = handler(Request { path: path.clone() })\n        \
                         println(f\"{path} {answered.status} {answered.body}\")\n        \
                         return Server { routes: self.routes + 1 }\n    \
                     }\n\
                 \n    \
                     pub fn listen(ref self, at: ref String) {\n        \
                         println(f\"listening on {at} with {self.routes}\")\n    \
                     }\n\
                 }\n",
            ),
            (
                "app/src/main.nika",
                "use tiny\n\
                 \n\
                 fn main() {\n    \
                     tiny::Server()\n        \
                         .route(\"/a\") fn(r) { return tiny::Response { status: 200, body: f\"A {r.path}\" } }\n        \
                         .route(\"/b\") fn(r) { return tiny::Response { status: 404, body: f\"B {r.path}\" } }\n        \
                         .listen(\"127.0.0.1:8080\")\n\
                 }\n",
            ),
        ],
    );

    let ran = nikaia(&["run"], &dir.join("app"));
    assert!(ran.status.success(), "{}", said(&ran));
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).trim(),
        "/a 200 A /a\n/b 404 B /b\nlistening on 127.0.0.1:8080 with 2",
        "{}",
        said(&ran)
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// **And `NK1110` fires for the first time**: a name a package does not publish
/// cannot be reached from outside it (Part I, 9.2).
///
/// Until a second package existed this check had nothing that could trigger it —
/// the files of one package share a namespace, so a qualified name was a name
/// from a package nobody could depend on ([ADR-047](../../../docs/specification/adr/adr-047.md)
/// §5). This is the day `pub` starts to mean something a program can observe.
#[test]
fn a_name_a_package_does_not_publish_is_refused() {
    let dir = a_program_and_a_package(
        "package-privacy",
        "http",
        &[
            (
                "http/src/main.nika",
                "fn secret() -> i64 {\n    return 41\n}\n\
                 \n\
                 pub fn answer() -> i64 {\n    return secret() + 1\n}\n",
            ),
            (
                "app/src/main.nika",
                "use http\n\nfn main() {\n    println(f\"{http::secret()}\")\n}\n",
            ),
        ],
    );

    let ran = nikaia(&["build"], &dir.join("app"));
    assert!(!ran.status.success(), "{}", said(&ran));
    let out = said(&ran);
    assert!(out.contains("NK1110"), "{out}");
    assert!(
        out.contains("You can't use `secret` from here: it's private to `http`."),
        "{out}"
    );
    assert!(
        out.contains("Everything is private to its package unless it's declared `pub`."),
        "{out}"
    );

    // …and what it does publish is reachable, so the rule is a rule and not a
    // blanket refusal.
    std::fs::write(
        dir.join("app/src/main.nika"),
        "use http\n\nfn main() {\n    println(f\"{http::answer()}\")\n}\n",
    )
    .expect("the program");
    let ran = nikaia(&["run"], &dir.join("app"));
    assert!(ran.status.success(), "{}", said(&ran));
    assert_eq!(String::from_utf8_lossy(&ran.stdout).trim(), "42");
    std::fs::remove_dir_all(&dir).ok();
}

/// **A Nikaia dependency is part of the program, not a foreign package**
/// ([ADR-047](../../../docs/specification/adr/adr-047.md) D2 rule 5).
///
/// [ADR-043](../../../docs/specification/adr/adr-043.md) D6 turns the overflow
/// check on for the program and off for every foreign package. A dependency
/// written in this language carries this language's promise, and without the rule
/// stated it would land on the foreign side by accident — that is where a
/// dependency mechanically appears — and a library would compute silently wrong
/// numbers while the program calling it aborted.
///
/// So the test is that the **library's** arithmetic aborts.
#[test]
fn arithmetic_in_a_package_aborts_like_the_programs_own() {
    let dir = a_program_and_a_package(
        "package-overflow",
        "maths",
        &[
            (
                "maths/src/main.nika",
                "pub fn grow(n: i32) -> i32 {\n    return n * 2\n}\n",
            ),
            (
                "app/src/main.nika",
                "use maths\n\nfn main() {\n    println(f\"{maths::grow(2000000000)}\")\n}\n",
            ),
        ],
    );

    let ran = nikaia(&["run"], &dir.join("app"));
    assert!(
        !ran.status.success(),
        "a library's overflow is the program's overflow: {}",
        said(&ran)
    );
    assert!(
        said(&ran).contains("attempt to multiply with overflow"),
        "{}",
        said(&ran)
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// **A trait a package publishes, implemented by a body that calls back into
/// that package** ([ADR-100](../../../docs/specification/adr/adr-100.md) D1,
/// D5) — the shape issue #228 carried as a defect for as long as the entry
/// existed.
///
/// It was `NK1129`: *"`Fixed::greet` can pause, and `lib::Greeter` declares it
/// as a method that cannot"*, for a body whose only call is a `String` two
/// files away in another package. The consumer's inference could not see into
/// `lib` at all, so `reach_of` set `blocked` — and `Sync::No` means *"can pause
/// **or** could not be vouched for"*, which every reader takes as the first.
///
/// **Driven through the binary and run**, because what closed it is an ordering
/// between two lowerings: `lib` is lowered first, its ledger is written in its
/// own root, and `app`'s inference reads that answer rather than deriving a
/// worse one. None of that exists inside one `Program`.
#[test]
fn a_package_trait_is_implemented_by_a_body_that_calls_that_package() {
    let dir = a_program_and_a_package(
        "package-trait-call",
        "lib",
        &[
            (
                "lib/src/main.nika",
                "pub trait Greeter {\n\
                 \x20   fn greet(ref self) -> String\n\
                 }\n\
                 \n\
                 pub fn hello() -> String {\n\
                 \x20   return \"hello\"\n\
                 }\n",
            ),
            (
                "app/src/main.nika",
                "use lib\n\
                 \n\
                 struct Fixed { n: i64 }\n\
                 \n\
                 impl lib::Greeter for Fixed {\n\
                 \x20   fn greet(ref self) -> String {\n\
                 \x20       return lib::hello()\n\
                 \x20   }\n\
                 }\n\
                 \n\
                 fn main() {\n\
                 \x20   let f = Fixed { n: 1 }\n\
                 \x20   println(f\"{f.greet()}\")\n\
                 }\n",
            ),
        ],
    );
    let app = dir.join("app");

    let ran = nikaia(&["run"], &app);
    assert!(
        ran.status.success(),
        "a body that calls the package it implements against is not a pausing body: {}",
        said(&ran)
    );
    assert_eq!(String::from_utf8_lossy(&ran.stdout).trim(), "hello");

    // **And the dependency's own ledger is in the dependency's own root** (D5),
    // which is what the consumer read: a package's answers are written by the
    // package's build and committed with it, the way `std` has always worked.
    let shipped = std::fs::read_to_string(dir.join("lib/nikaia.contracts"))
        .expect("a package's build writes its own ledger");
    assert!(!shipped.contains("[sources]"), "{shipped}");
    assert!(shipped.contains("[fn.\"hello\"]"), "{shipped}");
    // **And what it was derived from beside it** (ADR-251 D1).
    let derived = std::fs::read_to_string(dir.join("lib/nikaia.derived"))
        .expect("and the record of what it was derived from");
    assert!(derived.contains("[sources]"), "{derived}");

    std::fs::remove_dir_all(&dir).ok();
}

/// The three rules of [ADR-047](../../../docs/specification/adr/adr-047.md) D2
/// that are about the **set** of dependencies rather than any one of them.
///
/// Each is a message and not a silence, because each is a thing somebody will do:
/// name one package twice, depend on a package that depends on packages, or hand
/// a dependency build settings of its own.
#[test]
fn the_rules_that_come_with_a_path_dependency() {
    let dir = a_program_and_a_package(
        "package-rules",
        "http",
        &[
            (
                "http/src/main.nika",
                "pub fn ok() -> i64 {\n    return 1\n}\n",
            ),
            (
                "app/src/main.nika",
                "use http\n\nfn main() {\n    println(f\"{http::ok()}\")\n}\n",
            ),
        ],
    );
    let app = dir.join("app");
    let manifest = app.join("nikaia.toml");
    let both = std::fs::read_to_string(&manifest).expect("the manifest");

    // Rule 3: the same path is the same package, so two names for it are not two
    // packages - they would be two copies of every type it declares.
    std::fs::write(
        &manifest,
        format!("{both}http2 = {{ path = \"../http\" }}\n"),
    )
    .expect("two names");
    let ran = nikaia(&["build"], &app);
    assert!(!ran.status.success(), "{}", said(&ran));
    assert!(
        said(&ran).contains("are the same package"),
        "{}",
        said(&ran)
    );
    std::fs::write(&manifest, &both).expect("back to one");

    // Rule 2: transitive dependencies are not visible - and since
    // [ADR-053](../../../docs/specification/adr/adr-053.md) D3 that is
    // structural rather than a refusal. A package that depends on a package
    // **builds**, because it is generated as its own crate naming its own
    // dependencies; what the program cannot do is reach past `http` to what
    // `http` depends on, and it cannot because that crate is not one of its own.
    std::fs::create_dir_all(dir.join("deeper/src")).expect("the third package");
    std::fs::write(
        dir.join("deeper/nikaia.toml"),
        "[package]\nname = \"deeper\"\nversion = \"0.1.0\"\n",
    )
    .expect("its manifest");
    std::fs::write(
        dir.join("deeper/src/main.nika"),
        "pub fn two() -> i64 {\n    return 2\n}\n",
    )
    .expect("its source");
    std::fs::write(
        dir.join("http/nikaia.toml"),
        "[package]\nname = \"http\"\nversion = \"0.1.0\"\n\n\
         [dependencies]\ndeeper = { path = \"../deeper\" }\n",
    )
    .expect("a transitive dependency");
    std::fs::write(
        dir.join("http/src/main.nika"),
        "use deeper\n\npub fn ok() -> i64 {\n    return deeper::two()\n}\n",
    )
    .expect("the library uses it");
    let ran = nikaia(&["run"], &app);
    assert!(
        ran.status.success(),
        "a library that uses a library is a library: {}",
        said(&ran)
    );
    assert_eq!(String::from_utf8_lossy(&ran.stdout).trim(), "2");

    // …and the program still cannot name `deeper` itself.
    let program = std::fs::read_to_string(app.join("src/main.nika")).expect("the program");
    std::fs::write(
        app.join("src/main.nika"),
        "use http\n\nfn main() {\n    println(f\"{deeper::two()}\")\n}\n",
    )
    .expect("reaching past http");
    let ran = nikaia(&["build"], &app);
    assert!(!ran.status.success(), "{}", said(&ran));
    let out = said(&ran);
    // The words are `rustc`'s and change between its versions; what this test is
    // about is that the refusal names `deeper` and sits on the line of the
    // program that wrote it.
    assert!(
        out.contains("`deeper`") && out.contains("main.nika:4:5"),
        "a transitive package is not a dependency of this crate, so naming it \
         does not resolve: {out}"
    );
    std::fs::write(app.join("src/main.nika"), &program).expect("back to the program");
    std::fs::write(
        dir.join("http/src/main.nika"),
        "pub fn ok() -> i64 {\n    return 1\n}\n",
    )
    .expect("back to the library");

    // Rule 4: a dependency's own `[build]` is ignored, and the compiler says so -
    // a package is built with the settings of the program that uses it.
    std::fs::write(
        dir.join("http/nikaia.toml"),
        "[package]\nname = \"http\"\nversion = \"0.1.0\"\n\n\
         [build]\nuser-parallelism = \"yes\"\n",
    )
    .expect("its own build section");
    let ran = nikaia(&["run"], &app);
    assert!(ran.status.success(), "{}", said(&ran));
    let out = said(&ran);
    assert_eq!(
        out.matches("is ignored").count(),
        1,
        "said once, not once per pass: {out}"
    );
    assert!(
        out.contains("settings of the program that uses it"),
        "{out}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// **A field a package does not publish cannot be reached or written from
/// outside it** (Part I 9.2, [ADR-047](../../../docs/specification/adr/adr-047.md) D2).
///
/// The hole this closes had the shortest fuse in the list: the ledger recorded no
/// per-field `pub`, so a type whose fields were private could be **built by
/// name** from another package with nothing saying no — and the language below
/// cannot help here the way it does for an item, because the emitted struct is in
/// the same crate.
///
/// Two shapes, because they are two pieces of machinery: reading a field, and
/// giving one a value in a struct literal.
#[test]
fn a_field_a_package_does_not_publish_is_refused() {
    let dir = a_program_and_a_package(
        "package-fields",
        "http",
        &[
            (
                "http/src/main.nika",
                "pub struct Request { pub id: i64, method: i64 }\n\
                 \n\
                 pub fn get(id: i64) -> Request {\n    \
                     return Request { id: id, method: 1 }\n\
                 }\n\
                 \n\
                 pub fn method_of(r: ref Request) -> i64 {\n    \
                     return r.method\n\
                 }\n",
            ),
            (
                "app/src/main.nika",
                "use http\n\nfn main() {\n    \
                     let r = http::get(7)\n    \
                     println(f\"{r.method}\")\n\
                 }\n",
            ),
        ],
    );
    let app = dir.join("app");

    let ran = nikaia(&["build"], &app);
    assert!(!ran.status.success(), "{}", said(&ran));
    let out = said(&ran);
    assert!(out.contains("NK1110"), "{out}");
    assert!(
        out.contains("You can't reach `http::Request.method` from here: it's private to `http`."),
        "{out}"
    );

    // Building it from outside is the same rule, and the one that was silent.
    std::fs::write(
        app.join("src/main.nika"),
        "use http\n\nfn main() {\n    \
             let r = http::Request { id: 7, method: 2 }\n    \
             println(f\"{r.id}\")\n\
         }\n",
    )
    .expect("the program");
    let ran = nikaia(&["build"], &app);
    assert!(!ran.status.success(), "{}", said(&ran));
    assert!(
        said(&ran).contains("`http::Request.method` from here: it's private"),
        "{}",
        said(&ran)
    );

    // **And the public field, and the method that reaches the private one, are
    // fine** - the rule is a rule and not a blanket refusal. A type may keep its
    // fields private and offer functions, which is Part I 9.3's whole point.
    std::fs::write(
        app.join("src/main.nika"),
        "use http\n\nfn main() {\n    \
             let r = http::get(7)\n    \
             println(f\"{r.id} {http::method_of(r)}\")\n\
         }\n",
    )
    .expect("the program");
    let ran = nikaia(&["run"], &app);
    assert!(ran.status.success(), "{}", said(&ran));
    assert_eq!(String::from_utf8_lossy(&ran.stdout).trim(), "7 1");

    std::fs::remove_dir_all(&dir).ok();
}

/// **Every abort points at the Nikaia line**
/// ([ADR-044](../../../docs/specification/adr/adr-044.md)).
///
/// [ADR-012](../../../docs/specification/adr/adr-012.md) decides that a
/// diagnostic names the `.nika` file the user wrote, and the compiler kept that
/// promise everywhere it *reported* something. An abort at run time was the one
/// path where it could not: there is no compiler left to translate anything, so
/// Rust's own message named the generated file and line.
///
/// **Four paths in one test, because D2's whole argument is that they are one
/// fix.** An overflow, a conversion that does not fit, a negative index and an
/// index past the end all arrive at the same hook, and a per-case fix would have
/// been four fixes and a fifth the day a fifth abort path arrives.
#[test]
fn every_abort_names_the_nikaia_line() {
    let dir = a_project(
        "project-aborts",
        "[package]\nname = \"aborts\"\nversion = \"0.1.0\"\n",
        HELLO,
    );

    for (what, program, says) in [
        (
            "an overflow",
            "fn double(n: i32) -> i32 {\n    return n * 2\n}\n\
             \n\
             fn main() {\n    \
                 let start = 2000000000\n    \
                 println(f\"{double(start)}\")\n\
             }\n",
            "attempt to multiply with overflow",
        ),
        (
            "a conversion",
            "fn narrow(n: i64) -> i32 {\n    return n as i32\n}\n\
             \n\
             fn main() {\n    \
                 let big = 5000000000\n    \
                 println(f\"{narrow(big)}\")\n\
             }\n",
            "does not fit in an `i32`",
        ),
        (
            "a negative index",
            "fn pick(xs: ref Vec[i64], at: i64) -> i64 {\n    return xs[at]\n}\n\
             \n\
             fn main() {\n    \
                 let mut xs = Vec()\n    \
                 xs.push(1)\n    \
                 let zero = 0\n    \
                 println(f\"{pick(xs, zero - 1)}\")\n\
             }\n",
            "index out of bounds: the index is -1",
        ),
        (
            "an index past the end",
            "fn pick(xs: ref Vec[i64], at: i64) -> i64 {\n    return xs[at]\n}\n\
             \n\
             fn main() {\n    \
                 let mut xs = Vec()\n    \
                 xs.push(1)\n    \
                 let far = 5\n    \
                 println(f\"{pick(xs, far)}\")\n\
             }\n",
            "index out of bounds: the len is 1",
        ),
    ] {
        std::fs::write(dir.join("src/main.nika"), program).expect("the program");
        let ran = nikaia(&["run"], &dir);
        assert!(!ran.status.success(), "{what} must abort: {}", said(&ran));
        let out = said(&ran);

        // The line the abort happened on - line 2 in every one of these, which is
        // the function's body and not the `main` that called it.
        assert!(
            out.contains("src/main.nika:2: the program stopped:"),
            "{what}: {out}"
        );
        assert!(out.contains(says), "{what}: {out}");
        // And nothing about the file nobody wrote.
        assert!(!out.contains("/gen/"), "{what}: {out}");
        assert!(
            !out.contains("nikaia-std/src/"),
            "{what}: not a line of `std`'s own Rust either: {out}"
        );
    }

    std::fs::remove_dir_all(&dir).ok();
}

/// …and a panic with no Nikaia line behind it is left in the words of whoever
/// wrote it, which is the half that keeps a program from being worse off.
///
/// The table covers the lines the emitter wrote *from* a Nikaia line. A panic
/// inside `std`'s own Rust, or inside a foreign crate, has no Nikaia line to
/// name — and the hook installed before ours, which is Rust's, is what says so.
#[test]
fn an_abort_with_no_nikaia_line_keeps_rusts_own_report() {
    let dir = a_project(
        "project-foreign-abort",
        "[package]\nname = \"foreign\"\nversion = \"0.1.0\"\n",
        HELLO,
    );
    // `expect` on a missing environment variable aborts inside `std`'s Rust: the
    // panic's location is `Option::expect`'s caller, which is a line of the
    // generated file that no `.nika` line maps to, because the emitter wrote it
    // for the f-string rather than from a statement.
    std::fs::write(
        dir.join("src/main.nika"),
        "use std::cli\n\nfn main() {\n    \
             println(f\"{cli::args().nth(40)}\")\n\
         }\n",
    )
    .expect("the program");

    let ran = nikaia(&["run"], &dir);
    let out = said(&ran);
    // Either it runs or it aborts - what must not happen is a line this table
    // invented. This asserts the shape of the claim rather than a particular
    // failure, because which of the two it is belongs to `std`'s ledger and not
    // to this record.
    if !ran.status.success() {
        assert!(
            !out.contains("src/main.nika:1:"),
            "a line the table does not know must not be answered: {out}"
        );
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// **`use http as h` shortens the prefix, once, in one place**
/// ([ADR-046](../../../docs/specification/adr/adr-046.md) D3).
///
/// The one thing that record *adds* rather than refuses, and what makes the
/// qualified-only rule affordable. It also settles a collision: two libraries
/// that both want to be `http` are the consumer's to name apart, which is
/// authority the consumer needs anyway.
///
/// **The alias is resolved away rather than emitted.** A Rust `use … as …` would
/// not do: every file's items are in one crate root (ADR-047 D1), so an alias
/// there would be the whole program's name for the package rather than this
/// file's. `Parsed::unaliased` is the one place it happens, on the way to every
/// name — which is why a call, a type, and a struct literal all work without
/// three separate rules.
#[test]
fn a_package_may_be_given_another_name() {
    let dir = a_program_and_a_package(
        "package-alias",
        "http",
        &[
            (
                "http/src/main.nika",
                "pub struct Request { pub id: i64 }\n\
                 \n\
                 pub fn get(id: i64) -> Request {\n    \
                     return Request { id: id }\n\
                 }\n\
                 \n\
                 pub fn id_of(r: ref Request) -> i64 {\n    \
                     return r.id\n\
                 }\n",
            ),
            (
                "app/src/main.nika",
                "use http as h\n\
                 \n\
                 fn main() {\n    \
                     let made: h::Request = h::get(7)\n    \
                     let built = h::Request { id: 35 }\n    \
                     println(f\"{h::id_of(made)} {built.id}\")\n\
                 }\n",
            ),
        ],
    );
    let app = dir.join("app");

    let ran = nikaia(&["run"], &app);
    assert!(ran.status.success(), "{}", said(&ran));
    assert_eq!(String::from_utf8_lossy(&ran.stdout).trim(), "7 35");

    // **The refusals reach through it**, and they name the *package* rather than
    // the alias: the type is `http::Request` whatever this file calls the package,
    // and it is the package that keeps a name private.
    std::fs::write(
        app.join("src/main.nika"),
        "use http as h\n\nfn main() {\n    let n: i64 = h::get(1)\n}\n",
    )
    .expect("the program");
    let ran = nikaia(&["build"], &app);
    assert!(!ran.status.success(), "{}", said(&ran));
    assert!(
        said(&ran).contains("This value is `http::Request`, but the `let` declares `i64`."),
        "{}",
        said(&ran)
    );

    // **And one name per file counts the alias** (D5): the alias is the name this
    // file introduces, so two of them under one name is the collision D3 exists to
    // let a consumer fix — and therefore the one D5 has to catch.
    std::fs::write(
        app.join("src/main.nika"),
        "use http as h\nuse http as h\n\nfn main() { }\n",
    )
    .expect("the program");
    let ran = nikaia(&["build"], &app);
    assert!(!ran.status.success(), "{}", said(&ran));
    let out = said(&ran);
    assert!(out.contains("`h`") && out.contains("twice"), "{out}");
    assert!(
        out.contains("Write `use … as …` to give one of them another name."),
        "{out}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// **A warning reaches the user once, and in this language's words.**
///
/// `nikaia run` used to print one twice: the translated one, and then `rustc`'s
/// own spanned against `target/nikaia/gen/….rs`. The cause was the second Cargo
/// invocation — a `run` cannot use `--message-format=json`, because the
/// program's own output is on that stdout, so Cargo rendered its **cached**
/// diagnostics to stderr while checking freshness and nothing intercepted them.
///
/// The build now runs the binary itself, at the path its own JSON named. Part III
/// C.1 is what this is about: a message about the generated Rust is not a message
/// the user gets.
#[test]
fn a_warning_reaches_the_user_once_and_in_nikaia_terms() {
    let dir = a_project(
        "project-warning-once",
        "[package]\nname = \"noisy\"\nversion = \"0.1.0\"\n",
        // `unused_assignments`: the value `null` is written and never read,
        // which is exactly Part I 2.3's own example.
        "fn main() {\n    \
         let mut maybe: String? = null\n    \
         maybe = \"World\"\n    \
         let shown = maybe ?? \"x\"\n    \
         println(f\"{shown}\")\n\
         }\n",
    );

    let run = nikaia(&["run"], &dir);
    assert!(run.status.success(), "{}", said(&run));
    assert!(
        String::from_utf8_lossy(&run.stdout).contains("World"),
        "the program still runs: {}",
        said(&run)
    );

    let stderr = String::from_utf8_lossy(&run.stderr);
    let warnings = stderr.matches("is never read").count();
    assert_eq!(
        warnings,
        1,
        "the warning is said once, not once translated and once raw: {}",
        said(&run)
    );
    assert!(
        stderr.contains("main.nika:"),
        "and against the `.nika` line: {}",
        said(&run)
    );
    assert!(
        !stderr.contains("target/nikaia/gen"),
        "nothing may point at the generated Rust (Part III C.1): {}",
        said(&run)
    );
}

/// **The committed `http` package, built and run by its committed consumer.**
///
/// The diamond test above proves path dependencies work; it builds its packages
/// in a scratch directory, which means the mechanism is covered and no example
/// of it was. [ADR-069](../../../docs/specification/adr/adr-069.md) made `http` a
/// package and §4 said in as many words that the package did not exist — so
/// `use http` answered *"no dependency is called `http`"* and a reader had
/// nothing to copy.
///
/// This runs the real directories rather than a copy, for the reason
/// `examples.rs` gives about its own: the example is the file, so it cannot
/// drift.
///
/// **It also covers the trap that cost the first attempt.** The two dependency
/// arms resolve their paths against different things: a Nikaia path is read by
/// `project::packages_of` and is relative to the manifest's own directory, while
/// a `type = "rust"` one is handed to Cargo verbatim (ADR-002 D1) and is
/// therefore relative to the *generated* manifest under `target/nikaia/build/`.
/// `examples/foreign-runtime/serve` climbs four levels; this climbs one.
/// **And it is a server now** ([ADR-194](../../../docs/specification/adr/adr-194.md)
/// D5), which is what this test grew into: it used to build the pair and read
/// one rendered response off stdout, because there was nothing to hand a
/// response to.
///
/// What it drives is the whole of the MVP's protocol over a real socket — the
/// two methods, a body by `Content-Length`, and each of the five refusals — and
/// it drives them against the **example**, not a copy, for `examples.rs`'s
/// reason about its own: the example is the file, so it cannot drift.
///
/// **Port `0` and the address read back.** A test that picked a port would race
/// every other test on the machine for it; `net::Listener::address` exists so
/// that a program which asked for any free one can say which it got, and the
/// line `listen` prints is how this finds out. That is also why the child is
/// spawned rather than run to completion: a server does not end.
#[test]
fn the_http_package_serves_its_example() {
    let consumer = repo_root().join("examples/hello-http");
    let built = nikaia(&["build"], &consumer);
    assert!(
        built.status.success(),
        "examples/hello-http builds against examples/http: {}",
        said(&built)
    );

    let mut server = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .args(["run", "--project"])
        .arg(&consumer)
        .args(["--", "127.0.0.1:0"])
        .env("NIKAIA_CACHE_DIR", shared_cache_dir())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the nikaia binary runs");
    // **Killed however this ends**, including a panicking assertion: a test that
    // left a server listening would leave it listening for the rest of the run.
    let stopped = Stop(&mut server);
    let address = the_address_it_printed(stopped.0);

    let (status, body) = ask(&address, b"GET / HTTP/1.1\r\nhost: x\r\n\r\n");
    assert_eq!(status, "HTTP/1.1 200 OK", "the handler answered `/`");
    assert_eq!(
        body, "Hello, world",
        "and `??` said what an absent query means"
    );

    // **The query string is its own question, and nothing is not an empty
    // string** ([ADR-018](../../../docs/specification/adr/adr-018.md) D4).
    let (status, body) = ask(&address, b"GET /?name=Ada HTTP/1.1\r\nhost: x\r\n\r\n");
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert_eq!(body, "Hello, Ada", "the handler read the query string");

    // **And a header, case-insensitive as the protocol is** — asked in a case
    // neither the client nor the handler wrote.
    let (status, body) = ask(&address, b"GET /who HTTP/1.1\r\nHOST: example.org\r\n\r\n");
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert_eq!(body, "you asked example.org");

    let (status, body) = ask(&address, b"GET /nowhere HTTP/1.1\r\nhost: x\r\n\r\n");
    assert_eq!(
        status, "HTTP/1.1 404 Not Found",
        "and the program decides what it does not have"
    );
    assert_eq!(body, "not found");

    // **A body by `Content-Length`**, which is the whole of the MVP's framing.
    let (status, body) = ask(
        &address,
        b"POST /x HTTP/1.1\r\nhost: x\r\ncontent-length: 5\r\n\r\nhello",
    );
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert_eq!(body, "you sent 5 bytes", "the handler read the body");

    // **The refusals, each of them answered rather than dropped** — a malformed
    // request is one client's problem, and a server that stopped for it would be
    // answering the next client with silence.
    for (asked, expected, why) in [
        (
            b"PUT /x HTTP/1.1\r\nhost: x\r\n\r\n".to_vec(),
            "HTTP/1.1 400 Bad Request",
            "400 a method this does not answer",
        ),
        (
            b"GET / HTTP/9.9\r\nhost: x\r\n\r\n".to_vec(),
            "HTTP/1.1 400 Bad Request",
            "400 a version this does not speak",
        ),
        (
            b"nonsense\r\n\r\n".to_vec(),
            "HTTP/1.1 400 Bad Request",
            "400 a request line that is not three words",
        ),
        (
            b"GET / HTTP/1.1\r\ntransfer-encoding: chunked\r\n\r\n".to_vec(),
            "HTTP/1.1 400 Bad Request",
            "400 a transfer-encoding this does not speak",
        ),
        (
            // Over `head_cap`'s 16 KiB default, and refused as soon as the
            // buffer is over it rather than at the end of a head that never
            // comes.
            [
                b"GET /".to_vec(),
                vec![b'a'; 20_000],
                b" HTTP/1.1\r\n\r\n".to_vec(),
            ]
            .concat(),
            "HTTP/1.1 431 Request Header Fields Too Large",
            "431 a request head over the cap",
        ),
        (
            // Over `body_cap`'s 1 MiB, refused for **saying** so: not a byte of
            // the body is read first, which is the only point at which refusing
            // is cheap.
            b"POST / HTTP/1.1\r\ncontent-length: 99999999\r\n\r\n".to_vec(),
            "HTTP/1.1 413 Content Too Large",
            "413 a body over the cap",
        ),
    ] {
        let (status, body) = ask(&address, &asked);
        assert_eq!(status, expected, "{why}");
        assert_eq!(body, why);
    }
}

/// The child, killed whatever way the test ends.
struct Stop<'a>(&'a mut std::process::Child);

impl Drop for Stop<'_> {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The address the server bound, off the line it printed.
///
/// One line and then nothing more is read: the pipe stays open for the rest of
/// the run, and a read to the end would wait for a server to stop.
fn the_address_it_printed(server: &mut std::process::Child) -> String {
    use std::io::BufRead;
    let out = server.stdout.take().expect("the child's stdout is a pipe");
    let mut line = String::new();
    std::io::BufReader::new(out)
        .read_line(&mut line)
        .expect("the server says which address it bound");
    line.trim()
        .rsplit_once(' ')
        .map(|(_, address)| address.to_string())
        .unwrap_or_else(|| panic!("`http: listening on <address>`, and this said {line:?}"))
}

/// One request on its own connection, answered as a status line and a body.
///
/// A connection each, because `Connection: close` is the MVP's scope and the
/// server implements it that way: the answer goes out and the connection ends,
/// which is also what makes reading to the end of it the right stop.
fn ask(address: &str, request: &[u8]) -> (String, String) {
    use std::io::{Read, Write};
    let mut socket = std::net::TcpStream::connect(address).expect("the server is listening");
    socket
        .set_read_timeout(Some(std::time::Duration::from_secs(20)))
        .expect("a read that cannot hang the suite");
    socket.write_all(request).expect("the request goes out");
    socket.flush().expect("all of it");
    let mut answered = Vec::new();
    // A refused request whose sender is still writing gets a reset rather than
    // the answer, which is what a closing server looks like from the other end -
    // so what arrived before it counts, and only an empty answer is a failure.
    let _ = socket.read_to_end(&mut answered);
    let answered = String::from_utf8_lossy(&answered).into_owned();
    let (head, body) = answered
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("a head and a body, and this said {answered:?}"));
    let status = head.split("\r\n").next().unwrap_or_default().to_string();
    (status, body.to_string())
}

/// **A trait a package publishes is implemented and then called**
/// ([ADR-095](../../../docs/specification/adr/adr-095.md)).
///
/// The emitter wrote the `impl` and never brought the trait into scope, so the
/// method could not be called and the language below said so in its own words:
/// *"trait `Handler` which provides `handle` is implemented but not in scope;
/// perhaps you want to import it"* — [Part III C.1](../../../docs/specification/30-nikaia-tooling.md)'s
/// class, naming a rule the program has no way to satisfy, because
/// [ADR-046](../../../docs/specification/adr/adr-046.md) D2 gives Nikaia no
/// import to write.
///
/// **It has to be two packages.** The same shape in one file compiles, because
/// there the trait is in the same module and needs no import — which is what
/// said the gap was one emitted line rather than a question. `tests/traits.rs`
/// builds one unit and cannot see this.
///
/// The body is a struct literal rather than a call to `handler::plain()`, and
/// that is not tidiness: a call out of the unit is `NK1129` today
/// (issue #228, *a `sync` body is refused as pausing when the call leaves
/// the unit*), so a fixture that made one would be testing two things and
/// failing for the other.
#[test]
fn a_trait_a_package_publishes_can_be_implemented_and_called() {
    let dir = a_program_and_a_package(
        "foreign-trait",
        "handler",
        &[
            (
                "handler/src/main.nika",
                "pub struct Answer {\n\
                 \x20   pub text: String,\n\
                 }\n\
                 \n\
                 pub trait Handler {\n\
                 \x20   fn handle(ref self) -> Answer\n\
                 }\n\
                 \n\
                 pub fn render(answer: ref Answer) -> String {\n\
                 \x20   return answer.text.clone()\n\
                 }\n",
            ),
            (
                "app/src/main.nika",
                "use handler\n\n\
                 struct Fixed {\n\
                 \x20   n: i64,\n\
                 }\n\
                 \n\
                 impl handler::Handler for Fixed {\n\
                 \x20   fn handle(ref self) -> handler::Answer {\n\
                 \x20       return handler::Answer { text: \"handled\" }\n\
                 \x20   }\n\
                 }\n\
                 \n\
                 fn main() {\n\
                 \x20   let f = Fixed { n: 1 }\n\
                 \x20   println(f\"{handler::render(f.handle())}\")\n\
                 }\n",
            ),
        ],
    );
    let app = dir.join("app");
    let ran = nikaia(&["run"], &app);
    assert!(
        ran.status.success(),
        "a trait reached across a package is callable: {}",
        said(&ran)
    );
    assert_eq!(String::from_utf8_lossy(&ran.stdout).trim(), "handled");

    // **Not asserted against the generated file**, and the reason is worth a
    // line: a package's crate names the `.nika` source as its `path`, so there
    // is no `.rs` on disk to read. What this test has instead is the stronger
    // thing - the program runs, which it cannot do without the import.
}

/// **A dependency's ledger is re-derived before its consumer reads it**, which
/// is why [ADR-100](../../../docs/specification/adr/adr-100.md) D6's message
/// has nothing to fire on today.
///
/// D6 asks for a sentence to say when a *hand-edited* ledger makes the backend
/// complain at a package boundary — *the ledger of `<package>` does not match
/// its sources*. Getting there needs a ledger the build believed and could not
/// check, and a project build has none: `drive` lowers every member
/// dependencies-first and writes each one's `nikaia.contracts` **before** the
/// member that depends on it is lowered (D5), so an edit is overwritten by the
/// package's own build in the same run. Under `--locked` nothing is
/// overwritten and the comparison fails with D4's narrated diff, which is the
/// message that case is owed.
///
/// So the edit here is the strongest one available — `sync` taken off a
/// function that has it, which is the claim that would make the consumer
/// `.await` an `i64` — and the build repairs it and runs. **The day a ledger
/// can be believed without being checked** (a registry package's,
/// [ADR-103](../../../docs/specification/adr/adr-103.md); a hand-written
/// description of a foreign crate,
/// [ADR-104](../../../docs/specification/adr/adr-104.md)) this test is what
/// says so, and D6 becomes writable and testable in the same change.
#[test]
fn a_hand_edited_dependency_ledger_is_repaired_before_it_is_read() {
    let dir = a_program_and_a_package(
        "package-edited-ledger",
        "lib",
        &[
            (
                "lib/src/main.nika",
                "pub fn hello() -> i64 {\n    return 1\n}\n",
            ),
            (
                "app/src/main.nika",
                "use lib\n\nfn main() {\n    println(f\"{lib::hello()}\")\n}\n",
            ),
        ],
    );

    // One build to get a ledger beside the package, which is where a consumer
    // reads it from (D1).
    let first = nikaia(&["build"], &dir.join("app"));
    assert!(first.status.success(), "{}", said(&first));
    let shipped = dir.join("lib/nikaia.contracts");
    let written = std::fs::read_to_string(&shipped).expect("the package's ledger");
    assert!(
        written.contains("sync = \"inferred\""),
        "`hello` calls nothing that pauses: {written}"
    );

    // The edit: the promise taken away, with the source hashes left alone so
    // the belief in D3 still stands.
    std::fs::write(&shipped, written.replace("sync = \"inferred\"\n", "")).expect("edit it");

    let again = nikaia(&["run"], &dir.join("app"));
    assert!(
        again.status.success(),
        "the edit was repaired rather than believed: {}",
        said(&again)
    );
    assert_eq!(String::from_utf8_lossy(&again.stdout).trim(), "1");
    assert!(
        std::fs::read_to_string(&shipped)
            .expect("the ledger")
            .contains("sync = \"inferred\""),
        "the package's own build wrote its answer back over the edit"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// **Cargo's target-info probe survives whatever is on standard input**
/// ([ADR-166](../../../docs/specification/adr/adr-166.md) D1).
///
/// Cargo asks every wrapper what the target looks like by running
/// `rustc - --print=…` — `-` meaning *the program is on standard input* — and
/// writes nothing there. Inherited, that standard input is **whoever started
/// the build**, and anything sitting in it is read as a Rust program. What the
/// build said then was *failed to run `rustc` to learn about target-specific
/// information*, with a parse error about text nobody offered as source; the
/// text in the sighting that found this was another process's rendered
/// warning, arriving whole.
///
/// So: the real probe, through the real wrapper, with a rendered diagnostic on
/// its standard input.
///
/// The contaminant is handed over as a **file** rather than written into a
/// pipe: a file is already there when the child starts, so nothing about this
/// test depends on the order two processes reach a write.
///
/// **It runs in every sweep again** (0.0.215). It was `#[ignore]`d while
/// issue #201 said it flaked inside a fully parallel `-p nikaia`
/// run; measured again, it passed in two full parallel sweeps, in the first run
/// after a rebuild and beside the tests that share Cargo's package cache, and
/// without [ADR-166](../../../docs/specification/adr/adr-166.md) D1 it still
/// reproduces the original error every time.
#[test]
fn the_target_info_probe_ignores_what_is_on_standard_input() {
    let dir = common::scratch_dir("probe-stdin");
    let contaminant = dir.join("stdin.txt");
    std::fs::write(
        &contaminant,
        "warning: trait `Foo` is never used\n --> src/lib.rs:1:7\n",
    )
    .expect("write the contaminant");
    let fed = std::fs::File::open(&contaminant).expect("open the contaminant");

    let rustc = std::env::var("NIKAIA_RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let probe = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .arg(&rustc)
        .args([
            "-",
            "--crate-name",
            "___",
            "--print=file-names",
            "--crate-type",
            "bin",
            "--print=sysroot",
            "--print=cfg",
            "-Wwarnings",
        ])
        .env("NIKAIA_RUSTC_WRAPPER", "1")
        .stdin(std::process::Stdio::from(fed))
        .output()
        .expect("the wrapper runs");

    assert!(
        probe.status.success(),
        "the probe compiled its standard input instead of answering:\n{}",
        String::from_utf8_lossy(&probe.stderr)
    );
    let said = String::from_utf8_lossy(&probe.stdout);
    assert!(said.contains("target_arch="), "{said}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// **A grammar runs in a project build too**, which it did not
/// ([ADR-177](../../../docs/specification/adr/adr-177.md), 0.0.125).
///
/// The feature shipped able to lower a loose `.nika` file and unable to build
/// the project shape anybody would actually write it in. The cause was a
/// *second* lowering: `Project::report` rebuilds the source map whenever the
/// backend had anything to say — **a warning counts**, and the generated crate
/// emits one for the `grammar!` macro's `cfg` — and it rebuilt it with
/// `Reads::none()`. So that pass evaluated the `comptime` to nothing, the
/// emitter refused an item the build had already written a `const` for, and
/// what reached the user was this compiler's own internal sentence instead of
/// the program running.
///
/// **This test builds and runs**, because that is the only thing that sees it:
/// every unit test of the lowering passed throughout, and so did the loose-file
/// path the corpus sweep exercises.
#[test]
fn a_grammar_at_build_time_reaches_a_project_build() {
    let dir = a_project(
        "project-grammar",
        "[package]\nname = \"settings\"\nversion = \"0.1.0\"\n",
        "pub struct Setting {\n\
         \x20   key: ref String,\n\
         \x20   value: ref String,\n\
         }\n\
         \n\
         grammar Cfg {\n\
         \x20   rule WSE = multispace1 { }\n\
         \x20   rule WS = (WSE | COMMENT)* { }\n\
         \x20   rule COMMENT = \"#\" until(line_ending) { }\n\
         \x20   rule NAME -> ref String = s:raw_ident { s }\n\
         \x20   rule VALUE -> ref String = s:until(\"#\" | line_ending) { s.trim() }\n\
         \x20   rule setting -> Setting = key:NAME \"=\" value:VALUE { Setting { key, value } }\n\
         \x20   pub rule file -> Vec[Setting] = settings:setting* { settings }\n\
         }\n\
         \n\
         comptime SETTINGS: Array[Setting, 2] = \
         Cfg::file(\"host = example.com\\nport = 8080  # the usual one\\n\")\n\
         \n\
         fn main() {\n\
         \x20   for s in SETTINGS {\n\
         \x20       println(f\"{s.key}={s.value}\")\n\
         \x20   }\n\
         }\n",
    );

    let ran = nikaia(&["run"], &dir);
    assert!(ran.status.success(), "{}", said(&ran));
    let printed = String::from_utf8_lossy(&ran.stdout);
    assert!(
        printed.contains("host=example.com") && printed.contains("port=8080"),
        "the parse happened while the program was built: {}",
        said(&ran)
    );
    // **And the backend has nothing to say about it** (0.0.230, issue #163 before it):
    // the expansion tests `feature = "trace"` in this crate, and the generated
    // manifest declares it, so `rustc` has no unexpected `cfg` to warn about.
    let stderr = String::from_utf8_lossy(&ran.stderr);
    assert!(
        !stderr.contains("unexpected `cfg`") && !stderr.contains("warning"),
        "a correct program builds without the backend's words: {stderr}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// **A shape walk reaches across a file of the same package**, which it did not
/// ([ADR-181](../../../docs/specification/adr/adr-181.md) D2, 0.0.130).
///
/// `T::fields` shipped at 0.0.129 able to unroll a function whose calls stood
/// in the **same file** and unable to build the shape anybody would write it
/// in: a helper in `shapes.nika`, called from `main.nika`. Both halves of the
/// walk were collected per unit — which functions walk a shape (off the items
/// of `self.parsed`) and which types the calls gave them (off the calls in
/// `self.parsed`) — and the relation crosses files, because the files of a
/// package share one namespace (Part I 9.1).
///
/// So the declaring unit wrote **no copy** (it saw no call) and **no generic
/// original** (it knew the function walks a shape), and what reached the user
/// was *cannot find function `describe` in this scope* about a function
/// declared two lines up in the package —
/// [Part III C.2](../../../docs/specification/30-nikaia-tooling.md)'s way out
/// that cannot be taken.
///
/// **This test builds and runs.** Every unit test of the unrolling passed
/// throughout, and so did the single-file path the corpus sweep exercises.
#[test]
fn a_shape_walk_reaches_across_the_files_of_a_package() {
    let dir = a_project(
        "project-reflection",
        "[package]\nname = \"reflect\"\nversion = \"0.1.0\"\n",
        "fn main() {\n\
         \x20   describe(User { name: \"ada\", age: 36 })\n\
         \x20   describe(Point { x: 1, y: 2 })\n\
         }\n",
    );
    std::fs::write(
        dir.join("src/shapes.nika"),
        "struct User { name: ref String, age: i64 }\n\
         struct Point { x: i64, y: i64 }\n\
         \n\
         fn describe[T: Struct](value: T) {\n\
         \x20   for field in T::fields {\n\
         \x20       println(f\"{field.name} = {field.of(value)}\")\n\
         \x20   }\n\
         }\n",
    )
    .expect("the second file");

    let ran = nikaia(&["run"], &dir);
    assert!(ran.status.success(), "{}", said(&ran));
    let printed = String::from_utf8_lossy(&ran.stdout);
    assert!(
        printed.contains("name = ada")
            && printed.contains("age = 36")
            && printed.contains("x = 1")
            && printed.contains("y = 2"),
        "one copy per type, from a function a file away: {}",
        said(&ran)
    );

    // **And the report says the same thing**, which is the other half of the
    // defect: `--comptime` read one unit's tables, so it printed nothing at all
    // for this program.
    let told = nikaia(&["build", "--comptime"], &dir);
    assert!(told.status.success(), "{}", said(&told));
    let report = String::from_utf8_lossy(&told.stdout);
    assert!(
        report.contains("`describe` unrolled over `User` as `describe__User`")
            && report.contains("`describe` unrolled over `Point` as `describe__Point`"),
        "{}",
        said(&told)
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// **A head the package's *other* file declares is not `NK1181`**
/// ([ADR-183](../../../docs/specification/adr/adr-183.md) D1).
///
/// The files of a package share one namespace (Part I 9.1), so `Shade::Even`
/// in `main.nika` names an `enum` that may stand in `shapes.nika` — and the
/// tables the refusal asks first are this **unit's**. Written against those
/// alone the refusal is a correct program refused
/// ([Part III C.4](../../../docs/specification/30-nikaia-tooling.md)), which
/// is the one thing this compiler may never do, and the same shape
/// [ADR-182](../../../docs/specification/adr/adr-182.md)'s package had one
/// construct over.
///
/// **This builds and runs**, because a single-file test cannot see it: every
/// unit test of the refusal passed while this program did not build.
#[test]
fn a_head_the_next_file_declares_is_not_refused() {
    let dir = a_project(
        "project-head",
        "[package]\nname = \"shades\"\nversion = \"0.1.0\"\n",
        "fn main() {\n\
         \x20   let s = Shade::Even\n\
         \x20   match s {\n\
         \x20       Shade::Even => { println(\"even\") }\n\
         \x20       Shade::Odd => { println(\"odd\") }\n\
         \x20   }\n\
         }\n",
    );
    std::fs::write(dir.join("src/shades.nika"), "enum Shade { Even, Odd }\n")
        .expect("the second file");

    let ran = nikaia(&["run"], &dir);
    assert!(ran.status.success(), "{}", said(&ran));
    assert!(
        String::from_utf8_lossy(&ran.stdout).contains("even"),
        "{}",
        said(&ran)
    );

    // **And a head nothing declares is still refused**, in this compiler's
    // words rather than the language below's *unresolved module or unlinked
    // crate*.
    std::fs::write(
        dir.join("src/main.nika"),
        "fn main() {\n\
         \x20   println(f\"{nowhere::wobble}\")\n\
         }\n",
    )
    .expect("rewrite the entry");
    let refused = nikaia(&["run"], &dir);
    assert!(!refused.status.success(), "{}", said(&refused));
    let told = said(&refused);
    assert!(
        told.contains("NK1181") && told.contains("but `nowhere` isn't declared anywhere"),
        "{told}"
    );
    assert!(
        !told.contains("unlinked crate"),
        "the language below does not get to answer this: {told}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// **A grammar entry in tail position over a local that owns its input**
/// ([ADR-185](../../../docs/specification/adr/adr-185.md) D2, closing
/// issue #139's entry for it at 0.0.136).
///
/// The lowering of an entry is a block holding `let _source = &*data` and a
/// stream over it. In `Ok({ … }?)` those temporaries live to the end of the
/// enclosing block — **past the local that owns the text** — and `rustc` said
/// *`data` does not live long enough* about a file nobody wrote
/// ([Part III C.1](../../../docs/specification/30-nikaia-tooling.md)). Its own
/// hint was the fix: *save the expression's value in a new local variable*.
///
/// **It was the tail and not the grammar**: the same call bound to a name
/// first compiled and ran, and so did the tail form where the input is a
/// **parameter**, because then no local owns the buffer.
/// `examples/report/src/main.nika` writes the first shape, which is why the corpus was
/// green while this was broken.
///
/// **This builds and runs**, because that is the only thing that sees it: the
/// emitted text looked right and `rustc` is what refused it.
#[test]
fn a_grammar_entry_in_tail_position_over_a_local_runs() {
    let dir = a_project(
        "project-tail-entry",
        "[package]\nname = \"tail\"\nversion = \"0.1.0\"\n",
        "grammar Tiny {\n\
         \x20   rule WS = multispace0 { }\n\
         \x20   pub rule number -> i64 = n:dec[i64](digit+) { n }\n\
         }\n\
         \n\
         fn both(text: ref String) -> i64 throws {\n\
         \x20   let data = text.clone()\n\
         \x20   return Tiny::number(data)\n\
         }\n\
         \n\
         fn main() throws {\n\
         \x20   println(f\"{both(\"42\")}\")\n\
         }\n",
    );

    let ran = nikaia(&["run"], &dir);
    assert!(ran.status.success(), "{}", said(&ran));
    assert!(
        String::from_utf8_lossy(&ran.stdout).contains("42"),
        "{}",
        said(&ran)
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// **A bound takes a path, and a trait a package publishes is one a consumer may
/// name** ([ADR-106](../../../docs/specification/adr/adr-106.md) D1 and D3).
///
/// `fn tell[T: greet::Speaks](…)` was a parse error at the `:`,
/// `expected one of: +, ,`, and `Ledger::traits` lived in memory and was never
/// written — so nothing outside a unit could name one of its traits.
/// [ADR-078](../../../docs/specification/adr/adr-078.md) §4 left that as *a
/// question about modules*; D1 and D3 answered it, and this is the pair built.
///
/// **It has to be two packages.** Inside one, a module's names are the package's
/// (`ADR-047` D1), so a bound on a trait of another file needs no path at all
/// and a one-unit test cannot see this.
#[test]
fn a_bound_takes_a_path_across_a_package() {
    let dir = a_program_and_a_package(
        "bound-path",
        "handler",
        &[
            (
                "handler/src/main.nika",
                "pub struct Answer {\n\
                 \x20   pub text: String,\n\
                 }\n\
                 \n\
                 pub trait Handler {\n\
                 \x20   fn handle(ref self) -> Answer\n\
                 }\n",
            ),
            (
                "app/src/main.nika",
                "use handler\n\n\
                 struct Fixed {\n\
                 \x20   n: i64,\n\
                 }\n\
                 \n\
                 impl handler::Handler for Fixed {\n\
                 \x20   fn handle(ref self) -> handler::Answer {\n\
                 \x20       return handler::Answer { text: \"handled\" }\n\
                 \x20   }\n\
                 }\n\
                 \n\
                 fn dispatch[H: handler::Handler](h: H) -> String {\n\
                 \x20   return h.handle().text\n\
                 }\n\
                 \n\
                 fn main() {\n\
                 \x20   let f = Fixed { n: 1 }\n\
                 \x20   println(dispatch(f))\n\
                 }\n",
            ),
        ],
    );
    let ran = nikaia(&["run"], &dir.join("app"));
    assert!(ran.status.success(), "{}", said(&ran));
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).trim(),
        "handled",
        "the call through the bound resolves to the implementing type's own entry (D5)"
    );

    // **The trait is in the package's ledger file**, which is what made the
    // bound answerable: the table carries the one word a checker needs and no
    // methods, because its methods are the `fn` entries beside it.
    let ledger = std::fs::read_to_string(dir.join("handler/nikaia.contracts"))
        .expect("the package writes its ledger");
    assert!(ledger.contains("[trait.\"Handler\"]"), "{ledger}");
    assert!(ledger.contains("[fn.\"Handler::handle\"]"), "{ledger}");
}

/// **A bound a package declares is checked at a consumer's call**
/// ([ADR-205](../../../docs/specification/adr/adr-205.md) D1).
///
/// It was the one thing left in issue #162: `Checker::declared_bounds` is
/// built from the AST of the unit being checked, under the key a call in *that*
/// unit resolves to, and a call written `handler::dispatch` resolves to a key no
/// unit's AST produced. So what a reader got was the backend's words on their own
/// line — *the trait bound `Bare: Handler` is not satisfied*, with `Handler`
/// written without the path the program must write and `Static` named as an
/// implementor the program never mentions ([Part III
/// C.1](../../../docs/specification/30-nikaia-tooling.md)).
///
/// **Both directions**, because the refusal is only worth having if the correct
/// program still runs.
#[test]
fn a_bound_a_package_declares_is_checked_at_a_consumers_call() {
    let package = (
        "handler/src/main.nika",
        "pub struct Answer {\n         \x20   pub text: String,\n         }\n         \n         pub trait Handler {\n         \x20   fn handle(ref self) -> Answer\n         }\n         \n         pub fn dispatch[H: Handler](h: H) -> String {\n         \x20   return h.handle().text\n         }\n",
    );

    // **The type that implements nothing**, which is the shape issue #162 recorded.
    let refused = a_program_and_a_package(
        "package-bound-refused",
        "handler",
        &[
            package,
            (
                "app/src/main.nika",
                "use handler\n\n                 struct Bare {\n                 \x20   n: i64,\n                 }\n                 \n                 fn main() {\n                 \x20   println(handler::dispatch(Bare { n: 1 }))\n                 }\n",
            ),
        ],
    );
    let built = nikaia(&["build"], &refused.join("app"));
    assert!(!built.status.success(), "{}", said(&built));
    let told = said(&built);
    assert!(told.contains("NK1164"), "{told}");
    // **The path the program must write**, which the backend's message left out.
    assert!(told.contains("handler::Handler"), "{told}");
    assert!(
        told.contains("impl handler::Handler for Bare"),
        "the help is one a program can copy:\n{told}"
    );
    // And nothing names an implementor this program never mentions.
    assert!(!told.contains("is implemented for"), "{told}");
    std::fs::remove_dir_all(&refused).ok();

    // **The `impl` written, and it runs.** A refusal that also refused this would
    // be the worse mistake ([Part III C.4](../../../docs/specification/30-nikaia-tooling.md)).
    let allowed = a_program_and_a_package(
        "package-bound-met",
        "handler",
        &[
            package,
            (
                "app/src/main.nika",
                "use handler\n\n                 struct Fixed {\n                 \x20   n: i64,\n                 }\n                 \n                 impl handler::Handler for Fixed {\n                 \x20   fn handle(ref self) -> handler::Answer {\n                 \x20       return handler::Answer { text: \"handled\" }\n                 \x20   }\n                 }\n                 \n                 fn main() {\n                 \x20   println(handler::dispatch(Fixed { n: 1 }))\n                 }\n",
            ),
        ],
    );
    let ran = nikaia(&["run"], &allowed.join("app"));
    assert!(ran.status.success(), "{}", said(&ran));
    assert_eq!(String::from_utf8_lossy(&ran.stdout).trim(), "handled");

    // **The bound is in the package's ledger file**, inside the one string a
    // caller already parses.
    let ledger = std::fs::read_to_string(allowed.join("handler/nikaia.contracts"))
        .expect("the package writes its ledger");
    assert!(
        ledger.contains(r#"signature = "[H: Handler](h: H) -> String""#),
        "{ledger}"
    );
    std::fs::remove_dir_all(&allowed).ok();
}

/// **An `enum` a package declares is a type a consumer can `match` totally**
/// (Part I 3.4).
///
/// Nothing wrote an `enum` to the ledger at all, so a `match` over one of a
/// dependency's could not be shown complete: `NK1151` asked for an `else`, which
/// is a correct program refused ([Part III
/// C.4](../../../docs/specification/30-nikaia-tooling.md)) *and* a way out that
/// makes **a type gaining a variant** silent in that program forever after —
/// which is the one thing [ADR-146](../../../docs/specification/adr/adr-146.md)
/// D1 exists to prevent.
///
/// **It has to be two packages**, for the reason the bound above needs two:
/// inside one, an `enum`'s cases come from the source.
#[test]
fn an_enum_a_package_declares_is_matched_completely_by_a_consumer() {
    let dir = a_program_and_a_package(
        "package-enum",
        "verdict",
        &[
            (
                "verdict/src/main.nika",
                "pub enum Answer {\n                 \x20   Yes,\n                 \x20   No,\n                 }\n                 \n                 pub fn of(word: ref String) -> Answer {\n                 \x20   if word == \"ja\" {\n                 \x20       return Answer::Yes\n                 \x20   }\n                 \x20   return Answer::No\n                 }\n",
            ),
            (
                "app/src/main.nika",
                "use verdict\n\n                 fn main() {\n                 \x20   let word = \"ja\"\n                 \x20   match verdict::of(word) {\n                 \x20       verdict::Answer::Yes => { println(\"yes\") }\n                 \x20       verdict::Answer::No => { println(\"no\") }\n                 \x20   }\n                 \x20   let again = \"nein\"\n                 \x20   if verdict::of(again) == verdict::Answer::No {\n                 \x20       println(\"compared\")\n                 \x20   }\n                 }\n",
            ),
        ],
    );
    let ran = nikaia(&["run"], &dir.join("app"));
    assert!(ran.status.success(), "{}", said(&ran));
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).trim(),
        "yes\ncompared",
        "the `match` is total and the comparison has a lowering"
    );

    // **The cases are in the package's ledger file**, which is what made the
    // `match` answerable — the same table a `struct`'s `fields` are in, for the
    // other shape of type.
    let ledger = std::fs::read_to_string(dir.join("verdict/nikaia.contracts"))
        .expect("the package writes its ledger");
    assert!(ledger.contains("[type.\"Answer\"]"), "{ledger}");
    assert!(ledger.contains("variants = [\"Yes\", \"No\"]"), "{ledger}");
}

/// **And the `impl` may be the package's**
/// ([ADR-106](../../../docs/specification/adr/adr-106.md) D4): *whether a type
/// implements a trait is the union of what the program and its dependencies'
/// ledgers say*, so a consumer that writes no `impl` of its own gets the answer
/// from the file.
///
/// The one that was missing until the table was written: the ledger recorded
/// `Handler for Static` under the **unqualified** trait name, and a bound asks
/// under `handler::Handler`.
#[test]
fn an_impl_a_package_wrote_answers_a_consumers_bound() {
    let dir = a_program_and_a_package(
        "bound-impl",
        "handler",
        &[
            (
                "handler/src/main.nika",
                "pub struct Answer {\n\
                 \x20   pub text: String,\n\
                 }\n\
                 \n\
                 pub trait Handler {\n\
                 \x20   fn handle(ref self) -> Answer\n\
                 }\n\
                 \n\
                 pub struct Static {\n\
                 \x20   pub what: String,\n\
                 }\n\
                 \n\
                 impl Handler for Static {\n\
                 \x20   fn handle(ref self) -> Answer {\n\
                 \x20       return Answer { text: self.what.clone() }\n\
                 \x20   }\n\
                 }\n",
            ),
            (
                "app/src/main.nika",
                "use handler\n\n\
                 fn dispatch[H: handler::Handler](h: H) -> String {\n\
                 \x20   return h.handle().text\n\
                 }\n\
                 \n\
                 fn main() {\n\
                 \x20   let s = handler::Static { what: \"from the package\" }\n\
                 \x20   println(dispatch(s))\n\
                 }\n",
            ),
        ],
    );
    let ran = nikaia(&["run"], &dir.join("app"));
    assert!(ran.status.success(), "{}", said(&ran));
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).trim(),
        "from the package"
    );
    let ledger = std::fs::read_to_string(dir.join("handler/nikaia.contracts"))
        .expect("the package writes its ledger");
    assert!(ledger.contains("[impl.\"Handler for Static\"]"), "{ledger}");
}

/// **And a type that answers for nothing is still refused**, which is the half a
/// wider lookup could have taken away: an extra spelling makes a check fail
/// *open*, so the test that matters is the one where it must not.
#[test]
fn a_type_that_implements_nothing_is_refused_at_a_path_bound() {
    let dir = a_program_and_a_package(
        "bound-path-refused",
        "handler",
        &[
            (
                "handler/src/main.nika",
                "pub struct Answer {\n\
                 \x20   pub text: String,\n\
                 }\n\
                 \n\
                 pub trait Handler {\n\
                 \x20   fn handle(ref self) -> Answer\n\
                 }\n",
            ),
            (
                "app/src/main.nika",
                "use handler\n\n\
                 struct Bare {\n\
                 \x20   n: i64,\n\
                 }\n\
                 \n\
                 fn dispatch[H: handler::Handler](h: H) -> String {\n\
                 \x20   return h.handle().text\n\
                 }\n\
                 \n\
                 fn main() {\n\
                 \x20   println(dispatch(Bare { n: 1 }))\n\
                 }\n",
            ),
        ],
    );
    let built = nikaia(&["build"], &dir.join("app"));
    assert!(!built.status.success(), "{}", said(&built));
    let said = said(&built);
    assert!(said.contains("NK1164"), "{said}");
    assert!(
        said.contains("`handler::Static`") || said.contains("`Bare` isn't one"),
        "the message names the type the caller picked: {said}"
    );
}

/// **A package's own bound is answered in the package's own namespace**
/// (issue #159, closed).
///
/// `pub fn dispatch[H: Handler](h: H)` compiled on its own and was `NK1126` —
/// *nothing says it has a method* — the moment a program depended on the
/// package, **without the program calling it**: every unit of a build is
/// checked, and a dependency's was checked against the *program's* ledger, whose
/// keys `absorb` had qualified. The package's own file writes the bare word, as
/// its author must ([ADR-046](../../../docs/specification/adr/adr-046.md) D2
/// gives no import to write).
///
/// **And the first fix was wrong in a way this test would not have caught**, so
/// the second claim below is here on purpose: un-qualifying the *keys* left the
/// types **inside** each signature qualified, and `examples/http`'s own
/// `answer(connection, handler)` was told that its `fn(Request) -> Response` is
/// not a `fn(http::Request) -> http::Response`. What the checker gets now is the
/// ledger that package was inferred with, kept rather than reconstructed.
#[test]
fn a_packages_own_bound_and_its_own_types_are_answered_in_its_own_namespace() {
    let dir = a_program_and_a_package(
        "own-namespace",
        "handler",
        &[
            (
                "handler/src/main.nika",
                "pub struct Answer {\n\
                 \x20   pub text: String,\n\
                 }\n\
                 \n\
                 pub trait Handler {\n\
                 \x20   fn handle(ref self) -> Answer\n\
                 }\n\
                 \n\
                 pub struct Static {\n\
                 \x20   pub what: String,\n\
                 }\n\
                 \n\
                 impl Handler for Static {\n\
                 \x20   fn handle(ref self) -> Answer {\n\
                 \x20       return Answer { text: self.what.clone() }\n\
                 \x20   }\n\
                 }\n\
                 \n\
                 // The bound is the package's own, written without a path\n\
                 pub fn dispatch[H: Handler](h: H) -> String {\n\
                 \x20   return say(h.handle())\n\
                 }\n\
                 \n\
                 // And a parameter whose type is the package's own, which is\n\
                 // what the first fix broke: the key was bare and the type was\n\
                 // not.\n\
                 pub fn say(answer: Answer) -> String {\n\
                 \x20   return answer.text.clone()\n\
                 }\n",
            ),
            (
                "app/src/main.nika",
                "use handler\n\n\
                 fn main() {\n\
                 \x20   let s = handler::Static { what: \"its own\" }\n\
                 \x20   println(handler::dispatch(s))\n\
                 }\n",
            ),
        ],
    );
    let ran = nikaia(&["run"], &dir.join("app"));
    assert!(ran.status.success(), "{}", said(&ran));
    assert_eq!(String::from_utf8_lossy(&ran.stdout).trim(), "its own");
}

/// **A published parameter both kinds of text flow into**
/// ([ADR-232](../../../docs/specification/adr/adr-232.md) D1): the package
/// hands its own `pub fn` a view and text of its own, and a program in
/// another package hands it text of its own as the ledger says. Refused
/// before, with `.clone()` named for the package's view.
#[test]
fn a_published_parameter_takes_either_kind_from_its_package_and_its_own_from_others() {
    let dir = a_program_and_a_package(
        "published-either",
        "tags",
        &[
            (
                "tags/src/main.nika",
                "pub struct Tag {\n    label: String,\n}\n\n\
                 impl Tag {\n    pub fn label(ref self) -> String {\n        return f\"[{self.label}]\"\n    }\n}\n\n\
                 pub fn tag(s: String) -> Tag {\n    return Tag { label: s }\n}\n\n\
                 pub fn both(text: ref String) -> String {\n\
                 \x20   let a = tag(text.trim())\n\
                 \x20   let b = tag(f\"own\")\n\
                 \x20   return f\"{a.label()} {b.label()}\"\n}\n",
            ),
            (
                "app/src/main.nika",
                "use tags\n\nfn main() {\n\
                 \x20   let t = tags::tag(f\"app\")\n\
                 \x20   println(f\"{t.label()} {tags::both(\"  x  \")}\")\n}\n",
            ),
        ],
    );
    let ran = nikaia(&["run"], &dir.join("app"));
    assert!(ran.status.success(), "{}", said(&ran));
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).trim(),
        "[app] [x] [own]"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// **A cleanup point the ledger moved is narrated at the call, once**
/// ([ADR-236](../../../docs/specification/adr/adr-236.md), ADR-094 D5): the
/// committed ledger says `look` only read its `Handle`, the edited body keeps
/// it, and the build that records the change says where the cleanup went. The
/// build after it says nothing: the committed ledger is the acknowledgement.
#[test]
fn a_moved_cleanup_point_is_narrated_once() {
    const BEFORE: &str = "struct Handle {\n    name: String,\n}\n\n\
        impl Drop for Handle {\n    fn drop(ref mut self) {\n        println(f\"closing {self.name}\")\n    }\n}\n\n\
        fn look(h: Handle) -> i64 {\n    return h.name.len()\n}\n\n\
        fn main() {\n    let h = Handle { name: f\"a\" }\n    let n = look(h)\n    println(f\"end of main {n}\")\n}\n";
    let dir = a_project(
        "project-cleanup-moved",
        "[package]\nname = \"keeper\"\nversion = \"0.1.0\"\n",
        BEFORE,
    );
    let first = nikaia(&["run"], &dir);
    assert!(first.status.success(), "{}", said(&first));
    assert!(!said(&first).contains("NK2403"), "{}", said(&first));
    assert!(
        String::from_utf8_lossy(&first.stdout).contains("end of main 1\nclosing a"),
        "{}",
        said(&first)
    );

    let after = BEFORE.replace(
        "fn look(h: Handle) -> i64 {\n    return h.name.len()\n}",
        "fn look(h: Handle) -> i64 {\n    let n = h.name.len()\n    let held: Vec[Handle] = [h]\n    return n + held.len() - 1\n}",
    );
    std::fs::write(dir.join("src/main.nika"), after).expect("edit");
    let second = nikaia(&["run"], &dir);
    assert!(second.status.success(), "{}", said(&second));
    assert!(
        said(&second).contains("warning[NK2403]: `look` now keeps `h`"),
        "{}",
        said(&second)
    );
    assert!(
        String::from_utf8_lossy(&second.stdout).contains("closing a\nend of main 1"),
        "{}",
        said(&second)
    );

    let third = nikaia(&["run"], &dir);
    assert!(third.status.success(), "{}", said(&third));
    assert!(!said(&third).contains("NK2403"), "{}", said(&third));

    std::fs::remove_dir_all(&dir).ok();
}

/// A project calling one function of a path Rust crate, `fremd::drei`, which
/// hands back a `Result` - described by `nikaia describe` as it would be.
fn a_project_with_a_failing_crate(purpose: &str) -> PathBuf {
    let dir = a_project(
        purpose,
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\n\n\
         [dependencies]\nfremd = { type = \"rust\", path = \"fremd\" }\n",
        "fn main() throws {\n    let n = fremd::drei()\n    println(f\"{n + 1}\")\n}\n",
    );
    std::fs::create_dir_all(dir.join("fremd/src")).expect("the crate");
    std::fs::write(
        dir.join("fremd/Cargo.toml"),
        "[package]\nname = \"fremd\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("the crate's manifest");
    std::fs::write(
        dir.join("fremd/src/lib.rs"),
        "pub fn drei() -> Result<i64, String> { Ok(3) }\n",
    )
    .expect("the crate's source");
    let described = nikaia(&["describe", "fremd"], &dir);
    assert!(described.status.success(), "{}", said(&described));
    dir
}

/// **A described call that can fail is a place that can fail, in the Rust
/// too** ([ADR-237](../../../docs/specification/adr/adr-237.md) D1). The
/// checker read the description's `throws` and the emitter did not, so the
/// crate's `Result` reached `n + 1` and `rustc` said so about a file nobody
/// wrote.
#[test]
fn a_described_call_that_can_fail_is_propagated() {
    let dir = a_project_with_a_failing_crate("project-described-throws");
    let description =
        std::fs::read_to_string(dir.join("contracts/fremd.contracts")).expect("described");
    assert!(description.contains("throws = [\"?\"]"), "{description}");
    let run = nikaia(&["run"], &dir);
    assert!(run.status.success(), "{}", said(&run));
    assert!(
        String::from_utf8_lossy(&run.stdout).contains('4'),
        "{}",
        said(&run)
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// **A boundary the backend disagrees with is said as a stale description**
/// (ADR-100 D6, ADR-237 D2). The description is edited by hand, as D5 expects
/// a person to, and loses `throws`: the crate's sources have not moved, so
/// nothing refuses the build, and the lowering believes the description. What
/// `rustc` says about the `Result` is reported as the description of `fremd`
/// not matching the crate, with the backend's words kept beside it.
#[test]
fn a_boundary_mismatch_names_the_description_to_redo() {
    let dir = a_project_with_a_failing_crate("project-stale-description");
    let path = dir.join("contracts/fremd.contracts");
    let description = std::fs::read_to_string(&path).expect("described");
    std::fs::write(&path, description.replace("throws = [\"?\"]\n", "")).expect("edit");
    let built = nikaia(&["build"], &dir);
    assert!(!built.status.success(), "{}", said(&built));
    let complaint = said(&built);
    assert!(
        complaint.contains("The description of `fremd` doesn't match the crate."),
        "{complaint}"
    );
    assert!(complaint.contains("nikaia describe fremd"), "{complaint}");
    assert!(
        complaint.contains("= note: The Rust compiler said:"),
        "{complaint}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// **A text literal handed to a Rust crate's generic parameter is a `String`**
/// (0.0.230, issue #155 before it).
///
/// `examples/foreign-runtime/serve` stopped building when a literal began
/// lowering as `&str`: `across_a_thread<T: Describe>(value: T)` has `Describe`
/// for `String` only, and a `$T` that only this argument decides takes the
/// argument's own type. The checker said so; the emitter's own walk read `std`'s
/// ledger alone, never resolved the callee, and wrote the literal as a view.
///
/// **Here, and not only in `foreign_runtime.rs`**, because that test fetches
/// `hyper` from crates.io and is ignored, so no CI run saw the break. This crate
/// has no dependencies at all.
#[test]
fn a_literal_handed_to_a_foreign_generic_is_text_of_its_own() {
    let dir = a_project(
        "project-foreign-generic",
        "[package]\nname = \"described\"\nversion = \"0.1.0\"\n\n\
         [dependencies]\nlabels = { type = \"rust\", path = \"labels\" }\n",
        "fn main() {\n\
         \x20   let said = labels::describe(\"a literal\")\n\
         \x20   println(f\"said: {said}\")\n\
         }\n",
    );
    std::fs::create_dir_all(dir.join("labels/src")).expect("the crate's src");
    std::fs::write(
        dir.join("labels/Cargo.toml"),
        "[package]\nname = \"labels\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("the crate's manifest");
    std::fs::write(
        dir.join("labels/src/lib.rs"),
        "pub trait Describe { fn describe(&self) -> String; }\n\
         impl Describe for String { fn describe(&self) -> String { format!(\"owned {self}\") } }\n\
         pub fn describe<T: Describe + Send + 'static>(value: T) -> String { value.describe() }\n",
    )
    .expect("the crate's source");
    std::fs::create_dir_all(dir.join("contracts")).expect("contracts");
    std::fs::write(
        dir.join("contracts/labels.contracts"),
        "version = 2\n\
         inference = \"described-from-signatures\"\n\n\
         [fn.\"labels::describe\"]\n\
         pub = true\n\
         sync = true\n\
         keeps = [\"value\"]\n\
         signature = \"(value: $T) -> String\"\n",
    )
    .expect("the description");

    let ran = nikaia(&["run"], &dir);
    assert!(ran.status.success(), "{}", said(&ran));
    assert!(
        String::from_utf8_lossy(&ran.stdout).contains("said: owned a literal"),
        "{}",
        said(&ran)
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// **A file inside a project is a file of that project**
/// ([ADR-260](../../../docs/specification/adr/adr-260.md) D1): `nikaia lower` on a
/// project's entry reads the dependencies its `nikaia.toml` declares, as
/// `nikaia build` does. It used to read the `[build]` section and hand the
/// lowering no packages, so `use http` was refused with *add it to
/// `[dependencies]`* about a line that was already there (issue #130).
///
/// **And what it is checked against is part of the key**: an edit to the
/// dependency is a miss, not a lowering from cache.
#[test]
fn a_file_of_a_project_is_lowered_with_the_projects_dependencies() {
    let dir = a_program_and_a_package(
        "file-in-project",
        "http",
        &[
            (
                "http/src/main.nika",
                "pub fn ok(body: ref String) -> String {\n    \
                     return f\"200 {body}\"\n\
                 }\n",
            ),
            (
                "app/src/main.nika",
                "use http\n\nfn main() {\n    println(http::ok(\"/\"))\n}\n",
            ),
        ],
    );
    let lower = || {
        Command::new(env!("CARGO_BIN_EXE_nikaia"))
            .arg("lower")
            .arg(dir.join("app/src/main.nika"))
            .arg("--output")
            .arg(dir.join("app.rs"))
            .env("NIKAIA_CACHE_DIR", dir.join("cache"))
            .output()
            .expect("the nikaia binary runs")
    };

    let first = lower();
    assert!(first.status.success(), "{}", said(&first));
    assert!(
        std::fs::read_to_string(dir.join("app.rs"))
            .expect("the Rust is written")
            .contains("http::ok("),
        "the call reaches the package"
    );
    let again = lower();
    assert!(
        String::from_utf8_lossy(&again.stdout).contains("lowering from cache"),
        "nothing changed, so the second lowering is a hit: {}",
        said(&again)
    );

    std::fs::write(
        dir.join("http/src/main.nika"),
        "pub fn ok(body: ref String) -> String {\n    return f\"201 {body}\"\n}\n",
    )
    .expect("edit the package");
    let edited = lower();
    assert!(edited.status.success(), "{}", said(&edited));
    assert!(
        !String::from_utf8_lossy(&edited.stdout).contains("lowering from cache"),
        "the dependency changed, so the key did: {}",
        said(&edited)
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// **A file outside any project is standalone, and says so**
/// ([ADR-260](../../../docs/specification/adr/adr-260.md) D2): it has no
/// dependencies, and a `use` of one is told that - and where one is declared -
/// rather than to add a line to a manifest that does not exist.
#[test]
fn a_file_outside_a_project_is_told_it_has_no_dependencies() {
    let dir = common::scratch_dir("file-alone");
    let input = dir.join("main.nika");
    std::fs::write(&input, "use http\n\nfn main() { }\n").expect("source");

    let run = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .arg("lower")
        .arg(&input)
        .arg("--output")
        .arg(dir.join("main.rs"))
        .env("NIKAIA_CACHE_DIR", dir.join("cache"))
        .output()
        .expect("the nikaia binary runs");
    assert!(!run.status.success(), "{}", said(&run));
    let message = String::from_utf8_lossy(&run.stderr);
    assert!(
        message.contains("a single file outside a project has no dependencies"),
        "{}",
        said(&run)
    );
    assert!(message.contains("nikaia.toml"), "{}", said(&run));
    assert!(
        !message.contains("there's no dependency called"),
        "not the message for a project that forgot a line: {}",
        said(&run)
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// `nikaia run <file>`, through the cache this suite shares, so the compiled
/// `std` is the one the other project tests built.
fn run_file(file: &Path, cwd: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .arg("run")
        .arg(file)
        .current_dir(cwd)
        .env("NIKAIA_CACHE_DIR", shared_cache_dir())
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("the nikaia binary runs")
}

/// **A file outside any project runs as a program of its own**
/// ([ADR-260](../../../docs/specification/adr/adr-260.md) D4): built through
/// the same Cargo workspace `nikaia build` makes, in a project kept for it in
/// the cache, and started - not interpreted. Run again unchanged, nothing is
/// compiled: the kept copy is rewritten only when the file is, and the
/// generated Rust is written before Cargo starts (#280) - a new project was
/// compiled once more on its second build.
#[test]
fn a_file_outside_a_project_runs_as_a_program_of_its_own() {
    let dir = common::scratch_dir("run-a-file");
    let file = dir.join("hallo.nika");
    std::fs::write(&file, HELLO).expect("source");

    let first = run_file(&file, &dir);
    assert!(first.status.success(), "{}", said(&first));
    assert_eq!(
        String::from_utf8_lossy(&first.stdout).trim(),
        "Hallo Welt, Nikaia!",
        "{}",
        said(&first)
    );

    let again = run_file(&file, &dir);
    assert!(again.status.success(), "{}", said(&again));
    assert!(
        !String::from_utf8_lossy(&again.stderr).contains("Compiling"),
        "an unchanged file is not compiled again: {}",
        said(&again)
    );

    std::fs::write(&file, "fn main() {\n    println(\"zweiter Versuch\")\n}\n").expect("edit");
    let edited = run_file(&file, &dir);
    assert_eq!(
        String::from_utf8_lossy(&edited.stdout).trim(),
        "zweiter Versuch",
        "and an edited one is: {}",
        said(&edited)
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// **A refusal names the file the user wrote**, not the copy the kept project
/// holds: the file is checked where it is before anything is copied.
#[test]
fn running_a_file_that_is_refused_names_that_file() {
    let dir = common::scratch_dir("run-a-refused-file");
    let file = dir.join("kaputt.nika");
    std::fs::write(&file, "fn main() {\n    let x: i32 = \"nein\"\n}\n").expect("source");

    let run = run_file(Path::new("kaputt.nika"), &dir);
    assert!(!run.status.success(), "{}", said(&run));
    let told = String::from_utf8_lossy(&run.stderr);
    assert!(told.contains("NK1103"), "{}", said(&run));
    assert!(told.contains("kaputt.nika:2:5"), "{}", said(&run));
    assert!(
        !told.contains("src/main.nika"),
        "not the kept copy's path: {}",
        said(&run)
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// **Inside a project, the file has to be the program's entry** (D4): a
/// project has one program, and running another of its files would be running
/// something the project does not build. The refusal names the entry and both
/// ways out.
#[test]
fn a_file_of_a_project_runs_only_as_its_entry() {
    let dir = a_project("run-a-project-file", "[package]\nname = \"p\"\n", HELLO);
    std::fs::create_dir_all(dir.join("tools")).expect("tools");
    std::fs::write(dir.join("tools/x.nika"), HELLO).expect("another file");

    let other = run_file(Path::new("tools/x.nika"), &dir);
    assert!(!other.status.success(), "{}", said(&other));
    let told = String::from_utf8_lossy(&other.stderr);
    assert!(
        told.contains("is not the entry point") && told.contains("src/main.nika"),
        "{}",
        said(&other)
    );
    assert!(told.contains("nikaia run --project"), "{}", said(&other));

    let entry = run_file(Path::new("src/main.nika"), &dir);
    assert!(entry.status.success(), "{}", said(&entry));
    assert_eq!(
        String::from_utf8_lossy(&entry.stdout).trim(),
        "Hallo Welt, Nikaia!",
        "{}",
        said(&entry)
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// **`nikaia run` becomes the program it built** (#281): the process a
/// supervisor started is the program, so a signal that ends it ends the
/// program. Started as a child, the program outlived a killed `nikaia run` and
/// kept what it held - a port, for the HTTP example this suite runs.
#[cfg(target_os = "linux")]
#[test]
fn nikaia_run_becomes_the_program() {
    use std::io::BufRead;

    let dir = common::scratch_dir("run-becomes-the-program");
    let file = dir.join("waits.nika");
    std::fs::write(
        &file,
        "use std::time\n\nfn main() {\n    println(\"started\")\n    time::sleep(30000.millis())\n}\n",
    )
    .expect("source");

    let mut run = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .arg("run")
        .arg(&file)
        .env("NIKAIA_CACHE_DIR", shared_cache_dir())
        .env_remove("CARGO_TARGET_DIR")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the nikaia binary runs");
    let mut first = String::new();
    std::io::BufReader::new(run.stdout.take().expect("stdout"))
        .read_line(&mut first)
        .expect("a line");
    assert_eq!(first.trim(), "started", "the program is running");

    // The process `nikaia run` was started as is now the program itself.
    let exe = std::fs::read_link(format!("/proc/{}/exe", run.id())).expect("its executable");
    let named = exe.file_name().map(|n| n.to_string_lossy().into_owned());
    run.kill().expect("end it");
    run.wait().expect("it ends");
    assert_eq!(
        named.as_deref(),
        Some("waits"),
        "the process is the program, not a `nikaia` waiting on it: {}",
        exe.display()
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// **Another package's function is `sync` where its source says so, and only
/// there** ([ADR-244](../../../docs/specification/adr/adr-244.md) D1, D3).
///
/// `lib::plus` cannot pause and does not promise it. A `sync` function of the
/// program may not count on it, and is told that the word is missing in `lib`
/// rather than that `plus` pauses. A plain caller builds and runs: the call is
/// checked against the promise and **lowered by the body**, so it is not
/// awaited - an `.await` on a plain function is what `rustc` would have refused.
#[test]
fn another_packages_sync_is_what_its_source_promised() {
    let dir = a_program_and_a_package(
        "sync-across-a-package",
        "lib",
        &[
            (
                "lib/src/main.nika",
                "pub fn plus(x: i64) -> i64 {\n    return x + 1\n}\n",
            ),
            (
                "app/src/main.nika",
                "use lib\n\nfn total() -> i64 sync {\n    return lib::plus(1)\n}\n\n\
                 fn main() {\n    println(total())\n}\n",
            ),
        ],
    );
    let refused = nikaia(&["build"], &dir.join("app"));
    assert!(!refused.status.success(), "{}", said(&refused));
    let told = String::from_utf8_lossy(&refused.stderr);
    assert!(
        told.contains("NK2202") && told.contains("doesn't promise that it never pauses"),
        "{}",
        said(&refused)
    );
    assert!(
        told.contains("Write `sync` after the result of `plus` in `lib`"),
        "{}",
        said(&refused)
    );

    std::fs::write(
        dir.join("app/src/main.nika"),
        "use lib\n\nfn total() -> i64 {\n    return lib::plus(1)\n}\n\n\
         fn main() {\n    println(total())\n}\n",
    )
    .expect("a plain caller");
    let ran = nikaia(&["run"], &dir.join("app"));
    assert!(ran.status.success(), "{}", said(&ran));
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).trim(),
        "2",
        "{}",
        said(&ran)
    );

    std::fs::write(
        dir.join("lib/src/main.nika"),
        "pub fn plus(x: i64) -> i64 sync {\n    return x + 1\n}\n",
    )
    .expect("promise it");
    std::fs::write(
        dir.join("app/src/main.nika"),
        "use lib\n\nfn total() -> i64 sync {\n    return lib::plus(1)\n}\n\n\
         fn main() {\n    println(total())\n}\n",
    )
    .expect("the sync caller again");
    let promised = nikaia(&["run"], &dir.join("app"));
    assert!(promised.status.success(), "{}", said(&promised));
    assert_eq!(String::from_utf8_lossy(&promised.stdout).trim(), "2");
    std::fs::remove_dir_all(&dir).ok();
}

/// **The author is told which `pub` functions could promise more about pausing
/// than they do** ([ADR-244](../../../docs/specification/adr/adr-244.md) D2),
/// with the word that would promise it - and, once it is written, nothing.
///
/// `parse` never pauses: `sync`. `apply` pauses only where its lambda does:
/// `sync(f)`. A function that pauses, one that already promises, and one that
/// is not `pub` are not named, because none has anything to add.
#[test]
fn a_build_names_what_its_pub_functions_could_promise() {
    const BEFORE: &str = "use std::time\n\n\
        pub fn parse(x: i64) -> i64 {\n    return x + 1\n}\n\n\
        pub fn apply(f: fn(i64) -> i64, x: i64) -> i64 {\n    return f(x)\n}\n\n\
        pub fn waits() -> i64 {\n    time::sleep(1.millis())\n    return 1\n}\n\n\
        pub fn promised(x: i64) -> i64 sync {\n    return x\n}\n\n\
        fn mine(x: i64) -> i64 {\n    return x\n}\n\n\
        fn main() {\n    println(parse(1) + apply(fn(v) { v * 2 }, 3) + waits() + promised(1) + mine(1))\n}\n";
    let dir = a_project("could-promise", "[package]\nname = \"p\"\n", BEFORE);
    let lower = || {
        Command::new(env!("CARGO_BIN_EXE_nikaia"))
            .args([
                "lower",
                "src/main.nika",
                "--output",
                "main.rs",
                "--no-cache",
            ])
            .current_dir(&dir)
            .output()
            .expect("the nikaia binary runs")
    };

    let before = lower();
    assert!(before.status.success(), "{}", said(&before));
    let told = String::from_utf8_lossy(&before.stderr);
    assert!(
        told.contains("note: 1 pub function never pauses and does not promise it: `parse`"),
        "{}",
        said(&before)
    );
    assert!(
        told.contains("does not promise it: `apply` (`sync(f)`)"),
        "{}",
        said(&before)
    );
    for silent in ["`waits`", "`promised`", "`mine`"] {
        assert!(
            !told.contains(silent),
            "{silent} has nothing to add: {}",
            said(&before)
        );
    }

    std::fs::write(
        dir.join("src/main.nika"),
        BEFORE
            .replace(
                "pub fn parse(x: i64) -> i64 {",
                "pub fn parse(x: i64) -> i64 sync {",
            )
            .replace(
                "pub fn apply(f: fn(i64) -> i64, x: i64) -> i64 {",
                "pub fn apply(f: fn(i64) -> i64, x: i64) -> i64 sync(f) {",
            ),
    )
    .expect("promise both");
    let after = lower();
    assert!(after.status.success(), "{}", said(&after));
    assert!(
        !String::from_utf8_lossy(&after.stderr).contains("note:"),
        "written, there is nothing left to say: {}",
        said(&after)
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// **The author is warned when a `pub` function loses the `sync` it had**
/// ([ADR-244](../../../docs/specification/adr/adr-244.md) D5): the committed
/// ledger says `parse` could not pause, a line two calls down now pauses, and
/// the build says so at that line with the calls that lead there. Committed,
/// it is not said again.
#[test]
fn a_build_warns_when_a_pub_function_loses_its_sync() {
    const PURE: &str = "use std::time\n\n\
        fn read_section(x: i64) -> i64 {\n    return x * 2\n}\n\n\
        pub fn parse(x: i64) -> i64 {\n    return read_section(x) + 1\n}\n\n\
        fn main() {\n    println(parse(1))\n}\n";
    let dir = a_project("loses-sync", "[package]\nname = \"p\"\n", PURE);
    let first = nikaia(&["build"], &dir);
    assert!(first.status.success(), "{}", said(&first));
    assert!(!String::from_utf8_lossy(&first.stderr).contains("NK2211"));

    std::fs::write(
        dir.join("src/main.nika"),
        PURE.replace(
            "    return x * 2",
            "    time::sleep(1.millis())\n    return x * 2",
        ),
    )
    .expect("a pause two calls down");
    let lost = nikaia(&["build"], &dir);
    assert!(
        lost.status.success(),
        "a warning stops nothing: {}",
        said(&lost)
    );
    let told = String::from_utf8_lossy(&lost.stderr);
    assert!(
        told.contains("warning[NK2211]: `parse` could not pause before, and now it can."),
        "{}",
        said(&lost)
    );
    assert!(told.contains("src/main.nika:4:5"), "{}", said(&lost));
    assert!(
        told.contains("`parse` calls `read_section`, which pauses here."),
        "{}",
        said(&lost)
    );

    let committed = nikaia(&["build"], &dir);
    assert!(committed.status.success(), "{}", said(&committed));
    assert!(
        !String::from_utf8_lossy(&committed.stderr).contains("NK2211"),
        "the ledger written is the acknowledgement: {}",
        said(&committed)
    );
    std::fs::remove_dir_all(&dir).ok();
}
