#!/usr/bin/env python3
"""Attribute a callgrind profile's cost to `Ty` (ADR-294 D16).

A function is **in the set** when its name says it clones, drops or compares
something whose type mentions `contracts::ty::Ty` (the build must use v0
mangling, which keeps generic arguments in the name). The cost charged to the
set is the inclusive cost of every call **from outside the set into it**, so a
clone inside a clone is not counted twice. What the optimiser inlined into a
caller outside the set is not a call and is not counted (ADR-294 E6).

Two sets are reported:

* `narrow` - the functions whose own type is `Ty`: `<Ty as Clone>::clone`,
  `drop_in_place::<Ty>`, `<Ty as PartialEq>::eq`/`ne`.
* `broad` - also every container of it: `Vec<Ty>`, `Box<Ty>`,
  `(String, Ty)`, `Signature`, ... A `(String, Ty)`'s `String` is charged too,
  which interning would not remove, so this is an upper bound.

Usage: attribute.py <callgrind.out> -> JSON on stdout.
"""
import json
import re
import sys

TY = "contracts::ty::Ty"
LEDGER = re.compile(r"contracts::(Signature|FnContract|TypeContract|FieldContract|VariantContract|ConfigContract|Ledger)\b")

def kind(name):
    if "drop_in_place" in name:
        return "drop"
    if re.search(r"as core::clone::Clone>::clone", name):
        return "clone"
    if re.search(r"as core::cmp::PartialEq(<[^>]*>)?>::(eq|ne)", name):
        return "eq"
    return None

def own_type_is_ty(name):
    """`<Ty as …>` or `drop_in_place::<Ty>` - the function is about a `Ty` itself."""
    n = re.sub(r"'\d+$", "", name.split(" [")[0].strip())
    return bool(re.search(r"<nikaia::contracts::ty::Ty as core::", n)
                or re.search(r"drop_in_place::<nikaia::contracts::ty::Ty>$", n))

def in_set(name, broad):
    k = kind(name)
    if k is None:
        return None
    if own_type_is_ty(name):
        return k
    if broad and (TY in name or LEDGER.search(name)):
        return k
    return None

def main(path):
    events = None
    total = None
    fn = None
    cfn = None
    pending_call = None
    arcs = []  # (caller, callee, calls, costs)
    fn_self = {}
    with open(path, errors="replace") as f:
        for line in f:
            line = line.rstrip("\n")
            if line.startswith("events:"):
                events = line.split()[1:]
                continue
            if line.startswith("totals:") or line.startswith("summary:"):
                total = [int(x) for x in line.split()[1:]]
                continue
            if line.startswith("fn="):
                fn = line[3:]
                continue
            if line.startswith("cfn="):
                cfn = line[4:]
                continue
            if line.startswith("calls="):
                pending_call = int(line[6:].split()[0])
                continue
            if line and (line[0].isdigit() or line[0] in "+-*"):
                parts = line.split()
                costs = [int(x) for x in parts[1:]]
                costs += [0] * (len(events) - len(costs))
                if pending_call is not None:
                    arcs.append((fn, cfn, pending_call, costs))
                    pending_call = None
                else:
                    acc = fn_self.setdefault(fn, [0] * len(events))
                    for i, c in enumerate(costs):
                        acc[i] += c
    out = {"events": events, "total": total}
    for label, broad in (("narrow", False), ("broad", True)):
        by_kind = {}
        for caller, callee, calls, costs in arcs:
            k = in_set(callee, broad)
            if k is None or in_set(caller, broad) is not None:
                continue
            entry = by_kind.setdefault(k, {"calls": 0, "cost": [0] * len(events)})
            entry["calls"] += calls
            for i, c in enumerate(costs):
                entry["cost"][i] += c
        # malloc/free reached from inside the set, counted by calls
        out[label] = by_kind
    alloc_calls = 0
    for caller, callee, calls, costs in arcs:
        if re.search(r"(^|[^\w])(malloc|__rust_alloc|__libc_malloc)\b", callee) and not re.search(r"(^|[^\w])(malloc|__rust_alloc|__libc_malloc)\b", caller or ""):
            alloc_calls += calls
    out["malloc_calls"] = alloc_calls
    # the phases, inclusive, by the arcs into them from outside
    phases = {
        "ledger_parse": r"<nikaia::contracts::Ledger>::parse$",
        "parse_to_ast": r"nikaia::parser::parse_to_ast$",
        "check": r"nikaia::check::check\w*$",
        "ty_parse": r"<nikaia::contracts::ty::Ty>::parse$",
    }
    for name, pat in phases.items():
        cost = [0] * len(events)
        rx = re.compile(pat)
        for caller, callee, calls, costs in arcs:
            c = re.sub(r"'\d+$", "", callee.split(" [")[0].strip())
            k = re.sub(r"'\d+$", "", (caller or "").split(" [")[0].strip())
            if rx.search(c) and not rx.search(k):
                for i, x in enumerate(costs):
                    cost[i] += x
        out[name] = cost
    json.dump(out, sys.stdout, indent=1)

if __name__ == "__main__":
    main(sys.argv[1])
