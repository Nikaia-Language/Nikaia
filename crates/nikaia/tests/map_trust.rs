//! **A map's trust is the map's** (ADR-010 D1, #431): in a program that reads
//! both a file and a socket, the map filled from the file keeps the fast hash
//! and the one filled from the socket gets the keyed one. Whatever this cannot
//! follow is untrusted.

mod common;

use nikaia::emit::emit_program;
use nikaia::parser::parse_to_ast;

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Default::default())
        .expect("it lowers")
        .rust
}

/// The two maps of one program, each by the line that makes it.
fn the_map_made_by(rust: &str, name: &str) -> String {
    rust.lines()
        .find(|l| l.contains(&format!("let mut {name}")))
        .unwrap_or_else(|| panic!("no `{name}` in:\n{rust}"))
        .to_string()
}

const BOTH: &str = "use std::net\n\
     use std::fs\n\
     use std::collections\n\
     fn main() throws {\n\
     \x20   let text = fs::read_to_string(\"words.txt\", fs::Root::Anywhere)\n\
     \x20   let mut from_file = collections::HashMap()\n\
     \x20   for w in text.split(\" \") { from_file.insert(w, 1) }\n\
     \x20   let mut c = net::connect(\"127.0.0.1:1\")\n\
     \x20   let b = c.read()\n\
     \x20   let mut from_socket = collections::HashMap()\n\
     \x20   from_socket.insert(b.len(), 1)\n\
     \x20   println(f\"{from_file.len()} {from_socket.len()}\")\n\
     }\n";

#[test]
fn a_file_map_beside_a_socket_map_keeps_the_fast_hash() {
    let rust = lowered(BOTH);
    let file = the_map_made_by(&rust, "from_file");
    let socket = the_map_made_by(&rust, "from_socket");
    assert!(file.contains("TrustedMap"), "{file}");
    assert!(!socket.contains("TrustedMap"), "{socket}");
    // **And the two kinds of map stand in one program** that `rustc` takes.
    let dir = common::scratch_dir("map-trust");
    let path = dir.join("program.rs");
    std::fs::write(&path, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let compiled = common::compile(
        &path,
        &["--crate-type", "bin", "-o", &binary.to_string_lossy()],
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        compiled.status.success(),
        "{}\n{rust}",
        String::from_utf8_lossy(&compiled.stderr)
    );
}

/// **A map that leaves its function is not followed**: handed to another
/// function, its type is named there too, so it takes the program's answer.
#[test]
fn a_map_that_leaves_its_function_takes_the_programs_answer() {
    let source = BOTH.replace(
        "    println(f\"{from_file.len()} {from_socket.len()}\")\n",
        "    println(f\"{count(from_file)} {from_socket.len()}\")\n",
    ) + "fn count(m: collections::HashMap[ref String, i64]) -> i64 {\n    return m.len()\n}\n";
    let rust = lowered(&source);
    assert!(!rust.contains("TrustedMap"), "{rust}");
}

/// **A key from a function of the program's own is not followed**: it may read
/// anything.
#[test]
fn a_key_from_an_own_function_is_untrusted() {
    let source = BOTH.replace("from_file.insert(w, 1)", "from_file.insert(word(w), 1)")
        + "fn word(w: ref String) -> String {\n    return w.clone()\n}\n";
    let rust = lowered(&source);
    let file = the_map_made_by(&rust, "from_file");
    assert!(!file.contains("TrustedMap"), "{file}");
}

/// **A program with no untrusted source is as before**: every map is fast.
#[test]
fn a_program_that_reads_only_files_keeps_every_map_fast() {
    let source = "use std::fs\n\
         use std::collections\n\
         fn main() throws {\n\
         \x20   let text = fs::read_to_string(\"words.txt\", fs::Root::Anywhere)\n\
         \x20   let mut m = collections::HashMap()\n\
         \x20   for w in text.split(\" \") { m.insert(w, 1) }\n\
         \x20   println(f\"{m.len()}\")\n\
         }\n";
    let rust = lowered(source);
    assert!(the_map_made_by(&rust, "m").contains("TrustedMap"), "{rust}");
}
