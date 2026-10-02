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
TOOLS = ROOT / "crates" / "nikaia-std" / "src" / "tools"

# The toolchain's Nikaia, and the Rust module each one took the place of or
# serves. `http1.nika` is in the same directory and is not here: it is `std`'s
# HTTP server, which a program runs, and not a piece of the compiler.
COMPILER_NIKA = {
    "spelling.nika": "check: *did you mean* (0.0.238)",
    "dsl.nika": "dsl: a body's parameters (0.0.248)",
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
    "throws.nika": "contracts::throws: the error sets' fixpoint, what a `throw` names (0.0.321)",
    "locks.nika": "contracts::locks: which functions can open a lock, the fixpoint (0.0.322)",
    "touch.nika": "contracts::touch: when two resources force an order, the touches fixpoint (0.0.324)",
    "calls.nika": "contracts::sync::reached: what a call resolves to, for every analysis (0.0.325)",
    "foreign.nika": "foreign: every qualified name a unit writes, the first walk of the tree (0.0.334)",
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
