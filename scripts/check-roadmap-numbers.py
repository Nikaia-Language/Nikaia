#!/usr/bin/env python3
"""The roadmap's percentages are counted from its own boxes, or they are a mood.

`docs/project_status_and_roadmap.md` opens with a table saying how far along four
areas are. That table is the one thing on the page nothing else would catch going
stale: every box below it is a sentence somebody edits, and the number at the top
is a sentence nobody re-derives. The file's own history is the argument — the
order used to live twice, and the copy that went stale is the one that was not
checked.

So this recounts the boxes and compares. A box is `[x]` done, `[~]` part-built
(a half) or `[ ]` open; which area it belongs to is decided by the headings the
table itself names, and a box this cannot place is an error rather than a
silently-absent one.
"""

import re
import sys
from pathlib import Path

ROADMAP = Path(__file__).resolve().parent.parent / "docs" / "project_status_and_roadmap.md"

# The three areas that are named item by item. Everything else is the language,
# which is most of the page — listing *it* would mean editing this file for every
# box the compiler gains, and the point of the script is that it needs no upkeep.
AREAS = {
    "Libraries": [
        "A database is reachable",
        "The query DSL",
        "The runtime's second half",
        "Standard Library",
    ],
    "Tools": [
        "Orchestrator",
        "Incremental Compilation",
        "`nikaia describe",
        "`nikaia test",
        "`nikaia fmt",
        "`nikaia doc",
        "LSP Server",
        "Self-hosting",
    ],
    "Extended targets": [
        "A library for other languages",
        "A WebAssembly library",
        "A Python binding",
        "A target without an operating system",
    ],
}
LANGUAGE = "The language"

BOX = re.compile(r"^\*   \[( |x|~)\] (.*)$")
ROW = re.compile(r"^\| \*\*(.+?)\*\*.*?\| \*\*([\d.]+) %\*\* \| ([\d.]+) of (\d+) \|")


def area_of(text: str) -> str:
    plain = text.replace("**", "")
    for area, starts in AREAS.items():
        if any(plain.startswith(start.replace("**", "")) for start in starts):
            return area
    return LANGUAGE


def main() -> int:
    lines = ROADMAP.read_text(encoding="utf-8").split("\n")

    counted: dict[str, list[str]] = {name: [] for name in AREAS}
    counted[LANGUAGE] = []
    for line in lines:
        box = BOX.match(line)
        if box:
            counted[area_of(box.group(2))].append(box.group(1))

    # Every name the three listed areas promise has to be a box that exists: a
    # renamed item would otherwise fall quietly into the language's count.
    problems = []
    for area, starts in AREAS.items():
        for start in starts:
            found = any(
                BOX.match(line) and BOX.match(line).group(2).replace("**", "").startswith(start.replace("**", ""))
                for line in lines
            )
            if not found:
                problems.append(f"  the table's `{area}` names `{start}` and no box starts with it")

    written = {}
    for line in lines:
        row = ROW.match(line)
        if row:
            written[row.group(1)] = (float(row.group(2)), float(row.group(3)), int(row.group(4)))

    total_done = 0.0
    total_n = 0
    for area, marks in counted.items():
        done = marks.count("x") + 0.5 * marks.count("~")
        total_done += done
        total_n += len(marks)
        if area not in written:
            problems.append(f"  no row in the table for `{area}`")
            continue
        said_pct, said_done, said_n = written[area]
        pct = 100 * done / len(marks) if marks else 0.0
        if (said_done, said_n) != (done, len(marks)):
            problems.append(
                f"  {area}: the table says {said_done:g} of {said_n}, the boxes say {done:g} of {len(marks)}"
            )
        elif abs(said_pct - pct) > 0.5:
            problems.append(f"  {area}: the table says {said_pct:g} %, the boxes say {pct:.1f} %")

    overall = 100 * total_done / total_n if total_n else 0.0
    if "Overall" not in written:
        problems.append("  no `Overall` row in the table")
    else:
        said_pct, said_done, said_n = written["Overall"]
        if (said_done, said_n) != (total_done, total_n):
            problems.append(
                f"  Overall: the table says {said_done:g} of {said_n}, the boxes say {total_done:g} of {total_n}"
            )
        elif abs(said_pct - overall) > 0.5:
            problems.append(f"  Overall: the table says {said_pct:g} %, the boxes say {overall:.1f} %")

    if problems:
        print("the roadmap's numbers do not match its own boxes:")
        print("\n".join(problems))
        print(
            "\nRecount and edit the table at the top of the page. A box is `[x]` done,\n"
            "`[~]` part-built (a half) or `[ ]` open."
        )
        return 1

    print(f"the roadmap's numbers match its boxes ({total_done:g} of {total_n}, {overall:.1f} %)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
