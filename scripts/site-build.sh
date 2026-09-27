#!/usr/bin/env bash
# Builds the documentation site into `_site/`. This is the build command
# Cloudflare Workers Builds runs for nikaia-lang.org (see `wrangler.jsonc`);
# it runs locally too.
#
# Every step runs on a throwaway copy of the repository, never on the checkout
# itself: fencing the braces and writing the menu change files, and a local
# run must leave the working tree as it found it.
#
# Needs Ruby with Bundler, Python 3 and Node with npm.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# The copy: everything but what is built, installed or fetched.
tar -C "$ROOT" -cf - \
  --exclude=./.git --exclude=./target --exclude=./_site \
  --exclude=./vendor --exclude=./.bundle --exclude=node_modules \
  --exclude=./.jekyll-cache --exclude=./.sass-cache \
  --exclude=./.wrangler . | tar -C "$WORK" -xf -

# Literal braces, fenced off from Liquid. Nikaia writes a literal brace as
# `{{`, so `{{` occurs in the specification, in decision records, the roadmap
# notes and the changelog. Jekyll hands every Markdown file to Liquid first,
# which reads `{{` as the start of a template variable: unguarded, some pages
# fail the build outright and the rest lose the text between one brace pair
# and the next, silently. Wrapping each file in `{% raw %}` tells Liquid to
# leave it alone. The marker sits inside an HTML comment, so it is invisible
# in the built page, and it goes *after* a file's opening heading, so that
# jekyll-titles-from-headings still finds that heading at the very start.
find "$WORK" -name '*.md' -print0 |
  while IFS= read -r -d '' f; do
    awk '
      NR == 1 && /^#/ { print; print "<!-- {% raw %} -->"; o = 1; next }
      NR == 1         { print "<!-- {% raw %} -->"; print; o = 1; next }
                      { print }
      END             { if (o) { print ""; print "<!-- {% endraw %} -->" } }
    ' "$f" >"$f.pages" && mv "$f.pages" "$f"
  done

# The menu and the example pages.
python3 "$ROOT/scripts/site-prepare.py" "$WORK"

# Jekyll, with the gems GitHub Pages pins.
export BUNDLE_GEMFILE="$ROOT/scripts/site-jekyll/Gemfile"
export JEKYLL_ENV=production
# Sass reads the theme's stylesheets in the locale's encoding, and they are
# UTF-8; a build machine's default locale may be plain ASCII.
export RUBYOPT="${RUBYOPT:+$RUBYOPT }-Eutf-8"
bundle install --quiet

# The theme, laid into the copy as files rather than named as `theme:`. A
# theme gem's own dependencies are loaded with it, and `jekyll-theme-primer`'s
# include `jekyll-github-metadata` (`scripts/site-jekyll/Gemfile` says why that
# stays out). The repository's own files win: `_layouts/default.html` overrides the
# theme's layout of that name, here as on Pages.
THEME="$(bundle exec ruby -e 'print Gem.loaded_specs["jekyll-theme-primer"].full_gem_path')"
python3 - "$THEME" "$WORK" <<'PY'
import shutil, sys
from pathlib import Path
theme, work = Path(sys.argv[1]), Path(sys.argv[2])
for part in ("_includes", "_layouts", "_sass", "assets"):
    for src in (theme / part).rglob("*"):
        dst = work / src.relative_to(theme)
        if src.is_file() and not dst.exists():
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(src, dst)
PY
# ... and the copy's `theme:` line goes, since a later config file cannot
# unset a key an earlier one set.
sed '/^theme:/d' "$WORK/_config.yml" >"$WORK/_config.yml.new"
mv "$WORK/_config.yml.new" "$WORK/_config.yml"

# Set up the way GitHub Pages sets it up: the plugins are the ones Pages turns
# on by default plus `_config.yml`'s own and the theme's, the options below are
# the ones the `github-pages` gem imposes on every Pages build (its
# `configuration.rb`),
# and `site.github` holds the fields the layout reads, which on Pages come
# from the GitHub API. The commit is the one a Workers build is running for,
# and what the stylesheet and script URLs are versioned by.
REVISION="${WORKERS_CI_COMMIT_SHA:-$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || true)}"
cat >"$WORK/_config.build.yml" <<YAML
plugins_dir: $ROOT/scripts/site-jekyll/plugins
plugins:
  - jekyll-optional-front-matter
  - jekyll-readme-index
  - jekyll-relative-links
  - jekyll-titles-from-headings
  - jekyll-default-layout
  - jekyll-seo-tag
  - jekyll-sitemap
future: true
markdown: kramdown
kramdown:
  input: GFM
  hard_wrap: false
  gfm_quirks: paragraph_end
  math_engine: mathjax
  syntax_highlighter: rouge
  syntax_highlighter_opts:
    default_lang: plaintext
  template: ""
sass:
  style: compressed
github:
  build_revision: "$REVISION"
  private: false
  license:
    key: apache-2.0
  repository_url: https://github.com/Nikaia-Language/Nikaia
  branch: main
YAML
rm -rf "$ROOT/_site"
# From inside the copy: Jekyll 3 also reads `_layouts/` in the directory it is
# started from, and that must be the copy's.
cd "$WORK"
bundle exec jekyll build \
  --source "$WORK" --destination "$ROOT/_site" \
  --config "$WORK/_config.yml,$WORK/_config.build.yml"

# The redirects `site-prepare.py` wrote, which Cloudflare reads from the assets
# directory. Jekyll skips every file whose name starts with `_`.
cp "$WORK/_redirects" "$ROOT/_site/_redirects"

# Nikaia code blocks, highlighted from the editor grammar.
npm ci --prefix "$ROOT/scripts/site-highlight" --no-audit --no-fund
node "$ROOT/scripts/site-highlight/highlight.mjs" "$ROOT/_site"
