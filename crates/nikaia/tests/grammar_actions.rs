//! A grammar's action may not pause
//! ([ADR-142](../../../docs/specification/adr/adr-142.md)).
//!
//! A rule's action is arbitrary Nikaia, so it may call something that pauses —
//! and nothing refused it. The emitter wrote `.await` inside the synchronous
//! parser the `grammar!` macro generates, and the backend answered *`await` is
//! only allowed inside `async` functions and blocks*: its words about a
//! construct this compiler let through, which is
//! [Part III C.1](../../../docs/specification/30-nikaia-tooling.md)'s class for
//! a shape the language allows.
//!
//! **Found from the other end**, by building
//! [ADR-140](../../../docs/specification/adr/adr-140.md) D3: entering a grammar
//! through a path made the entry a call by name, so the analyses read its
//! contract instead of answering nothing about an unresolvable receiver — and
//! the contract carried no `sync`, so every function that parses became
//! `async`. Both answers were answers to a question nobody had asked.

use nikaia::contracts::{Ledger, STD, Sync};
use nikaia::parser::parse_to_ast;

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's ledger");
    nikaia::check::check(&parsed, &own, &library).findings
}

fn ledger_for(source: &str) -> Ledger {
    Ledger::infer(&parse_to_ast(source).expect("the source parses"))
}

/// **The entry's own reproduction** (D1).
#[test]
fn a_pausing_call_in_an_action_is_refused() {
    let found: Vec<_> = findings(
        "use std::io\n\ngrammar Nums {\n\
         \x20   pub rule number -> i64 = d:dec[i64](digit+) { let t = io::read_to_string() return d }\n\
         }\n\
         fn read(text: ref String) -> i64 { return Nums::number(text) catch { 0 } }\n",
    )
    .into_iter()
    .filter(|f| f.code == "NK2209")
    .collect();
    assert_eq!(found.len(), 1, "one refusal: {found:#?}");
    // **The message names the rule**, because a grammar is a page of rules and
    // a caret on a call inside one of them is not enough to find it.
    assert!(found[0].message.contains("`number`'s action"), "{found:#?}");
    assert!(
        found[0].message.contains("io::read_to_string"),
        "and what it calls: {found:#?}"
    );
}

/// **A fold's `step` is action code too**
/// ([ADR-092](../../../docs/specification/adr/adr-092.md)), so the flag is set
/// around the pattern's lambdas and not around the block alone.
#[test]
fn a_pausing_call_in_a_folds_step_is_refused() {
    let found: Vec<_> = findings(
        "use std::io\n\ngrammar Nums {\n\
         \x20   rule N -> i64 = d:dec[i64](digit+) { d }\n\
         \x20   pub rule file -> i64 = fold(N, zero, fn(acc, m) { io::read_to_string() acc })\n\
         }\n\
         fn zero() -> i64 { return 0 }\n",
    )
    .into_iter()
    .filter(|f| f.code == "NK2209")
    .collect();
    assert_eq!(found.len(), 1, "one refusal: {found:#?}");
    assert!(found[0].message.contains("`file`'s action"), "{found:#?}");
}

/// **A pause reached through a method on the accumulator** (0.0.230, `open-work.md`
/// §1.17). The step's parameters had no type, so `acc.record(m)` resolved to
/// nothing and a pausing `record` compiled into a future the fold dropped: the
/// parse counted nothing. The accumulator is typed from `init` - here a type's
/// name, which is its constructor - and the item from the rule it folds.
#[test]
fn a_pause_through_the_accumulators_method_is_refused() {
    let source = |merge: &str, record: &str| {
        format!(
            "use std::fs

             grammar G {{
                 rule N -> i64 = d:dec[i64](digit+) {{ d }}
                 pub rule file -> Tally = par_fold(N, Tally, fn(acc, m) {{ acc.record(m) }}, {merge})
             }}
             pub struct Tally {{ sum: i64 }}
             impl Tally {{
                 pub fn() -> Tally sync {{ return Tally {{ sum: 0 }} }}
                 fn record(ref mut self, m: i64) {{ {record} self.sum += m }}
                 fn merge(ref mut self, other: Tally) {{ self.sum += other.sum }}
                 fn wait(ref mut self, other: Tally) {{ let t = fs::read_to_string(\"t\", fs::Root::Anywhere) catch {{ \"\" }} self.sum += other.sum }}
             }}
"
        )
    };
    let refusals = |source: String| -> Vec<nikaia::check::Finding> {
        findings(&source)
            .into_iter()
            .filter(|f| f.code == "NK2209" || f.code == "NK2206")
            .collect()
    };

    let paused = refusals(source(
        "Tally::merge",
        "let t = fs::read_to_string(\"t\", fs::Root::Anywhere) catch { \"\" }",
    ));
    assert_eq!(
        paused.len(),
        1,
        "one refusal, and it is the action's: {paused:#?}"
    );
    assert_eq!(paused[0].code, "NK2209", "{paused:#?}");
    assert!(paused[0].message.contains("`record`"), "{paused:#?}");

    // **A named `merge` is a call the fold makes**, asked the same question.
    let merged = refusals(source("Tally::wait", ""));
    assert_eq!(merged.len(), 1, "{merged:#?}");
    assert!(merged[0].message.contains("Tally::wait"), "{merged:#?}");

    // And the same fold with nothing that pauses is left alone.
    assert!(refusals(source("Tally::merge", "")).is_empty());
}

/// **An action that does not pause is untouched**, and so is a body *around* a
/// parse that does: what is refused is pausing inside the parse, not near it
/// (D3).
#[test]
fn an_ordinary_action_and_a_pausing_caller_are_left_alone() {
    let found: Vec<_> = findings(
        "use std::io\n\ngrammar Nums {\n\
         \x20   pub rule number -> i64 = d:dec[i64](digit+) { d + 1 }\n\
         }\n\
         fn read() -> i64 throws {\n\
         \x20   let text = io::read_to_string()\n\
         \x20   return Nums::number(text) catch { 0 }\n\
         }\n",
    );
    assert!(found.iter().all(|f| f.code != "NK2209"), "{found:#?}");
}

/// **The entry's contract says `sync`** (D2) — asserted, because D1 is a rule of
/// the language rather than a property of this grammar.
#[test]
fn an_entry_is_sync_in_the_ledger() {
    let ledger = ledger_for(
        "grammar Nums {\n\
         \x20   pub rule number -> i64 = d:dec[i64](digit+) { d }\n\
         }\n",
    );
    assert_eq!(ledger.functions["Nums::number"].sync, Sync::Asserted);
}

/// **And a caller keeps its own claim**, which is the half that needed more than
/// the column: the fixpoint reads this unit's call graph, a grammar entry has no
/// node in it, and an absent node is read as *pauses*. The entry is inserted as
/// a leaf that holds, exactly as a trait method's declaration is
/// ([ADR-109](../../../docs/specification/adr/adr-109.md) D1).
///
/// Without it every function that parses is `async`, which is what
/// [ADR-140](../../../docs/specification/adr/adr-140.md) D3 had cost the corpus.
#[test]
fn a_function_that_parses_stays_sync() {
    let ledger = ledger_for(
        "grammar Nums {\n\
         \x20   pub rule number -> i64 = d:dec[i64](digit+) { d }\n\
         }\n\
         fn read(text: ref String) -> i64 { return Nums::number(text) catch { 0 } }\n",
    );
    assert!(
        ledger.functions["read"].sync.is_sync(),
        "{:?}",
        ledger.functions["read"].sync
    );
}
