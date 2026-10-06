//! **Build-time code calls its package's Nikaia and its Nikaia dependencies**
//! ([ADR-321](../../../docs/specification/adr/adr-321.md) D13, #479): a
//! `comptime` and a grammar action at build time reach a dependency's
//! functions, compiled into the build-time program as the module the program
//! names the package by; a path to C is refused, and nothing runs.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A program `app` that depends on a package `lib`, with these sources.
fn projects(purpose: &str, lib: &str, app: &str) -> PathBuf {
    let root = common::scratch_dir(purpose);
    std::fs::create_dir_all(root.join("lib/src")).expect("the package");
    std::fs::write(
        root.join("lib/nikaia.toml"),
        "[package]\nname = \"lib\"\nversion = \"0.1.0\"\n",
    )
    .expect("a manifest");
    std::fs::write(root.join("lib/src/lib.nika"), lib).expect("a source");
    std::fs::create_dir_all(root.join("app/src")).expect("the program");
    std::fs::write(
        root.join("app/nikaia.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nlib = { path = \"../lib\" }\n",
    )
    .expect("a manifest");
    std::fs::write(root.join("app/src/main.nika"), app).expect("a source");
    root
}

fn run(root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .current_dir(root.join("app"))
        .args(["run", "--no-cache"])
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("the nikaia binary runs")
}

fn said(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

const LIB: &str = "pub fn tripled(n: i64) -> i64 sync {\n    return n * 3\n}\n";

/// A `comptime` that calls a dependency's function is computed while the
/// program is built - it was refused as *can pause*, against a ledger that
/// had no entry for the dependency.
#[test]
fn a_comptime_calls_a_dependency() {
    let root = projects(
        "build-time-dependency-comptime",
        LIB,
        "use lib\n\ncomptime N: i64 = lib::tripled(14)\n\nfn main() {\n    println(f\"{N}\")\n}\n",
    );
    let out = run(&root);
    let generated =
        std::fs::read_to_string(root.join("app/target/nikaia/gen/app/app.rs")).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&root);
    assert!(out.status.success(), "{}", said(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "42");
    assert!(generated.contains("const N: i64 = 42;"), "{generated}");
}

/// A grammar's action at build time calls a helper of its own file and a
/// function of a dependency.
#[test]
fn a_grammar_action_calls_a_dependency_at_build_time() {
    let root = projects(
        "build-time-dependency-grammar",
        LIB,
        "use lib\n\n\
         fn plus_one(n: i64) -> i64 {\n    return n + 1\n}\n\n\
         grammar Num {\n    entry rule n -> i64 = d:dec[i64](digit+) { lib::tripled(plus_one(d)) }\n}\n\n\
         comptime N: i64 = Num::n(\"13\")\n\n\
         fn main() {\n    println(f\"{N}\")\n}\n",
    );
    let out = run(&root);
    let generated =
        std::fs::read_to_string(root.join("app/target/nikaia/gen/app/app.rs")).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&root);
    assert!(out.status.success(), "{}", said(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "42");
    assert!(generated.contains("const N: i64 = 42;"), "{generated}");
}

/// A dependency's function whose path reaches C may not run while the program
/// is built (D13, D15).
#[test]
fn a_dependency_that_reaches_c_is_refused_at_build_time() {
    let root = projects(
        "build-time-dependency-c",
        "extern {\n    fn abs(n: i32) -> i32\n}\n\n\
         pub fn tripled(n: i64) -> i64 {\n    return n * 3 + unsafe { abs(0) } as i64\n}\n",
        "use lib\n\ncomptime N: i64 = lib::tripled(14)\n\nfn main() {\n    println(f\"{N}\")\n}\n",
    );
    let out = run(&root);
    let _ = std::fs::remove_dir_all(&root);
    assert!(!out.status.success(), "{}", said(&out));
    assert!(said(&out).contains("NK1152"), "{}", said(&out));
}

fn has(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// **D14: the build-time program holds no foreign code, not even linked.**
/// `lib` declares a C function from a library whose constructor writes a
/// marker file when it is loaded, and offers a pure function too. A
/// `comptime` calls the pure one: the build succeeds and the marker does not
/// exist. The program, which calls the C function when it runs, does link the
/// library, and running it writes the marker.
#[test]
fn the_build_time_program_links_no_c_library() {
    if !has("pkg-config") || !has("cc") || !has("ar") {
        eprintln!("skipped: needs pkg-config, cc and ar");
        return;
    }
    let root = projects(
        "build-time-no-c",
        "extern(library: \"nikaia_test_marker\") {\n    fn marker_value(n: i32) -> i32\n}\n\n\
         pub fn tripled(n: i64) -> i64 sync {\n    return n * 3\n}\n\n\
         pub fn marked(n: i32) -> i32 {\n    return unsafe { marker_value(n) }\n}\n",
        "use lib\n\ncomptime N: i64 = lib::tripled(14)\n\n\
         fn main() {\n    println(f\"{N} {lib::marked(1)}\")\n}\n",
    );
    std::fs::write(
        root.join("lib/nikaia.toml"),
        "[package]\nname = \"lib\"\nversion = \"0.1.0\"\n\n\
         [library.nikaia_test_marker]\npkg-config = \"nikaia_test_marker\"\n",
    )
    .expect("the manifest");
    // The C library: a constructor that leaves a marker where it is told to.
    let c = root.join("c");
    std::fs::create_dir_all(&c).expect("the C directory");
    std::fs::write(
        c.join("marker.c"),
        "#include <stdio.h>\n#include <stdlib.h>\n\
         __attribute__((constructor)) static void mark(void) {\n\
         \x20   const char *p = getenv(\"NIKAIA_TEST_MARKER\");\n\
         \x20   if (p) { FILE *f = fopen(p, \"w\"); if (f) { fputs(\"loaded\\n\", f); fclose(f); } }\n\
         }\n\
         int marker_value(int n) { return n + 1; }\n",
    )
    .expect("the C source");
    let compiled = Command::new("cc")
        .current_dir(&c)
        .args(["-c", "-fPIC", "marker.c", "-o", "marker.o"])
        .status()
        .expect("cc runs");
    assert!(compiled.success());
    let archived = Command::new("ar")
        .current_dir(&c)
        .args(["rcs", "libnikaia_test_marker.a", "marker.o"])
        .status()
        .expect("ar runs");
    assert!(archived.success());
    std::fs::write(
        c.join("nikaia_test_marker.pc"),
        format!(
            "Name: m\nDescription: m\nVersion: 1\nLibs: -L{} -lnikaia_test_marker\n",
            c.display()
        ),
    )
    .expect("the .pc file");
    let marker = root.join("MARKER");
    let nikaia = |verb: &str| {
        Command::new(env!("CARGO_BIN_EXE_nikaia"))
            .current_dir(root.join("app"))
            .args([verb, "--no-cache"])
            .env_remove("CARGO_TARGET_DIR")
            .env("PKG_CONFIG_PATH", &c)
            .env("NIKAIA_TEST_MARKER", &marker)
            .output()
            .expect("the nikaia binary runs")
    };
    let built = nikaia("build");
    let after_the_build = marker.exists();
    // **What the build-time program loads** (D14, step 5): the system's C
    // runtime and the toolchain's `std`, with the library build-time code
    // links against - and nothing of the package's C.
    let loaded: Vec<String> = build_time_binaries(&root.join("app/target"))
        .iter()
        .filter_map(|binary| Command::new("ldd").arg(binary).output().ok())
        .flat_map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|line| line.trim().to_string())
                .collect::<Vec<_>>()
        })
        .collect();
    const ALLOWED: [&str; 11] = [
        "linux-vdso",
        "libnikaia_bundle",
        "libstd-",
        "libgcc_s",
        "libc.",
        "libm.",
        "librt.",
        "libpthread.",
        "libdl.",
        "libutil.",
        "/lib",
    ];
    let foreign: Vec<&String> = loaded
        .iter()
        .filter(|line| !ALLOWED.iter().any(|ok| line.starts_with(ok)))
        .collect();
    let ran = nikaia("run");
    let after_the_run = marker.exists();
    let _ = std::fs::remove_dir_all(&root);
    assert!(built.status.success(), "{}", said(&built));
    assert!(
        !after_the_build,
        "the C library's constructor ran while the program was built"
    );
    assert!(
        has("ldd") && !loaded.is_empty() || !has("ldd"),
        "no build-time program was found to ask `ldd` about"
    );
    assert!(
        foreign.is_empty(),
        "the build-time program loads {foreign:#?}"
    );
    assert!(ran.status.success(), "{}", said(&ran));
    assert_eq!(String::from_utf8_lossy(&ran.stdout).trim(), "42 2");
    assert!(after_the_run, "the program did not link the C library");
}

/// The programs a build compiled to run its `comptime`s.
fn build_time_binaries(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(build_time_binaries(&path));
        } else if path.file_name().is_some_and(|n| n == "run")
            && path.components().any(|c| c.as_os_str() == "comptime")
        {
            found.push(path);
        }
    }
    found
}

/// **A helper in another file of the program's own package** (D13): a
/// package's files are one namespace (Part I 9.1), at build time too.
#[test]
fn a_grammar_action_calls_another_file_of_its_package() {
    let root = projects(
        "build-time-other-file",
        LIB,
        "use lib\n\n\
         grammar Num {\n    entry rule n -> i64 = d:dec[i64](digit+) { lib::tripled(plus_one(d)) }\n}\n\n\
         comptime N: i64 = Num::n(\"13\")\n\n\
         fn main() {\n    println(f\"{N}\")\n}\n",
    );
    std::fs::write(
        root.join("app/src/helpers.nika"),
        "pub fn plus_one(n: i64) -> i64 sync {\n    return n + 1\n}\n",
    )
    .expect("a second file");
    let out = run(&root);
    let generated =
        std::fs::read_to_string(root.join("app/target/nikaia/gen/app/app.rs")).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&root);
    assert!(out.status.success(), "{}", said(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "42");
    assert!(generated.contains("const N: i64 = 42;"), "{generated}");
}

/// **D14, a Rust crate** (#479, step 4): a dependency calls a Rust crate whose
/// library leaves a marker when it is loaded (a function in `.init_array`, as
/// the `ctor` crate places it), and offers a pure function too. A `comptime`
/// calls the pure one: the build succeeds and the marker does not exist. The
/// program, which calls the crate when it runs, writes it.
#[test]
fn the_build_time_program_holds_no_rust_crate() {
    let root = projects(
        "build-time-no-rust",
        "\
         pub fn tripled(n: i64) -> i64 sync {\n    return n * 3\n}\n\n\
         pub fn shimmed(n: i64) -> i64 {\n    return shim::plus_one(n)\n}\n",
        "use lib\n\ncomptime N: i64 = lib::tripled(14)\n\n\
         fn main() {\n    println(f\"{N} {lib::shimmed(1)}\")\n}\n",
    );
    std::fs::write(
        root.join("lib/nikaia.toml"),
        "[package]\nname = \"lib\"\nversion = \"0.1.0\"\n\n\
         [dependencies]\nshim = { type = \"rust\", path = \"../shim\" }\n",
    )
    .expect("the manifest");
    std::fs::create_dir_all(root.join("lib/contracts")).expect("the contracts");
    std::fs::write(
        root.join("lib/contracts/shim.contracts"),
        "version = 3\n\n[fn.\"shim::plus_one\"]\npub = true\nsync = true\n\
         signature = \"(n: i64) -> i64\"\n",
    )
    .expect("the description");
    std::fs::create_dir_all(root.join("shim/src")).expect("the crate");
    std::fs::write(
        root.join("shim/Cargo.toml"),
        "[package]\nname = \"shim\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
    )
    .expect("the crate's manifest");
    std::fs::write(
        root.join("shim/src/lib.rs"),
        "extern \"C\" fn mark() {\n\
         \x20   if let Some(p) = std::env::var_os(\"NIKAIA_TEST_MARKER\") {\n\
         \x20       let _ = std::fs::write(p, \"loaded\\n\");\n\
         \x20   }\n\
         }\n\n\
         #[used]\n\
         #[unsafe(link_section = \".init_array\")]\n\
         static MARK: extern \"C\" fn() = mark;\n\n\
         pub fn plus_one(n: i64) -> i64 {\n    n + 1\n}\n",
    )
    .expect("the crate's source");
    let marker = root.join("MARKER");
    let nikaia = |verb: &str| {
        Command::new(env!("CARGO_BIN_EXE_nikaia"))
            .current_dir(root.join("app"))
            .args([verb, "--no-cache"])
            .env_remove("CARGO_TARGET_DIR")
            .env("NIKAIA_TEST_MARKER", &marker)
            .output()
            .expect("the nikaia binary runs")
    };
    let built = nikaia("build");
    let after_the_build = marker.exists();
    let ran = nikaia("run");
    let after_the_run = marker.exists();
    let _ = std::fs::remove_dir_all(&root);
    assert!(built.status.success(), "{}", said(&built));
    assert!(
        !after_the_build,
        "the Rust crate's constructor ran while the program was built"
    );
    assert!(ran.status.success(), "{}", said(&ran));
    assert_eq!(String::from_utf8_lossy(&ran.stdout).trim(), "42 2");
    assert!(after_the_run, "the program did not link the Rust crate");
}
