//! `nikaia describe <crate>` — the draft a boundary is described by
//! ([ADR-104](../../../docs/specification/adr/adr-104.md) D2, D3, D4).
//!
//! D1 refuses a call into a crate nothing describes and names this command.
//! What it writes is the file every analysis then reads there, so what it gets
//! wrong is wrong at the boundary of a program — which is why the strongest
//! test here is not a fixture: it reads the repository's own
//! `examples/foreign-runtime/shim` and asserts the draft against the file a
//! reviewer wrote by hand, entry for entry.
//!
//! **What a scraper cannot do is asserted too.** It reads `pub fn` and
//! `pub struct` out of `.rs` text; a signature it cannot translate is written
//! `?`, which is the absence of a claim and D5's own `?` for a person to fill.
//! A test that only showed the cases it gets right would be describing a
//! different tool.

use nikaia::contracts::LedgerOps;
use std::path::{Path, PathBuf};

use nikaia::describe::draft;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A project with one Rust path dependency, and whatever that crate's sources
/// and the program's text are.
fn project(name: &str, crate_source: &str, program: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "nikaia-describe-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).expect("a directory to work in");
    // The path is relative to **`nikaia.toml`**
    // ([ADR-197](../../../docs/specification/adr/adr-197.md) D1), so the crate
    // sits beside `src/` and is named as such. It used to climb three levels,
    // against a generated manifest — which is how this test and `cargo build`
    // came to look in two different places for one crate's sources.
    std::fs::create_dir_all(root.join("fremd/src")).expect("a crate to describe");
    std::fs::write(
        root.join("nikaia.toml"),
        "[package]\n\
         name = \"probe\"\n\
         version = \"0.1.0\"\n\
         \n\
         [dependencies]\n\
         fremd = { type = \"rust\", path = \"fremd\" }\n",
    )
    .expect("write the manifest");
    std::fs::write(
        root.join("fremd/Cargo.toml"),
        "[package]\nname = \"fremd\"\nversion = \"2.1.0\"\nedition = \"2021\"\n",
    )
    .expect("write the crate's manifest");
    std::fs::write(root.join("fremd/src/lib.rs"), crate_source).expect("write the crate");
    std::fs::write(root.join("src/main.nika"), program).expect("write the program");
    root
}

/// The same, with more of the crate than its `lib.rs`.
///
/// A module is a **file** as often as it is a block, and what module a file is
/// takes the `mod foo;` in its parent to say — so a crate with one file cannot
/// show that half at all.
fn project_of_files(name: &str, crate_files: &[(&str, &str)], program: &str) -> PathBuf {
    let root = project(name, "", program);
    for (relative, text) in crate_files {
        let at = root.join("fremd/src").join(relative);
        if let Some(parent) = at.parent() {
            std::fs::create_dir_all(parent).expect("a directory for the module");
        }
        std::fs::write(at, text).expect("write the module");
    }
    root
}

/// The entries of a draft, as the file would read them.
fn entries(root: &Path) -> String {
    let (ledger, described) = draft(root, "fremd").expect("the draft is written");
    let text = ledger.render_description("fremd", &described.version, &described.notes);
    let _ = std::fs::remove_dir_all(root);
    text
}

/// **What the scanner reported that is not there**, and the one thing it could
/// never report.
///
/// The reading half is a grammar written in Nikaia now
/// ([ADR-195](../../../docs/specification/adr/adr-195.md) D3,
/// [ADR-196](../../../docs/specification/adr/adr-196.md) D1), and this is the
/// difference stated as an assertion. On the crate below the character scanner
/// wrote entries for **four functions that do not exist** — `spectre` inside a
/// block comment, `phantom` on the second line of a string literal, `hidden`
/// and `buried` inside private modules — and put `seen` at the crate root
/// rather than under `shown`.
///
/// **And `rescued` is the direction that matters more.** It is `hidden` again,
/// carried out of a private `mod` by a `pub use`, so a program that calls it
/// calls something this crate really offers. Nothing answered it before, and
/// refusing a call a crate answers is
/// [Part III C.4](../../../docs/specification/30-nikaia-tooling.md) — which is
/// the whole reason the grammar reads `pub use` and keeps a private `mod`'s
/// items instead of throwing the body away.
#[test]
fn an_item_that_is_only_text_is_not_described_and_a_re_export_is() {
    let root = project(
        "phantoms",
        "/* A block comment that says\n\
         pub fn spectre(a: i32) -> i32\n\
         and means nothing by it. */\n\
         \n\
         pub fn real(a: i32) -> i32 {\n\
         let template = \"fn main() {\n\
         pub fn phantom(a: i32) -> i32 { a }\n\
         }\";\n\
         let _ = template;\n\
         a\n\
         }\n\
         \n\
         mod private {\n\
         pub fn hidden(x: i32) -> i32 { x }\n\
         }\n\
         \n\
         pub use private::hidden as rescued;\n\
         \n\
         pub mod shown {\n\
         pub fn seen(x: i32) -> i32 { x }\n\
         mod deeper {\n\
         pub fn buried() {}\n\
         }\n\
         }\n",
        "fn main() {\n\
         let a = fremd::real(1)\n\
         let b = fremd::spectre(2)\n\
         let c = fremd::phantom(3)\n\
         let d = fremd::hidden(4)\n\
         let e = fremd::buried()\n\
         let f = fremd::seen(5)\n\
         let g = fremd::shown::seen(6)\n\
         let h = fremd::rescued(7)\n\
         println(f\"{a}{b}{c}{d}{e}{f}{g}{h}\")\n\
         }\n",
    );

    let text = entries(&root);

    for phantom in ["spectre", "phantom", "hidden", "buried"] {
        assert!(
            !text.contains(&format!("fremd::{phantom}")),
            "`fremd::{phantom}` is not a function of this crate:\n{text}"
        );
    }
    // At the crate root it is not reachable; under its module it is.
    assert!(!text.contains("[fn.\"fremd::seen\"]"), "{text}");
    assert!(text.contains("[fn.\"fremd::shown::seen\"]"), "{text}");
    assert!(text.contains("[fn.\"fremd::real\"]"), "{text}");
    assert!(text.contains("[fn.\"fremd::rescued\"]"), "{text}");
}

/// **What module a file is**, which takes the `mod foo;` in its parent to say.
///
/// The scanner read every `.rs` under `src/` as the crate's own, so `seen` and
/// `buried` were both `fremd::…` and neither was where a caller writes it. The
/// grammar reads the declarations: `pub mod shown;` offers `shown::seen`, and
/// `mod private;` offers nothing — and a `pub use` out of it offers one name.
///
/// **A module nothing declares is not offered**, which is fail-closed
/// ([ADR-010](../../../docs/specification/adr/adr-010.md) D1): the absence is
/// *nobody said this is public*, and the name reaches the reviewer as a `?`
/// rather than the draft as a claim.
#[test]
fn a_module_is_a_file_as_often_as_it_is_a_block() {
    let root = project_of_files(
        "files",
        &[
            (
                "lib.rs",
                "pub mod shown;\nmod private;\npub use private::hidden as rescued;\nmod orphan_is_declared_nowhere {}\n",
            ),
            ("shown.rs", "pub fn seen(x: i32) -> i32 { x }\n"),
            ("private.rs", "pub fn hidden(x: i32) -> i32 { x }\n"),
            ("loose.rs", "pub fn adrift(x: i32) -> i32 { x }\n"),
        ],
        "fn main() {\n\
         let a = fremd::shown::seen(1)\n\
         let b = fremd::seen(2)\n\
         let c = fremd::private::hidden(3)\n\
         let d = fremd::rescued(4)\n\
         let e = fremd::loose::adrift(5)\n\
         let f = fremd::adrift(6)\n\
         println(f\"{a}{b}{c}{d}{e}{f}\")\n\
         }\n",
    );

    let text = entries(&root);

    // Declared `pub`: offered, under its module.
    assert!(text.contains("[fn.\"fremd::shown::seen\"]"), "{text}");
    assert!(!text.contains("[fn.\"fremd::seen\"]"), "{text}");
    // Declared without `pub`: offered only by what the `pub use` carries out.
    assert!(!text.contains("[fn.\"fremd::private::hidden\"]"), "{text}");
    assert!(text.contains("[fn.\"fremd::rescued\"]"), "{text}");
    // Declared nowhere: fail-closed, at either path a caller might guess.
    assert!(!text.contains("adrift"), "{text}");
}

/// **The describer proposes and never claims**
/// ([ADR-193](../../../docs/specification/adr/adr-193.md) D3), and **flags the
/// promise a toolchain cannot check** (D5) — both on the repository's own shim,
/// which has all three of D4's shapes in it on purpose.
///
/// * `across_a_thread<T: Describe + Send + 'static>` — the bound is there, so
///   the note is. Every **safe** way of reaching another thread carries it and
///   Rust's own type system does the propagation, which is why this is not a
///   heuristic.
/// * `across_a_thread_unchecked<T: Describe + 'static>` — the bound is gone,
///   because an `unsafe impl Send` took it away, and only a call graph reaches
///   the `spawn`. **Correctly silent.**
/// * `unsafe impl<T> Send for Smuggled<T>` — one syntactic pattern, and sound
///   in the only sense that matters: the item is in the text or it is not. It
///   is the hole every other step in D4 is blind to, and `Smuggled` is a
///   **private** type nothing else here can see.
///
/// And what none of it can do, which the note has to say: a tool can see that
/// the promise was made and not whether it is true.
#[test]
fn what_the_describer_saw_is_a_note_and_never_a_column() {
    let described = |project: &str| {
        let root = repo_root().join("examples/foreign-runtime").join(project);
        let (ledger, described) = draft(&root, "hyper_shim").expect("the draft is written");
        ledger.render_description("hyper_shim", &described.version, &described.notes)
    };

    // **D5 is about the crate**, so every draft of it carries the flag - the
    // program that calls the lying function and the one that does not.
    for project in ["crossing", "smuggled"] {
        let text = described(project);
        assert!(
            text.contains("unsafe impl Send for Smuggled<T>"),
            "{project}: the promise is named:\n{text}"
        );
        assert!(
            text.contains("cannot see whether it is true"),
            "{project}: and what the tool cannot do is said:\n{text}"
        );
        assert!(
            !text.contains("\nthreads ="),
            "{project}: and nothing is claimed - the column stays a person's:\n{text}"
        );
    }

    // **D3 is about one entry**, and stands above it.
    let text = described("crossing");
    let above = text
        .split("[fn.\"hyper_shim::across_a_thread\"]")
        .next()
        .expect("the text before the entry");
    assert!(
        above.contains("the parameter `value` is bound `Send`"),
        "the evidence is named:\n{text}"
    );
    assert!(
        above.contains("threads = true | false"),
        "and the question is asked:\n{text}"
    );

    // And the row the bound does not reach: `across_a_thread_unchecked` takes
    // the same value and says nothing about sending it.
    let text = described("smuggled");
    let above = text
        .split("[fn.\"hyper_shim::across_a_thread_unchecked\"]")
        .next()
        .expect("the text before the entry");
    let immediately = above.rsplit("\n\n").next().unwrap_or("");
    assert!(
        !immediately.contains("is bound `Send`"),
        "the bound is gone, so the note is:\n{text}"
    );
}

/// **The row a bound cannot answer** — [ADR-193](../../../docs/specification/adr/adr-193.md)
/// D4's call graph, on the shim it was written from.
///
/// `across_a_thread_unchecked` asks its caller for nothing: an
/// `unsafe impl<T> Send for Smuggled<T>` took the bound away. The only thing
/// between it and a `tokio::spawn` is `on_one_worker`, which is **private** —
/// nothing outside the crate can call it, and a reader that dropped a private
/// function could not follow the calls at all.
///
/// And the sentence the note has to carry, which is why this is a note: *a sink
/// reached through a call says this function threads **something**, never this
/// function threads **your argument***. Connecting those is dataflow through a
/// closure capture and is not built.
#[test]
fn a_bound_that_was_taken_away_is_found_by_following_the_calls() {
    let described = |project: &str| {
        let root = repo_root().join("examples/foreign-runtime").join(project);
        let (ledger, described) = draft(&root, "hyper_shim").expect("the draft is written");
        ledger.render_description("hyper_shim", &described.version, &described.notes)
    };

    let text = described("smuggled");
    let above = text
        .split("[fn.\"hyper_shim::across_a_thread_unchecked\"]")
        .next()
        .expect("the text before the entry");
    assert!(
        above.contains("reaches `tokio::spawn` through `on_one_worker`"),
        "the path is named:\n{text}"
    );
    assert!(
        above.contains("this function threads something"),
        "and what a sink does say:\n{text}"
    );
    assert!(
        above.contains("never *this function threads your argument*"),
        "and what it does not:\n{text}"
    );

    // The one with the bound gets **both**, because they are two answers to
    // two questions and a reviewer wants each.
    let text = described("crossing");
    let above = text
        .split("[fn.\"hyper_shim::across_a_thread\"]")
        .next()
        .expect("the text before the entry");
    assert!(above.contains("is bound `Send`"), "{text}");
    assert!(above.contains("reaches `tokio::spawn`"), "{text}");

    // And a function that reaches no sink says nothing. `local_handle` calls
    // `std::rc::Rc::new` and nothing else.
    let above = text
        .split("[fn.\"hyper_shim::local_handle\"]")
        .next()
        .expect("the text before the entry");
    let immediately = above.rsplit("\n\n").next().unwrap_or("");
    assert!(
        !immediately.contains("`local_handle`"),
        "a function that threads nothing is not asked about:\n{text}"
    );
}

/// **A `use` makes one function two strings**, and the table is what puts them
/// back together ([ADR-193](../../../docs/specification/adr/adr-193.md) D4).
///
/// Written as its own test because the shim writes `tokio::spawn` in full, so
/// the repository's own evidence never exercises the table — and a crate that
/// imports what it calls is the common case rather than the exception.
#[test]
fn a_call_is_resolved_through_the_files_use_items() {
    let root = project(
        "imports",
        "use std::thread::spawn;\n\
         \n\
         pub fn losschicken(x: i32) -> i32 {\n\
         spawn(move || x);\n\
         x\n\
         }\n\
         \n\
         pub fn ruhig(x: i32) -> i32 { x }\n",
        "fn main() {\n    fremd::losschicken(1)\n    fremd::ruhig(2)\n}",
    );
    let text = entries(&root);
    let above = text
        .split("[fn.\"fremd::losschicken\"]")
        .next()
        .expect("the text before the entry");
    assert!(
        above.contains("calls `std::thread::spawn`"),
        "`spawn` is `std::thread::spawn` in this file:\n{text}"
    );

    // And the one that calls nothing says nothing, which is what keeps the
    // note worth reading.
    let above = text
        .split("[fn.\"fremd::ruhig\"]")
        .next()
        .expect("the text before the entry");
    let immediately = above.rsplit("\n\n").next().unwrap_or("");
    assert!(!immediately.contains("`ruhig`"), "{text}");
}

/// **`Send` is a word**, and a bound that merely contains the letters is not it.
///
/// The same rule the grammar's keywords are written under, one layer up: a
/// scanner that matched the text alone would propose a note about `Sender`,
/// `Resend` and `NoSendMarker`, and a note about a parameter nothing sends is
/// worse than no note — it asks a reviewer a question with no answer.
#[test]
fn a_bound_that_only_contains_the_letters_is_not_a_send_bound() {
    let root = project(
        "lookalikes",
        "pub fn tarnung<T: Into<Sender>, U: Resend + 'static, V: Send>(a: T, b: U, c: V) {}\n",
        "fn main() {\n    fremd::tarnung(1, 2, 3)\n}",
    );
    let text = entries(&root);
    let above = text
        .split("[fn.\"fremd::tarnung\"]")
        .next()
        .expect("the text before the entry");
    assert!(
        above.contains("the parameter `c` is bound `Send`"),
        "the real one is found:\n{text}"
    );
    assert!(
        !above.contains("`a`") && !above.contains("`b`"),
        "and the lookalikes are not:\n{text}"
    );
}

/// **D3's table, row by row**, on signatures written to exercise each one.
#[test]
fn a_signature_is_translated_by_the_table() {
    let root = project(
        "table",
        "pub struct Handle { inner: u32 }\n\
         pub fn plain(port: i64) -> String { String::new() }\n\
         pub fn lent(text: &str) -> bool { true }\n\
         pub fn owned(text: String) -> Handle { Handle { inner: 0 } }\n\
         pub async fn pausing(n: i64) -> i64 { n }\n\
         pub fn failing(n: i64) -> Result<i64, Handle> { Ok(n) }\n\
         pub fn maybe(n: i64) -> Option<String> { None }\n\
         pub fn over<T>(value: T) -> String where T: Send { String::new() }\n\
         pub fn unreadable(x: std::collections::HashMap<u8, u8>) -> u8 { 0 }\n",
        "fn main() {\n\
         \x20   fremd::plain(1)\n\
         \x20   fremd::lent(\"a\")\n\
         \x20   fremd::owned(\"a\")\n\
         \x20   fremd::pausing(1)\n\
         \x20   fremd::failing(1)\n\
         \x20   fremd::maybe(1)\n\
         \x20   fremd::over(1)\n\
         \x20   fremd::unreadable(1)\n\
         }\n",
    );
    let text = entries(&root);

    // `T` by value is **kept**: the crate takes ownership, so the caller may
    // not lend it. A number is not, because there is nothing a caller could
    // otherwise have gone on using.
    assert!(
        text.contains("[fn.\"fremd::plain\"]\npub = true\nsync = true\nsignature = \"(port: i64) -> String\"\n"),
        "{text}"
    );
    // `&T` is a view, and a view is not kept.
    assert!(
        text.contains(
            "[fn.\"fremd::lent\"]\npub = true\nsync = true\nsignature = \"(text: ref String) -> bool\"\n"
        ),
        "{text}"
    );
    assert!(text.contains("keeps = [\"text\"]"), "{text}");
    assert!(
        text.contains("signature = \"(text: String) -> fremd::Handle\""),
        "the crate's own type carries the crate's word: {text}"
    );
    // `async fn` may pause; a plain `fn` is `sync`.
    assert!(
        text.contains("[fn.\"fremd::pausing\"]\npub = true\nsignature ="),
        "an `async fn` gets no `sync` line: {text}"
    );
    // `Result<T, E>` is a `throws`, with the error named where it can be.
    assert!(text.contains("throws = [\"fremd::Handle\"]"), "{text}");
    assert!(
        text.contains("[fn.\"fremd::failing\"]")
            && text.contains("signature = \"(n: i64) -> i64\""),
        "{text}"
    );
    // `Option<T>` is `T?`.
    assert!(
        text.contains("signature = \"(n: i64) -> String?\""),
        "{text}"
    );
    // A type parameter is the ledger's variable, declared in brackets the way
    // Nikaia declares one (ADR-251 D4).
    assert!(
        text.contains("signature = \"[T](value: T) -> String\""),
        "{text}"
    );
    // And a type this cannot account for is `?` — the absence of a claim, for
    // a reviewer to fill (D5), never a guess.
    assert!(text.contains("signature = \"(x: ?) -> u8\""), "{text}");

    // The type an entry names gets an entry of its own, and its `crosses`:
    // whether a value of it may cross a thread follows from its *fields*, which
    // the describer reads (ADR-123 D2): `Handle`'s one field is a `u32`.
    assert!(
        text.contains("[type.\"fremd::Handle\"]\npub = true\ncrosses = true\n"),
        "{text}"
    );
}

/// **The one claim that comes from a field** — `crosses`
/// ([ADR-123](../../../docs/specification/adr/adr-123.md) D2), and the one
/// thing here a Rust *signature* could never say.
///
/// Three answers and the third is the common one: `false` where a field holds
/// something the language below marks as not sendable, `true` where every field
/// is something the scraper knows to be sendable, and **nothing** where it
/// cannot tell. Silence is *nobody said*, which is not permission
/// ([ADR-010](../../../docs/specification/adr/adr-010.md) D1) and not a refusal
/// either.
#[test]
fn a_types_fields_answer_whether_it_crosses() {
    let root = project(
        "crosses",
        "pub struct Held { inner: std::rc::Rc<String> }\n\
         pub struct Pointed { at: *const u8 }\n\
         pub struct Plain { n: i64, name: String, more: Vec<i64> }\n\
         pub struct Opaque { held: OtherCratesThing }\n\
         pub struct Counted { shared: std::sync::Arc<String> }\n\
         pub struct Tuple(i64);\n\
         pub fn a(x: Held) -> i64 { 0 }\n\
         pub fn b(x: Pointed) -> i64 { 0 }\n\
         pub fn c(x: Plain) -> i64 { 0 }\n\
         pub fn d(x: Opaque) -> i64 { 0 }\n\
         pub fn e(x: Counted) -> i64 { 0 }\n\
         pub fn f(x: Tuple) -> i64 { 0 }\n",
        "fn main() {\n\
         \x20   fremd::a(1)\n\
         \x20   fremd::b(1)\n\
         \x20   fremd::c(1)\n\
         \x20   fremd::d(1)\n\
         \x20   fremd::e(1)\n\
         \x20   fremd::f(1)\n\
         }\n",
    );
    let text = entries(&root);
    let says = |name: &str| {
        let at = text
            .find(&format!("[type.\"fremd::{name}\"]"))
            .unwrap_or_else(|| panic!("no entry for {name}:\n{text}"));
        let rest = &text[at..];
        let end = rest[1..].find("\n[").map(|e| e + 1).unwrap_or(rest.len());
        rest[..end].to_string()
    };

    // An `Rc` and a raw pointer are what D2 names, and a field reader sees both.
    assert!(says("Held").contains("crosses = false"), "{}", says("Held"));
    assert!(
        says("Pointed").contains("crosses = false"),
        "{}",
        says("Pointed")
    );

    // Every field a scalar, a `String`, or one of those in a container that
    // changes nothing.
    assert!(
        says("Plain").contains("crosses = true"),
        "{}",
        says("Plain")
    );

    // **And silence for the rest**, which is the common answer rather than the
    // exception: a field of another crate's type says nothing, and an `Arc<T>`
    // is `Send` exactly when its `T` is `Send` *and* `Sync` — two questions a
    // scraper does not have.
    assert!(!says("Opaque").contains("crosses"), "{}", says("Opaque"));
    assert!(!says("Counted").contains("crosses"), "{}", says("Counted"));

    // A tuple struct's body is not read at all, and *nobody looked* is not
    // *it holds nothing*.
    assert!(!says("Tuple").contains("crosses"), "{}", says("Tuple"));
}

/// **A type's `derive` says whether it copies and compares** ([ADR-252](../../../docs/specification/adr/adr-252.md)
/// D4.3): `Copy` is what a declared struct holding it needs to copy too, and
/// `PartialEq` is what `==` on it needs. A type with no `derive` says neither.
#[test]
fn a_derive_says_whether_a_type_copies_and_compares() {
    let root = project(
        "derives",
        "#[derive(Clone, Copy, PartialEq, Debug)]\n\
         pub struct Pair { a: i64 }\n\
         #[derive(Clone, std::cmp::PartialEq)]\n\
         pub struct Compared { a: String }\n\
         pub struct Plain { a: i64 }\n\
         pub fn a(x: Pair) -> i64 { 0 }\n\
         pub fn b(x: Compared) -> i64 { 0 }\n\
         pub fn c(x: Plain) -> i64 { 0 }\n",
        "fn main() {\n\
         \x20   fremd::a(1)\n\
         \x20   fremd::b(1)\n\
         \x20   fremd::c(1)\n\
         }\n",
    );
    let text = entries(&root);
    let says = |name: &str| {
        let at = text
            .find(&format!("[type.\"fremd::{name}\"]"))
            .unwrap_or_else(|| panic!("no entry for {name}:\n{text}"));
        let rest = &text[at..];
        let end = rest[1..].find("\n[").map(|e| e + 1).unwrap_or(rest.len());
        rest[..end].to_string()
    };

    assert!(says("Pair").contains("copies = true"), "{}", says("Pair"));
    assert!(says("Pair").contains("compares = true"), "{}", says("Pair"));

    // A path in the `derive` is read by its last segment.
    assert!(
        says("Compared").contains("compares = true"),
        "{}",
        says("Compared")
    );
    assert!(!says("Compared").contains("copies"), "{}", says("Compared"));

    assert!(!says("Plain").contains("copies"), "{}", says("Plain"));
    assert!(!says("Plain").contains("compares"), "{}", says("Plain"));
}

/// **An entry exists because a program asked for it** ([ADR-028](../../../docs/specification/adr/adr-028.md)
/// D5), so the draft is proportional to use and not to the crate.
#[test]
fn only_what_the_program_calls_is_described() {
    let root = project(
        "asked",
        "pub fn used(n: i64) -> i64 { n }\n\
         pub fn unused(n: i64) -> i64 { n }\n",
        "fn main() { fremd::used(1) }\n",
    );
    let text = entries(&root);
    assert!(text.contains("fremd::used"), "{text}");
    assert!(!text.contains("fremd::unused"), "{text}");
}

/// **A name no signature answered is named**, because that is what a reviewer
/// does next: it is a macro's item, or a `mod` this scraper read flat, or a
/// typo — and a draft that silently left it out would look complete.
#[test]
fn a_name_no_signature_answers_is_reported() {
    let root = project(
        "unanswered",
        "pub fn used(n: i64) -> i64 { n }\n",
        "fn main() {\n\
         \x20   fremd::used(1)\n\
         \x20   fremd::from_a_macro(1)\n\
         }\n",
    );
    let (_, described) = draft(&root, "fremd").expect("the draft is written");
    assert_eq!(described.unanswered, ["fremd::from_a_macro"]);
    let _ = std::fs::remove_dir_all(&root);
}

/// **A version dependency says why it cannot be read**, rather than guessing at
/// Cargo's registry cache — which would be resolving a version, the one thing
/// [ADR-002](../../../docs/specification/adr/adr-002.md) D1 hands to Cargo and
/// never does itself.
#[test]
fn a_version_dependency_says_what_is_missing() {
    let root = std::env::temp_dir().join(format!("nikaia-describe-version-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("a directory to work in");
    std::fs::write(
        root.join("nikaia.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\n\n\
         [dependencies]\nregex = { type = \"rust\", version = \"1\" }\n",
    )
    .expect("write the manifest");
    let said = format!(
        "{:#}",
        draft(&root, "regex").expect_err("no sources to read")
    );
    assert!(
        said.contains("`regex` is a dependency by version"),
        "{said}"
    );
    assert!(
        said.contains("can only read crates given by `path`"),
        "{said}"
    );
    // And the way out, in the reader's words rather than the specification's.
    assert!(
        said.contains("write `contracts/regex.contracts` by hand"),
        "{said}"
    );
    assert!(!said.contains("ADR-"), "{said}");
    let _ = std::fs::remove_dir_all(&root);
}

/// And a crate the manifest does not declare is not describable, because the
/// project is what says a build links against it (D1).
#[test]
fn a_crate_nothing_declares_is_not_described() {
    let root = project(
        "undeclared",
        "pub fn used(n: i64) -> i64 { n }\n",
        "fn main() { }\n",
    );
    let said = format!(
        "{:#}",
        draft(&root, "regex").expect_err("nothing declares it")
    );
    assert!(
        said.contains("`regex` isn't in this project's `[dependencies]`"),
        "{said}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// **The measurement**: the draft for the repository's own experiment, against
/// the file a reviewer wrote by hand before this command existed.
///
/// `examples/foreign-runtime/*/contracts/hyper_shim.contracts` is what
/// [ADR-104](../../../docs/specification/adr/adr-104.md) §5 calls *the file
/// `nikaia describe` will write, produced the way a reviewer would check it*.
/// This is that claim, tested — and it needs no network, because the crate is
/// in the tree.
///
/// **One thing in the reviewed file is not in the draft, and it is the
/// comments** — a reviewer's, and the only part of D5's review a command cannot
/// do. `crosses = false` on `LocalHandle` **is** in the draft since the
/// describer reads fields ([ADR-123](../../../docs/specification/adr/adr-123.md)
/// D2), which is the line ADR-104 §5 said the crossing refusals were waiting
/// for.
#[test]
fn the_draft_for_the_experiment_is_the_file_a_reviewer_wrote() {
    for (project, expected) in [
        (
            "serve",
            vec![
                "[fn.\"hyper_shim::across_a_thread\"]\npub = true\nsync = true\nkeeps = [\"value\"]\nsignature = \"[T](value: T) -> String\"",
                "[fn.\"hyper_shim::serve_once\"]\npub = true\nsync = true\nsignature = \"(port: i64) -> String\"",
            ],
        ),
        (
            "crossing",
            vec![
                "[fn.\"hyper_shim::across_a_thread\"]\npub = true\nsync = true\nkeeps = [\"value\"]\nsignature = \"[T](value: T) -> String\"",
                "[fn.\"hyper_shim::local_handle\"]\npub = true\nsync = true\nkeeps = [\"name\"]\nsignature = \"(name: String) -> hyper_shim::LocalHandle\"",
                "[type.\"hyper_shim::LocalHandle\"]\npub = true\ncrosses = false",
            ],
        ),
        (
            "smuggled",
            vec![
                "[fn.\"hyper_shim::across_a_thread_unchecked\"]\npub = true\nsync = true\nkeeps = [\"value\"]\nsignature = \"[T](value: T) -> String\"",
            ],
        ),
    ] {
        let root = repo_root().join("examples/foreign-runtime").join(project);
        let (ledger, described) = draft(&root, "hyper_shim").expect("the draft is written");
        assert_eq!(described.version, "0.1.0", "{project}");
        let text = ledger.render_description("hyper_shim", &described.version, &described.notes);
        for entry in expected {
            assert!(text.contains(entry), "{project}:\n{text}");
        }
        // The hash the reviewed record holds is the hash of the crate's source,
        // which is what makes the description believable while it holds (D5) -
        // in the record beside the description (ADR-251 D1).
        let text = ledger.render_derived();
        let reviewed = std::fs::read_to_string(root.join("contracts/hyper_shim.derived"))
            .expect("the reviewed record");
        let hash = reviewed
            .lines()
            .find(|line| line.starts_with("\"src/lib.rs\""))
            .expect("the reviewed file records the source's hash");
        assert!(
            text.contains(hash),
            "{project}: the draft records {hash}\n{text}"
        );
    }
}

/// **The whole chain, from a crate to a refusal.**
///
/// `nikaia describe` reads a `pub struct`'s `Rc` field and writes
/// `crosses = false` ([ADR-123](../../../docs/specification/adr/adr-123.md)
/// D2); the build merges the description into the ledger the analyses read
/// ([ADR-104](../../../docs/specification/adr/adr-104.md) D1); and `NK2501`
/// refuses a `spawn` that takes the value with it — in **this compiler's**
/// words, on the `.nika` line, with a way out.
///
/// **That chain is what three entries were waiting on**, each from its own end:
/// ADR-104 §5's *the day the describer reads fields*, ADR-123's *`NK2501` and
/// `NK2502` can fire, for the first time, on a described foreign type*, and
/// `open-work.md`'s task refusals. Asserted from a **program** rather than from
/// a hand-written ledger, because a hand-written one proves the last link and
/// none of the others.
#[test]
fn a_crate_a_description_and_a_refusal() {
    let root = project(
        "chain",
        "pub struct Held { inner: std::rc::Rc<String> }\n\
         pub fn hold(name: String) -> Held {\n\
         \x20   Held { inner: std::rc::Rc::new(name) }\n\
         }\n",
        "fn main() {\n\
         \x20   let h = fremd::hold(\"here\".clone())\n\
         \x20   spawn fn { println(f\"{h}\") }\n\
         }\n",
    );
    let (ledger, described) = draft(&root, "fremd").expect("the draft is written");
    assert!(
        ledger
            .render_description("fremd", &described.version, &described.notes)
            .contains("[type.\"fremd::Held\"]\npub = true\ncrosses = false"),
        "the `Rc` field is what says so"
    );

    // The description written where a build reads it, and then the build's own
    // answer about the program.
    std::fs::create_dir_all(root.join("contracts")).expect("a place for it");
    std::fs::write(
        root.join("contracts/fremd.contracts"),
        ledger.render_description("fremd", &described.version, &described.notes),
    )
    .expect("write the description");

    let source = std::fs::read_to_string(root.join("src/main.nika")).expect("the program");
    let parsed = nikaia::parser::parse_to_ast(&source).expect("the source parses");
    let own = nikaia::contracts::Ledger::infer(&parsed);
    let foreign = nikaia::project::Foreign::of(&root);
    let library = foreign.library().expect("std's ledger and the description");
    let modules = foreign.packages(&std::collections::BTreeSet::new());
    let found = nikaia::check::check_program(&parsed, &own, &library, &modules).findings;
    let _ = std::fs::remove_dir_all(&root);

    let refusal = found
        .iter()
        .find(|f| f.code == "NK2501")
        .unwrap_or_else(|| panic!("{found:#?}"));
    assert!(
        refusal
            .message
            .contains("uses `h`, which can't be passed to a task"),
        "{refusal:#?}"
    );
    assert!(
        refusal.notes.join(" ").contains("`fremd::Held`"),
        "the refusal names the type the description named: {:#?}",
        refusal.notes
    );
    // Part III C.2: a way out, and C.1: nothing of `rustc`'s in it.
    assert!(refusal.help.is_some(), "{refusal:#?}");
    for word in ["Send", "Rc", "E0277", "rustc"] {
        assert!(
            !refusal.notes.join(" ").contains(word),
            "`{word}`: {refusal:#?}"
        );
    }
}

/// **Nothing is written by a draft**, which is what makes the test above able
/// to read the repository's own projects without putting them back.
#[test]
fn a_draft_writes_nothing() {
    let root = repo_root().join("examples/foreign-runtime/serve");
    let before = std::fs::read_to_string(root.join("contracts/hyper_shim.contracts"))
        .expect("the reviewed file");
    let _ = draft(&root, "hyper_shim").expect("the draft is written");
    let after = std::fs::read_to_string(root.join("contracts/hyper_shim.contracts"))
        .expect("the reviewed file");
    assert_eq!(before, after);
}
