//! The ADR-038 D7 experiment, kept re-runnable.
//!
//! D7 says a Rust crate may bring its own runtime and its own threads, and
//! names two rules that keep that sound. `examples/foreign-runtime/` is the
//! program that tests them: a Nikaia project depending on a shim crate over
//! `hyper` and `tokio`, through ADR-002 D1's crates.io passthrough. What the
//! experiment found is written up in `docs/history/foreign-runtime.md`.
//!
//! **One test here runs on every `cargo test`** - the one that needs neither
//! Cargo nor the network, and which is the one that checks a guarantee rather
//! than a finding: a call into a foreign crate has no ledger entry, so ADR-033
//! D4 makes it order against everything.
//!
//! **The rest are `#[ignore]`d**, because each one runs `cargo`, and the first
//! run fetches `hyper`, `tokio` and twenty-four other crates from crates.io.
//! Run them with:
//!
//! ```text
//! cargo test --test foreign_runtime -- --ignored --nocapture
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;

use nikaia::contracts::{Ledger, LedgerOps, STD, order};
use nikaia::parser::parse_to_ast;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn experiment() -> PathBuf {
    repo_root().join("examples/foreign-runtime")
}

/// One `CARGO_TARGET_DIR` for all three projects, so `hyper` is built once.
fn shared_target_dir() -> PathBuf {
    repo_root().join("target").join("nikaia-foreign-runtime")
}

fn nikaia(subcommand: &str, project: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .args([subcommand, "--project"])
        .arg(project)
        .env("CARGO_TARGET_DIR", shared_target_dir())
        .output()
        .expect("the nikaia binary runs")
}

fn said(output: &std::process::Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

// ---------------------------------------------------------------------------
// D7's third paragraph: what needs no rule.
// ---------------------------------------------------------------------------

/// A foreign crate has no ledger entry, so `touches` is absent, so ADR-033 D4
/// makes the call reach everything and order against everything.
///
/// This is the one claim in D7 that is a *guarantee* rather than a finding, so
/// it is the one asserted on every build. It needs no network: the ordering
/// analysis is a question about the program, and the answer does not depend on
/// the foreign crate existing.
#[test]
fn a_call_into_a_foreign_crate_orders_against_everything() {
    let path = experiment().join("overlaps.nika");
    let source = std::fs::read_to_string(&path).expect("the probe is in the repository");
    let parsed = parse_to_ast(&source).expect("it parses");

    let library = Ledger::parse(STD).expect("std ships a ledger");
    let own = Ledger::infer(&parsed);
    // **Every adjacent pair, asked directly.** It used to go through the
    // `--overlaps` report; ADR-050 D1 withdrew the reordering that report was
    // about, and the claim under test is the verdict rather than the schedule -
    // a foreign call reaches everything, so no pair holding one may overlap.
    let mut lines = Vec::new();
    for pair in parsed
        .program
        .items
        .iter()
        .filter_map(|item| match &item.node {
            nikaia::ast::Item::Fn { body, .. } => Some(body),
            _ => None,
        })
    {
        for two in pair.stmts.windows(2) {
            let earlier = order::accounted(&parsed, &two[0].node, &own, &library);
            let later = order::accounted(&parsed, &two[1].node, &own, &library);
            let line = match (&earlier, &later) {
                (order::Accounted::Operation(a), order::Accounted::Operation(b)) => {
                    let verdict = order::verdict(a, b);
                    let mark = if verdict.is_overlap() {
                        "together"
                    } else {
                        "in order"
                    };
                    format!("{mark}  {} / {} - {}", a.callee, b.callee, verdict.why())
                }
                (refused, other) | (other, refused)
                    if !matches!(refused, order::Accounted::Operation(_)) =>
                {
                    let named = match other {
                        order::Accounted::Operation(operation) => operation.callee.clone(),
                        _ => "…".to_string(),
                    };
                    format!("in order  {named} - {}", refused.why())
                }
                _ => continue,
            };
            lines.push(line);
        }
    }
    let report = lines.join("\n");

    // The control: two reads of two files meet on nothing and run together.
    // Without it, "everything is in order" would also be true of an analysis
    // that refuses everything, and the test would prove nothing.
    assert!(
        report.contains("together  fs::read_to_string / fs::read_to_string"),
        "the control pair must overlap, or this test cannot tell D4 from a \
         broken analysis:\n{report}"
    );

    // Every pair with a foreign call in it, and the reason must be D4's.
    for line in report.lines() {
        if line.contains("hyper_shim::serve_once") {
            assert!(
                line.trim_start().starts_with("in order"),
                "a pair with a foreign call in it overlapped:\n{report}"
            );
            assert!(
                line.contains("touches, so it might touch anything"),
                "and the reason has to be the absent `touches`, not something \
                 that happens to coincide with it:\n{report}"
            );
        }
    }
    assert!(
        report.matches("hyper_shim::serve_once").count() >= 4,
        "the probe is supposed to put a foreign call in four pairs:\n{report}"
    );
}

// ---------------------------------------------------------------------------
// D7's first paragraph: a crate may bring its own runtime.
// ---------------------------------------------------------------------------

/// Question 1: a Nikaia program starts a Rust HTTP server and serves one
/// request, and a value that may cross any thread crosses into one the foreign
/// runtime owns.
///
/// Needs the network on the first run, and `cargo`.
#[test]
#[ignore = "runs cargo and fetches hyper and tokio from crates.io"]
fn a_nikaia_program_serves_one_request_through_hyper() {
    let run = nikaia("run", &experiment().join("serve"));
    assert!(run.status.success(), "{}", said(&run));

    let out = String::from_utf8_lossy(&run.stdout);
    assert!(
        out.contains("served: GET /hello"),
        "hyper parsed the request and the shim handed the body back: {}",
        said(&run)
    );
    assert!(
        out.contains("crossed: a String is Send on tokio-"),
        "and a Nikaia value really did run on a thread the foreign runtime \
         owns, not on ours: {}",
        said(&run)
    );
}

/// Question 2, first rule: a value that may **not** cross a thread, handed to
/// a thread the foreign runtime owns.
///
/// It is refused **by this compiler**, and that sentence is new.
///
/// It used to be `rustc`'s `Send` bound, translated onto the `.nika` line by
/// [ADR-005](../../../docs/specification/adr/adr-005.md) D7 — the place fixed
/// and the **text** still Rust's, which that record carried as its open half.
/// The structural `Send` check had nothing to say: the value's type comes from
/// the foreign crate, so its verdict is `Undecided`, which is neither
/// permission nor a refusal.
///
/// What decides it now is two things a **person wrote down**
/// ([ADR-193](../../../docs/specification/adr/adr-193.md) D1, D2):
/// `crosses = false` on `hyper_shim::LocalHandle`, because the type holds an
/// `Rc`, and `threads = true` on `hyper_shim::across_a_thread`, because it
/// builds a `tokio` runtime and spawns. Neither is inferred and neither could
/// be; both are in the committed description a reviewer reads
/// ([ADR-104](../../../docs/specification/adr/adr-104.md) D5).
#[test]
#[ignore = "runs cargo and fetches hyper and tokio from crates.io"]
fn a_value_that_may_not_cross_a_thread_is_refused_against_the_nika_line() {
    let built = nikaia("build", &experiment().join("crossing"));
    assert!(
        !built.status.success(),
        "an `Rc` reached a foreign thread and the build succeeded. If this \
         fires, read `docs/history/foreign-runtime.md` - it is either a real \
         unsoundness or something about the crossing has changed: {}",
        said(&built)
    );

    let complaint = said(&built);

    // **Nikaia's own code, and the author's own line** — Part III C.1's Iron
    // Rule with nothing of `rustc`'s left in it.
    assert!(
        complaint.contains("NK2502"),
        "the refusal is this compiler's: {complaint}"
    );
    assert!(
        complaint.contains("src/main.nika:"),
        "the refusal names the `.nika` line the author wrote: {complaint}"
    );
    assert!(
        complaint.contains("across_a_thread"),
        "and shows the statement it is about: {complaint}"
    );
    assert!(
        !complaint.contains("gen/foreign_runtime_crossing.rs:"),
        "and no longer points into a file the author has never read: {complaint}"
    );

    // And it says **which claim** decided it, because that is what a reader has
    // to check: the description is a person's and a wrong one is a person's to
    // correct.
    assert!(
        complaint.contains("threads = true"),
        "the note names the word that made the call answerable: {complaint}"
    );
    assert!(
        complaint.contains("may not go to another thread"),
        "and the claim about the value: {complaint}"
    );
}

/// The same crossing through a foreign API that lies about `Send` — **and it is
/// refused too**, which `docs/history/foreign-runtime.md` §3.5 did not expect.
///
/// That section says a structural `Send` check *would not have caught this one
/// either: the value it would check is `Send`-by-declaration at the point
/// Nikaia can see it*. The sentence is still true, and the check that catches
/// it is not the one it is about. A structural check reads what the language
/// below says about a type; this reads what a **person wrote down** about it —
/// `crosses = false`, because `LocalHandle` holds an `Rc`. An
/// `unsafe impl<T> Send for Smuggled<T>` cannot change that line, because the
/// line never asked Rust.
///
/// **None of this is soundness** ([ADR-193](../../../docs/specification/adr/adr-193.md)
/// D3). A description that claimed `crosses = true` about the same type would
/// get exactly as far as it did before. What moved is *who* has to be honest,
/// and a `.contracts` file is committed and reviewed like code.
#[test]
#[ignore = "runs cargo and fetches hyper and tokio from crates.io"]
fn a_foreign_api_that_lies_about_send_is_caught_by_what_a_person_wrote() {
    let run = nikaia("run", &experiment().join("smuggled"));
    assert!(
        !run.status.success(),
        "an `Rc` reached a foreign thread through a crate that lies about \
         `Send`, and nothing stopped it: {}",
        said(&run)
    );

    let complaint = said(&run);
    assert!(
        complaint.contains("NK2502"),
        "and what stops it is this compiler: {complaint}"
    );
    assert!(
        complaint.contains("across_a_thread_unchecked"),
        "at the call that lies: {complaint}"
    );
    assert!(
        !complaint.contains("crossed: not Send"),
        "the program did not run: {complaint}"
    );
}

/// The experiment is three projects and a shim, and a test that drives them by
/// path says nothing useful once one of them has been renamed away.
#[test]
fn the_experiment_is_where_the_tests_say_it_is() {
    for name in [
        "README.md",
        "overlaps.nika",
        "shim/Cargo.toml",
        "shim/src/lib.rs",
        "serve/nikaia.toml",
        "crossing/nikaia.toml",
        "smuggled/nikaia.toml",
    ] {
        assert!(
            experiment().join(name).exists(),
            "examples/foreign-runtime/{name} is missing"
        );
    }
}
