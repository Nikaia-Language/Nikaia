//! What the C boundary lends: a view, and no raw pointer
//! ([ADR-302](../../../docs/specification/adr/adr-302.md) D5).
//!
//! `extern "C" { fn malloc(size: usize) -> Pointer[u8] }` was refused with
//! `NK1135`, and [ADR-302](../../../docs/specification/adr/adr-302.md) left
//! it that way on purpose — *a pointer that outlives what it points at wants a
//! record with a lifetime story, not a name.* What that cost was most of C:
//! every function whose signature has a pointer in it was unwritable, so
//! `getpid` compiled and nothing with a buffer did.
//!
//! **The thing to prevent is the dangling dereference**, and D1's answer is
//! that a buffer is a **view** and lives for the call — which is what a view is
//! everywhere else in this language
//! ([ADR-094](../../../docs/specification/adr/adr-094.md)). Nothing is stored
//! and nothing escapes, so the shape that dangles cannot be written.

mod common;

use nikaia::contracts::ty::Ty;
use nikaia::contracts::ty::TyOps;
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    nikaia::check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new())
        .findings
}

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

/// **The four forms lower to the pointer C wants** (D1).
///
/// Rust's own `&[T]` is a **fat** pointer, so a declaration that wrote it would
/// be a signature the two languages disagree about — and what a reader would
/// get for it is `rustc`'s `improper_ctypes` about a file nobody wrote
/// ([Part III C.1](../../../docs/specification/30-nikaia-tooling.md)).
#[test]
fn a_view_in_a_declaration_is_the_pointer() {
    let rust = lowered(
        "extern \"C\" {\n\
         \x20   fn strlen(s: ref Array[u8]) -> usize\n\
         \x20   fn read(fd: i32, buf: ref mut Array[u8], count: usize) -> i64\n\
         \x20   fn takes_one(n: ref i32) -> i32\n\
         \x20   fn fills_one(n: ref mut i32) -> i32\n\
         }\n\
         \n\
         fn main() { println(\"x\") }\n",
    );
    for expected in [
        "fn strlen(s: *const u8) -> usize;",
        "fn read(fd: i32, buf: *mut u8, count: usize) -> i64;",
        "fn takes_one(n: *const i32) -> i32;",
        "fn fills_one(n: *mut i32) -> i32;",
    ] {
        assert!(rust.contains(expected), "{expected}\n--- got ---\n{rust}");
    }
}

/// **And the call is where the address is made.** A declaration says `ref Array[u8]`
/// and C takes the first element's address, so `bytes` becomes
/// `bytes.as_ptr()` — one call that a `Vec`, an `Array` and text all answer.
#[test]
fn a_call_hands_over_the_address() {
    let rust = lowered(
        "extern \"C\" {\n\
         \x20   fn strlen(s: ref Array[u8]) -> usize\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let text = \"hello\"\n\
         \x20   let n = unsafe { strlen(text) }\n\
         \x20   println(f\"{n}\")\n\
         }\n",
    );
    assert!(rust.contains("strlen(text.as_ptr())"), "{rust}");
}

/// **An ordinary value is untouched**: an `i32` is an `i32` at both ends, and
/// nothing is written around it.
#[test]
fn a_value_parameter_is_passed_as_it_was_written() {
    let rust = lowered(
        "extern \"C\" {\n\
         \x20   fn abs(n: i32) -> i32\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let a = unsafe { abs(-3) }\n\
         \x20   println(f\"{a}\")\n\
         }\n",
    );
    assert!(rust.contains("abs(-3)"), "{rust}");
}

/// **A `ref Array[u8]` takes what lends a run of bytes** (D1), and the fit is where the
/// declaration and the caller meet: a list, a fixed-size array and text all
/// hand over the same thing.
#[test]
fn what_a_caller_may_hand_a_run() {
    let run = Ty::parse("ref Array[u8]");
    for written in ["Vec[u8]", "Array[u8, 3]", "String", "ref String", "?"] {
        assert!(
            Ty::parse(written).fits(&run),
            "`{written}` lends a run of bytes"
        );
    }
    for written in ["i64", "Vec[i64]", "bool"] {
        assert!(!Ty::parse(written).fits(&run), "`{written}` does not");
    }
}

/// **The three shapes read back as they were written**, which is what lets a
/// declaration ship in the ledger (Part III 13.5).
#[test]
fn the_boundary_types_round_trip_through_their_text() {
    for written in ["ref Array[u8]", "ref mut Array[u8]", "ref mut i32"] {
        let ty = Ty::parse(written);
        assert_eq!(ty.text(), written);
        assert_eq!(Ty::parse(&ty.text()), ty);
        assert!(matches!(ty, Ty::Pointed { .. }), "{ty:?}");
    }
    // **And every spelling this one replaced reads back as the same type**
    // ([ADR-184](../../../docs/specification/adr/adr-184.md) D1, D3, D4), which
    // is what lets a ledger written before 0.0.134 still be read: `&` left the
    // **language** and this is a machine format the compiler reads, not a
    // program a person writes.
    for (old, new) in [
        ("&[u8]", "ref Array[u8]"),
        ("ref [u8]", "ref Array[u8]"),
        ("&mut [u8]", "ref mut Array[u8]"),
        ("&mut i32", "ref mut i32"),
    ] {
        assert_eq!(Ty::parse(old), Ty::parse(new), "{old}");
        assert_eq!(Ty::parse(old).text(), new, "{old}");
    }
    // A plain view is the one every declaration in this language writes, and
    // it stays what it was: one type, one spelling.
    assert!(matches!(Ty::parse("ref String"), Ty::Named { .. }));
}

/// **A `ref mut Array[u8]` is not a `ref Array[u8]`**, because the second promises not to
/// write; and neither is a `&u8`, because one is a run and the other is one
/// element.
#[test]
fn the_shapes_do_not_fit_each_other() {
    assert!(!Ty::parse("ref mut Array[u8]").fits(&Ty::parse("ref Array[u8]")));
    assert!(!Ty::parse("ref Array[u8]").fits(&Ty::parse("ref mut Array[u8]")));
    assert!(!Ty::parse("ref mut i32").fits(&Ty::parse("ref mut Array[i32]")));
    assert!(Ty::parse("ref mut Array[u8]").fits(&Ty::parse("ref mut Array[u8]")));
}

/// **`&mut` is the C boundary's and nowhere else's** (`NK1158`), and since
/// 0.0.127 that is the whole of the rule
/// ([ADR-179](../../../docs/specification/adr/adr-179.md) D1, which supersedes
/// [ADR-302](../../../docs/specification/adr/adr-302.md) D5 in part).
///
/// A parameter this language may change is written `mut name: T`
/// ([ADR-094](../../../docs/specification/adr/adr-094.md) D3) — the word goes
/// in front of the **name**, because what it decides is also what the caller
/// sees.
///
/// **`&[T]` is a type of the language now.** The reasoning that refused it was
/// *a run of elements whose length the type does not carry has no lowering
/// here*, which was true of a **declaration** and never of a **view**: `&str`
/// is exactly such a run and is what Part I 2.2 gives text.
#[test]
fn the_mut_is_the_boundarys_and_the_run_is_not() {
    let found = findings(
        "fn fill(out: ref mut Vec[i64]) {\n\
         \x20   out.push(1)\n\
         }\n\
         \n\
         fn total(xs: ref Array[i64]) -> i64 {\n\
         \x20   return xs.len()\n\
         }\n\
         \n\
         fn main() { println(\"x\") }\n",
    );
    let refused: Vec<_> = found.iter().filter(|f| f.code == "NK1158").collect();
    assert_eq!(refused.len(), 1, "{found:#?}");
    assert!(refused[0].message.contains("`&mut`"), "{refused:#?}");
    assert!(
        refused[0]
            .help
            .as_deref()
            .unwrap_or_default()
            .contains("mut out"),
        "and the help names the shape this language does have:\n{refused:#?}"
    );
    // …and nothing at all about `total`, whose `&[i64]` is an ordinary type.
    assert!(
        found.iter().all(|f| !f.message.contains("`[T]`")),
        "{found:#?}"
    );
}

/// **A declaration that uses them is not refused**, which is the other half of
/// the same rule.
#[test]
fn the_boundary_itself_is_left_alone() {
    let found = findings(
        "extern \"C\" {\n\
         \x20   fn strlen(s: ref Array[u8]) -> usize\n\
         \x20   fn read(fd: i32, buf: ref mut Array[u8], count: usize) -> i64\n\
         }\n\
         \n\
         fn main() { println(\"x\") }\n",
    );
    assert!(
        found
            .iter()
            .all(|f| f.code != "NK1158" && f.code != "NK1135"),
        "{found:#?}"
    );
}

/// **The declaration reaches the ledger as it was written** (Part III 13.5), so
/// a consumer reads `ref Array[u8]` and not the pointer it lowers to — which is
/// [Part III C.1](../../../docs/specification/30-nikaia-tooling.md)'s rule for
/// every message this compiler writes.
#[test]
fn a_declaration_ships_in_this_languages_words() {
    let parsed = parse_to_ast(
        "extern \"C\" {\n\
         \x20   fn read(fd: i32, buf: ref mut Array[u8], count: usize) -> i64\n\
         }\n",
    )
    .expect("the source parses");
    let written = Ledger::infer(&parsed).render();
    assert!(
        written.contains("signature = \"(fd: i32, buf: ref mut Array[u8], count: usize) -> i64\""),
        "{written}"
    );
}

/// **Measured where it matters: it compiles against real C, and it runs.**
///
/// `strlen` and `abs` are libc, which every Rust program links already — so
/// this needs no library the machine may not have, and it is the whole of D1
/// end to end: the declaration, the pointer, the call, the answer.
#[test]
fn the_boundary_compiles_and_runs() {
    let rust = lowered(
        r#"
extern "C" {
    fn strlen(s: ref Array[u8]) -> usize
    fn abs(n: i32) -> i32
}

fn main() {
    let text = "hello\0"
    let n = unsafe { strlen(text) }
    let a = unsafe { abs(-3) }
    println(f"{n} {a}")
}
"#,
    );
    let dir = common::scratch_dir("foreign-pointers");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    let said = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        out.status.success(),
        "the lowering compiles:\n{said}\n--- the Rust ---\n{rust}"
    );
    assert!(
        !said.contains("improper_ctypes"),
        "and `rustc` says nothing about the signature it was handed:\n{said}"
    );
    let ran = std::process::Command::new(&binary)
        .output()
        .expect("run the program");
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).trim(),
        "5 3",
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// **A length beside a view is checked at the call** (D2).
///
/// A pointer and a count are one fact in C — the first says where and the
/// second says how far — so the declaration is where this language says they
/// belong together, and a call that passes a longer count is the buffer
/// overrun the boundary exists to stop.
#[test]
fn a_length_a_buffer_covers_is_accepted() {
    for count in ["room.len()", "32", "0"] {
        let source = format!(
            "extern \"C\" {{\n\
             \x20   fn read(fd: i32, buf: ref mut Array[u8], count: usize) -> i64\n\
             }}\n\
             \n\
             fn main() {{\n\
             \x20   let mut room: Array[u8, 2] = [0, 0]\n\
             \x20   let n = unsafe {{ read(0, room, {count}) }}\n\
             \x20   println(f\"{{n}}\")\n\
             }}\n",
        );
        // `32` is longer than two, so only the other two stand on their own
        // here; the length that does not fit is the test below.
        let found: Vec<_> = findings(&source)
            .into_iter()
            .filter(|f| f.code == "NK1159")
            .collect();
        match count {
            "32" => assert_eq!(found.len(), 1, "`{count}` is longer than the array"),
            _ => assert!(found.is_empty(), "`{count}` fits: {found:#?}"),
        }
    }
}

/// **And a constant the array's own length covers is accepted**, because an
/// `Array[T, N]` carries its length in its type
/// ([ADR-152](../../../docs/specification/adr/adr-152.md) D1).
#[test]
fn a_constant_within_a_known_length_is_accepted() {
    let source = "extern \"C\" {\n\
                  \x20   fn read(fd: i32, buf: ref mut Array[u8], count: usize) -> i64\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   let mut room: Array[u8, 4] = [0, 0, 0, 0]\n\
                  \x20   let n = unsafe { read(0, room, 4) }\n\
                  \x20   println(f\"{n}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
}

/// **A count nothing can show to fit is refused**, naming both ways out — which
/// is D2's *narrow on purpose*: the buffer's own `len()`, or a constant a known
/// length covers, and anything else asks for one of those two.
#[test]
fn a_length_that_cannot_be_shown_to_fit_is_refused() {
    let found: Vec<_> = findings(
        "extern \"C\" {\n\
         \x20   fn read(fd: i32, buf: ref mut Array[u8], count: usize) -> i64\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let mut buf: Vec[u8] = []\n\
         \x20   buf.push(0)\n\
         \x20   let n = unsafe { read(0, buf, 8) }\n\
         \x20   println(f\"{n}\")\n\
         }\n",
    )
    .into_iter()
    .filter(|f| f.code == "NK1159")
    .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("`buf`"), "{found:#?}");
    assert!(
        found[0]
            .help
            .as_deref()
            .unwrap_or_default()
            .contains("buf.len()"),
        "{found:#?}"
    );
}

/// **A `usize` at the boundary takes this language's own integer** (D2,
/// [ADR-285](../../../docs/specification/adr/adr-285.md) D1).
///
/// A length here is an `i64` and the machine-width type left the surface a
/// program can write, so a declaration that says `size_t` is handed an `i64`
/// and the **conversion is emitted** — a user writes `buf.len()` and never
/// `buf.len() as usize`, which is a cast into a type Part I 2.2 does not offer.
#[test]
fn a_size_takes_an_i64_and_the_conversion_is_written() {
    let source = "extern \"C\" {\n\
                  \x20   fn read(fd: i32, buf: ref mut Array[u8], count: usize) -> i64\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   let mut buf: Vec[u8] = []\n\
                  \x20   buf.push(0)\n\
                  \x20   let n = unsafe { read(0, buf, buf.len()) }\n\
                  \x20   println(f\"{n}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let rust = lowered(source);
    assert!(
        rust.contains("nikaia_std::count::of(buf.len() as i64)"),
        "{rust}"
    );
    // **And no `&` in front of it.** `keeps::moves` did not name `usize` or
    // `isize`, so a type it did not name was one that *moves* and the compiler
    // lent the count ([ADR-094](../../../docs/specification/adr/adr-094.md)
    // D1) — `read(0, buf.as_mut_ptr(), &buf.len() as i64)`, which is not Rust.
    // No program could write a `usize` before this record, so the first
    // declaration to name one is the first program to meet it.
    assert!(!rust.contains("&nikaia_std::count::of"), "{rust}");
    assert!(!rust.contains(", &buf.len()"), "{rust}");
}

/// **The pair is the declaration's own types**, so a `usize` with no buffer in
/// front of it is an ordinary parameter and nothing is claimed about it.
#[test]
fn a_size_with_no_buffer_before_it_is_left_alone() {
    let source = "extern \"C\" {\n\
                  \x20   fn sleep(seconds: usize) -> i32\n\
                  }\n\
                  \n\
                  fn main() {\n\
                  \x20   let n = unsafe { sleep(1) }\n\
                  \x20   println(f\"{n}\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
}

/// **A handle is opaque, declared, and released by a `cleanup`** (D3).
///
/// The shape every C library with a session in it is made of — a database
/// connection, an HTTP client, a compressor — and the reason the record exists
/// beside the buffer: a library that hands one out was unwritable, because
/// `Pointer[T]` is a type nothing declares.
#[test]
fn an_opaque_handle_is_an_address_and_a_cleanup() {
    let rust = lowered(
        "extern \"C\" {\n\
         \x20   opaque type FILE released by fclose\n\
         \x20   fn fopen(path: ref Array[u8], mode: ref Array[u8]) -> FILE\n\
         \x20   fn fclose(f: FILE) -> i32\n\
         }\n\
         \n\
         fn main() { println(\"x\") }\n",
    );
    // `repr(transparent)`, because the whole point of the type is its layout:
    // a handle **is** the address, so what C is handed is the pointer.
    assert!(rust.contains("#[repr(transparent)]"), "{rust}");
    // The hull is **non-null**, which is
    // [ADR-302](../../../docs/specification/adr/adr-302.md) D11: a handle holds
    // an address and `T?` is the absence of one, so `Option<T>` is the same
    // machine word and `ref mut T?` is `T **`.
    assert!(
        rust.contains("pub struct FILE(core::ptr::NonNull<core::ffi::c_void>);"),
        "{rust}"
    );
    // Part I 6.4's `cleanup` read at the C boundary: the release runs at the
    // end of the handle's scope, so a handle cannot be forgotten.
    assert!(rust.contains("impl Drop for FILE"), "{rust}");
    assert!(
        rust.contains("let _released = unsafe { fclose(self.lent()) };"),
        "{rust}"
    );
}

/// **A handle is lent to every declaration but its release** (D3).
///
/// `fileno(f)` reads the handle and `f` is still the caller's to close, so the
/// address goes by value and the value stays here; `fclose(f)` **is** the
/// cleanup, so the handle goes with it and Rust's own move is what keeps it
/// from being released twice.
///
/// The `&` [ADR-094](../../../docs/specification/adr/adr-094.md) D1 would
/// otherwise write is the wrong address as well as the wrong ownership:
/// `&FILE` is `FILE**` where C wants `FILE*`.
#[test]
fn a_handle_is_lent_and_only_its_release_takes_it() {
    let rust = lowered(
        "extern \"C\" {\n\
         \x20   opaque type FILE released by fclose\n\
         \x20   fn fopen(path: ref Array[u8], mode: ref Array[u8]) -> FILE\n\
         \x20   fn fclose(f: FILE) -> i32\n\
         \x20   fn fileno(f: FILE) -> i32\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let f = unsafe { fopen(\"/etc/hosts\\0\", \"r\\0\") }\n\
         \x20   let fd = unsafe { fileno(f) }\n\
         \x20   let g = unsafe { fopen(\"/etc/hosts\\0\", \"r\\0\") }\n\
         \x20   let closed = unsafe { fclose(g) }\n\
         \x20   println(f\"{fd} {closed}\")\n\
         }\n",
    );
    assert!(rust.contains("fileno(f.lent())"), "{rust}");
    assert!(rust.contains("fclose(g)"), "{rust}");
    assert!(!rust.contains("fileno(&f)"), "{rust}");
}

/// **A handle has no fields and no indexing** (`NK1160`, D3): it is an address
/// this language never dereferences, so there is nothing inside it to name.
#[test]
fn nothing_reaches_inside_a_handle() {
    let found: Vec<_> = findings(
        "extern \"C\" {\n\
         \x20   opaque type FILE released by fclose\n\
         \x20   fn fopen(path: ref Array[u8], mode: ref Array[u8]) -> FILE\n\
         \x20   fn fclose(f: FILE) -> i32\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let f = unsafe { fopen(\"/etc/hosts\\0\", \"r\\0\") }\n\
         \x20   println(f\"{f.handle}\")\n\
         \x20   println(f\"{f[0]}\")\n\
         }\n",
    )
    .into_iter()
    .filter(|f| f.code == "NK1160")
    .collect();
    assert_eq!(found.len(), 2, "{found:#?}");
    assert!(
        found
            .iter()
            .any(|f| f.message.contains("the field `handle`")),
        "{found:#?}"
    );
    assert!(
        found.iter().any(|f| f.message.contains("an index")),
        "{found:#?}"
    );
}

/// **None of the four words is reserved.** The grammar is scannerless, so a
/// word means something only where a rule asks for it — and `opaque`, `type`,
/// `released` and `by` are all names a program may want. Reserving a word buys
/// exactly one thing, the sentence a reader who writes it gets
/// ([ADR-298](../../../docs/specification/adr/adr-298.md) D7), and this
/// position says that sentence without taking the word away.
#[test]
fn the_four_words_are_still_names() {
    for word in ["opaque", "type", "released", "by"] {
        let source = format!(
            "fn main() {{\n\
             \x20   let {word} = 3\n\
             \x20   println(f\"{{{word}}}\")\n\
             }}\n"
        );
        assert!(
            parse_to_ast(&source).is_ok(),
            "`{word}` is a name everywhere but the one position that asks for it"
        );
    }
}

/// **Measured where it matters: it compiles against real C, and it runs.**
///
/// `fopen`, `fclose` and `fileno` are libc, which every Rust program links
/// already — so this needs no library the machine may not have. It is the whole
/// of D3 end to end: the handle, the declarations that take it, the lending,
/// and the cleanup that fires once at the end of the scope.
#[test]
fn a_handle_compiles_and_runs_and_is_released_once() {
    let rust = lowered(
        r#"
extern "C" {
    opaque type FILE released by fclose
    fn fopen(path: ref Array[u8], mode: ref Array[u8]) -> FILE
    fn fclose(f: FILE) -> i32
    fn fileno(f: FILE) -> i32
}

fn main() {
    let f = unsafe { fopen("/etc/hosts\0", "r\0") }
    let fd = unsafe { fileno(f) }
    println(f"{fd > 2}")
}
"#,
    );
    let dir = common::scratch_dir("opaque-handle");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    let said = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        out.status.success(),
        "the lowering compiles:\n{said}\n--- the Rust ---\n{rust}"
    );
    assert!(
        !said.contains("improper_ctypes") && !said.contains("warning:"),
        "and `rustc` says nothing about the file it was handed:\n{said}"
    );
    let ran = std::process::Command::new(&binary)
        .output()
        .expect("run the program");
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).trim(),
        "true",
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// **Returned text is a handle a `std` function copies** (D4).
///
/// `getenv` hands back memory the caller does not own, whose lifetime is the
/// library's, and which ends at a zero byte rather than carrying a length.
/// None of those three is something a `String` can be made of without reading
/// it — so the reading is `std`'s, written **once**, inside `unsafe`, where
/// every program would otherwise write the same loop.
#[test]
fn returned_text_is_copied_by_std() {
    let source = "use std::foreign\n\nextern \"C\" {\n\
                  \x20   fn getenv(name: ref Array[u8]) -> foreign::CStr\n\
                  }\n\
                  \n\
                  fn main() throws {\n\
                  \x20   let raw = unsafe { getenv(\"HOME\\0\") }\n\
                  \x20   let home = raw.to_string()\n\
                  \x20   println(home)\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let rust = lowered(source);
    // The declaration hands back the **address**, and the hull goes on at the
    // call, where the claim it made can be checked
    // ([ADR-302](../../../docs/specification/adr/adr-302.md) D12). The copy is a
    // `std` call with no `unsafe` of the program's own around it.
    assert!(
        rust.contains("fn getenv(name: *const u8) -> Option<nikaia_std::foreign::CText>;"),
        "{rust}"
    );
    assert!(
        rust.contains("foreign::CStr::from_c(\"getenv\", getenv("),
        "{rust}"
    );
    assert!(rust.contains("let home = raw.to_string()?;"), "{rust}");
    assert!(
        !rust.contains("unsafe { raw.to_string()"),
        "the `unsafe` is `std`'s and not the program's:\n{rust}"
    );
}

/// **The copy fails rather than guessing**, in the two ways it can — a null
/// address, and bytes that are not UTF-8 — so a caller that does not say
/// `throws` is refused here rather than by the language below.
#[test]
fn the_copy_is_a_call_that_can_fail() {
    let found: Vec<_> = findings(
        "use std::foreign\n\nextern \"C\" {\n\
         \x20   fn getenv(name: ref Array[u8]) -> foreign::CStr\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let raw = unsafe { getenv(\"HOME\\0\") }\n\
         \x20   println(raw.to_string())\n\
         }\n",
    )
    .into_iter()
    .filter(|f| f.code == "NK2605")
    .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
}

/// **Measured where it matters: it compiles against real C, and it runs.**
///
/// `getenv` is libc, and `PATH` is set in every environment a test runs in —
/// so this is the whole of D4 end to end: the declaration, the handle, the
/// copy, and text the program owns.
#[test]
fn a_c_string_compiles_and_runs() {
    let rust = lowered(
        r#"
use std::foreign

extern "C" {
    fn getenv(name: ref Array[u8]) -> foreign::CStr
}

fn main() throws {
    let raw = unsafe { getenv("PATH\0") }
    let path = raw.to_string()
    println(f"{path.len() > 0}")
}
"#,
    );
    let dir = common::scratch_dir("c-string");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    let said = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        out.status.success(),
        "the lowering compiles:\n{said}\n--- the Rust ---\n{rust}"
    );
    let ran = std::process::Command::new(&binary)
        .output()
        .expect("run the program");
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).trim(),
        "true",
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// **A handle that may be absent is a `T?`**
/// ([ADR-302](../../../docs/specification/adr/adr-302.md) D10), meaning what it
/// means everywhere else: `??` and `?.` are how a program gets past it.
#[test]
fn a_handle_may_be_absent() {
    let source = "use std::foreign\n\nextern \"C\" {\n\
                  \x20   fn getenv(name: ref Array[u8]) -> foreign::CStr?\n\
                  }\n\
                  \n\
                  fn main() throws {\n\
                  \x20   let home = unsafe { getenv(\"HOME\\0\") }\n\
                  \x20   println(home?.to_string() ?? \"none\")\n\
                  }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
    let rust = lowered(source);
    assert!(rust.contains("foreign::CStr::maybe(getenv("), "{rust}");
    // The declaration hands back the address C returned, because the hull
    // under a handle is non-null: a null arriving in one is undefined before
    // any check could run.
    assert!(
        rust.contains("fn getenv(name: *const u8) -> Option<nikaia_std::foreign::CText>;"),
        "{rust}"
    );
}

/// **A declaration that does not say `?` is a claim, and it is checked** (D3).
///
/// C may still hand back nothing, and what the program gets then is an abort
/// naming the declaration — never a handle that is secretly null.
#[test]
fn a_declaration_that_claims_a_handle_is_checked() {
    let rust = lowered(
        "extern \"C\" {\n\
         \x20   opaque type FILE released by fclose\n\
         \x20   fn fopen(path: ref Array[u8], mode: ref Array[u8]) -> FILE\n\
         \x20   fn fclose(f: FILE) -> i32\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let f = unsafe { fopen(\"/etc/hosts\\0\", \"r\\0\") }\n\
         \x20   println(\"opened\")\n\
         }\n",
    );
    assert!(rust.contains("FILE::from_c(\"fopen\", fopen("), "{rust}");
    assert!(
        rust.contains("nikaia_std::foreign::nothing_came_back(declaration)"),
        "{rust}"
    );
}

/// **The hull under a handle is non-null, which is what makes `T?` free** (D2).
///
/// A handle holds an address and `T?` is the absence of one — the two states C
/// spells with a pointer and `NULL` — so Rust lays `Option<T>` over the same
/// word and `ref mut T?` is `T **` exactly as C writes it.
#[test]
fn a_nullable_handle_is_one_machine_word() {
    let rust = lowered(
        "extern \"C\" {\n\
         \x20   opaque type Block released by free\n\
         \x20   fn posix_memalign(out: ref mut Block?, alignment: usize, size: usize) -> i32\n\
         \x20   fn free(b: Block)\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   let mut room: Block? = null\n\
         \x20   let rc = unsafe { posix_memalign(room, 64, 128) }\n\
         \x20   println(f\"{rc}\")\n\
         }\n",
    );
    assert!(
        rust.contains("pub struct Block(core::ptr::NonNull<core::ffi::c_void>);"),
        "{rust}"
    );
    assert!(
        rust.contains("fn posix_memalign(out: *mut Option<Block>, alignment: usize, size: usize)"),
        "{rust}"
    );
    assert!(
        rust.contains("let mut room: Option<Block> = None;"),
        "{rust}"
    );
    assert!(
        rust.contains("posix_memalign(&mut room, 64, 128)"),
        "{rust}"
    );
}

/// **At the boundary a `?` on a view belongs to what it points at** (D5), and
/// it is the one place Part I 2.3's own reading is turned around — only for
/// `ref mut T` and `ref Array[T]`. A plain `ref T?` stays a nullable view.
#[test]
fn the_question_mark_binds_to_the_pointee() {
    let out = Ty::parse("ref mut sqlite3?");
    assert_eq!(out.text(), "ref mut sqlite3?");
    assert_eq!(Ty::parse(&out.text()), out);
    assert_eq!(
        Ty::parse("ref mut sqlite3?"),
        out,
        "the spelling it replaces"
    );
    let Ty::Pointed { item, .. } = &out else {
        panic!("{out:?}")
    };
    assert!(matches!(item.as_ref(), Ty::Nullable(_)), "{out:?}");

    // Untouched: `let mut m: ref String? = null` is Part I 2.3's own example.
    assert!(matches!(Ty::parse("ref String?"), Ty::Nullable(_)));
}

/// **`there is no text` stopped being a failure and became a value** (D4), so
/// the copy fails in one way rather than two.
#[test]
fn the_copy_fails_in_one_way() {
    let library = Ledger::parse(STD).expect("std ships a ledger");
    let copy = library
        .functions
        .get("foreign::CStr::to_string")
        .expect("std describes the copy");
    // **One way, and since [ADR-280](../../../docs/specification/adr/adr-280.md)
    // D1 it has a name for it.** The doc below has always said *the same
    // failure `fs::read_to_string` has*, and a set of one that says which is
    // stronger evidence for *one way* than a `"?"` ever was.
    assert_eq!(copy.fails_with, vec!["io::IoError".to_string()]);
}

/// **Measured where it matters: the nullable runs, both ways.**
///
/// `HOME` is set and `NIKAIA_NOT_SET` is not, so one call finds text and the
/// other finds nothing — and finding nothing is a **value** the program reads
/// with `??` rather than a failure it has to catch.
#[test]
fn an_absent_handle_is_a_value_and_runs() {
    let rust = lowered(
        r#"
use std::foreign

extern "C" {
    fn getenv(name: ref Array[u8]) -> foreign::CStr?
}

fn main() throws {
    let home = unsafe { getenv("HOME\0") }
    let found = home?.to_string() ?? "none"
    println(f"{found.len() > 0}")
    let nope = unsafe { getenv("NIKAIA_NOT_SET\0") }
    println(nope?.to_string() ?? "none")
}
"#,
    );
    let dir = common::scratch_dir("absent-handle");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(&file, &["-o", binary.to_str().expect("utf-8 path")]);
    let said = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        out.status.success(),
        "the lowering compiles:\n{said}\n--- the Rust ---\n{rust}"
    );
    assert!(
        !said.contains("warning:"),
        "and `rustc` says nothing about the file it was handed:\n{said}"
    );
    let ran = std::process::Command::new(&binary)
        .env("HOME", "/home/somebody")
        .env_remove("NIKAIA_NOT_SET")
        .output()
        .expect("run the program");
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).trim(),
        "true\nnone",
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// **`sqlite3` from end to end**
/// ([ADR-302](../../../docs/specification/adr/adr-302.md) step 5), which is
/// that record's own check that its four decisions are enough — for a **real
/// library's** surface rather than for four libc calls.
///
/// Everything the boundary offers is in `examples/sqlite/main.nika` and nothing
/// else is: a buffer lent for the call, an opaque handle with its `cleanup`, a
/// handle that may be absent filled through an out-parameter
/// ([ADR-302](../../../docs/specification/adr/adr-302.md) D10), and text the
/// library owns copied once by `std`. What closes the database and the
/// statement is written **nowhere**.
///
/// **It skips rather than fails where the machine has no `libsqlite3`.** A gate
/// that depends on a library not every machine has is a gate people learn to
/// ignore, and what this test is for is the four decisions rather than the
/// presence of a package — libc already proves those, on any machine, in the
/// four tests above.
#[test]
fn sqlite3_from_end_to_end() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/sqlite/main.nika"),
    )
    .expect("the example is in the tree");
    let rust = lowered(&source);

    // What the record's four decisions look like in one program.
    for expected in [
        // D1: a buffer lent for the call.
        "sql: *const u8",
        // D3: a handle, its cleanup, and the lending.
        "impl Drop for sqlite3",
        "sqlite3_step(stmt.lent())",
        // ADR-302 D10 and D11: the out-parameter, one pointer to one pointer.
        "out: *mut Option<sqlite3>",
        "let mut slot: Option<sqlite3> = None;",
        // D4: text the library owns, copied by `std`.
        "foreign::CStr::maybe(sqlite3_column_text(",
    ] {
        assert!(
            rust.contains(expected),
            "{expected}\n--- the Rust ---\n{rust}"
        );
    }

    let dir = common::scratch_dir("sqlite3");
    let file = dir.join("main.rs");
    std::fs::write(&file, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let out = common::compile(
        &file,
        &["-l", "sqlite3", "-o", binary.to_str().expect("utf-8 path")],
    );
    let said = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() {
        // **The one thing that is allowed to be missing.** Anything else is a
        // failure of the lowering and is reported as one.
        assert!(
            said.contains("-lsqlite3") || said.contains("library 'sqlite3' not found"),
            "the lowering compiles where the library is there:\n{said}\n--- the Rust ---\n{rust}"
        );
        let _ = std::fs::remove_dir_all(&dir);
        eprintln!("skipped: this machine has no `libsqlite3`");
        return;
    }
    assert!(
        !said.contains("warning:"),
        "and `rustc` says nothing about the file it was handed:\n{said}"
    );
    let ran = std::process::Command::new(&binary)
        .output()
        .expect("run the program");
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).trim(),
        "opened 0\nprepared 0\nstepped 100\nhello, C",
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// **`NK1161`: a `throw` of something that is not an error** (Part I 7.1), and
/// the second thing step 5 found.
///
/// *"What is thrown implements `Error`, and the `impl` line says so."* A number,
/// a `bool`, a `char` and text are Part I 2.2's own types and none of them does
/// — nor ever will. `throw "no database"` read like a program and `rustc`
/// answered *the trait bound `str: Error` is not satisfied* about a file nobody
/// wrote, which is [Part III C.1](../../../docs/specification/30-nikaia-tooling.md)'s
/// class. It had been writable since `throw` existed, and no program in the
/// tree had written one.
#[test]
fn what_is_thrown_is_an_error() {
    for (thrown, named) in [
        ("\"no database\"", "text"),
        ("3", "a number"),
        ("true", "a `bool`"),
    ] {
        let found: Vec<_> = findings(&format!("fn main() throws {{\n\x20   throw {thrown}\n}}\n"))
            .into_iter()
            .filter(|f| f.code == "NK1161")
            .collect();
        assert_eq!(found.len(), 1, "`throw {thrown}`: {found:#?}");
        assert!(found[0].message.contains(named), "{found:#?}");
    }
}

/// **And it is silent about everything else** (C.4), which is what makes the
/// rule safe: a type this file declares may have its `impl Error` in another
/// file of the same package, a package's type is not this compiler's to answer
/// for, and a caught error re-thrown is a `?`.
#[test]
fn a_throw_this_compiler_cannot_type_is_left_alone() {
    let quiet = [
        "enum Refused { NotFound }\n\
         \n\
         fn main() throws {\n\x20   throw Refused::NotFound\n}\n",
        "fn main() throws {\n\
         \x20   let n = \"3\".parse() catch { throw error }\n\
         \x20   println(f\"{n}\")\n\
         }\n",
    ];
    for source in quiet {
        assert!(
            findings(source).iter().all(|f| f.code != "NK1161"),
            "{:#?}",
            findings(source)
        );
    }
}
