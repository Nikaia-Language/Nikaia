// scripts/site-highlight/highlight.mjs
//
// Re-highlights every fenced code block on the built site, from this
// repository's own TextMate grammar.
//
// Jekyll highlights with Rouge, and Rouge has no lexer for Nikaia: the 174
// ```nika blocks in the specification, the decision records and the README
// reach the page as undifferentiated text, while the Rust and TOML beside
// them are coloured. `editors/vscode/syntaxes/nikaia.tmLanguage.json` already
// says what a Nikaia token is, and is already tested against
// `vscode-textmate` in `editors/test/` — so the missing piece is not a
// grammar but somewhere to apply it outside an editor. Shiki tokenizes with
// vscode-textmate too, so what the editors show and what the site shows come
// from one file and cannot drift apart.
//
// It runs over `_site` after Jekyll, not before: the input is generated HTML
// with a shape this asserts rather than assumes. If a block ever reaches the
// end unconverted, the run fails instead of publishing a page where one block
// is themed and the next is not.
//
// Both themes are written into every token — `color:` for light,
// `--shiki-dark:` beside it — and `assets/css/nikaia.css` says which to read.
// That is what lets the theme picker recolour code without a second copy of
// the page.
//
// Usage:
//     npm ci --prefix scripts/site-highlight
//     node scripts/site-highlight/highlight.mjs _site

import { readFile, writeFile, readdir } from "node:fs/promises";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { createHighlighter } from "shiki";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(HERE, "..", "..");
const GRAMMAR = join(REPO, "editors", "vscode", "syntaxes", "nikaia.tmLanguage.json");

const LIGHT = "github-light";
const DARK = "github-dark";

// What a fence says, and what to tokenize it as. Anything not named here is
// asked of Shiki directly, and anything Shiki does not know is set in plain
// text — a block whose language nobody can read is still a block that has to
// carry the reader's theme.
const ALIASES = {
  nika: "nika",
  nikaia: "nika",
  text: "text",
  plaintext: "text",
  mermaid: "text",
};

// The two shapes Jekyll produces, and between them every `<pre>` on the site.
// A language Rouge knows is highlighted inside two wrapper divs; one it does
// not know — `nika` — arrives as a bare `<pre><code>` with the fence's word on
// the `class`.
//
// The `\\s*` between the wrappers' closing tags is not decoration: a fenced
// block inside a list item is indented by Kramdown, so the two `</div>` are
// not adjacent there. `docs/technical_notes.md` has one, and the check at the
// end of this file is what found it.
const ROUGE_BLOCK =
  /<div class="language-([\w+.-]+) highlighter-rouge">\s*<div class="highlight">\s*<pre class="highlight"><code>([\s\S]*?)<\/code><\/pre>\s*<\/div>\s*<\/div>/g;
const PLAIN_BLOCK = /<pre><code class="language-([\w+.-]+)">([\s\S]*?)<\/code><\/pre>/g;

function textOf(html) {
  return html
    .replace(/<[^>]*>/g, "")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&#x27;/gi, "'")
    .replace(/&amp;/g, "&"); // last, so `&amp;lt;` survives as `&lt;`
}

// Shiki writes the block's background onto the `<pre>` itself, one property
// per theme. Taking those off and letting `--nk-code-bg` give the background
// back is what lets Sepia have a code background of its own without a third
// set of token colours, and keeps every block agreeing with the page it is on.
function dropBackground(node) {
  const style = String(node.properties.style || "")
    .split(";")
    .map((d) => d.trim())
    .filter((d) => d && !/^(background-color|--shiki-[\w-]*-bg)\s*:/.test(d));
  node.properties.style = style.join(";");
}

async function* htmlFiles(dir) {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) yield* htmlFiles(path);
    else if (entry.name.endsWith(".html")) yield path;
  }
}

const site = resolve(process.argv[2] || "_site");

const grammar = JSON.parse(await readFile(GRAMMAR, "utf8"));
const highlighter = await createHighlighter({
  themes: [LIGHT, DARK],
  langs: [{ ...grammar, name: "nika" }],
});

const known = new Set(highlighter.getLoadedLanguages());
const counted = new Map();

async function languageFor(fence) {
  const name = ALIASES[fence] ?? fence;
  if (known.has(name)) return name;
  try {
    await highlighter.loadLanguage(name);
    known.add(name);
    return name;
  } catch {
    known.add(fence); // asked once; `text` from here on
    ALIASES[fence] = "text";
    return "text";
  }
}

async function render(fence, body) {
  const lang = await languageFor(fence);
  counted.set(`${fence} → ${lang}`, (counted.get(`${fence} → ${lang}`) || 0) + 1);
  return highlighter.codeToHtml(textOf(body).replace(/\n+$/, ""), {
    lang,
    themes: { light: LIGHT, dark: DARK },
    // No literal `color:` on any token — `--shiki-light` and `--shiki-dark`
    // beside each other, and the stylesheet picks. An inline `color` would
    // outrank every selector in `nikaia.css`, so the alternative is a file of
    // `!important`.
    defaultColor: false,
    // The GitHub themes' comment grey, and two of the light theme's reds,
    // fall under WCAG AA's 4.5:1 on this site's code backgrounds — the light
    // colours are read on Sepia's too — and the examples are mostly comment.
    // Each replacement is the same hue, moved only as far as 4.5:1 against
    // every background it is shown on (light: #f5f7fa, #ece1c6; dark:
    // #131a26, #0c1119). Comments get a little more than the minimum.
    colorReplacements: {
      [LIGHT]: { "#6a737d": "#57606a", "#d73a49": "#bd2635", "#e36209": "#a34606" },
      [DARK]: { "#6a737d": "#8b949e" },
    },
    transformers: [
      {
        pre(node) {
          dropBackground(node);
          this.addClassToHast(node, `language-${fence}`);
        },
      },
    ],
  });
}

// One pass per shape. `replaceAll` takes no async replacer, so the matches are
// collected first and spliced back in reverse, which keeps every earlier index
// valid.
async function convert(html) {
  const edits = [];
  for (const pattern of [ROUGE_BLOCK, PLAIN_BLOCK]) {
    pattern.lastIndex = 0;
    for (const match of html.matchAll(pattern)) {
      edits.push({
        start: match.index,
        end: match.index + match[0].length,
        html: await render(match[1], match[2]),
      });
    }
  }
  edits.sort((a, b) => b.start - a.start);
  for (const edit of edits) {
    html = html.slice(0, edit.start) + edit.html + html.slice(edit.end);
  }
  return { html, count: edits.length };
}

let blocks = 0;
let pages = 0;
const missed = [];

for await (const file of htmlFiles(site)) {
  const before = await readFile(file, "utf8");
  const { html, count } = await convert(before);
  if (!count) continue;

  // Every `<pre>` on the page is now Shiki's, or this script no longer knows
  // the shape Jekyll emits and the site would publish half-themed.
  for (const [tag] of html.matchAll(/<pre[^>]*>/g)) {
    if (!/class="[^"]*\bshiki\b/.test(tag)) missed.push(`${file}: ${tag}`);
  }

  await writeFile(file, html);
  blocks += count;
  pages += 1;
}

for (const [what, n] of [...counted].sort((a, b) => b[1] - a[1])) {
  console.log(`  ${String(n).padStart(4)}  ${what}`);
}
console.log(`${blocks} code blocks on ${pages} pages, from ${GRAMMAR.slice(REPO.length + 1)}`);

if (missed.length) {
  console.error(`\n${missed.length} block(s) left unconverted:`);
  for (const line of missed.slice(0, 10)) console.error(`  ${line}`);
  process.exit(1);
}
