//! **A type's own methods are its declarer's** (#375, Part III C.1): an `impl`
//! with no trait on a type the program does not declare is refused here
//! (`NK1209`), not by `rustc` in words about generated code. And what was found
//! on the way: a list of text asked about an owned text, and the type of an
//! `if` whose other arm leaves.

mod common;

use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn codes(source: &str) -> Vec<String> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    nikaia::check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new())
        .findings
        .into_iter()
        .map(|f| f.code.to_string())
        .collect()
}

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

/// The issue's program, and `String` beside it: both are refused with
/// `NK1209`.
#[test]
fn an_impl_on_a_type_the_program_does_not_declare_is_refused() {
    let duration = "use std::time\n\
         \n\
         impl time::Duration {\n\
         \x20   fn doubled(self) -> time::Duration { return self * 2 }\n\
         }\n\
         \n\
         fn main() { let d = 5.seconds().doubled() }\n";
    assert!(
        codes(duration).contains(&"NK1209".to_string()),
        "{:?}",
        codes(duration)
    );
    let text = "impl String {\n    fn shout(self) -> String { return self }\n}\n\nfn main() {}\n";
    assert!(
        codes(text).contains(&"NK1209".to_string()),
        "{:?}",
        codes(text)
    );
}

/// The ways that work are not refused: an `impl` of the program's own type, and
/// a program trait implemented for a `std` type (ADR-295).
#[test]
fn an_impl_of_the_programs_own_type_or_trait_is_not() {
    let own = "struct Point { x: i64 }\n\
         \n\
         impl Point {\n\
         \x20   fn left(self) -> i64 { return self.x }\n\
         }\n\
         \n\
         fn main() {}\n";
    assert!(
        !codes(own).contains(&"NK1209".to_string()),
        "{:?}",
        codes(own)
    );
    let traited = "use std::time\n\
         \n\
         trait Doubled { fn doubled(self) -> time::Duration }\n\
         \n\
         impl Doubled for time::Duration {\n\
         \x20   fn doubled(self) -> time::Duration { return self * 2 }\n\
         }\n\
         \n\
         fn main() {}\n";
    assert!(
        !codes(traited).contains(&"NK1209".to_string()),
        "{:?}",
        codes(traited)
    );
}

/// **Found writing the check in Nikaia**: `NAMES.contains(name)` for a
/// `comptime NAMES: Array[ref String, N]` and an owned `name` took a `&&str`
/// below and was given a `String`; and a `let` from an `if` whose `else`
/// returns had no type at all, so the call was not seen as a lookup. Both
/// compile and answer now, and the function does not pause.
#[test]
fn an_owned_text_is_looked_up_in_an_array_of_views() {
    let source = "comptime NAMES: Array[ref String, 2] = [\"a\", \"b\"]\n\
         \n\
         fn known(x: i64) -> bool {\n\
         \x20   let name = if x > 0 { \"b\".to_string() } else { return false }\n\
         \x20   return NAMES.contains(name)\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(known(1))\n\
         \x20   println(known(0))\n\
         }\n";
    assert!(codes(source).is_empty(), "{:?}", codes(source));
    let rust = lowered(source);
    assert!(rust.contains("fn known(x: i64) -> bool"), "{rust}");
    let dir = common::scratch_dir("own-methods-contains");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    assert!(
        out.status.success(),
        "{}\n{rust}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ran = std::process::Command::new(&binary).output().expect("run");
    assert_eq!(String::from_utf8_lossy(&ran.stdout), "true\nfalse\n");
    let _ = std::fs::remove_dir_all(&dir);
}
