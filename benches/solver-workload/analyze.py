#!/usr/bin/env python3
"""Distributions over the CSV the instrumented solver writes (docs/solver-workload.md §2).

Usage: analyze.py <measure.csv> <directory of the queries>."""
import csv, sys, re, statistics, hashlib, collections, os
rows = list(csv.DictReader(open(sys.argv[1])))
qdir = sys.argv[2]
I = lambda r,k: int(r[k])
def dist(name, vals):
    v = sorted(vals); n=len(v)
    q = lambda p: v[min(n-1, int(p*(n-1)+0.5))]
    print(f"  {name:<22} min {v[0]:>7}  median {q(.5):>7}  p90 {q(.9):>7}  p99 {q(.99):>7}  max {v[-1]:>7}  mean {sum(v)/n:>9.1f}")
print(f"queries {len(rows)}", collections.Counter(r['answer'] for r in rows))
for k in ['vars','atoms','ors','alts','nodes','fm_calls','elims','combines','max_held','bounds_made']:
    dist(k, [I(r,k) for r in rows])
# density
dens_atoms = [I(r,'nz_atoms')/(I(r,'atoms')*I(r,'vars')) for r in rows if I(r,'atoms') and I(r,'vars')]
dens_made = [I(r,'nz_made')/(I(r,'bounds_made')*I(r,'vars')) for r in rows if I(r,'bounds_made') and I(r,'vars')]
nz_per_atom = [I(r,'nz_atoms')/I(r,'atoms') for r in rows if I(r,'atoms')]
print(f"  nonzeros per atom: mean {statistics.mean(nz_per_atom):.2f}, max {max(nz_per_atom):.2f}")
print(f"  density atoms (nz/(atoms*vars)): median {statistics.median(dens_atoms):.2f}, mean {statistics.mean(dens_atoms):.2f}, min {min(dens_atoms):.2f}")
if dens_made: print(f"  density derived bounds: median {statistics.median(dens_made):.2f}, mean {statistics.mean(dens_made):.2f}, n={len(dens_made)}")
tot = {k: sum(I(r,k) for r in rows) for k in ['read_ns','index_ns','solve_ns','verify_ns','fm_ns']}
search = tot['solve_ns'] - tot['index_ns']
print(f"time totals (us): read {tot['read_ns']/1e3:.0f}, index {tot['index_ns']/1e3:.0f}, solve(all) {tot['solve_ns']/1e3:.0f} [FM {tot['fm_ns']/1e3:.0f}, search rest {(search-tot['fm_ns'])/1e3:.0f}], verify {tot['verify_ns']/1e3:.0f}")
dist('solve_ns', [I(r,'solve_ns') for r in rows])
# top 5 by solve
for r in sorted(rows, key=lambda r: -I(r,'solve_ns'))[:5]:
    print("   slow:", r['file'], r['answer'], 'vars',r['vars'],'atoms',r['atoms'],'ors',r['ors'],'nodes',r['nodes'],'ns',r['solve_ns'])
share = sorted([I(r,'solve_ns') for r in rows], reverse=True)
top = sum(share[:max(1,len(share)//10)])/sum(share)
print(f"  slowest 10% of queries take {100*top:.0f}% of solve time")
# duplicates
texts = collections.Counter(); canon = collections.Counter()
for r in rows:
    t = open(os.path.join(qdir, r['file'])).read()
    body = "\n".join(l for l in t.splitlines() if not l.startswith('(declare'))
    texts[body]+=1
    names = {}
    def sub(m):
        s=m.group(0)
        if s in ('and','or','not','assert','check-sat','set-logic','QF_LIA','true','false') or s.isdigit(): return s
        names.setdefault(s, f"v{len(names)}"); return names[s]
    canon[re.sub(r"\|[^|]*\||[A-Za-z_][A-Za-z0-9_.!?$%&*+<>=/~^@-]*", sub, body)] += 1
print(f"distinct queries: exact {len(texts)} of {len(rows)}, up to renaming {len(canon)}")
