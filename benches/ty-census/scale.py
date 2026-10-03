#!/usr/bin/env python3
"""ADR-294 E1: one program made k times larger, so that size changes and the
kind of code does not.

Everything from the first declaration to `fn main` is copied k times, and each
copy's declared names (`struct`, `enum`, `fn`) get a suffix; `main` stays once
and uses the first copy. Every copy is checked and lowered, as unused code is.

Usage: scale.py <program.nika> <k> > <out.nika>

**What this overstates**: every copy holds the same types, so a count of
distinct types does not grow with k, and a real program's would.
"""
import re
import sys

path, k = sys.argv[1], int(sys.argv[2])
lines = open(path).read().split("\n")
decl = re.compile(r"^(pub )?(struct|enum|fn|impl) ")
first = next(i for i, l in enumerate(lines) if decl.match(l))
main = next(i for i, l in enumerate(lines) if re.match(r"^fn main\(", l))
region = "\n".join(lines[first:main])
names = sorted(set(re.findall(r"^(?:pub )?(?:struct|enum|fn) (\w+)", region, re.M)), key=len, reverse=True)
out = lines[:first]
out.append(region)
for c in range(1, k):
    copy = region
    for n in names:
        copy = re.sub(rf"\b{n}\b", f"{n}_c{c}", copy)
    out.append(copy)
out.extend(lines[main:])
print("\n".join(out))
