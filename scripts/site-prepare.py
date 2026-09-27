#!/usr/bin/env python3
"""Write the pages and the menu the site needs and the repository does not have.

Run by `site-build.sh` on its throwaway copy of the checkout, after it has
fenced literal braces off from Liquid and before Jekyll builds. Nothing here changes a file in
the repository, and nothing here restates documentation: every page it writes
either quotes a program that exists or is a list of links to pages that exist.

Six things:

1. **A page per example program.** `examples/*.nika` are published as files, and
   a browser asking for one gets a download rather than a page — no layout, no
   menu, no highlighting. Each now has a page beside it that shows the program,
   with the file itself still one link away, as `tally.nika.txt`: a browser
   shows plain text rather than downloading it, and the link's `download`
   attribute names the saved file `tally.nika` again.

6. **`_redirects`, which sends `/examples/tally.nika` to that page.** The
   address a reader has — from the README, from a search result, from a link
   written before the pages existed — is the program's own, and it should
   open the page, not a download. Cloudflare follows a redirect before it
   looks for an asset, so the rule wins over the file of that name; the file
   is still served, at `tally.nika.txt`. `site-build.sh` lays this file into
   `_site`, since Jekyll publishes nothing whose name starts with `_`.

2. **A directory index where a directory of programs has no README.**
   `examples/inventory/` is linked as a directory from the examples index and
   had nothing to serve.

3. **`_data/nav.json`, which is the menu.** Generated rather than written down,
   because the site grows: sixty decision records today, and a menu nobody
   maintains would be wrong by the next one. Each entry's label is that page's
   own first heading, so a retitled page retitles itself in the menu.

4. **A permalink on `docs/README.md`.** `jekyll-readme-index` follows GitHub's
   rule that a `docs/README.md` is a candidate for the *root* index, and drops
   it when a root `README.md` already won that slot — so `/docs/` was a 404 and
   the note index was published as raw Markdown.

5. **`/llms.txt`, the menu again, for language models** (llmstxt.org): the
   site's name and description from `_config.yml`, then every page the menu
   lists, under the menu's own sections, with its full address. Written from
   the same list as the menu, so the two cannot disagree.

Usage:
    scripts/site-prepare.py [checkout-root]
"""

import json
import re
import sys
from pathlib import Path

RAW_OPEN = "<!-- {% raw %} -->"
RAW_CLOSE = "<!-- {% endraw %} -->"

FENCE = re.compile(r"^\s*(```|~~~)")
HEADING = re.compile(r"^#\s+(.+?)\s*$")


def first_heading(path):
    """The page's own title: its first `# ` heading outside a code fence."""
    in_fence = False
    for line in path.read_text(encoding="utf-8").splitlines():
        if FENCE.match(line):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        found = HEADING.match(line)
        if found:
            # `**bold**` and `` `code` `` are markup in a heading and noise in a
            # menu; the words are the label.
            return re.sub(r"[`*]", "", found.group(1)).strip()
    return None


def page_url(root, path):
    """Where Jekyll will publish this Markdown file."""
    relative = path.relative_to(root)
    if path.name.lower() == "readme.md":
        parent = relative.parent.as_posix()
        return "/" if parent == "." else f"/{parent}/"
    return "/" + relative.with_suffix(".html").as_posix()


def write(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def example_pages(root):
    """One page per program, and an index for a directory that has no README."""
    examples = root / "examples"
    written = []
    if not examples.is_dir():
        return written

    for program in sorted(examples.rglob("*.nika")):
        page = program.with_suffix(".md")
        name = program.name
        title = program.relative_to(examples).as_posix()
        # Relative, because the site is served under `/Nikaia` and a
        # root-absolute link would leave it.
        up = "../" * len(program.relative_to(examples).parts[:-1]) or "./"
        # `{{` is how Nikaia writes a literal brace and `{` opens a hole in an
        # `f"…"`, so a program is the last thing that may reach Liquid.
        write(
            page,
            f"# {title}\n\n"
            f"{RAW_OPEN}\n"
            "```nika\n"
            f"{program.read_text(encoding='utf-8').rstrip()}\n"
            "```\n"
            f"{RAW_CLOSE}\n\n"
            f"[The file itself]({name}.txt){{: download=\"{name}\"}}"
            f" · [all the examples]({up})\n",
        )
        write(program.with_name(name + ".txt"), program.read_text(encoding="utf-8"))
        written.append(page)

    directories = {p.parent for p in examples.rglob("*.nika")}
    for directory in sorted(directories):
        if (directory / "README.md").exists() or directory == examples:
            continue
        programs = sorted(p.name for p in directory.glob("*.nika"))
        listing = "\n".join(
            f"- [{name}]({Path(name).with_suffix('.html')})" for name in programs
        )
        write(
            directory / "README.md",
            f"# {directory.relative_to(examples).as_posix()}/\n\n{listing}\n",
        )

    return written


def relink_examples_index(root, written):
    """The examples index links at the files; point it at the pages."""
    index = root / "examples" / "README.md"
    if not index.exists():
        return
    names = {p.with_suffix(".nika").name for p in written}
    text = index.read_text(encoding="utf-8")

    def swap(match):
        target = match.group(1)
        if target in names:
            return "](" + target[: -len(".nika")] + ".html)"
        return match.group(0)

    index.write_text(re.sub(r"\]\(([^)/]+\.nika)\)", swap, text), encoding="utf-8")


def redirects(root, written):
    """`/examples/tally.nika` → `/examples/tally`, one line per program."""
    lines = []
    for page in written:
        url = "/" + page.relative_to(root).with_suffix("").as_posix()
        lines.append(f"{url}.nika {url} 301")
    return "\n".join(lines) + "\n"


def docs_index_permalink(root):
    readme = root / "docs" / "README.md"
    if not readme.exists():
        return
    text = readme.read_text(encoding="utf-8")
    if text.startswith("---"):
        return
    readme.write_text("---\npermalink: /docs/\n---\n" + text, encoding="utf-8")


def entry(root, path, title=None):
    return {"title": title or first_heading(path) or path.stem, "url": page_url(root, path)}


def menu(root):
    spec = root / "docs" / "specification"
    adr = spec / "adr"

    notes = sorted(
        p for p in (root / "docs").glob("*.md") if p.name.lower() != "readme.md"
    )
    records = sorted(
        p for p in adr.glob("*.md") if p.name.lower() != "readme.md"
    )
    examples = sorted(
        (root / "examples").rglob("*.md"),
        key=lambda p: (p.parent.as_posix(), p.name.lower() != "readme.md", p.name),
    )

    sections = [
        {
            "title": "Nikaia",
            "pages": [
                # The README opens with a banner rather than a heading, so this
                # is the one label that is not a page's own first line.
                entry(root, root / "README.md", "Home"),
                entry(root, root / "guide" / "getting-started.md"),
                entry(root, root / "guide" / "syntax.md"),
                entry(root, root / "manifesto.md"),
                entry(root, root / "CHANGELOG.md", "Changelog"),
            ],
        },
        {
            "title": "Specification",
            "pages": [entry(root, spec / "README.md")]
            + [
                entry(root, p)
                for p in sorted(spec.glob("*.md"))
                if p.name.lower() != "readme.md"
            ],
        },
        {
            "title": "Decisions",
            "pages": [entry(root, adr / "README.md")] + [entry(root, p) for p in records],
        },
        {
            "title": "Notes",
            "pages": [entry(root, root / "docs" / "README.md")]
            + [entry(root, p) for p in notes],
        },
        {
            "title": "Examples",
            "pages": [entry(root, p) for p in examples],
        },
    ]

    for section in sections:
        section["pages"] = [p for p in section["pages"] if p["title"]]
    return sections


def config_value(root, key):
    """A top-level scalar from `_config.yml`, folded (`>-`) or not."""
    lines = (root / "_config.yml").read_text(encoding="utf-8").splitlines()
    for i, line in enumerate(lines):
        found = re.match(rf"^{key}:\s*(.*?)\s*$", line)
        if not found:
            continue
        if found.group(1) not in (">", ">-", "|", "|-"):
            return found.group(1)
        folded = []
        for more in lines[i + 1 :]:
            if not more.startswith(" "):
                break
            folded.append(more.strip())
        return " ".join(folded)
    return None


def link_text(title):
    """A title as Markdown link text: `Array[T, N]` and a lone `[` would
    otherwise end or break the link."""
    return title.replace("[", "\\[").replace("]", "\\]")


def llms_txt(root, sections):
    base = (config_value(root, "url") or "").rstrip("/")
    out = [f"# {config_value(root, 'title')}", "", f"> {config_value(root, 'description')}", ""]
    out.append(
        "Every page below is published from a Markdown file in "
        "https://github.com/Nikaia-Language/Nikaia, at the same path."
    )
    for section in sections:
        out += ["", f"## {section['title']}", ""]
        out += [f"- [{link_text(p['title'])}]({base}{p['url']})" for p in section["pages"]]
    return "\n".join(out) + "\n"


def project(root):
    """What the header says about the project: the version, from the same
    `**Version:**` line of the specification that `badges.py` reads, so the
    header and the badge cannot disagree."""
    part = root / "docs" / "specification" / "10-nikaia-light.md"
    found = re.search(r"^\*\*Version:\*\* ([\w.]+)", part.read_text(encoding="utf-8"), re.M)
    return {"version": found.group(1) if found else ""}


def main():
    root = Path(sys.argv[1] if len(sys.argv) > 1 else ".").resolve()

    written = example_pages(root)
    relink_examples_index(root, written)
    docs_index_permalink(root)

    sections = menu(root)
    write(root / "_data" / "nav.json", json.dumps(sections, indent=2) + "\n")
    write(root / "_data" / "project.json", json.dumps(project(root), indent=2) + "\n")
    write(root / "llms.txt", llms_txt(root, sections))
    write(root / "_redirects", redirects(root, written))

    print(f"{len(written)} example pages")
    for section in sections:
        print(f"  {len(section['pages']):3}  {section['title']}")


if __name__ == "__main__":
    main()
