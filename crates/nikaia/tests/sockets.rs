//! **A Nikaia program binds a socket and talks over it**
//! ([ADR-289](../../../docs/specification/adr/adr-289.md) D6), which is the
//! step issue #90 calls the blocker:
//! [ADR-289](../../../docs/specification/adr/adr-289.md) entire,
//! [ADR-289](../../../docs/specification/adr/adr-289.md), and the roadmap's
//! route hashing all wait on something for a handler to run *for*.
//!
//! The whole of it is a `.nika` file: `net::listen`, `accept`, `read`, `write`.
//! Nothing here says how the waiting is done, which is
//! [ADR-038](../../../docs/specification/adr/adr-038.md) D3's rule — the
//! runtime is invisible from a Nikaia program.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

/// A client and a server in one program, which is what makes the test need no
/// second process and no fixed port.
const BOTH_ENDS: &str = "\
use std::net

fn talk(address: String) throws {
    let mut client = net::connect(address)
    client.write(\"ping\")
    let back = client.read()
    println(f\"client read {back.len()}\")
}

fn main() throws {
    let listener = net::listen(\"127.0.0.1:0\")
    let address = listener.address()
    println(f\"bound {address.len() > 10}\")

    let asking = spawn fn() { talk(address) catch { } }

    let mut connection = listener.accept()
    let asked = connection.read()
    println(f\"server read {asked.len()}\")
    connection.write(\"pong\")
    asking.join()
}
";

fn lower(dir: &Path, source: &str, flags: &[&str]) -> String {
    let input = dir.join("main.nika");
    std::fs::write(&input, source).expect("the source");
    let output = dir.join("main.rs");
    let run = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .args(["lower", input.to_str().unwrap()])
        .args(["--output", output.to_str().unwrap()])
        .args(["--no-cache"])
        .args(flags)
        .env("NIKAIA_CACHE_DIR", dir.join("cache"))
        .output()
        .expect("the nikaia binary runs");
    assert!(
        run.status.success(),
        "lowering failed:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    std::fs::read_to_string(&output).expect("the emitted Rust")
}

fn build(dir: &Path, source: &str, flags: &[&str]) -> PathBuf {
    let rust = lower(dir, source, flags);
    let binary = dir.join("program");
    let compiled = common::compile(
        &dir.join("main.rs"),
        &["--crate-type", "bin", "-o", binary.to_str().unwrap()],
    );
    assert!(
        compiled.status.success(),
        "the emitted Rust did not compile:\n{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    binary
}

/// **Both ends, at both settings of `user_parallelism`**, and the same output
/// from each.
///
/// That is the claim the switch rests on (Part I 1.2) and the one a socket
/// could quietly break: `accept` gives the thread up rather than holding it, so
/// a program that waits for a connection and then makes one is a program that
/// deadlocks the moment the wait is a block. At `no` there is one thread and it
/// is the same answer.
#[test]
fn a_program_binds_a_socket_and_both_ends_talk() {
    let mut said = Vec::new();
    for switch in ["no", "yes"] {
        let dir = common::scratch_dir(&format!("sockets-{switch}"));
        let binary = build(&dir, BOTH_ENDS, &["--user-parallelism", switch]);
        let out = Command::new(&binary)
            .current_dir(&dir)
            .output()
            .expect("run the compiled program");
        assert!(
            out.status.success(),
            "at `{switch}`: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let printed = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(printed.contains("bound true"), "at `{switch}`: {printed}");
        assert!(
            printed.contains("server read 4"),
            "at `{switch}`: {printed}"
        );
        assert!(
            printed.contains("client read 4"),
            "at `{switch}`: {printed}"
        );
        let _ = std::fs::remove_dir_all(&dir);
        said.push(printed);
    }
    assert_eq!(
        said[0], said[1],
        "a socket means the same thing at both settings"
    );
}

/// **The runtime is invisible from the program**
/// ([ADR-038](../../../docs/specification/adr/adr-038.md) D3), which a socket is
/// the easiest thing to break: every other language makes a reader choose a
/// runtime before it can bind one.
#[test]
fn nothing_in_the_program_says_how_it_waits() {
    for forbidden in ["async", "await", "epoll", "io_uring", "Runtime", "poll"] {
        assert!(
            !BOTH_ENDS.contains(forbidden),
            "a program that binds a socket says `{forbidden}`"
        );
    }
}
