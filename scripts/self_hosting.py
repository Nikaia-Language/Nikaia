#!/usr/bin/env python3
"""How much of the compiler is written in Nikaia (ADR-294 D4).

Stage 1 of ADR-001 D4 is Nikaia compiling Nikaia, and ADR-294 reaches it a
module at a time. This counts where the road stands: the lines of the
toolchain that are Rust (`crates/nikaia/src`), the lines that are Nikaia (the
`.nika` files under `crates/nikaia-std/src/tools/` the toolchain calls), and the
share of the second in both.

A line is one that says something: blank lines and lines that are only a
comment are not counted in either language, so a well-commented module does
not move the number and a moved one is measured by its code.

Usage:
    scripts/self_hosting.py           print the table and the share
    scripts/self_hosting.py --share   print the share alone, as `N.N %`
"""

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RUST = ROOT / "crates" / "nikaia" / "src"
TOOLS = ROOT / "crates" / "nikaia-std" / "src" / "tools"

# The toolchain's Nikaia, and the Rust module each one took the place of or
# serves. `http1.nika` is in the same directory and is not here: it is `std`'s
# HTTP server, which a program runs, and not a piece of the compiler.
COMPILER_NIKA = {
    "spelling.nika": "check: *did you mean* (0.0.238)",
    "dsl.nika": "dsl: a body's parameters, the shadow types, the drivers and the check of a call (0.0.248, 0.0.335)",
    "rust.nika": "describe: reading a crate's Rust (ADR-290)",
    "fixed.nika": "fixed: FNV-1a and CHD (0.0.250)",
    "template.nika": "emit::template: the HTML scan (0.0.252)",
    "ledger.nika": "contracts: reading a ledger back, whole, and writing a signature in its spelling (0.0.258, 0.0.292, 0.0.333)",
    "manifest.nika": "manifest: what nikaia.toml may say (0.0.260)",
    "ast.nika": "ast: the syntax tree (ADR-294, 0.0.275)",
    "fold.nika": "fold: a constant's value (ADR-294 D11, 0.0.278), on integers.nika's arithmetic (0.0.420)",
    "trust.nika": "contracts::trust: what --trust says (0.0.281), and which maps keep the fast hash (0.0.743)",
    "ty.nika": "contracts::ty, the ledger's records and what the checker asks of a type (ADR-294, 0.0.285-295)",
    "sources.nika": "describe: the files a crate is read from (0.0.308)",
    "paths.nika": "describe: a crate's module paths and `pub use` (0.0.309)",
    "crossing.nika": "describe: what crosses a thread, and the notes (0.0.311, 0.0.316)",
    "signature.nika": "describe: a Rust signature in the ledger's words (0.0.314, 0.0.316)",
    "surface.nika": "describe: what a crate offers (0.0.315)",
    "findings.nika": "a refusal a check of the toolchain reports, as one record (0.0.320)",
    "traits.nika": "traits: an impl against the trait it implements (0.0.320)",
    "throws.nika": "contracts::throws: the error sets' fixpoint, what a `throw` names, and the walk of what a body throws (0.0.321, 0.0.341), and the graph over a unit with the checker's method calls (0.0.396)",
    "locks.nika": "contracts::locks: which functions can open a lock, the fixpoint (0.0.322), the walk, the doors and the graph over a unit (0.0.395)",
    "touch.nika": "contracts::touch: when two resources force an order, the touches fixpoint (0.0.324), what a body reaches and the graph over a unit (0.0.397)",
    "calls.nika": "contracts::sync::reached: what a call resolves to, for every analysis (0.0.325)",
    "foreign.nika": "the walk of the tree, for foreign, contracts::trust, contracts::locks, contracts::keep, contracts::keeps, contracts::touch and contracts::sync (0.0.334-0.0.343)",
    "names.nika": "contracts::order and contracts::send: every name an expression or a statement mentions (0.0.338, 0.0.340)",
    "specbook.nika": "specbook: the specification's blocks and the readings a block can have (0.0.344)",
    "rustc_words.nika": "diagnostics: a rustc message in the words the program wrote, and how a message is set down (0.0.345, 0.0.346)",
    "parse_notes.nika": "parser: a parse error's reading, the text half (0.0.347)",
    "diffs.nika": "project: what changed in a ledger, and an output test's difference (0.0.350)",
    "types.nika": "types: a name declared twice, a bound naming no trait, a type nothing declares (0.0.354)",
    "views.nika": "views: where a naked view parameter's view ends up, NK2302 and the buffer the emitter names (0.0.355)",
    "render.nika": "diagnostics: a finding laid out on the lines it is about, each place underlined, and NK2202's message (0.0.356, 0.0.358), a relayed rustc message, laid out (0.0.748)",
    "boundaries.nika": "diagnostics: a backend mismatch at a boundary said as the stale ledger it is (0.0.358)",
    "threads.nika": "contracts::send: whether a value may cross a thread, to our own code or to code nothing describes (0.0.361)",
    "order.nika": "contracts::order: a statement reduced to an operation or the reason it cannot be, and whether two statements, or a run of them, keep their order, and the `--overlaps` report (0.0.362-0.0.364)",
    "tether.nika": "contracts::tether: the states a signature's views solve to, a parse that views its input, a body that makes a buffer of its own (0.0.374), the --tethers report (0.0.752)",
    "sharing.nika": "contracts::sharing: the reasons a count stays atomic, the decisions and their report, the slot keys, what holds a `Shared` and what a hull lowers to, the classes the handles join and what each one gets, and what the walk asks of one expression (0.0.375, 0.0.377, 0.0.387), and the walk that joins every handle into its allocation class (0.0.401)",
    "keep.nika": "contracts::keep: which types hold a view and why one cannot go into a handle, the methods that keep, drop, take or own, the keeping method's key, a callee's and a local's name (0.0.376)",
    "modules.nika": "modules: the names a file declares, and the refusal for a `use` that names a path, a name twice, or a package the file cannot reach (0.0.379); what a build says about its `pub` functions' pausing, and a test build's dispatcher (0.0.409)",
    "tiers.nika": "text_tiers: what a method's name says about the text it hands back, and which containers text is followed into (0.0.383)",
    "lends.nika": "contracts::keeps: whether a callee lends a parameter, whether a ledger says a type copies, and whether a value moves (0.0.384), and the least fixpoint of what each function keeps (0.0.399)",
    "parse_numbers.nika": "parser: a number as it is read - the four spellings, the separator, a scale - and every way it is refused (0.0.744)",
    "interpreter.nika": "interpreter: what `nikaia interpret` walks and says of `main` (0.0.744)",
    "parse_errors.nika": "parser: a parse error in the reader's words - what was expected, the mistake where it was made, the notes (0.0.747)",
    "tree_build.nika": "parser: what the grammar's actions build the tree with - the postfix steps, the folds of a head and its tails, a lambda's parameters, a guarded jump (0.0.749)",
    "parsed_tree.nika": "parser: the aliases a file declares, and a file's declared types with their views written back as lists (0.0.751)",
    "libraries.nika": "libraries: the linker flags a package may be handed from `pkg-config`'s words, and the flags as `rustc` takes them (0.0.742)",
    "assets.nika": "assets: why a read at build time is refused and the way out, the allowlist's lines, a path that leaves the root, the entries nothing read (0.0.389)",
    "sync.nika": "contracts::sync: the greatest fixpoint of which functions keep their claim to be `sync`, the ones only their lambdas pause, and the shortest way to a pause (0.0.390), the walk of what a body does to its claim and the check of what a `sync` function may not call (0.0.398)",
    "escapes.nika": "build_time: what the `\\` in a written literal means, the escape nothing names, and a value spelled back as a literal (0.0.391)",
    "dump.nika": "grammar_run: the encoder a grammar run at build time prints its result with, written from the declaration (0.0.392)",
    "describe.nika": "describe: what a description holds - the entries, the types they name, what the describer saw and did not claim (0.0.394)",
    "keeps.nika": "contracts::keeps: what a body does with each of its parameters - the walk, what one expression keeps, and the method candidates (0.0.400)",
    "buffers.nika": "contracts::keep: where each buffer lives - the walk that follows a view to every place it leaves, and the plan of keeps and refusals (0.0.404)",
    "text_tiers.nika": "text_tiers: what a declared `String` is below - the walk of every value's kind of text into the positions it reaches, the fixpoint over them, and where a value is handed into a mixed one; and a name's alias spelled out (0.0.405)",
    "ledger_text.nika": "contracts: a ledger as it is written out - `nikaia.contracts`, the record of what it was derived from, and a description's header - and the `throws` list a diagnostic quotes (0.0.406)",
    "declared.nika": "contracts: what a declaration says before any body is read - the type a `.nika` declaration names, and the entry a function's and a trait method's declaration makes (0.0.407, 0.0.408), and an enum's cases and a struct's fields (0.0.745)",
    "ledger_items.nika": "contracts: what the ledger's item loop reads off a declaration - the types a file declares, an impl head's parameters, a literal, the entries of an extern block and a grammar, a build-time value as the literal both languages spell alike, and the ensures of an expression function (0.0.750)",
    "ledger_ops.nika": "contracts: what a build does with a package's ledger as a whole - the entries it publishes, whether it may be believed against its sources, and taking it in under the names a caller writes (0.0.408)",
    "written.nika": "check: an expression or a pattern written back the way the source wrote it, for an `assert`'s failure and a refusal's quote (0.0.410)",
    "check_words.nika": "check: the words its refusals are made of - an article, a count, a list of names, the nearest spelling, the note on a word another language reserves - and which lookups lend their key (0.0.411)",
    "check_types.nika": "check: what it asks of one type by itself - whether it moves or copies, a view of it and what a view is of, what a hull holds and what goes into one, lists, collections and text, and the way out of a mismatch (0.0.412), and what a `for` binds (0.0.415)",
    "check_calls.nika": "check: what it reads off a callee's signature - the arguments a call passes, what the receiver's and the arguments' types bind, and what a sequence a call hands back is as a whole (0.0.415)",
    "integers.nika": "build_time: an integer while the program is built - a magnitude and a sign - and what the evaluator does with two of them (0.0.416), asked with `checked_` (ADR-315, 0.0.420)",
    "build_values.nika": "build_time: what a build-time expression comes to - a number, a truth value, text, a list, a tuple, a variant or a struct - and what an operator and an index make of them (0.0.417)",
    "check_numbers.nika": "check: a number and the type it has to fit - NK1116, said of an Integer and of a fold past the 65 bits (0.0.426)",
    "bounds_basic.nika": "bounds: the loop over a list's own length that makes an index check unneeded (ADR-306 D3), which methods keep a list's length, and whether a body binds a name again (0.0.428, 0.0.429)",
    "bounds_body.nika": "bounds: what a loop's body does - a break that leaves it, the lists a turn pushes onto once, and the names a deferred body changes (ADR-306 D4, 0.0.431)",
    "bounds_shape.nika": "bounds: what the aggressive walk reads off a body once - its constants, unsigned names, the lists whose every write it sees and their aliases (ADR-306 D7, 0.0.436)",
    "cexport_model.nika": "cexport: the entry points of a library C calls and the shapes that cross the boundary, read off the declarations (0.0.746)",
    "cexport.nika": "cexport: the wrapper around each entry point and the header (0.0.746)",
    "cexport_mirror.nika": "cexport: an `extern` struct or enum as C lays it out (0.0.746)",
    "cexport_python.nika": "cexport: `nikaia bind python`'s ctypes module (0.0.746)",
    "cexport_node.nika": "cexport: `nikaia bind node`'s entries, module and binding.gyp (0.0.746)",
    "cexport_node_text.nika": "cexport: the fixed C of the N-API module (0.0.746)",
    "logic_lia.nika": "logic: the reference solver's types, the bounds of a query, the elimination and the certificate it ends on (0.0.754)",
    "logic_search.nika": "logic: the search with a split on demand, the checker, and models (0.0.754)",
    "logic_normal.nika": "logic: a query in normal form, certificates and models as text (0.0.754)",
    "logic_smtlib.nika": "logic: a query as SMT-LIB 2, and a script read back (0.0.754)",
    "logic_alethe.nika": "logic: a proof as Alethe (0.0.754)",
    "grammar_decode.nika": "grammar_run: what a grammar run at build time printed read back, and the text of the sub-program and its manifest (0.0.755)",
    "comptime_driver.nika": "comptime_run: the sub-program's main and its site table, and the bundle it links against (0.0.757)",
    "build_eval.nika": "build_time: the evaluator of a build-time expression - calls, loops, text, structs, variants and a grammar run (0.0.760)",
    "bounds_matched.nika": "bounds: what a grammar alternative's bindings matched, and what a callee's ledger entry ensures (0.0.763)",
    "proofs_book.nika": "proofs: nikaia.proofs as text, read back and rendered (0.0.763)",
    "check_state.nika": "check: the records the checker's state holds - a name in scope, what was taken or read of a path, a way out, a body that repeats - and the questions each answers (0.0.774)",
    "check_walk.nika": "check: the expressions and the blocks an expression or a statement holds, for the checker's walks (0.0.774)",
    "check_decls.nika": "check: what the program declares - structs, enums, bounds, grammars - and what a type is called to the checker (0.0.775)",
    "check_imports.nika": "check: what a `use` may name, a grammar's names and a `sync(f)` (0.0.775)",
    "check_scope.nika": "check: the names in scope and the function a written name resolves to (0.0.775)",
    "check_refusals.nika": "check: refusals that need a name, a type or two and nothing else of the walk (0.0.775)",
    "check_names.nika": "check: a reserved word used as a name, a module used without its line, a bit operator on something with no bits (0.0.775)",
    "check_patterns.nika": "check: what a `match` arm's pattern binds, and to what type (0.0.775)",
    "check_values.nika": "check: whether a value is taken away or copied, and a value handed to a callee that keeps it (0.0.775)",
    "check_access.nika": "check: what a name reaches - private fields, options, struct literals, patterns that bind unevenly (0.0.775)",
    "check_literals.nika": "check: tables, an index computed at build time, a thrown value and a C handle (0.0.775)",
    "check_buildtime.nika": "check: values built at build time, lists, constructors and the members of a reflected field or variant (0.0.775)",
    "check_with.nika": "check: `asset`, `with`, a missing field, a `catch` over nothing and a statement after a jump (0.0.775)",
    "check_shapes.nika": "check: the shape of an expression, a pattern or a type - where a place roots, what a `const` is called, whether a block leaves (0.0.775)",
    "check_lookups.nika": "check: what a name in an expression stands for, and how the checker learns what the program and `std` declare (0.0.775)",
    "check_boundary.nika": "check: what a declared type is called below, what a field of a `const` may be, where a `??` lends (0.0.775)",
}


def code_lines(path):
    count = 0
    block = False
    for line in path.read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if block:
            if "*/" in stripped:
                block = False
            continue
        if not stripped or stripped.startswith("//"):
            continue
        if stripped.startswith("/*"):
            block = "*/" not in stripped
            continue
        count += 1
    return count


def measure():
    rust = sum(code_lines(p) for p in sorted(RUST.rglob("*.rs")))
    nika = {name: code_lines(TOOLS / name) for name in COMPILER_NIKA}
    total = rust + sum(nika.values())
    share = 100.0 * sum(nika.values()) / total if total else 0.0
    return rust, nika, share


def share_text():
    return f"{measure()[2]:.1f} %"


def main():
    rust, nika, share = measure()
    if "--share" in sys.argv[1:]:
        print(f"{share:.1f} %")
        return
    print(f"{'Rust, crates/nikaia/src':<44} {rust:>7}")
    for name, what in COMPILER_NIKA.items():
        print(f"{'Nikaia, ' + name:<44} {nika[name]:>7}   {what}")
    print(f"{'Nikaia in all':<44} {sum(nika.values()):>7}   {share:.1f} % of the toolchain")


if __name__ == "__main__":
    main()
