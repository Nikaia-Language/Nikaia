//! `Bytes`, and the tether refused where it would be needed
//! ([ADR-156](../../../docs/specification/adr/adr-156.md)).
//!
//! Two halves of one answer. `Bytes` is **the language's** (D1): a name written
//! bare, like `Vec` and `String`, lowered to one shared buffer (D2), and what
//! `fs::read` hands back (D3) — which is what Part III 17.2 has said all along.
//!
//! And the mechanism it hangs on is **not built**: a view that outlives the
//! buffer it points into is Part I 6.6's `Tethered`, which is
//! [ADR-008](../../../docs/specification/adr/adr-008.md)'s unbuilt state. D4 is
//! what happens meanwhile — `NK2303`, on the Nikaia line, naming the buffer.
//! The alternative was lowering the function and letting `rustc` explain a file
//! the author never wrote ([Part III C.1](../../../docs/specification/30-nikaia-tooling.md)).

use nikaia::check::Finding;
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library).findings
}

fn refusals(source: &str) -> Vec<Finding> {
    findings(source)
        .into_iter()
        .filter(|f| f.code == "NK2303")
        .collect()
}

/// Whether a function's result takes a keep from its caller: its views
/// outlive the buffer the body read, so the buffer lives in the caller's keep
/// ([ADR-209](../../../docs/specification/adr/adr-209.md) D2).
fn tethers_its_result(source: &str, key: &str) -> bool {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    own.functions[key].views.iter().any(|h| {
        h.position == nikaia::contracts::tether::RESULT
            && h.state == nikaia::contracts::tether::State::Tethered
    })
}

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

// ---------------------------------------------------------------------------
// D1–D3: the type
// ---------------------------------------------------------------------------

/// **`Bytes` is a name, written bare** (D1). Part I 1.3 has listed it since
/// [ADR-154](../../../docs/specification/adr/adr-154.md) D1 and nothing
/// declared it; a type on that list and nowhere else is the one direction a
/// prelude can be wrong in without anybody noticing.
#[test]
fn bytes_is_a_type_this_compiler_knows() {
    let found: Vec<_> = findings("fn hold(data: Bytes) { }\nfn main() { }\n")
        .into_iter()
        .filter(|f| f.code == "NK1135")
        .collect();
    assert!(found.is_empty(), "{found:#?}");
}

/// **And it needs no `use`**, which is what being on the list means: a module
/// prefix would be the opposite of it.
#[test]
fn bytes_needs_no_use() {
    assert!(
        findings("fn hold(data: Bytes) -> i64 { return data.len() as i64 }\nfn main() { }\n")
            .is_empty()
    );
}

/// **It lowers to the shared buffer** (D2) — the name the prelude publishes,
/// not a `Vec<u8>` by another spelling.
///
/// It is written exactly as the source writes it, in both positions: the
/// prelude publishes the name, so nothing has to be spelled out below.
#[test]
fn bytes_lowers_to_the_shared_buffer() {
    let rust = lowered("fn hold(data: Bytes) -> Bytes { return data }\nfn main() { }\n");
    assert!(rust.contains("fn hold(data: Bytes) -> Bytes"), "{rust}");
}

/// **`fs::read` hands one back** (D3), which Part III 17.2 already said and the
/// ledger did not: it keyed `Vec[u8]`.
#[test]
fn fs_read_hands_back_bytes() {
    let library = Ledger::parse(STD).expect("std's ledger");
    let (_, read) = library.lookup("fs::read").expect("`fs::read` is described");
    let result = read
        .signature
        .as_ref()
        .and_then(|s| s.result.as_ref())
        .expect("it has a result");
    assert_eq!(result.to_string(), "Bytes");
}

/// **And a program may write down what it holds**: the result of `fs::read` is
/// a type the source can name, which is the half a name behind a `use` would
/// cost.
#[test]
fn a_program_may_declare_what_fs_read_hands_back() {
    let source = "use std::fs\n\
                  fn load(path: ref String) -> Bytes throws {\n\
                  \x20   return fs::read(ref path, fs::Root::Anywhere)\n\
                  }\n\
                  fn main() { }\n";
    assert!(findings(source).is_empty(), "{:#?}", findings(source));
}

// ---------------------------------------------------------------------------
// D4: what used to be a refusal is a tether
// ---------------------------------------------------------------------------

/// **A view of a buffer the body made is tethered** — it was refused while the
/// state did not exist ([ADR-156](../../../docs/specification/adr/adr-156.md)
/// D4), and it is what [ADR-209](../../../docs/specification/adr/adr-209.md)
/// builds: `data` goes into the caller's keep, and the result points into it.
#[test]
fn a_view_of_a_local_buffer_is_tethered() {
    let source = "use std::fs\n\
                  fn header(path: ref String) -> ref String throws {\n\
                  \x20   let data = fs::read_to_string(ref path, fs::Root::Anywhere)\n\
                  \x20   return data.trim()\n\
                  }\n\
                  fn main() { }\n";
    assert!(refusals(source).is_empty(), "{:#?}", refusals(source));
    assert!(tethers_its_result(source, "header"));
    let rust = lowered(source);
    assert!(rust.contains("__keep.put_viewed("), "{rust}");
}

/// **A `Bytes` is the same answer**, and it is the buffer the mechanism is
/// named after.
#[test]
fn a_view_of_a_local_bytes_is_tethered() {
    let source = "use std::fs\n\
                  fn first(path: ref String) -> ref String throws {\n\
                  \x20   let data = fs::read(ref path, fs::Root::Anywhere)\n\
                  \x20   return data.text()\n\
                  }\n\
                  fn main() { }\n";
    assert!(refusals(source).is_empty(), "{:#?}", refusals(source));
    assert!(tethers_its_result(source, "first"));
}

/// **The tail expression counts too** — a body need not write `return` for the
/// value to leave it.
#[test]
fn a_tail_expression_is_handed_back_too() {
    let source = "use std::fs\n\
                  fn header(path: ref String) -> ref String throws {\n\
                  \x20   let data = fs::read_to_string(ref path, fs::Root::Anywhere)\n\
                  \x20   data.trim()\n\
                  }\n\
                  fn main() { }\n";
    assert!(refusals(source).is_empty());
    assert!(tethers_its_result(source, "header"));
}

/// **A method's result is no different from a function's**, so an `impl` is
/// walked the same way.
#[test]
fn a_method_is_tethered_the_same_way() {
    let source = "use std::fs\n\
                  struct Loader { }\n\
                  impl Loader {\n\
                  \x20   fn header(ref self, path: ref String) -> ref String throws {\n\
                  \x20       let data = fs::read_to_string(ref path, fs::Root::Anywhere)\n\
                  \x20       return data.trim()\n\
                  \x20   }\n\
                  }\n\
                  fn main() { }\n";
    assert!(refusals(source).is_empty(), "{:#?}", refusals(source));
    assert!(tethers_its_result(source, "Loader::header"));
}

// ---------------------------------------------------------------------------
// D4's other half: what it must never refuse
// ---------------------------------------------------------------------------

/// **A view of the caller's buffer is not a tether.** The buffer is the
/// caller's and outlives the call, which is `Borrowed` and costs nothing.
#[test]
fn a_view_of_a_parameter_is_left_alone() {
    let source = "fn trimmed(input: ref String) -> ref String {\n\
                  \x20   return input.trim()\n\
                  }\n\
                  fn main() { }\n";
    assert!(refusals(source).is_empty(), "{:#?}", refusals(source));
}

/// **A buffer the body owns but does not hand back is not a tether either.**
/// The body may make a `String` and still return something else, and a refusal
/// that fired on *owning* one would refuse a correct program.
#[test]
fn a_buffer_that_is_not_handed_back_is_left_alone() {
    let source = "fn name(input: ref String) -> ref String {\n\
                  \x20   let copy = input.clone()\n\
                  \x20   return input\n\
                  }\n\
                  fn main() { }\n";
    assert!(refusals(source).is_empty(), "{:#?}", refusals(source));
}

/// **A view of text that outlives the program is not a tether**, which is
/// [ADR-008](../../../docs/specification/adr/adr-008.md) D9's own example.
#[test]
fn a_literal_is_left_alone() {
    let source = "fn name() -> ref String {\n\
                  \x20   let unused = \"Grace\".clone()\n\
                  \x20   return \"Ada\"\n\
                  }\n\
                  fn main() { }\n";
    assert!(refusals(source).is_empty(), "{:#?}", refusals(source));
}

/// **And a call no ledger describes never raises it.** The *column* errs
/// towards Tethered for one, because a wide state costs a wide representation;
/// a refusal may not err that way, because refusing a correct program is the
/// worse of the two mistakes
/// ([Part III C.4](../../../docs/specification/30-nikaia-tooling.md)).
#[test]
fn a_call_nothing_describes_never_raises_it() {
    let source = "fn hold(input: ref String) -> ref String {\n\
                  \x20   let made = whatever(input)\n\
                  \x20   return made\n\
                  }\n\
                  fn main() { }\n";
    assert!(refusals(source).is_empty(), "{:#?}", refusals(source));
}

/// **A result that is not a view is not this rule's**, whatever the body owns.
#[test]
fn an_owned_result_is_left_alone() {
    let source = "use std::fs\n\
                  fn header(path: ref String) -> String throws {\n\
                  \x20   let data = fs::read_to_string(ref path, fs::Root::Anywhere)\n\
                  \x20   return data\n\
                  }\n\
                  fn main() { }\n";
    assert!(refusals(source).is_empty(), "{:#?}", refusals(source));
}

/// **And nothing in the corpus reaches it**, which is the claim that makes this
/// change safe to make at all: the whole tree is the free case.
#[test]
fn the_corpus_needs_no_tether() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let library = Ledger::parse(STD).expect("std's ledger");
    let mut refused = Vec::new();
    let mut walk = vec![root];
    while let Some(dir) = walk.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if path.is_dir() {
                if !matches!(name.as_str(), "target" | ".git" | "vendor" | "node_modules") {
                    walk.push(path);
                }
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("nika") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(parsed) = parse_to_ast(&text) else {
                continue;
            };
            let own = Ledger::infer(&parsed);
            for finding in nikaia::check::check(&parsed, &own, &library).findings {
                if finding.code == "NK2303" {
                    refused.push(format!("{}: {}", path.display(), finding.message));
                }
            }
        }
    }
    assert!(refused.is_empty(), "tethered:\n{}", refused.join("\n"));
}
