//! **The touch sets, and the verdict they reach** — what
//! [ADR-292](../../../docs/specification/adr/adr-292.md) built and
//! [ADR-292](../../../docs/specification/adr/adr-292.md) kept.
//!
//! D1 withdrew the automatic reordering: statements run in the order they are
//! written, and no analysis stands between the source and the schedule. **The
//! analysis survives its original purpose**, because D3 gives it a better one —
//! it no longer decides whether the compiler *may* overlap two statements, it
//! decides whether the programmer was *right* to say so in an `overlap { … }`.
//! Checking a claim is a stronger use of it than making one.
//!
//! So these tests ask the verdict directly rather than through a lowering: what
//! two statements reach, whether they meet, and why not. The construct that
//! consumes the answer is in `overlap.rs`; this file is the vocabulary
//! underneath it, and every entry in it earned its place by being a pair some
//! program in `examples/` wrote.

use nikaia::contracts::order::{self, Accounted};
use nikaia::contracts::touch::TouchOps;
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::parser::parse_to_ast;

/// The control this whole file needs: two reads of two files, which meet on
/// nothing. Without a pair that *does* overlap, every assertion below would
/// also hold against an analysis that refused everything.
const TWO_READS: &str = "use std::fs\n\
     fn main() throws {\n\
         let a = fs::read_to_string(\"eins.txt\", fs::Root::Anywhere) catch { \"\" }\n\
         let b = fs::read_to_string(\"zwei.txt\", fs::Root::Anywhere) catch { \"\" }\n\
         println(f\"{a.len()} {b.len()}\")\n\
     }";

/// **Whether the first two statements of `main` meet on nothing.**
///
/// The verdict, asked of the pair directly. It used to be asked by lowering the
/// program and looking for a join in the emitted Rust — which was the right
/// question while the compiler was the one deciding, and is the wrong one now
/// that nothing overlaps unless a program asks.
///
/// `false` where either statement is one this compiler cannot account for,
/// which is the fail-closed answer D4 asks for and is what several of the tests
/// below are about.
fn overlaps(source: &str) -> bool {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");

    let body = parsed
        .program
        .items
        .iter()
        .find_map(|item| match &item.node {
            nikaia::ast::Item::Fn { name, body, .. }
                if name.map(|n| parsed.text(n)) == Some("main") =>
            {
                Some(body)
            }
            _ => None,
        })
        .expect("a `main` to read");

    let pair: Vec<_> = body
        .stmts
        .iter()
        .take(2)
        .map(|stmt| order::operation(&parsed, &stmt.node, &own, &library))
        .collect();
    match pair.as_slice() {
        [Some(earlier), Some(later)] => order::verdict(earlier, later).is_overlap(),
        _ => false,
    }
}

/// The verdict on a pair, rendered — for the tests that are about the **reason**
/// rather than about the answer.
///
/// `library` lets a test hand in a probe ledger of its own; `None` is `std`'s.
fn why_with(source: &str, library: Option<Ledger>) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = library.unwrap_or_else(|| Ledger::parse(STD).expect("std's ledger"));
    let body = parsed
        .program
        .items
        .iter()
        .find_map(|item| match &item.node {
            nikaia::ast::Item::Fn { body, .. } => Some(body),
            _ => None,
        })
        .expect("a function to read");

    let mut out = Vec::new();
    for two in body.stmts.windows(2) {
        let earlier = order::accounted(&parsed, &two[0].node, &own, &library);
        let later = order::accounted(&parsed, &two[1].node, &own, &library);
        out.push(match (&earlier, &later) {
            (Accounted::Operation(a), Accounted::Operation(b)) => {
                let verdict = order::verdict(a, b);
                let mark = if verdict.is_overlap() {
                    "together"
                } else {
                    "in order"
                };
                format!("{mark}  {} / {} - {}", a.callee, b.callee, verdict.why())
            }
            (refused, other) | (other, refused) if !matches!(refused, Accounted::Operation(_)) => {
                let named = match other {
                    Accounted::Operation(operation) => operation.callee.clone(),
                    _ => "…".to_string(),
                };
                format!("in order  {named} - {}", refused.why())
            }
            _ => continue,
        });
    }
    out.join("\n")
}

/// The same, against `std`'s ledger.
fn why(source: &str) -> String {
    why_with(source, None)
}

/// A data dependency: the second uses what the first bound.
#[test]
fn a_data_dependency_keeps_the_order() {
    assert!(!overlaps(
        "use std::fs\n\
         fn main() throws {\n\
             let a = fs::read_to_string(\"eins.txt\", fs::Root::Anywhere) catch { \"\" }\n\
             let b = fs::read_to_string(a, fs::Root::Anywhere) catch { \"\" }\n\
             println(f\"{b.len()}\")\n\
         }"
    ));
}

/// The same file, written by one of them.
#[test]
fn a_write_to_the_same_file_keeps_the_order() {
    assert!(!overlaps(
        "use std::fs\n\
         fn main() throws {\n\
             let a = fs::write(\"log.txt\", fs::Root::Anywhere, \"x\") catch { }\n\
             let b = fs::read_to_string(\"log.txt\", fs::Root::Anywhere) catch { \"\" }\n\
             println(f\"{b.len()}\")\n\
         }"
    ));
}

/// A file this compiler cannot name is every file of its kind (ADR-292 D3).
///
/// `fs::read_to_string(pfad, fs::Root::Anywhere)` where `pfad` is computed could be the file the
/// other one writes. The alternative to keeping the order is a program that is
/// right on some inputs and wrong on others.
#[test]
fn a_file_that_cannot_be_named_keeps_the_order() {
    assert!(!overlaps(
        "use std::fs\n\
         fn main(pfad: ref String) throws {\n\
             let a = fs::write(\"log.txt\", fs::Root::Anywhere, \"x\") catch { }\n\
             let b = fs::read_to_string(pfad, fs::Root::Anywhere) catch { \"\" }\n\
             println(f\"{b.len()}\")\n\
         }"
    ));
}

/// A function nobody described touches everything.
///
/// `etwas_unbekanntes` has no entry in any ledger, so it orders against
/// everything (D4) - and the refusal is about a contract somebody could write
/// rather than about this compiler's own limits.
#[test]
fn a_function_with_no_contract_keeps_the_order() {
    assert!(!overlaps(
        "use std::fs\n\
         fn main() throws {\n\
             let a = fs::read_to_string(\"eins.txt\", fs::Root::Anywhere) catch { \"\" }\n\
             let b = etwas_unbekanntes() catch { \"\" }\n\
             println(f\"{a.len()}\")\n\
         }"
    ));
}

/// A handler that can leave the function makes the next statement conditional.
///
/// This is what building the increment found, and it was not in ADR-292 D1 when
/// it was written (ADR-292). If the first read fails and its handler `return`s,
/// the sequential program never performs the second read at all - so performing
/// it early is speculation, which D5 forbids.
#[test]
fn a_diverting_handler_keeps_the_order() {
    assert!(!overlaps(
        "use std::fs\n\
         fn main() throws {\n\
             let a = fs::read_to_string(\"eins.txt\", fs::Root::Anywhere) catch { return }\n\
             let b = fs::read_to_string(\"zwei.txt\", fs::Root::Anywhere) catch { \"\" }\n\
             println(f\"{a.len()} {b.len()}\")\n\
         }"
    ));
}

/// A data dependency inside a `catch` handler keeps the order too.
///
/// `f("a") catch { … }` is one expression, so a name its handler mentions is a
/// name the statement mentions. Looking only past the `catch` missed it, and
/// the lowering puts the handler in the same closure - so the second statement
/// would have read what the first had not finished binding.
#[test]
fn a_dependency_in_a_handler_keeps_the_order() {
    assert!(!overlaps(
        "use std::fs\n\
         fn main() throws {\n\
             let a = fs::read_to_string(\"eins.txt\", fs::Root::Anywhere) catch { \"\" }\n\
             let b = fs::read_to_string(\"zwei.txt\", fs::Root::Anywhere) catch { a }\n\
             println(f\"{a.len()} {b.len()}\")\n\
         }"
    ));

    // … including where the name is inside the handler's interpolated text,
    // whose holes this analysis has not parsed. Every word of the raw text
    // counts, which is over-approximate on purpose.
    assert!(!overlaps(
        "use std::fs\n\
         fn main() throws {\n\
             let a = fs::read_to_string(\"eins.txt\", fs::Root::Anywhere) catch { \"\" }\n\
             let b = fs::read_to_string(\"zwei.txt\", fs::Root::Anywhere) catch { f\"{a}\" }\n\
             println(f\"{a.len()} {b.len()}\")\n\
         }"
    ));
}

/// An argument that is not a literal is not sent to another thread.
///
/// The first increment's own restriction rather than the rule's: a closure that
/// captures nothing cannot capture something that must not cross a thread, and
/// what may cross one deserves its own decision.
#[test]
fn a_non_literal_argument_keeps_the_order() {
    assert!(!overlaps(
        "use std::fs\n\
         fn main(eins: ref String, zwei: ref String) throws {\n\
             let a = fs::read_to_string(eins, fs::Root::Anywhere) catch { \"\" }\n\
             let b = fs::read_to_string(zwei, fs::Root::Anywhere) catch { \"\" }\n\
             println(f\"{a.len()} {b.len()}\")\n\
         }"
    ));
}

/// Two `println`s keep their order, which is D6's own test of the model.
///
/// ADR-292 D5 asks for the obvious without an exception for it, and this is
/// where a wider analysis could have lost it: a bare `println(x)` is now a
/// shape the analysis reads, so the answer has to come from somewhere. It comes
/// from the place D6 says it should - `println` is entered as reaching `stdout`
/// and writing it, so two of them meet, and there is no special case for
/// printing anywhere in this compiler.
#[test]
fn two_printlns_keep_their_order() {
    assert!(!overlaps(
        "fn main() {\n\
             println(\"a\")\n\
             println(\"b\")\n\
         }"
    ));
}

/// A failure nobody catches keeps the order (ADR-292 D1).
///
/// The same rule as a diverting handler, reached from the other side: if the
/// first write fails, the failure leaves the function and the sequential
/// program never performs the second. Starting it early would perform work the
/// program as written might never have performed - which is exactly what D5
/// forbids, and what a bare expression statement makes easy to write.
#[test]
fn an_uncaught_failure_keeps_the_order() {
    assert!(!overlaps(
        "use std::fs\n\
         fn main() throws {\n\
             fs::write(\"eins.txt\", fs::Root::Anywhere, \"a\")\n\
             fs::write(\"zwei.txt\", fs::Root::Anywhere, \"b\")\n\
             println(\"x\")\n\
         }"
    ));

    // … and the same statement with the failure caught does overlap, so the
    // test cannot pass because the shape was not read at all.
    assert!(overlaps(
        "use std::fs\n\
         fn main() throws {\n\
             fs::write(\"eins.txt\", fs::Root::Anywhere, \"a\") catch { }\n\
             fs::write(\"zwei.txt\", fs::Root::Anywhere, \"b\") catch { }\n\
             println(\"x\")\n\
         }"
    ));
}

/// An assignment is never an operation, however plain it looks.
///
/// Not an implementation gap but a rule: `x = 1` changes a name without binding
/// one, so the data-dependency test - does the later statement mention what the
/// earlier one bound - would find nothing to compare. A shape whose
/// dependencies the analysis cannot see is a shape it may not read.
#[test]
fn an_assignment_is_not_an_operation() {
    assert!(!overlaps(
        "use std::fs\n\
         fn main() throws {\n\
             let mut a = \"\"\n\
             a = fs::read_to_string(\"eins.txt\", fs::Root::Anywhere) catch { \"\" }\n\
             let b = fs::read_to_string(\"zwei.txt\", fs::Root::Anywhere) catch { \"\" }\n\
             println(f\"{a.len()} {b.len()}\")\n\
         }"
    ));
}

/// A method call keeps the order: which ledger entry it is depends on the type
/// of its receiver, and that is the type checker's answer (ADR-288).
#[test]
fn a_method_call_keeps_the_order() {
    assert!(!overlaps(
        "use std::fs\n\
         fn main() throws {\n\
             let mut out = Vec()\n\
             out.push(\"eins\")\n\
             let b = fs::read_to_string(\"zwei.txt\", fs::Root::Anywhere) catch { \"\" }\n\
             println(f\"{b.len()}\")\n\
         }"
    ));
}

/// Two reads of different files meet on nothing.
#[test]
fn two_reads_of_different_files_overlap() {
    assert!(overlaps(TWO_READS));
}

/// The arguments the program was started with are a resource like any other.
///
/// `cli::args` was the most-refused callee in `examples/` - ten adjacent pairs
/// named it - and it was refused because nobody had written down what it
/// reaches. Two reads of it meet on nothing, and a read of it meets nothing a
/// `println` writes either.
#[test]
fn the_programs_arguments_are_a_resource() {
    assert!(overlaps(
        "use std::cli\n\
         fn main() {\n\
             let a = cli::args()\n\
             let b = cli::args()\n\
             println(f\"{a.len()} {b.len()}\")\n\
         }"
    ));

    // … and `args` is a kind of its own, so reading it does not meet what
    // `println` writes.
    let reason = why("use std::cli\n\
         fn main() {\n\
             let a = cli::args()\n\
             println(\"x\")\n\
             println(f\"{a.len()}\")\n\
         }");
    assert!(reason.contains("together"), "{reason}");
}

/// The two console handles keep their order, because `2>&1` makes them one.
///
/// What widening the groups turned up, and it is the same shape as the `catch`
/// handler whose effects nobody counted (§8.3): an effect that *was* in the
/// touch set, against a resource whose identity nobody had checked. `stdout`
/// and `stderr` are two handles and one destination as soon as anybody
/// redirects one onto the other, so three console writes would have been a
/// group of three whose output interleaves differently on every run.
#[test]
fn the_two_console_handles_keep_their_order() {
    assert!(!overlaps(
        "fn main() {\n\
             println(\"out\")\n\
             eprintln(\"err\")\n\
             println(\"out2\")\n\
         }"
    ));

    // … and the refusal is about the resource rather than about ignorance:
    // both are described, and what they meet on is what is printed.
    let reason = why("fn main() {\n\
             println(\"out\")\n\
             eprintln(\"err\")\n\
             println(\"out2\")\n\
         }");
    assert!(
        reason.contains("which might be the same place") && reason.contains("stderr"),
        "{reason}"
    );
    // … and the way out is named, because a refusal a reader can act on is
    // worth several they cannot (D9).
    assert!(reason.contains("`seq`"), "{reason}");
}

/// A resource named in a word this compiler does not know reaches everything.
///
/// The fail-open shape this closes is the worst one an analysis like this can
/// have. Two touches of *different* kinds never conflict, so a kind nobody
/// knows is disjoint from every kind there is - which would make a typo in a
/// hand-maintained ledger *buy* an overlap. D4's polarity says what to do
/// instead, and it is the same answer it gives everywhere else.
#[test]
fn a_resource_this_compiler_cannot_name_reaches_everything() {
    const LEDGER: &str = "version = 2\n\
         toolchain = \"nikaia 0.1.0\"\n\
         inference = \"stage0-signatures\"\n\
         \n\
         [fn.\"net::post\"]\n\
         pub = true\n\
         sync = true\n\
         touches = [\"endpoint(url) write\"]\n\
         signature = \"(url: ?) -> ?\"\n";

    let library = nikaia::contracts::Ledger::parse(LEDGER).expect("the ledger parses");
    let source = "fn main() {\n\
             net::post(\"https://eins\")\n\
             net::post(\"https://zwei\")\n\
             println(\"x\")\n\
         }";
    let reason = why_with(source, Some(library.clone()));

    assert!(
        reason.contains("which this version of Nikaia doesn't know"),
        "{reason}"
    );
    assert!(reason.contains("endpoint"), "{reason}");
    assert!(!reason.contains("together"), "{reason}");
}

/// `std`'s hand-maintained ledger names only resources this compiler knows.
///
/// The check the `kind_is_known` rule cannot make on its own: an unknown kind
/// costs a program its overlap silently and correctly, which means a typo in
/// `std.contracts` would be a pessimisation nobody notices. The file is
/// reviewed like code (ADR-020 D5), and this is the part of that review a
/// reviewer cannot do by eye.
#[test]
fn the_std_ledger_names_only_known_resources() {
    let library = nikaia::contracts::Ledger::parse(nikaia::contracts::STD).expect("std's ledger");
    for (name, contract) in &library.functions {
        for touch in &contract.touches {
            assert!(
                touch.kind_is_known(),
                "`{name}` reaches `{}`, which is not in the vocabulary",
                touch.kind
            );
        }
    }
}

/// A `catch` handler's own effects are part of what the statement touches.
///
/// The analysis looks *past* a `catch` at the call it guards, so without this
/// the handler below would be invisible: the pair would meet on nothing and
/// overlap, and the overlapped program could print its two lines in either
/// order. A handler is code that runs, so what it reaches counts (D2), and
/// where it cannot be read D4 says the statement reaches everything.
#[test]
fn a_handlers_own_effects_are_part_of_the_statement() {
    const HANDLER_WRITES_STDOUT: &str = "use std::fs\n\
         fn main() {\n\
         \x20   fs::write(\"a.txt\", fs::Root::Anywhere, \"x\") catch { println(\"failed\") }\n\
         \x20   println(\"next\")\n\
         }";

    assert!(
        !overlaps(HANDLER_WRITES_STDOUT),
        "the handler writes stdout and so does the next statement:\n{}",
        why(HANDLER_WRITES_STDOUT)
    );
    let reason = why(HANDLER_WRITES_STDOUT);
    assert!(
        reason.contains("stdout") && reason.contains("println"),
        "and the refusal must name what they meet on: {reason}"
    );
}

/// A handler this analysis cannot read makes the statement reach everything.
///
/// `catch { …; … }` is more than one statement, which the walk stops at. The
/// answer then has to be the fail-closed one, or a handler could smuggle an
/// effect past the touch set - which is exactly the hole this pair is here to
/// keep shut (ADR-010 D1's polarity, applied to a third question).
#[test]
fn an_unreadable_handler_is_not_an_empty_one() {
    const HANDLER_DOES_MORE: &str = "use std::fs\n\
         fn main() throws {\n\
         \x20   let a = fs::read_to_string(\"eins.txt\", fs::Root::Anywhere) catch { \
         fs::write(\"zwei.txt\", fs::Root::Anywhere, \"x\") catch { }; \"\" }\n\
         \x20   let b = fs::read_to_string(\"zwei.txt\", fs::Root::Anywhere) catch { \"\" }\n\
         \x20   println(f\"{a.len()} {b.len()}\")\n\
         }";

    assert!(
        !overlaps(HANDLER_DOES_MORE),
        "the first handler writes the file the second statement reads:\n{}",
        why(HANDLER_DOES_MORE)
    );
}

/// A `let` whose initialiser is not a bare call is weighed, and its touch set
/// is the union of every call in it.
///
/// The other half of §8.3's first item. Two reads inside one statement reach
/// two files, and the statement next to them is compared against both.
#[test]
fn a_value_that_is_not_a_bare_call_is_weighed() {
    let reason = why("use std::fs\n\
         fn main() throws {\n\
             let beide = (fs::read_to_string(\"eins.txt\", fs::Root::Anywhere), fs::read_to_string(\"zwei.txt\", fs::Root::Anywhere)) catch { (\"\", \"\") }\n\
             let c = fs::read_to_string(\"drei.txt\", fs::Root::Anywhere) catch { \"\" }\n\
             println(\"x\")\n\
         }");
    assert!(
        reason.contains("fs::read_to_string + fs::read_to_string / fs::read_to_string"),
        "{reason}"
    );
    assert!(reason.contains("together"), "{reason}");

    // … and the union is what is compared: one of the two inner reads meets the
    // write next to it, so the pair keeps its order.
    let reason = why("use std::fs\n\
         fn main() throws {\n\
             let beide = (fs::read_to_string(\"eins.txt\", fs::Root::Anywhere), fs::read_to_string(\"zwei.txt\", fs::Root::Anywhere)) catch { (\"\", \"\") }\n\
             fs::write(\"zwei.txt\", fs::Root::Anywhere, \"x\") catch { }\n\
             println(\"x\")\n\
         }");
    assert!(reason.contains("both reach file `zwei.txt`"), "{reason}");
}

/// A lambda's own effects are **not** in the statement's touch set, where a
/// `catch` handler's are - and that difference is why `from(f)` stops at `sync`.
///
/// For `sync` and for `throws`, a trailing lambda's body is walked *as part of
/// the function that writes it* (`contracts::sync`'s `visit_expr_blocks`), which
/// is what lets `sync = "from(f)"` be read as "this call adds no pausing of its
/// own": whatever the lambda does is already counted at the call site. This
/// analysis does no such walk - `contracts::order::walk` has no `Expr::Closure`
/// arm - so the lambda's `println` never reaches a touch set at all. The
/// statement is refused instead, which is D4's answer and the right one.
///
/// The contrast is the evidence. `a_handlers_own_effects_are_part_of_the_statement`
/// above refuses its pair **naming `stdout`**, because the handler's write was
/// counted into the statement. Here nothing names `stdout`, with `std`'s ledger
/// or with one where `Vec::sort_by_key` claims `touches = []` outright: the
/// effect was never counted, only stepped around. A `touches = "from(f)"` read
/// the way D3 reads `sync`'s - "adds nothing" - would therefore buy the overlap
/// on an incomplete touch set, and the two `println`s would interleave either
/// way: D1 broken by an effect nobody counted, which is §8.3's handler hole in a
/// third disguise.
///
/// The second source is the one that exercises the refusal itself. A *trailing*
/// lambda today is refused two or three times over - the receiver is not a
/// literal, what the method hands back has no `crosses` line - and those are the
/// two refusals D9's table and ADR-005 §1 Group B call liftable. So the lambda
/// arm is the one that has to hold when they are lifted, and a bare lambda
/// statement is where it can be seen holding on its own.
#[test]
fn a_lambdas_own_effects_are_not_in_the_statements_touch_set() {
    let trailing = "fn lauf(xs: Vec[i64]) {\n\
         \x20   xs.sort_by_key fn { println(\"aus dem lambda\") return a }\n\
         \x20   println(\"danach\")\n\
         }";
    let bare = "fn lauf() {\n\
         \x20   let f = fn { println(\"aus dem lambda\") }\n\
         \x20   println(\"danach\")\n\
         }";
    let permissive = format!(
        "{}\n[fn.\"Vec::sort_by_key\"]\ntouches = []\n",
        nikaia::contracts::STD
    );

    for source in [trailing, bare] {
        for ledger in [nikaia::contracts::STD.to_string(), permissive.clone()] {
            let library = nikaia::contracts::Ledger::parse(&ledger).expect("the ledger parses");
            let reason = why_with(source, Some(library.clone()));
            assert!(
                reason.contains("in order"),
                "a statement that runs a lambda may not overlap the one after it:\n{reason}"
            );
            assert!(
                !reason.contains("stdout"),
                "the lambda's own write reached a touch set after all, which would make \
                 `from(f)`'s reading sound here:\n{reason}"
            );
        }
    }

    // And the lambda is refused on its own account, not only by the limits
    // around it.
    let reason = why(bare);
    assert!(
        reason.contains("a lambda, whose body this analysis does not read"),
        "{reason}"
    );
}
