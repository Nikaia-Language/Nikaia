//! **A package names the system libraries it links** (Part III 13.4,
//! [ADR-324](../../../docs/specification/adr/adr-324.md), #483): an
//! `extern(library: "x")` block and a `[library.x] pkg-config = "…"` entry,
//! both required and agreeing (`NK1226`); found only through `pkg-config`
//! (`NK1224`); and only `-l`, `-L` and `-pthread` from what it says
//! (`NK1225`).

mod common;

use std::path::Path;
use std::process::{Command, Output};

const PROGRAM: &str = "extern(library: \"LIB\") {\n    fn abs(n: i32) -> i32\n}\n\n\
     fn main() {\n    println(f\"{unsafe { abs(-3) }}\")\n}\n";

/// A project of one program, with `entries` as its `[library]` tables.
fn project(purpose: &str, library: &str, entries: &str) -> std::path::PathBuf {
    let dir = common::scratch_dir(purpose);
    std::fs::create_dir_all(dir.join("src")).expect("the package");
    std::fs::write(
        dir.join("nikaia.toml"),
        format!("[package]\nname = \"libs\"\nversion = \"0.1.0\"\n\n{entries}"),
    )
    .expect("a manifest");
    std::fs::write(dir.join("src/main.nika"), PROGRAM.replace("LIB", library)).expect("a source");
    dir
}

fn build(dir: &Path, pkg_config_path: Option<&Path>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nikaia"));
    command
        .current_dir(dir)
        .args(["build", "--no-cache"])
        .env_remove("CARGO_TARGET_DIR");
    if let Some(path) = pkg_config_path {
        command.env("PKG_CONFIG_PATH", path);
    }
    command.output().expect("the nikaia binary runs")
}

fn said(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn has_pkg_config() -> bool {
    Command::new("pkg-config")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// **`NK1226` one way**: a block names a library the manifest has no entry for.
#[test]
fn a_library_without_an_entry_is_nk1226() {
    let dir = project("libs-no-entry", "m", "");
    let out = build(&dir, None);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!out.status.success());
    let said = said(&out);
    assert!(said.contains("NK1226"), "{said}");
    assert!(said.contains("[library.m]"), "{said}");
}

/// **And the other**: an entry no block names.
#[test]
fn an_entry_no_block_names_is_nk1226() {
    let dir = project(
        "libs-no-block",
        "m",
        "[library.m]\npkg-config = \"m\"\n\n[library.unused]\npkg-config = \"unused\"\n",
    );
    let out = build(&dir, None);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!out.status.success());
    let said = said(&out);
    assert!(said.contains("NK1226"), "{said}");
    assert!(said.contains("[library.unused]"), "{said}");
}

/// **`NK1224`**: `pkg-config` does not find the library, and the message names
/// the package, the library and the name looked for.
#[test]
fn a_library_pkg_config_does_not_find_is_nk1224() {
    if !has_pkg_config() {
        eprintln!("skipped: no `pkg-config` on this machine");
        return;
    }
    let dir = project(
        "libs-missing",
        "nikaia_test_missing",
        "[library.nikaia_test_missing]\npkg-config = \"nikaia_test_missing\"\n",
    );
    let out = build(&dir, None);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!out.status.success());
    let said = said(&out);
    assert!(said.contains("NK1224"), "{said}");
    assert!(said.contains("`libs`"), "{said}");
    assert!(said.contains("nikaia_test_missing"), "{said}");
}

/// **`NK1225`**: a `.pc` file whose `Libs:` would load a linker plugin. The
/// build is refused and the flag is named.
#[test]
fn a_flag_outside_the_list_is_nk1225() {
    if !has_pkg_config() {
        eprintln!("skipped: no `pkg-config` on this machine");
        return;
    }
    let pc = common::scratch_dir("libs-pc");
    std::fs::write(
        pc.join("nikaia_test_plugin.pc"),
        "Name: nikaia_test_plugin\nDescription: a test\nVersion: 1\nLibs: -lm -Wl,-plugin,/x.so\n",
    )
    .expect("a .pc file");
    let dir = project(
        "libs-plugin",
        "nikaia_test_plugin",
        "[library.nikaia_test_plugin]\npkg-config = \"nikaia_test_plugin\"\n",
    );
    let out = build(&dir, Some(&pc));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&pc);
    assert!(!out.status.success());
    let said = said(&out);
    assert!(said.contains("NK1225"), "{said}");
    assert!(said.contains("-Wl,-plugin,/x.so"), "{said}");
}

/// **A library `pkg-config` finds is linked, and the program writes nothing
/// for it**: `libm` through a `.pc` file, in a build that names no flag.
#[test]
fn a_named_library_is_linked_without_a_flag() {
    if !has_pkg_config() {
        eprintln!("skipped: no `pkg-config` on this machine");
        return;
    }
    let pc = common::scratch_dir("libs-pc-m");
    std::fs::write(
        pc.join("nikaia_test_m.pc"),
        "Name: nikaia_test_m\nDescription: libm\nVersion: 1\nLibs: -lm\n",
    )
    .expect("a .pc file");
    let dir = project(
        "libs-linked",
        "nikaia_test_m",
        "[library.nikaia_test_m]\npkg-config = \"nikaia_test_m\"\n",
    );
    let out = build(&dir, Some(&pc));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&pc);
    assert!(out.status.success(), "{}", said(&out));
}

/// **A program that uses a package which names a library writes nothing for
/// it** (D2): the package's `extern(library: …)` and its `[library]` entry are
/// enough, and the program links the library.
#[test]
fn a_program_links_what_its_package_names() {
    if !has_pkg_config() {
        eprintln!("skipped: no `pkg-config` on this machine");
        return;
    }
    let pc = common::scratch_dir("libs-pc-dep");
    std::fs::write(
        pc.join("nikaia_test_absent_dep.pc"),
        "Name: t\nDescription: t\nVersion: 1\nLibs: -lnikaia_test_absent_dep\n",
    )
    .expect("a .pc file");
    let root = common::scratch_dir("libs-dep");
    let package = root.join("cmath");
    std::fs::create_dir_all(package.join("src")).expect("the package");
    std::fs::write(
        package.join("nikaia.toml"),
        "[package]\nname = \"cmath\"\nversion = \"0.1.0\"\n\n\
         [library.nikaia_test_absent_dep]\npkg-config = \"nikaia_test_absent_dep\"\n",
    )
    .expect("a manifest");
    std::fs::write(
        package.join("src/lib.nika"),
        "extern(library: \"nikaia_test_absent_dep\") {\n    fn abs(n: i32) -> i32\n}\n\n\
         pub fn positive(n: i32) -> i32 {\n    return unsafe { abs(n) }\n}\n",
    )
    .expect("a source");
    let program = root.join("app");
    std::fs::create_dir_all(program.join("src")).expect("the program");
    std::fs::write(
        program.join("nikaia.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ncmath = { path = \"../cmath\" }\n",
    )
    .expect("a manifest");
    std::fs::write(
        program.join("src/main.nika"),
        "use cmath\n\nfn main() {\n    println(f\"{cmath::positive(-3)}\")\n}\n",
    )
    .expect("a source");
    let out = build(&program, Some(&pc));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&pc);
    // **The library the package names reaches the program's link**: one that
    // is not there fails it, by name, in a build that never wrote the flag.
    let said = said(&out);
    assert!(!out.status.success(), "{said}");
    assert!(said.contains("-lnikaia_test_absent_dep"), "{said}");
}
