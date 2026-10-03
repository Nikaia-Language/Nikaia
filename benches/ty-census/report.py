#!/usr/bin/env python3
"""ADR-294: one row per program from run.sh's profiles.

Usage: report.py <out-dir>  (the directory run.sh wrote)
"""
import glob
import json
import os
import subprocess
import sys

here = os.path.dirname(os.path.abspath(__file__))
out = sys.argv[1]

def attribute(path):
    text = subprocess.check_output([sys.executable, os.path.join(here, "attribute.py"), path])
    return json.loads(text)

def lines_of(name):
    log = os.path.join(out, name + ".log")
    return None

rows = []
for cg in sorted(glob.glob(os.path.join(out, "*.cg"))):
    name = os.path.basename(cg)[:-3]
    a = attribute(cg)
    ev = a["events"]
    def s(d, key="cost"):
        return [sum(v[key][i] for v in d.values()) for i in range(len(ev))] if key == "cost" else sum(v[key] for v in d.values())
    rows.append({
        "name": name.replace(".nika", ""),
        "events": ev,
        "total": a["total"],
        "narrow": s(a["narrow"]),
        "broad": s(a["broad"]),
        "narrow_calls": {k: v["calls"] for k, v in a["narrow"].items()},
        "ledger_parse": a["ledger_parse"],
        "parse": a["parse_to_ast"],
        "check": a["check"],
        "ty_parse": a["ty_parse"],
        "malloc_calls": a["malloc_calls"],
    })

base = next((r for r in rows if r["name"].endswith("empty")), None)
json.dump(rows, open(os.path.join(out, "rows.json"), "w"), indent=1)

def pct(a, b):
    return f"{100.0 * a / b:5.2f}" if b else "  -  "

print("| program | Ir | above empty | `std.contracts` read | Ty narrow | Ty broad | narrow, above empty | broad, above empty |")
print("| :--- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |")
for r in sorted(rows, key=lambda r: r["total"][0]) if base else []:
    t = r["total"][0]
    above = t - base["total"][0]
    na = r["narrow"][0] - base["narrow"][0]
    ba = r["broad"][0] - base["broad"][0]
    print(f"| {r['name']} | {t/1e6:.1f} M | {above/1e6:.1f} M | {pct(r['ledger_parse'][0], t)} % "
          f"| {pct(r['narrow'][0], t)} % | {pct(r['broad'][0], t)} % | {pct(na, above)} % | {pct(ba, above)} % |")
if len(rows[0]["events"]) > 1:
    print()
    print("| program | event | total | Ty narrow | share | Ty broad | share |")
    print("| :--- | :--- | ---: | ---: | ---: | ---: | ---: |")
    for r in sorted(rows, key=lambda r: r["total"][0]):
        for i, e in enumerate(r["events"]):
            if e in ("Ir", "D1mr", "DLmr", "D1mw", "DLmw"):
                print(f"| {r['name']} | {e} | {r['total'][i]} | {r['narrow'][i]} | {pct(r['narrow'][i], r['total'][i])} % | {r['broad'][i]} | {pct(r['broad'][i], r['total'][i])} % |")
