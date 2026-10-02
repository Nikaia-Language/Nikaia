#!/usr/bin/env python3
"""How much of the compiler is written in Nikaia (ADR-250 D4).

Stage 1 of ADR-001 D4 is Nikaia compiling Nikaia, and ADR-250 reaches it a
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
# The prover's logic layer is toolchain Rust too (ADR-265 D1): moved out of
# `crates/nikaia/src`, it would otherwise leave the count without anything
# having become Nikaia.
LOGIC = ROOT / "crates" / "nikaia-logic" / "src"
TOOLS = ROOT / "crates" / "nikaia-std" / "src" / "tools"

# The toolchain's Nikaia, and the Rust module each one took the place of or
# serves. `http1.nika` is in the same directory and is not here: it is `std`'s
# HTTP server, which a program runs, and not a piece of the compiler.
COMPILER_NIKA = {
    "spelling.nika": "check: *did you mean* (0.0.238)",
    "dsl.nika": "dsl: a body's parameters, the shadow types, the drivers and the check of a call (0.0.248, 0.0.335)",
    "rust.nika": "describe: reading a crate's Rust (ADR-195)",
    "fixed.nika": "fixed: FNV-1a and CHD (0.0.250)",
    "template.nika": "emit::template: the HTML scan (0.0.252)",
    "ledger.nika": "contracts: reading a ledger back, whole, and writing a signature in its spelling (0.0.258, 0.0.292, 0.0.333)",
    "manifest.nika": "manifest: what nikaia.toml may say (0.0.260)",
    "ast.nika": "ast: the syntax tree (ADR-252, 0.0.275)",
    "fold.nika": "fold: a constant's value (ADR-252 D6, 0.0.278)",
    "trust.nika": "contracts::trust: what --trust says (0.0.281)",
    "ty.nika": "contracts::ty, the ledger's records and what the checker asks of a type (ADR-257, 0.0.285-295)",
    "sources.nika": "describe: the files a crate is read from (0.0.308)",
    "paths.nika": "describe: a crate's module paths and `pub use` (0.0.309)",
    "crossing.nika": "describe: what crosses a thread, and the notes (0.0.311, 0.0.316)",
    "signature.nika": "describe: a Rust signature in the ledger's words (0.0.314, 0.0.316)",
    "surface.nika": "describe: what a crate offers (0.0.315)",
    "findings.nika": "a refusal a check of the toolchain reports, as one record (0.0.320)",
    "traits.nika": "traits: an impl against the trait it implements (0.0.320)",
    "throws.nika": "contracts::throws: the error sets' fixpoint, what a `throw` names, and the walk of what a body throws (0.0.321, 0.0.341)",
    "locks.nika": "contracts::locks: which functions can open a lock, the fixpoint (0.0.322)",
    "touch.nika": "contracts::touch: when two resources force an order, the touches fixpoint (0.0.324)",
    "calls.nika": "contracts::sync::reached: what a call resolves to, for every analysis (0.0.325)",
    "foreign.nika": "the walk of the tree, for foreign, contracts::trust, contracts::locks, contracts::keep, contracts::keeps, contracts::touch and contracts::sync (0.0.334-0.0.343)",
    "names.nika": "contracts::order and contracts::send: every name an expression or a statement mentions (0.0.338, 0.0.340)",
    "specbook.nika": "specbook: the specification's blocks and the readings a block can have (0.0.344)",
    "rustc_words.nika": "diagnostics: a rustc message in the words the program wrote, and how a message is set down (0.0.345, 0.0.346)",
    "parse_notes.nika": "parser: a parse error's reading, the text half (0.0.347)",
    "diffs.nika": "project: what changed in a ledger, and an output test's difference (0.0.350)",
    "types.nika": "types: a name declared twice, a bound naming no trait, a type nothing declares (0.0.354)",
    "views.nika": "views: where a naked view parameter's view ends up, NK2302 and the buffer the emitter names (0.0.355)",
    "render.nika": "diagnostics: a finding laid out on the lines it is about, each place underlined, and NK2202's message (0.0.356, 0.0.358)",
    "boundaries.nika": "diagnostics: a backend mismatch at a boundary said as the stale ledger it is (0.0.358)",
    "threads.nika": "contracts::send: whether a value may cross a thread, to our own code or to code nothing describes (0.0.361)",
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
    rust = sum(code_lines(p) for p in sorted([*RUST.rglob("*.rs"), *LOGIC.rglob("*.rs")]))
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
    print(f"{'Rust, crates/nikaia{,-logic}/src':<44} {rust:>7}")
    for name, what in COMPILER_NIKA.items():
        print(f"{'Nikaia, ' + name:<44} {nika[name]:>7}   {what}")
    print(f"{'Nikaia in all':<44} {sum(nika.values()):>7}   {share:.1f} % of the toolchain")


if __name__ == "__main__":
    main()
