#!/usr/bin/env python3
"""The stress families of docs/solver-workload.md, written as SMT-LIB 2.

Usage: stress.py <directory>. Each file's name says what z3 answers:
`-unsat` (the solver should prove it) or `-sat` (it should refute it).
"""
import os, sys

def write(directory, name, names, assertions):
    with open(os.path.join(directory, name + '.smt2'), 'w') as out:
        out.write('(set-logic QF_LIA)\n')
        for n in names:
            out.write(f'(declare-const {n} Int)\n')
        for a in assertions:
            out.write(f'(assert {a})\n')
        out.write('(check-sat)\n')

def main(directory):
    os.makedirs(directory, exist_ok=True)
    for k in (4, 8, 12, 16, 24, 32):
        # Guards: x in [0, k] and x != 0 .. k - 1 leave x = k.
        guards = [f'(not (= x {i}))' for i in range(k)]
        write(directory, f'guards{k:02}-unsat', ['x'],
              ['(>= x 0)', f'(<= x {k})'] + guards + [f'(not (= x {k}))'])
        write(directory, f'guards{k:02}-sat', ['x'],
              ['(>= x 0)', f'(<= x {k + 1})'] + guards + [f'(not (= x {k}))'])
        # Disjunctions a proof of x > 3 from x > 5 does not need.
        ys = [f'y{i}' for i in range(k)]
        write(directory, f'irrelevant{k:02}-unsat', ['x'] + ys,
              ['(> x 5)'] + [f'(not (= {y} 0))' for y in ys] + ['(not (> x 3))'])
        # A path of steps, each +1 or +2, ends at least k past its start.
        zs = [f'z{i}' for i in range(k + 1)]
        steps = [f'(or (= z{i + 1} (+ z{i} 1)) (= z{i + 1} (+ z{i} 2)))' for i in range(k)]
        write(directory, f'chain{k:02}-unsat', zs,
              ['(= z0 0)'] + steps + [f'(not (>= z{k} {k}))'])
    for k in (4, 6, 8, 10, 12):
        # k numbers, each -1 or 1, never sum to 1 when k is even.
        xs = [f'x{i}' for i in range(k)]
        each = [f'(and (>= {x} (- 1)) (<= {x} 1) (not (= {x} 0)))' for x in xs]
        write(directory, f'parity{k:02}-unsat', xs, each + [f'(= (+ {" ".join(xs)}) 1)'])

if __name__ == '__main__':
    main(sys.argv[1])
