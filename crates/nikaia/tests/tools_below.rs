//! **The tools below Nikaia take no orders from the project**
//! ([ADR-325](../../../docs/specification/adr/adr-325.md), Part III 13.2b,
//! #480): Cargo, `rustc` and rustup are started in a directory of the
//! compiler's own, so a project's `.cargo/config.toml` and
//! `rust-toolchain.toml` are never read, and the user's `CARGO_HOME` still is.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A project of one program, printing `word` so that no two tests share a
/// build Cargo could take as fresh.
fn project(purpose: &str, word: &str) -> PathBuf {
    let dir = common::scratch_dir(purpose);
    std::fs::create_dir_all(dir.join("src")).expect("the package");
    std::fs::write(
        dir.join("nikaia.toml"),
        "[package]\nname = \"below\"\nversion = \"0.1.0\"\n",
    )
    .expect("a manifest");
    std::fs::write(
        dir.join("src/main.nika"),
        format!("fn main() {{\n    println(\"{word}\")\n}}\n"),
    )
    .expect("a source");
    dir
}

fn nikaia(dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nikaia"));
    command
        .current_dir(dir)
        .args(["run", "--no-cache"])
        .env_remove("CARGO_TARGET_DIR");
    command
}

fn said(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// A linker that leaves `marker` behind and then links.
fn marking_linker(at: &Path, marker: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script = at.join("linker.sh");
    std::fs::write(
        &script,
        format!("#!/bin/sh\ntouch '{}'\nexec cc \"$@\"\n", marker.display()),
    )
    .expect("the linker");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("runnable");
    script
}

fn host() -> String {
    let out = Command::new(common::rustc())
        .arg("-vV")
        .output()
        .expect("rustc runs");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .expect("rustc names its host")
        .to_string()
}

/// **D1, the reproduction of #480**: a linker the project's
/// `.cargo/config.toml` names is never run.
#[test]
fn a_projects_cargo_config_is_not_read() {
    let dir = project("below-cargo-config", "not-linked-by-the-project");
    let marker = dir.join("MARKER");
    let linker = marking_linker(&dir, &marker);
    std::fs::create_dir_all(dir.join(".cargo")).expect("the config directory");
    std::fs::write(
        dir.join(".cargo/config.toml"),
        format!("[target.{}]\nlinker = '{}'\n", host(), linker.display()),
    )
    .expect("the config");
    let out = nikaia(&dir).output().expect("the nikaia binary runs");
    let marked = marker.exists();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success(), "{}", said(&out));
    assert!(said(&out).contains("not-linked-by-the-project"));
    assert!(!marked, "the project's linker ran");
}

/// **D3**: a `rust-toolchain.toml` naming a toolchain that is not installed
/// does not stop the build, which uses the user's default. Started as a user
/// starts it: the `cargo` and `rustc` on `PATH`, with no toolchain chosen by
/// the test's own Cargo. Without rustup, nothing reads the file anyway.
#[test]
fn a_projects_toolchain_file_is_not_read() {
    let has_default = Command::new("rustup")
        .arg("default")
        .output()
        .is_ok_and(|o| o.status.success());
    if !has_default {
        eprintln!("skipped: no rustup with a default toolchain");
        return;
    }
    let dir = project("below-toolchain", "the-users-default");
    std::fs::write(
        dir.join("rust-toolchain.toml"),
        "[toolchain]\nchannel = \"1.20.0\"\n",
    )
    .expect("the toolchain file");
    let out = nikaia(&dir)
        .env_remove("RUSTUP_TOOLCHAIN")
        .env_remove("CARGO")
        .env_remove("RUSTC")
        .output()
        .expect("the nikaia binary runs");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success(), "{}", said(&out));
    assert!(said(&out).contains("the-users-default"));
}

/// **D3**: the user's own `CARGO_HOME/config.toml` still applies. Its linker
/// runs; the registry and the git checkouts are the real ones, so nothing is
/// fetched.
#[test]
fn the_users_cargo_home_is_read() {
    let real = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))
        .expect("a cargo home");
    let dir = project("below-cargo-home", "linked-by-the-user");
    let home = dir.join("cargo-home");
    std::fs::create_dir_all(&home).expect("a cargo home");
    for kept in ["registry", "git"] {
        if real.join(kept).exists() {
            std::os::unix::fs::symlink(real.join(kept), home.join(kept)).expect("linked");
        }
    }
    let marker = dir.join("MARKER");
    let linker = marking_linker(&dir, &marker);
    std::fs::write(
        home.join("config.toml"),
        format!("[target.{}]\nlinker = '{}'\n", host(), linker.display()),
    )
    .expect("the config");
    let out = nikaia(&dir)
        .env("CARGO_HOME", &home)
        .output()
        .expect("the nikaia binary runs");
    let marked = marker.exists();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success(), "{}", said(&out));
    assert!(said(&out).contains("linked-by-the-user"));
    assert!(marked, "the user's linker did not run");
}
