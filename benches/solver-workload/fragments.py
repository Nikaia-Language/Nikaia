#!/usr/bin/env python3
"""Which fragment each query falls in (docs/solver-workload.md §3).

Usage: fragments.py <directory of .smt2 files>. Needs the z3 Python package."""
# Which fragment each query falls in, read from the SMT-LIB text: an atom is
# an interval atom (one variable), a difference atom (x - y with unit
# coefficients) or general.
import re, sys, glob, collections, z3
d = sys.argv[1]
kinds = collections.Counter(); atoms_kind = collections.Counter(); coef = collections.Counter()
for f in sorted(glob.glob(d + '/*.smt2')):
    s = z3.Solver(); s.from_file(f)
    worst = 0
    def visit(e):
        global worst
        if z3.is_app(e) and e.decl().kind() in (z3.Z3_OP_LE, z3.Z3_OP_GE, z3.Z3_OP_LT, z3.Z3_OP_GT, z3.Z3_OP_EQ) and e.arg(0).sort() == z3.IntSort():
            lin = z3.simplify(e.arg(0) - e.arg(1), som=True)
            vs = {}
            def lv(t, k=1):
                if z3.is_int_value(t): return
                if z3.is_add(t):
                    for c in t.children(): lv(c, k)
                elif z3.is_mul(t) and z3.is_int_value(t.arg(0)): lv(t.arg(1), k * t.arg(0).as_long())
                elif z3.is_const(t): vs[str(t)] = vs.get(str(t), 0) + k
                else: vs['?'] = 99
            lv(lin)
            vs = {k: v for k, v in vs.items() if v}
            for v in vs.values(): coef[min(abs(v), 3)] += 1
            k = 0 if len(vs) <= 1 else 1 if len(vs) == 2 and sorted(vs.values()) == [-1, 1] else 2
            atoms_kind[k] += 1; worst = max(worst, k)
            return
        for c in e.children(): visit(c)
    for a in s.assertions(): visit(a)
    kinds[worst] += 1
names = {0: 'interval', 1: 'difference', 2: 'general'}
print('queries by hardest atom:', {names[k]: v for k, v in sorted(kinds.items())})
print('atoms:', {names[k]: v for k, v in sorted(atoms_kind.items())})
print('coefficient magnitudes (3 = 3 or more):', dict(sorted(coef.items())))
