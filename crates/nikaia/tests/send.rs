//! The structural `Send` check - `NK2501` and `NK2502`.
//!
//! [ADR-005](../../../docs/specification/adr/adr-005.md) §1 Group B decides it:
//! "`Send`-ness checked **structurally in the frontend** at *every* setting of
//! `user_parallelism`, so a library written at [`no`] cannot turn out
//! un-compilable at [`yes`]".
//! [ADR-037](../../../docs/specification/adr/adr-037.md) §3 and
//! [ADR-038](../../../docs/specification/adr/adr-038.md) D7 both rely on it, and
//! the experiment in `docs/history/foreign-runtime.md` is what found it missing.
//!
//! **Two halves, and the first decides whether the tool is worth running.** A
//! check that refuses a correct program is worse than no check, so the corpus is
//! the guard and the deliberately-wrong programs are the proof that the guard is
//! not passing because the check is asleep. The type checker's own corpus guard
//! (`typecheck.rs`) covers these codes too, because they are its findings; what
//! is here is the cases a corpus cannot contain, because nothing in the
//! repository writes `Shared` - `Shared` is unbuilt, and this check is what has
//! to exist before it lands.
//!
//! **Into a task of the program's own, no type *in the language* is refused a
//! crossing** ([ADR-037](../../../docs/specification/adr/adr-037.md) D6,
//! [ADR-045](../../../docs/specification/adr/adr-045.md) D2), and that is
//! stated rather than worked around: a lock and a `Shared` both go there at
//! both settings, so the `spawn` programs below assert **silence**.
//!
//! **Into code nothing describes, two of them are refused.** A lock, since
//! ADR-045 D3 — and that one is `NK2503`'s now rather than this file's, because
//! what the refusal is about there is the call
//! ([ADR-039](../../../docs/specification/adr/adr-039.md) D6, and
//! `reaching_a_lock.rs`). A `Shared`, since
//! [ADR-061](../../../docs/specification/adr/adr-061.md) D1 — which was decided
//! and not built, and which this file asserted the *absence* of until
//! `NK2503`'s split went looking for the count.
//!
//! **What does answer *may not* is a described type that says so**
//! ([ADR-123](../../../docs/specification/adr/adr-123.md) D1). The ledger's
//! `crosses` column took two values, `true` and absent, so no described type
//! could make the claim and both codes had nothing to fire on for two records'
//! worth of time. `crosses = false` is the third value and the end of that: the
//! two tests at the bottom of this file are the first end-to-end rendering of
//! either code, and `examples/foreign-runtime/`'s handle over an `Rc<String>` is
//! the type that needed it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use nikaia::check::{self, Finding};
use nikaia::contracts::order;
use nikaia::contracts::send::{self, Crossing};
use nikaia::contracts::ty::TyOps;
use nikaia::contracts::{Ledger, LedgerOps, STD, ty::Ty};
use nikaia::parser::parse_to_ast;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn findings(source: &str) -> Vec<Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    check::check(&parsed, &own, &library).findings
}

/// Only what this check says, so that an unrelated finding cannot make a test
/// pass for the wrong reason.
fn crossings(source: &str) -> Vec<Finding> {
    findings(source)
        .into_iter()
        .filter(|f| f.code.starts_with("NK25"))
        .collect()
}

/// The same, against a library ledger written here rather than `std`'s.
///
/// What [ADR-123](../../../docs/specification/adr/adr-123.md) is about is a
/// **described** type's claim, and nothing `std` ships says `crosses = false` -
/// a `std` type that may not cross a thread would be a `std` bug. So the type
/// under test is described in the test, the way
/// `examples/foreign-runtime/crossing/contracts/hyper_shim.contracts` describes
/// the real one.
fn crossings_against(library: &str, source: &str) -> Vec<Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(library).expect("the test's ledger parses");
    check::check(&parsed, &own, &library)
        .findings
        .into_iter()
        .filter(|f| f.code.starts_with("NK25"))
        .collect()
}

/// A ledger describing one Rust function and the type it hands back, with
/// whatever `crosses` is given for that type.
fn describing(crosses: &str) -> String {
    format!(
        "version = 2\n\
         toolchain = \"probe\"\n\
         inference = \"probe\"\n\
         \n\
         [fn.\"fremd::ortsgebunden\"]\n\
         pub = true\n\
         sync = true\n\
         signature = \"(name: String) -> fremd::LocalHandle\"\n\
         \n\
         [type.\"fremd::LocalHandle\"]\n\
         pub = true\n\
         {crosses}"
    )
}

/// What `project::check` does with a program, which is where the build switch
/// meets the severity and nothing else.
fn refused_at(source: &str, user_parallelism: &str) -> Option<String> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    nikaia::project::check(
        &parsed,
        &own,
        &BTreeSet::new(),
        // A loose file declares no Rust crate, so ADR-104 D1 has nothing to
        // ask about here, and nothing has gained an error since a committed
        // ledger, which is every build but the one after a change (ADR-101 D1).
        nikaia::project::Around {
            foreign: &nikaia::project::Foreign::default(),
            newly: &nikaia::check::Newly::new(),
            // …and one file is the whole program here, so a `comptime` has
            // nowhere else to call into.
            beside: &[],
            // …and no allowlist, so it reads nothing while it builds
            // (ADR-072 D1).
            reads: &nikaia::assets::Reads::none(),
        },
        Path::new("app.nika"),
        source,
        user_parallelism,
    )
    .err()
    .map(|error| format!("{error:#}"))
}

// --- the guard ---------------------------------------------------------------

/// No program in the repository has a crossing refused.
///
/// Part III C.4's property, for this check: it never rejects a program that is
/// correct. Every `.nika` there is, including the ones in subdirectories that
/// the type checker's own guard does not walk - `examples/inventory`, and the
/// three projects of the ADR-038 D7 experiment, which are the programs that
/// actually hand values to a foreign runtime.
#[test]
fn no_program_in_the_repository_has_a_crossing_refused() {
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    let mut checked = 0;
    let mut reported = String::new();

    for path in every_program(&repo_root()) {
        let source = std::fs::read_to_string(&path).expect("read the program");
        let Ok(parsed) = parse_to_ast(&source) else {
            // A probe that is not meant to parse is not this test's business.
            continue;
        };
        let own = Ledger::infer(&parsed);
        let name = path.display().to_string();
        for finding in check::check(&parsed, &own, &library)
            .findings
            .iter()
            .filter(|f| f.code.starts_with("NK25"))
        {
            reported.push_str(&nikaia::diagnostics::render_finding(
                finding, &name, &source,
            ));
        }
        checked += 1;
    }

    assert!(checked >= 12, "only {checked} programs were checked");
    assert!(
        reported.is_empty(),
        "a program in the repository was refused a crossing:\n{reported}"
    );
}

fn every_program(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut pending = vec![root.join("examples"), root.join("crates/nikaia-std/src")];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("nika") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// An ordinary value crosses into a task, and nothing is said about it.
///
/// The other half of the guard, in the smallest program that shows it: the
/// corpus has no `spawn` at all, because Part II 11.2's runtime integration is
/// the next roadmap line - so without this, "the corpus is clean" would also be
/// true of a check that never looked at a task.
#[test]
fn an_ordinary_value_crosses_into_a_task() {
    let clean = crossings(
        "struct Reading { name: String, temp: f64 }\n\
         fn report(text: String, counts: Vec[i64], r: Reading) {\n\
             spawn fn { println(f\"{text} {counts.len()} {r.temp}\") }\n\
         }",
    );
    assert!(clean.is_empty(), "{clean:#?}");
}

/// A value whose type nothing says is **not refused**, which is the one thing
/// this check may never get wrong.
///
/// Stage 0 knows the type of rather less than half of what a program writes, so
/// a check that refused what it could not decide would refuse most correct
/// programs. `Undecided` is not accepted either - `rustc` still type-checks the
/// emitted crate and ADR-005 D7's `E0277` translation reports that refusal
/// against the `.nika` line - but it is not refused *here*.
#[test]
fn a_value_whose_type_is_not_written_down_is_not_refused() {
    let clean = crossings(
        "use std::fs\n\nfn report(path: String) throws {\n\
             let handle = fs::map(path, fs::Root::Anywhere) catch { return }\n\
             spawn fn { println(f\"{handle.len()}\") }\n\
         }",
    );
    assert!(clean.is_empty(), "{clean:#?}");
}

// --- what it catches --------------------------------------------------------

/// A `Shared` crosses into a task, and nothing is said about it - **ADR-037 D6**.
///
/// This program was `NK2501`'s own example until the count stopped moving with
/// the switch. `Shared[Vec[i64]]` is answered by `Vec[i64]` now, which may
/// cross, so the task may take it. The verdict is still a property of the type
/// and still never reads `user_parallelism`; what moved is one row of
/// `contracts::send`'s table.
#[test]
fn a_shared_crosses_into_a_task() {
    let clean = crossings(
        "fn zaehle(counts: Shared[Vec[i64]]) {\n\
             spawn fn { println(f\"{counts.len()}\") }\n\
         }",
    );
    assert!(clean.is_empty(), "{clean:#?}");
}

/// The rule is still **structural and transitive**, which is what Group B asks
/// for: a struct is exactly as crossable as its fields, in both directions.
#[test]
fn a_struct_that_holds_a_shared_crosses_into_a_task() {
    let clean = crossings(
        "struct Tally { hits: Shared[i64] }\n\
         fn zaehle(t: Tally) {\n\
             spawn fn { println(f\"{t.hits}\") }\n\
         }",
    );
    assert!(clean.is_empty(), "{clean:#?}");
}

/// **Part II 12.2's counter goes into a task** - ADR-045 D2, and the program that
/// could not be written before it.
///
/// This is what `user_parallelism = yes` exists to serve, and the verdict refused
/// it: a lock's implementation follows the switch, the verdict may not consult the
/// switch, so it took the worse of the two settings and answered "may not"
/// everywhere. At `yes` a real operating-system lock stands in the emitted
/// program, under which this counter is entirely safe, and it was refused on
/// account of an implementation that does not occur in it.
///
/// D1 asks the verdict about a destination, so there are two answers now and each
/// is the same at both settings - which is all Group B ever asked for.
#[test]
fn a_lock_goes_into_a_task_of_our_own() {
    let clean = crossings(
        "fn zaehle(counter: SharedMut[i32]) {\n\
             spawn fn { println(f\"{counter}\") }\n\
         }",
    );
    assert!(
        clean.is_empty(),
        "12.2's counter must be writable: {clean:#?}"
    );

    let parsed = parse_to_ast("fn zaehle(counter: SharedMut[i32]) { }").expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    assert_eq!(
        send::crossing(
            &Ty::parse("SharedMut[i32]"),
            &own,
            &library,
            send::Destination::Ours
        ),
        Crossing::May,
        "and permitted rather than merely unrefused"
    );
}

/// **…and the same value handed to code nothing describes is refused** - ADR-045
/// D3, which is `NK2502`'s first occupant.
///
/// The conservative answer, and the record says so rather than implying a proof:
/// at `user_parallelism = yes` this would be safe. It is refused at both settings
/// because a lock's Rust type differs per setting while a Rust library has one
/// signature, so a library written against one would compile at one setting and
/// fail at the other - the asymmetry Group B exists to prevent.
///
/// The way out is not "don't do that": the caller opens the lock and hands the
/// value inside it over, which is the shape an ordinary function already has
/// (ADR-042 D1, Part I 6.2).
#[test]
fn a_lock_does_not_go_into_code_nothing_describes() {
    let found = crossings(
        "fn ueber(counter: SharedMut[i32]) {\n\
             fremd::irgendwas(counter)\n\
         }",
    );
    assert_eq!(found.len(), 1, "exactly one refusal: {found:#?}");
    let finding = &found[0];
    // **`NK2503` and not `NK2502`** ([ADR-039](../../../docs/specification/adr/adr-039.md)
    // D6): a lock reachable through an argument is a refusal about the *call*,
    // and the shipped diagnostic used to be the one about the value crossing.
    assert_eq!(finding.code, "NK2503");
    assert!(
        finding
            .message
            .contains("`fremd::irgendwas` could reach a lock through `counter`"),
        "{}",
        finding.message
    );
    let notes = finding.notes.join(" ");
    // The note names the type the source wrote, not what it expands to
    // ([ADR-064](../../../docs/specification/adr/adr-064.md) D1).
    assert!(notes.contains("`SharedMut[i32]`"), "{notes}");
    assert!(
        notes.contains("code that might keep a lock could deadlock"),
        "{notes}"
    );
    let help = finding.help.as_deref().expect("a refusal has a way out");
    // Part III C.6 writes the way out as a line the program can be edited into.
    assert!(help.contains("fremd::irgendwas(counter.get())"), "{help}");

    // And into a task of our own the same value is fine, which is the pair D1
    // added. Without this half the refusal above would read as the old rule.
    assert!(
        crossings(
            "fn zaehle(counter: SharedMut[i32]) {\n\
             spawn fn { println(f\"{counter}\") }\n\
         }",
        )
        .is_empty()
    );
}

/// **A `Shared` does not go into a call this compiler cannot see the end of**
/// ([ADR-061](../../../docs/specification/adr/adr-061.md) D1), which is the
/// `examples/foreign-runtime/crossing` shape.
///
/// D1 was decided and **not built**: `Shared` sat in `contracts::send`'s
/// `CHOSEN` row and in its `CONTAINERS` row, the container row was reached
/// first, and the lock's own row was dead code. This test asserted the silence
/// that left behind.
///
/// The reason is the lock's reason without the lock: which count a value gets
/// is chosen per value (ADR-037 D7), so a `Shared[Conn]` is one shape for one
/// value and another for the next in the same program, and no signature outside
/// this language can name both. `NK2502` and not `NK2503`, because nothing here
/// is a lock.
#[test]
fn a_shared_does_not_go_into_a_call_this_compiler_cannot_see() {
    let found = crossings(
        "fn ueber(handle: Shared[String]) {\n\
             fremd::auf_einen_thread(handle)\n\
         }",
    );
    assert_eq!(found.len(), 1, "exactly one refusal: {found:#?}");
    let finding = &found[0];
    assert_eq!(finding.code, "NK2502");
    assert!(finding.message.contains("`handle`"), "{}", finding.message);
    let notes = finding.notes.join(" ");
    assert!(notes.contains("`Shared[String]`"), "{notes}");
    assert!(
        notes.contains("how a shared value is counted is decided per value"),
        "{notes}"
    );
    // The sentence is not the lock's: nothing here is a lock, and a refusal
    // whose reason is about a lock would be wrong about this.
    assert!(!notes.contains("lock"), "{notes}");
    let help = finding.help.as_deref().expect("a refusal has a way out");
    assert!(
        help.contains("a view or a copy, instead of the shared value"),
        "{help}"
    );

    // And into a task of our own the same value is fine, which is the pair: D1
    // is about a signature outside this language and nothing else.
    assert!(
        crossings(
            "fn ueber(handle: Shared[String]) {\n\
             spawn fn { println(f\"{handle}\") }\n\
         }",
        )
        .is_empty()
    );
}

/// A call into a *described* function is not this, however unpleasant its
/// arguments.
///
/// The rule is about a body this compiler cannot see, so a `std` function and a
/// function of this program are both outside it - their bodies are accounted
/// for, and what they do with what they are given is the same question asked
/// inside them.
#[test]
fn a_call_this_compiler_can_see_is_not_a_crossing() {
    let clean = crossings(
        "fn halte(s: Shared[String]) { }\n\
         fn ueber(handle: Shared[String]) { halte(handle) }",
    );
    assert!(clean.is_empty(), "{clean:#?}");
}

// --- the asymmetry Group B exists to prevent --------------------------------

/// **One verdict at both settings**, which is the property ADR-037 §5 asks this
/// file to assert - and it is asserted by comparing verdicts rather than exit
/// codes, because that is the only comparison that catches the failure.
///
/// Accepted at `yes` and refused at `no` - or silent at `no` and refused at
/// `yes` - is what Group B was written to prevent. Both halves are asserted:
/// the programs that are accepted are accepted at both, and the one that is
/// refused is refused at both.
///
/// **The severity split is what has no input now.** `lint_where_nothing_crosses`
/// downgrades `NK2501` at `no` and leaves `NK2502` alone, and both arms are
/// still there for the next type whose expansion moves with the switch - which
/// is exactly why D6 removed the cause and not the mechanism.
#[test]
fn the_verdict_is_the_same_at_both_settings() {
    for source in [
        "fn zaehle(counts: Shared[Vec[i64]]) {\n\
             spawn fn { println(f\"{counts.len()}\") }\n\
         }",
        "fn zaehle(counter: SharedMut[i32]) {\n\
             spawn fn { println(f\"{counter}\") }\n\
         }",
    ] {
        // The analysis never sees a switch, so there is one verdict to compare.
        let found = crossings(source);
        assert!(found.is_empty(), "{found:#?}");
        assert_eq!(
            refused_at(source, "no"),
            refused_at(source, "yes"),
            "{source}"
        );
        assert_eq!(refused_at(source, "no"), None, "{source}");
    }

    // And the same, from the refusing side: ADR-061 D1 and ADR-045 D3 are both
    // properties of a **type**, so a program either compiles at both settings
    // or at neither. This is the half the file could not assert while no
    // program was refused at all.
    for source in [
        "fn ueber(handle: Shared[String]) {\n\
             fremd::auf_einen_thread(handle)\n\
         }",
        "fn ueber(counter: SharedMut[i32]) {\n\
             fremd::irgendwas(counter)\n\
         }",
    ] {
        let at_no = refused_at(source, "no");
        assert_eq!(at_no, refused_at(source, "yes"), "{source}");
        assert!(at_no.is_some(), "refused at both settings: {source}");
    }
}

/// Part II 12.2's counter, at both settings, which is the program D3's open
/// question was wanted for.
///
/// `Shared` no longer stands in front of it: into a task of ours the handle may
/// cross at both settings - at `yes` under an atomic count, at `no` because
/// nothing crosses there at all (ADR-061 D2). What is left is the **lock**, and this
/// test says exactly what the compiler says about it today - nothing, because
/// nothing written down describes `Locked`. Which of `RefCell` and `Mutex` it
/// expands to is ADR-037 D3's second half and is not decided here, so this
/// asserts the silence rather than a verdict about the lock.
#[test]
fn the_shared_half_of_part_ii_12_2s_counter_no_longer_refuses() {
    let source = "fn zaehle(counter: SharedMut[i32]) {\n\
                      counter.access(fn(a) { a })\n\
                      spawn fn { println(f\"{counter}\") }\n\
                  }";
    for setting in ["no", "yes"] {
        assert_eq!(refused_at(source, setting), None, "at `{setting}`");
    }
    assert!(crossings(source).is_empty());
}

// --- the crossing the compiler chooses for itself ---------------------------

/// A ledger with one crossable result and one the compiler cannot decide about,
/// so that the overlapping analysis can be asked about both.
///
/// Hand-written rather than `std`'s, because nothing in `std` returns a value
/// that may not cross a thread - which is the whole reason this check could be
/// absent for so long without anything being unsound (`docs/history/foreign-runtime.md`
/// §3.2).
///
/// **`probe::held` hands back `SharedMut[i64]` rather than `Shared[i64]`
/// since ADR-037 D6.** The overlapping analysis refuses anything but `May`, so
/// what it needs here is a non-`May` answer and not specifically a refusal - and
/// since D6 a `Shared` of plain data *is* `May`, which is a change to what this
/// analysis lets through and is asserted below.
fn probe_library() -> Ledger {
    Ledger::parse(
        "version = 2\n\
         toolchain = \"probe\"\n\
         inference = \"probe\"\n\
         \n\
         [fn.\"probe::plain\"]\n\
         pub = true\n\
         sync = true\n\
         touches = []\n\
         signature = \"() -> i64\"\n\
         \n\
         [fn.\"probe::held\"]\n\
         pub = true\n\
         sync = true\n\
         touches = []\n\
         signature = \"() -> SharedMut[i64]\"\n\
         \n\
         [fn.\"probe::counted\"]\n\
         pub = true\n\
         sync = true\n\
         touches = []\n\
         signature = \"() -> Shared[i64]\"\n\
         \n\
         [fn.\"probe::opaque\"]\n\
         pub = true\n\
         sync = true\n\
         touches = []\n\
         signature = \"() -> Mapped\"\n",
    )
    .expect("the probe ledger parses")
}

/// What the first two statements of a function reduce to, and what the verdict
/// says about them.
///
/// It used to be the `--overlaps` report's text, which asked the same question
/// through a paragraph. [ADR-050](../../../docs/specification/adr/adr-050.md) D1
/// withdrew the reordering and the report became one about `overlap { … }`
/// blocks — so the verdict is asked directly here, which is what these tests
/// were ever about: **crossing a thread is a reason two operations may not run
/// together**, and that reason survives the construct that used to consume it.
fn verdict_against_probe(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = probe_library();
    let body = parsed
        .program
        .items
        .iter()
        .find_map(|item| match &item.node {
            nikaia::ast::Item::Fn { body, .. } => Some(body),
            _ => None,
        })
        .expect("a function to read");

    let pair: Vec<_> = body
        .stmts
        .iter()
        .take(2)
        .map(|stmt| order::accounted(&parsed, &stmt.node, &own, &library))
        .collect();
    match pair.as_slice() {
        [
            order::Accounted::Operation(earlier),
            order::Accounted::Operation(later),
        ] => {
            let verdict = order::verdict(earlier, later);
            let mark = if verdict.is_overlap() {
                "together"
            } else {
                "in order"
            };
            format!(
                "{mark}  {} / {} - {}",
                earlier.callee,
                later.callee,
                verdict.why()
            )
        }
        [refused, other] | [other, refused]
            if !matches!(refused, order::Accounted::Operation(_)) =>
        {
            let named = match other {
                order::Accounted::Operation(operation) => operation.callee.clone(),
                _ => "…".to_string(),
            };
            format!("in order  {named} - {}", refused.why())
        }
        _ => "in order  … - nothing to compare".to_string(),
    }
}

/// Two operations that meet on nothing overlap - the control, without which the
/// test below would also pass against an analysis that refuses everything.
#[test]
fn two_crossable_operations_still_overlap() {
    let report = verdict_against_probe(
        "fn main() {\n\
             let a = probe::plain()\n\
             let b = probe::plain()\n\
             println(f\"{a} {b}\")\n\
         }",
    );
    assert!(
        report.contains("together  probe::plain / probe::plain"),
        "{report}"
    );
}

/// A pair whose result is a `Shared` of plain data **does** overlap since
/// ADR-037 D6 - and that is the one thing step 1 changes about the overlapping.
///
/// It is also where the floor in `contracts::sharing` earns its keep. An
/// overlapped operation runs in a closure somewhere else and hands its value
/// back, so this `Shared`'s count is touched on two threads - which is why the
/// result of a call is a seed there and may never be lowered to a plain count.
#[test]
fn a_pair_whose_result_is_a_shared_of_plain_data_overlaps() {
    let report = verdict_against_probe(
        "fn main() {\n\
             let a = probe::counted()\n\
             let b = probe::counted()\n\
             println(f\"{a} {b}\")\n\
         }",
    );
    assert!(
        report.contains("together  probe::counted / probe::counted"),
        "{report}"
    );
}

/// … and a pair whose result nothing permits to cross keeps the order it was
/// written in, with no diagnostic at all.
///
/// This is the crossing the **compiler** chose: overlapping puts each statement
/// in a closure that runs elsewhere and hands its value back, so the result
/// crosses a thread. Not overlapping is a step the compiler was never obliged to
/// take, so fail-closed here costs speed and never a refusal - which is ADR-033
/// D4's own polarity, applied to the crossing question.
#[test]
fn an_operation_whose_result_nothing_permits_keeps_its_place() {
    let source = "fn main() {\n\
                      let a = probe::opaque()\n\
                      let b = probe::opaque()\n\
                      println(f\"{a} {b}\")\n\
                  }";
    let report = verdict_against_probe(source);
    assert!(!report.contains("together"), "{report}");
    assert!(
        report.contains("would have to move to another thread"),
        "and the report says which refusal it was (ADR-033 D9): {report}"
    );

    // And it is not a refusal: the program compiles, it simply does not overlap.
    assert!(crossings(source).is_empty(), "{:#?}", crossings(source));
}

/// **A result that holds a lock overlaps, which is ADR-045 D1 reaching the third
/// call site.**
///
/// `probe::held` hands back a `SharedMut[i64]` and kept its place while the
/// verdict took the worse of the two settings for a lock. The closure overlapping
/// builds is this compiler's own, on this compiler's own thread, so it is the
/// destination D2 is about and not a library we cannot read - and the pairs the
/// ordering analysis buys are bought here too.
///
/// The destination is the only thing that changed: the same signature handed to
/// foreign code is still refused (`a_lock_does_not_go_into_code_nothing_describes`).
#[test]
fn a_result_that_holds_a_lock_overlaps() {
    let report = verdict_against_probe(
        "fn main() {\n\
             let a = probe::held()\n\
             let b = probe::held()\n\
             println(f\"{a} {b}\")\n\
         }",
    );
    assert!(
        report.contains("together  probe::held / probe::held"),
        "{report}"
    );
}

/// A result nothing is written down about keeps its place too, and the way to
/// buy the overlap back is the one `touches` already has: write it down.
///
/// `cli::args` is the measured case. Its result is a Rust type whose parts this
/// compiler cannot walk, so the answer was "nobody said" - which is not
/// permission (ADR-010 D1) - and the ten pairs ADR-033 §8.1 bought with a
/// `touches` line would have gone again. The `crosses = true` on
/// `[type."cli::Args"]` is what keeps them.
#[test]
fn a_library_type_says_it_may_cross_and_the_overlap_stays() {
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    let args = library
        .types
        .get("cli::Args")
        .expect("`cli::Args` is described");
    assert!(
        args.crosses.may(),
        "the claim is what keeps `cli::args` overlapping"
    );
    assert_eq!(
        send::crossing(
            &Ty::named("Args"),
            &Ledger::empty(),
            &library,
            send::Destination::Ours
        ),
        Crossing::May
    );

    // And without the claim it would be undecided rather than permitted.
    let silent = Ledger::parse(
        "version = 2\n\
         toolchain = \"probe\"\n\
         inference = \"probe\"\n\
         \n\
         [type.\"cli::Args\"]\n\
         pub = true\n",
    )
    .expect("the probe ledger parses");
    assert!(matches!(
        send::crossing(
            &Ty::named("Args"),
            &Ledger::empty(),
            &silent,
            send::Destination::Ours
        ),
        Crossing::Undecided { .. }
    ));
}

// --- ADR-123: the column's third value, and the first refusals to reach it ----

/// **The three answers a `crosses` line gives**
/// ([ADR-123](../../../docs/specification/adr/adr-123.md) D1).
///
/// `true` is *may*, `false` is *may not*, and leaving the line out is
/// *undecided*, which is not permission and not a refusal. A boolean could hold
/// the first and the third; the middle one is what the destination's refusals
/// were built for and had never been handed.
#[test]
fn a_crosses_line_says_may_may_not_or_nothing() {
    let answer = |ledger: &str, ty: Ty| {
        let library = Ledger::parse(ledger).expect("the test's ledger parses");
        send::crossing(&ty, &Ledger::empty(), &library, send::Destination::Ours)
    };

    assert_eq!(
        answer(&describing("crosses = true\n"), Ty::named("LocalHandle")),
        Crossing::May,
        "`true` is the claim it may, as it always was"
    );
    assert!(matches!(
        answer(&describing(""), Ty::named("LocalHandle")),
        Crossing::Undecided { .. }
    ));
    let refused = answer(&describing("crosses = false\n"), Ty::named("LocalHandle"));
    assert!(
        matches!(&refused, Crossing::MayNot { part, .. } if part == "LocalHandle"),
        "`false` is the claim it may not, and names the type: {refused:#?}"
    );

    // **And it answers with arguments too, where `true` does not.** The
    // asymmetry is the polarity rather than an oversight (ADR-010 D1): a promise
    // is withheld where the line might not have spoken about the `T`, and a
    // restriction is kept, because nothing a value is wrapped in makes it
    // crossable.
    let wrapped = answer(
        &describing("crosses = false\n"),
        Ty::parse("LocalHandle[String]"),
    );
    assert!(
        matches!(wrapped, Crossing::MayNot { .. }),
        "a restriction is kept where a promise would be withheld: {wrapped:#?}"
    );
    assert!(matches!(
        answer(
            &describing("crosses = true\n"),
            Ty::parse("LocalHandle[String]")
        ),
        Crossing::Undecided { .. }
    ));
}

/// **`crosses = false` renders, re-parses, and a third spelling is refused.**
///
/// The round trip matters because the ledger is a **committed** file: a column
/// that could be written and not read again would make a derivation differ from
/// the file it came from, which is what `ledger_determinism.rs` exists about.
#[test]
fn the_column_survives_being_written_and_read_again() {
    let written = Ledger::parse(&describing("crosses = false\n")).expect("it parses");
    let rendered = written.render();
    assert!(
        rendered.contains("crosses = false"),
        "the claim is written out: {rendered}"
    );
    let again = Ledger::parse(&rendered).expect("what it wrote parses");
    assert_eq!(
        again.types.get("fremd::LocalHandle").map(|t| t.crosses),
        written.types.get("fremd::LocalHandle").map(|t| t.crosses),
    );

    // A ledger that never said `false` says exactly what it said, which is D1's
    // *every ledger already written keeps its meaning*.
    let silent = Ledger::parse(&describing("")).expect("it parses");
    assert!(!silent.render().contains("crosses"), "{}", silent.render());

    // And a spelling that is neither is refused rather than guessed at: reading
    // it as either value would put a claim in the file that nobody wrote.
    let wrong = Ledger::parse(&describing("crosses = \"maybe\"\n"));
    let said = format!("{:#}", wrong.expect_err("`maybe` is not an answer"));
    assert!(said.contains("`crosses` is `true` or `false`"), "{said}");
}

/// **`threads` renders, re-parses, and a third spelling is refused** — the same
/// round trip `crosses` has, for the same reason: the ledger is a **committed**
/// file, and a column that could be written and not read again would make a
/// derivation differ from the file it came from.
#[test]
fn the_threads_column_survives_being_written_and_read_again() {
    let entry = |extra: &str| {
        describing("")
            + "\n\
               [fn.\"fremd::ueber_einen_thread\"]\n\
               pub = true\n\
               sync = true\n"
            + extra
            + "signature = \"(value: $T) -> String\"\n"
    };

    for word in ["true", "false"] {
        let written = Ledger::parse(&entry(&format!("threads = {word}\n"))).expect("it parses");
        let rendered = written.render();
        assert!(
            rendered.contains(&format!("threads = {word}")),
            "the claim is written out: {rendered}"
        );
        let again = Ledger::parse(&rendered).expect("what it wrote parses");
        assert_eq!(
            again
                .functions
                .get("fremd::ueber_einen_thread")
                .map(|c| c.threads),
            written
                .functions
                .get("fremd::ueber_einen_thread")
                .map(|c| c.threads),
        );
    }

    // A ledger that never said it says exactly what it said, which is the third
    // value existing: *nobody said*, and every ledger already written keeps its
    // meaning.
    let silent = Ledger::parse(&entry("")).expect("it parses");
    assert!(!silent.render().contains("threads"), "{}", silent.render());

    // And a spelling that is neither is refused rather than guessed at.
    let wrong = Ledger::parse(&entry("threads = \"maybe\"\n"));
    let said = format!("{:#}", wrong.expect_err("`maybe` is not an answer"));
    assert!(said.contains("`threads` is `true` or `false`"), "{said}");
}

/// **`NK2501` fires for the first time**: a described type that may not cross,
/// used inside a task.
///
/// Part II 11.2's rule has been built and tested since ADR-005 §1 Group B, and
/// until [ADR-123](../../../docs/specification/adr/adr-123.md) nothing could
/// reach it - the only type `contracts::send` answered *may not* about was
/// `Shared`, and ADR-037 D6 took that away. This is the program the message was
/// always for.
#[test]
fn a_described_type_that_may_not_cross_is_refused_into_a_task() {
    let found = crossings_against(
        &describing("crosses = false\n"),
        "fn ueber() {\n\
             let handle = fremd::ortsgebunden(\"nicht Send\")\n\
             spawn fn { println(f\"{handle}\") }\n\
         }",
    );
    assert_eq!(found.len(), 1, "exactly one refusal: {found:#?}");
    let finding = &found[0];
    assert_eq!(finding.code, "NK2501");
    assert!(finding.message.contains("`handle`"), "{}", finding.message);
    let notes = finding.notes.join(" ");
    assert!(notes.contains("A task runs on its own thread"), "{notes}");
    assert!(
        notes.contains("`fremd::LocalHandle` is described as a value that can't move"),
        "{notes}"
    );

    // And the same program is silent where the line says nothing, which is what
    // keeps the refusal a claim about the ledger rather than about the shape.
    assert!(
        crossings_against(
            &describing(""),
            "fn ueber() {\n\
                 let handle = fremd::ortsgebunden(\"nicht Send\")\n\
                 spawn fn { println(f\"{handle}\") }\n\
             }",
        )
        .is_empty(),
        "nothing recorded is not a refusal (ADR-010 D1)"
    );
}

/// **`NK2502` fires for the first time**: the same value handed to a call this
/// compiler cannot see the end of.
///
/// ADR-038 D7's first rule, which is the one `examples/foreign-runtime/crossing`
/// is a program about: a foreign call may put what it is given on a thread it
/// owns, so a value that may not cross may not go into one.
#[test]
fn a_described_type_that_may_not_cross_is_refused_into_a_foreign_call() {
    let found = crossings_against(
        &describing("crosses = false\n"),
        "fn ueber() {\n\
             let handle = fremd::ortsgebunden(\"nicht Send\")\n\
             fremd::irgendwohin(handle)\n\
         }",
    );
    assert_eq!(found.len(), 1, "exactly one refusal: {found:#?}");
    let finding = &found[0];
    assert_eq!(finding.code, "NK2502");
    assert!(finding.message.contains("`handle`"), "{}", finding.message);
    assert!(
        finding.notes.join(" ").contains("LocalHandle"),
        "{:#?}",
        finding.notes
    );
}

/// **A described foreign call is asked where the description says the word, and
/// nowhere else** ([ADR-193](../../../docs/specification/adr/adr-193.md) D1, D2).
///
/// `examples/foreign-runtime/crossing` is the program: its handle says
/// `crosses = false`, and it is handed to a foreign function the description
/// names. `NK2502` used to ask its question only of a call **nothing**
/// describes (ADR-038 D7's own words), so a crate that answered every other
/// question honestly turned the check off by being described — and that program
/// was refused by `rustc`'s `Send` bound instead, against the `.nika` line.
///
/// `threads` is the word that turns it back on. It fires on the **claim** and
/// never on its absence, which is D2: a description that does not say is a
/// description that was not asked, and the program keeps the answer it had.
#[test]
fn a_described_call_that_says_it_threads_is_asked_and_a_silent_one_is_not() {
    let entry = |extra: &str| {
        describing("crosses = false\n")
            + "\n\
               [fn.\"fremd::ueber_einen_thread\"]\n\
               pub = true\n\
               sync = true\n"
            + extra
            + "keeps = [\"value\"]\n\
               signature = \"(value: $T) -> String\"\n"
    };
    let program = "fn ueber() {\n\
                       let handle = fremd::ortsgebunden(\"nicht Send\")\n\
                       let text = fremd::ueber_einen_thread(handle)\n\
                   }";

    // **Silence is not a claim.** Nothing is refused, exactly as before.
    assert!(
        crossings_against(&entry(""), program).is_empty(),
        "a description that does not say is one that was not asked (D2)"
    );

    // **And `threads = false` is a claim in the other direction**, which is the
    // third value existing at all: a person wrote *it does not*, and a refusal
    // may not be raised on that either.
    assert!(
        crossings_against(&entry("threads = false\n"), program).is_empty(),
        "`threads = false` is the claim that it does not (D1)"
    );

    let refused = crossings_against(&entry("threads = true\n"), program);
    let finding = refused
        .first()
        .unwrap_or_else(|| panic!("`threads = true` is asked: {refused:#?}"));
    assert_eq!(finding.code, "NK2502");
    assert!(finding.message.contains("`handle`"), "{}", finding.message);
    // The note names the **word** and not the absence of a description, because
    // this call *is* described — and a message that said otherwise would send a
    // reader to write a file that is already there.
    assert!(
        finding.notes.join(" ").contains("threads = true"),
        "{:#?}",
        finding.notes
    );
}
