#!/usr/bin/env python3
"""The issues the commits in a range say they fix (#440).

`promote` moves `main` with the workflow's own token, and a push made with it
closes no issue: GitHub's closing keywords act on a push to the default branch,
but not on one a workflow makes. So the job asks this script which issues the
commits it brought to `main` close, and closes them itself.

    scripts/fixed-issues.py OLD NEW

prints one line per issue, `NUMBER SHA`, the first commit that names it. A
commit names an issue with `Fixes`, `Closes` or `Resolves` (any tense, any
case), followed by one or more `#N` joined by commas or `and`:
`Fixes #375, #376`. `Part of #N` closes nothing.
"""

import re
import subprocess
import sys

KEYWORD = re.compile(
    r"\b(?:fix(?:es|ed)?|close[sd]?|resolve[sd]?)\b:?\s+"
    r"(#\d+(?:\s*(?:,|and)\s*#\d+)*)",
    re.IGNORECASE,
)


def fixed(message):
    """The issue numbers one commit message closes, in the order written."""
    found = []
    for match in KEYWORD.finditer(message):
        for number in re.findall(r"#(\d+)", match.group(1)):
            if int(number) not in found:
                found.append(int(number))
    return found


def main():
    old, new = sys.argv[1], sys.argv[2]
    log = subprocess.run(
        ["git", "log", "--reverse", "--format=%H%x00%B%x01", f"{old}..{new}"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    seen = set()
    for entry in log.split("\x01"):
        if "\x00" not in entry:
            continue
        sha, message = entry.strip("\n").split("\x00", 1)
        for number in fixed(message):
            if number not in seen:
                seen.add(number)
                print(number, sha)


if __name__ == "__main__":
    main()
