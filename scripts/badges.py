#!/usr/bin/env python3
"""Write the README's badges as SVG files into `assets/badges/`.

The badges used to be images on img.shields.io, and every reader of the
documentation site asked that server for them. These are the same flat badges,
drawn here and committed, so the site loads nothing from anywhere else and the
README shows the same files on GitHub.

The version is read from the specification's own `**Version:**` line, so a
release changes the badge by running this, not by editing an SVG.

Usage:
    scripts/badges.py           write the badges
    scripts/badges.py --check   exit 1 if a committed badge is out of date
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "assets" / "badges"

BLUE = "#007ec6"
ORANGE = "#fe7d37"
GREY = "#555"
GITHUB = "#24292f"

# Advance widths of Verdana at 11px, the face shields.io measures with. They
# only set the padding: `textLength` makes the text fit whatever it renders in.
WIDTHS = {
    **dict.fromkeys("0123456789", 7.0),
    **dict.fromkeys("abdghnopqu", 6.9),
    **dict.fromkeys("cksvxyz", 6.0),
    "e": 6.6, "f": 3.9, "i": 3.0, "j": 3.8, "l": 3.0, "m": 10.7, "r": 4.7,
    "t": 4.3, "w": 9.0, " ": 3.9, ".": 3.9, "-": 4.9, "/": 4.9, "+": 9.0,
    "%": 12.0,
}


def text_width(text):
    return sum(WIDTHS.get(c, 8.0 if c.isupper() else 6.5) for c in text)


def badge(label, message, color):
    lw = round(text_width(label) + 10)
    mw = round(text_width(message) + 10)
    width = lw + mw
    title = f"{label}: {message}"

    def text(value, x, w):
        common = f'x="{x * 10}" transform="scale(.1)" textLength="{(w - 10) * 10}"'
        return (
            f'<text aria-hidden="true" {common} y="150" fill="#010101" '
            f'fill-opacity=".3">{value}</text>'
            f'<text {common} y="140" fill="#fff">{value}</text>'
        )

    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="20" '
        f'role="img" aria-label="{title}"><title>{title}</title>'
        '<linearGradient id="s" x2="0" y2="100%">'
        '<stop offset="0" stop-color="#bbb" stop-opacity=".1"/>'
        '<stop offset="1" stop-opacity=".1"/></linearGradient>'
        f'<clipPath id="r"><rect width="{width}" height="20" rx="3" fill="#fff"/></clipPath>'
        f'<g clip-path="url(#r)"><rect width="{lw}" height="20" fill="{GREY}"/>'
        f'<rect x="{lw}" width="{mw}" height="20" fill="{color}"/>'
        f'<rect width="{width}" height="20" fill="url(#s)"/></g>'
        '<g fill="#fff" text-anchor="middle" '
        'font-family="Verdana,Geneva,DejaVu Sans,sans-serif" '
        'text-rendering="geometricPrecision" font-size="110">'
        f"{text(label, lw / 2, lw)}{text(message, lw + mw / 2, mw)}</g></svg>\n"
    )


def version():
    spec = ROOT / "docs" / "specification" / "10-nikaia-light.md"
    found = re.search(r"^\*\*Version:\*\*\s*(\S+)", spec.read_text(encoding="utf-8"), re.M)
    if not found:
        sys.exit(f"{spec}: no **Version:** line")
    return found.group(1)


def self_hosted():
    sys.path.insert(0, str(ROOT / "scripts"))
    sys.dont_write_bytecode = True
    import self_hosting

    return self_hosting.share_text()


def badges():
    return {
        "version.svg": badge("version", version(), BLUE),
        "self-hosted.svg": badge("self-hosted", self_hosted(), ORANGE),
        "status.svg": badge("status", "specification + bootstrap", ORANGE),
        "license.svg": badge("license", "Apache 2.0", BLUE),
        "docs.svg": badge("docs", "nikaia-lang.org", BLUE),
        "github.svg": badge("GitHub", "Nikaia-Language/Nikaia", GITHUB),
    }


def main():
    check = "--check" in sys.argv[1:]
    stale = []
    for name, svg in badges().items():
        path = OUT / name
        if path.exists() and path.read_text(encoding="utf-8") == svg:
            continue
        if check:
            stale.append(name)
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(svg, encoding="utf-8")
            print(f"wrote {path.relative_to(ROOT)}")
    if stale:
        sys.exit(f"out of date in assets/badges/: {', '.join(stale)} (run scripts/badges.py)")


if __name__ == "__main__":
    main()
