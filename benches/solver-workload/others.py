#!/usr/bin/env python3
"""z3 and cvc5 on the same queries, in-process, for scale (docs/solver-workload.md §5).

Usage: others.py <directory> [v]. Needs the z3 and cvc5 Python packages."""
import z3, cvc5, glob, time, sys, statistics
d = sys.argv[1]; verbose = len(sys.argv) > 2
def run_z3(f):
    s = z3.Solver(); s.from_file(f); return str(s.check())
def run_cvc5(f):
    tm = cvc5.TermManager(); s = cvc5.Solver(tm); s.setOption("produce-models", "false")
    p = cvc5.InputParser(s); p.setFileInput(cvc5.InputLanguage.SMT_LIB_2_6, f)
    sm = p.getSymbolManager(); r = None
    while True:
        cmd = p.nextCommand()
        if cmd.isNull(): break
        out = cmd.invoke(s, sm)
        if 'sat' in out: r = out.strip()
    return r
tot = {'z3': [], 'cvc5': []}
for f in sorted(glob.glob(d + '/*.smt2')):
    line = [f.split('/')[-1]]
    for name, fn in (('z3', run_z3), ('cvc5', run_cvc5)):
        reps = 5 if verbose else 3
        t = time.perf_counter()
        for _ in range(reps): r = fn(f)
        dt = (time.perf_counter() - t) / reps
        tot[name].append(dt); line.append(f"{name} {r} {dt*1e6:.0f}us")
    if verbose: print("  ".join(line))
for k, v in tot.items():
    print(f"{k}: total {sum(v)*1e3:.1f} ms, median {statistics.median(v)*1e6:.0f} us, mean {statistics.mean(v)*1e6:.0f} us")
