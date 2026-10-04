#!/usr/bin/env python3
"""How long a program is, in tokens, counted the same way for Rust and Nikaia.

A token is an identifier or keyword, a number, a literal (a string is one, an
`f"..."` too; a character is one), or an operator, where `::`, `->`, `==`,
`..<`, `??`, `?.` and the like are one each. Comments and whitespace are not
tokens; everything else is - `use` lines, attributes, a `main` that reads its
argument. Lines are not counted: they measure the formatter as much as the
program.

Usage:
    scripts/tokens.py FILE...     print each file's count
"""
import re, sys

OPS = sorted(["::", "->", "=>", "==", "!=", "<=", ">=", "&&", "||", "..=", "..<", "..",
              "+=", "-=", "*=", "/=", "%=", "<<", ">>", "??", "?.", "|>"], key=len, reverse=True)

def tokens(src):
    out, i, n = [], 0, len(src)
    while i < n:
        c = src[i]
        if c.isspace(): i += 1; continue
        if src.startswith("//", i):
            j = src.find("\n", i); i = n if j < 0 else j; continue
        if src.startswith("/*", i):
            j = src.find("*/", i + 2); i = n if j < 0 else j + 2; continue
        m = re.match(r'(f|b|r#*)?"', src[i:])
        if m:
            j = i + len(m.group(0))
            hashes = m.group(1).count("#") if m.group(1) and m.group(1).startswith("r") else 0
            close = '"' + "#" * hashes
            while j < n:
                if src[j] == "\\" and not hashes: j += 2; continue
                if src.startswith(close, j): j += len(close); break
                j += 1
            out.append(src[i:j]); i = j; continue
        m = re.match(r"b?'(\\.|[^'\\])'", src[i:])
        if m: out.append(m.group(0)); i += len(m.group(0)); continue
        m = re.match(r"'[A-Za-z_]\w*", src[i:])          # a Rust lifetime
        if m: out.append(m.group(0)); i += len(m.group(0)); continue
        m = re.match(r"[A-Za-z_]\w*", src[i:])
        if m: out.append(m.group(0)); i += len(m.group(0)); continue
        m = re.match(r"\d[\d_]*(\.\d[\d_]*)?([eE][+-]?\d+)?\w*", src[i:])
        if m: out.append(m.group(0)); i += len(m.group(0)); continue
        for op in OPS:
            if src.startswith(op, i): out.append(op); i += len(op); break
        else:
            out.append(c); i += 1
    return out

if __name__ == "__main__":
    for f in sys.argv[1:]:
        print(f"{len(tokens(open(f).read())):6d}  {f}")
