//! **A Nikaia program binds a socket and talks over it**
//! ([ADR-289](../../../docs/specification/adr/adr-289.md) D6), which is the
//! step issue #90 calls the blocker:
//! [ADR-289](../../../docs/specification/adr/adr-289.md) entire,
//! [ADR-289](../../../docs/specification/adr/adr-289.md), and the roadmap's
//! route hashing all wait on something for a handler to run *for*.
//!
//! The whole of it is a `.nika` file: `net::listen`, `accept`, `read`, `write`.
//! Nothing here says how the waiting is done, which is
//! [ADR-303](../../../docs/specification/adr/adr-303.md) D3's rule — the
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
/// ([ADR-303](../../../docs/specification/adr/adr-303.md) D3), which a socket is
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

/// **What a socket read is made text of** (ADR-320 D7, #489): `b.text()` over
/// the bytes the server read, compiled and run.
#[test]
fn the_bytes_a_socket_reads_are_made_text() {
    let source = "\
use std::net

fn talk(address: String) throws {
    let mut client = net::connect(address)
    client.write(\"ping\")
}

fn main() throws {
    let listener = net::listen(\"127.0.0.1:0\")
    let address = listener.address()
    let asking = spawn fn() { talk(address) catch { } }
    let mut connection = listener.accept()
    let asked = connection.read()
    println(f\"server read {asked.text()}\")
    asking.join()
}
";
    let dir = common::scratch_dir("sockets-text");
    let binary = build(&dir, source, &[]);
    let out = Command::new(&binary).output().expect("the program runs");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "server read ping\n",
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A server on `net::serve` (ADR-326 D1): it echoes, and a message of four
/// bytes makes its handler panic.
const SERVED: &str = "\
use std::net

fn echo(mut conn: net::Connection) throws {
    let got = conn.read()
    if got.len() == 4 {
        panic(\"boom\")
    }
    conn.write(got)
}

fn main() throws {
    net::serve(\"ADDRESS\") fn(mut conn) { echo(conn) }
}
";

/// A port nobody is listening on, which the server is then told to take.
fn free_address() -> String {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
    let address = probe.local_addr().expect("its address").to_string();
    drop(probe);
    address
}

/// The server, connected to once it has bound: a refused connection is the
/// one answer that means *not yet*, and it is asked a bounded number of times.
fn connect_to(address: &str, server: &mut std::process::Child) -> std::net::TcpStream {
    for _ in 0..500 {
        if let Ok(stream) = std::net::TcpStream::connect(address) {
            return stream;
        }
        assert!(
            server.try_wait().expect("the server's state").is_none(),
            "the server ended before it took a connection"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("the server never took a connection at {address}");
}

fn ask(stream: &mut std::net::TcpStream, what: &[u8]) -> Vec<u8> {
    use std::io::{Read, Write};
    stream.write_all(what).expect("write");
    let mut back = vec![0; 64];
    let n = stream.read(&mut back).expect("read");
    back.truncate(n);
    back
}

/// **Each connection is a task of its own** (ADR-326 D1, D4), at both settings:
/// a client that has connected and sent nothing does not hold up the next one,
/// and a handler that panics ends its own connection while the server goes on.
/// Asked by order, not by time: the second client's answer arrives while the
/// first one's connection is still waiting.
#[test]
fn net_serve_runs_each_connection_in_a_task_of_its_own() {
    for switch in ["no", "yes"] {
        let dir = common::scratch_dir(&format!("serve-{switch}"));
        let address = free_address();
        let binary = build(
            &dir,
            &SERVED.replace("ADDRESS", &address),
            &["--user-parallelism", switch],
        );
        let mut server = Command::new(&binary)
            .current_dir(&dir)
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("start the server");

        let mut waiting = connect_to(&address, &mut server);
        let mut second = connect_to(&address, &mut server);
        assert_eq!(ask(&mut second, b"hi"), b"hi", "at `{switch}`");
        assert_eq!(ask(&mut waiting, b"a"), b"a", "at `{switch}`");

        let mut crashing = connect_to(&address, &mut server);
        assert_eq!(
            ask(&mut crashing, b"boom"),
            b"",
            "at `{switch}`: the connection closes"
        );
        let mut after = connect_to(&address, &mut server);
        assert_eq!(
            ask(&mut after, b"ok"),
            b"ok",
            "at `{switch}`: the server goes on"
        );

        server.kill().ok();
        server.wait().ok();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
