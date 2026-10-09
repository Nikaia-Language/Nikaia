#!/usr/bin/env python3
"""Writes `issues.md`, the site's page of open issues, into a site copy.

    python3 scripts/site-issues.py <site copy>

`scripts/site-build.sh` runs it on its throwaway copy, so the page is as fresh
as the site's last build and is never committed. The issues are grouped by
their status label (`docs/README.md`): `decision` first, since those wait for
the owner, then `ready`, `blocked`, `note`, and any without one.

The repository is public, so the issues are read without a token. With
`GITHUB_TOKEN` or `GH_TOKEN` set, the token is sent, and each issue's comments
are included as well: the history and the *To build* lists live there, and
reading them for every issue would spend more requests than an anonymous
caller gets in an hour. If GitHub cannot be reached the page says so, and the
site is built anyway.
"""

import json
import os
import re
import sys
import urllib.request
from datetime import datetime, timezone

REPO = "Nikaia-Language/Nikaia"
API = f"https://api.github.com/repos/{REPO}"
STATUSES = [
    ("decision", "Waiting for a decision"),
    ("ready", "Ready to build"),
    ("blocked", "Blocked"),
    ("note", "Notes"),
]
TOKEN = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")


def get(url):
    request = urllib.request.Request(url, headers={"Accept": "application/vnd.github+json"})
    if TOKEN:
        request.add_header("Authorization", f"Bearer {TOKEN}")
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


def every(url):
    page, out = 1, []
    while True:
        batch = get(f"{url}{'&' if '?' in url else '?'}per_page=100&page={page}")
        out += batch
        if len(batch) < 100:
            return out
        page += 1


def anchor(issue):
    return f"issue-{issue['number']}"


def demoted(text):
    """A body's headings two levels down, below the issue's own `###`."""
    text = (text or "").replace("\r\n", "\n").strip()
    out, fenced = [], False
    for line in text.split("\n"):
        if line.lstrip().startswith("```"):
            fenced = not fenced
        if not fenced and re.match(r"#{1,4} ", line):
            line = "##" + line
        out.append(line)
    return "\n".join(out)


def day(stamp):
    return stamp[:10]


def page(issues, comments):
    now = datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M UTC")
    groups = {key: [] for key, _ in STATUSES}
    groups[None] = []
    for issue in issues:
        names = {label["name"] for label in issue["labels"]}
        status = next((key for key, _ in STATUSES if key in names), None)
        groups[status].append(issue)
    sections = STATUSES + [(None, "Without a status label")]

    out = ["# Open Issues", ""]
    out.append(
        f"{len(issues)} open issues of [{REPO}](https://github.com/{REPO}/issues), "
        f"read when this site was built ({now}). Each heading links to the issue "
        "on GitHub, where it is answered."
    )
    out.append("")
    for key, title in sections:
        if groups[key]:
            out.append(f"* [{title}](#{key or 'none'}) ({len(groups[key])})")
    out.append("")
    for key, title in sections:
        if not groups[key]:
            continue
        out.append(f'## {title} <a id="{key or "none"}"></a>')
        out.append("")
        for issue in groups[key]:
            out.append(f"* [#{issue['number']}](#{anchor(issue)}) {issue['title']}")
        out.append("")
        for issue in groups[key]:
            labels = ", ".join(f"`{label['name']}`" for label in issue["labels"])
            out.append(
                f'### <a id="{anchor(issue)}"></a>'
                f"[#{issue['number']}]({issue['html_url']}) {issue['title']}"
            )
            out.append("")
            out.append(
                f"{labels or 'no labels'} · opened {day(issue['created_at'])} · "
                f"updated {day(issue['updated_at'])} · {issue['comments']} comments"
            )
            out.append("")
            out.append(demoted(issue["body"]))
            out.append("")
            for comment in comments.get(issue["number"], []):
                out.append(
                    f"**Comment** by {comment['user']['login']}, "
                    f"[{day(comment['created_at'])}]({comment['html_url']}):"
                )
                out.append("")
                out.append(demoted(comment["body"]))
                out.append("")
            out.append(f"[Back to the list](#{key or 'none'})")
            out.append("")
    return "\n".join(out) + "\n"


def main():
    target = os.path.join(sys.argv[1], "issues.md")
    try:
        issues = [i for i in every(f"{API}/issues?state=open") if "pull_request" not in i]
        issues.sort(key=lambda i: i["number"], reverse=True)
        comments = {}
        if TOKEN:
            for issue in issues:
                if issue["comments"]:
                    comments[issue["number"]] = every(issue["comments_url"])
        text = page(issues, comments)
    except Exception as error:  # the site is built without the page's contents
        print(f"site-issues: GitHub could not be read: {error}", file=sys.stderr)
        text = (
            "# Open Issues\n\nGitHub could not be read when this site was built. "
            f"The issues are at [github.com/{REPO}/issues](https://github.com/{REPO}/issues).\n"
        )
    with open(target, "w", encoding="utf-8") as out:
        out.write(text)


if __name__ == "__main__":
    main()
