// editors/test/scope-test.mjs
//
// A grammar is a program, so it gets a test.
//
// This runs `editors/vscode/syntaxes/nikaia.tmLanguage.json` through
// `vscode-textmate` — the same library VS Code, Zed and Shiki tokenize with, so
// what passes here is what those editors do — and then over every `.nika` file
// in the repository.
//
// Two things it checks:
//
//   1. **Scope assertions.** For a list of snippets, that a named token carries
//      a named scope. This is where the constructs that are Nikaia's own are
//      pinned down: the `;` config zone, an `f"…"` hole, `} eod`, a commit
//      point, a lexical rule name.
//   2. **Coverage over the corpus.** How many tokens end up with nothing but
//      `source.nika` on them, and where. Run with `--uncovered` to list them.
//
// Usage:
//     npm install            # in this directory
//     node scope-test.mjs
//     node scope-test.mjs --uncovered
//     node scope-test.mjs --no-catchall   # count without `variable.other.nika`
//
// `--no-catchall` removes the grammar's last pattern, the one that says only
// "this is a name". With it in place almost nothing is unscoped and the number
// says little; without it the number says which constructs carry no meaning
// beyond being an identifier. Both are reported, because only the pair is
// honest.

import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, dirname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
// Both libraries ship CommonJS, so the namespace arrives on the default export.
import oniguruma from "vscode-oniguruma";
import vsctm from "vscode-textmate";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const grammarPath = join(repo, "editors", "vscode", "syntaxes", "nikaia.tmLanguage.json");

const args = process.argv.slice(2);
const showUncovered = args.includes("--uncovered");
const noCatchall = args.includes("--no-catchall");

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

async function makeRegistry(dropCatchall) {
  const wasm = readFileSync(
    join(here, "node_modules", "vscode-oniguruma", "release", "onig.wasm"),
  );
  await oniguruma.loadWASM(wasm);

  const raw = JSON.parse(readFileSync(grammarPath, "utf8"));
  if (dropCatchall) {
    raw.repository.main.patterns = raw.repository.main.patterns.filter(
      (p) => p.include !== "#identifier",
    );
  }

  return new vsctm.Registry({
    onigLib: Promise.resolve({
      createOnigScanner: (sources) => new oniguruma.OnigScanner(sources),
      createOnigString: (s) => new oniguruma.OnigString(s),
    }),
    loadGrammar: async (scopeName) =>
      scopeName === "source.nika" ? vsctm.parseRawGrammar(JSON.stringify(raw), grammarPath) : null,
  });
}

// ---------------------------------------------------------------------------
// Tokenizing
// ---------------------------------------------------------------------------

/** Tokenize `text`, returning a flat list of { line, text, scopes }. */
function tokenize(grammar, text) {
  return tokenizeWithState(grammar, text).tokens;
}

/**
 * Tokenize, keeping the rule stack the file ends on.
 *
 * The depth of that stack is what catches the failure this grammar has to not
 * have: a `dsl` block or an `f"…"` hole whose end is mis-detected leaves a
 * region open, and every line after it is coloured as part of it. A file that
 * ends deeper than the root has one.
 */
function tokenizeWithState(grammar, text) {
  const out = [];
  let state = vsctm.INITIAL;
  const lines = text.split("\n");
  for (let i = 0; i < lines.length; i++) {
    const result = grammar.tokenizeLine(lines[i], state);
    for (const token of result.tokens) {
      out.push({
        line: i + 1,
        text: lines[i].slice(token.startIndex, token.endIndex),
        scopes: token.scopes,
      });
    }
    state = result.ruleStack;
  }
  return { tokens: out, depth: state.depth, rootDepth: vsctm.INITIAL.depth };
}

/** The scopes on the first token whose text contains `needle`, at or after `from`. */
function scopesOf(tokens, needle, from = 0) {
  for (let i = from; i < tokens.length; i++) {
    if (tokens[i].text.includes(needle)) return { scopes: tokens[i].scopes, index: i };
  }
  return null;
}

// ---------------------------------------------------------------------------
// The assertions
// ---------------------------------------------------------------------------
//
// Each case: a snippet, an `expect` list of [needle, scope] pairs and an
// optional `reject` list of the same shape. A needle is the text of one token,
// and the `expect` list is walked in token order — so a spelling that occurs
// twice is addressed by putting a needle before it in the list, which is why
// several cases name a token they do not otherwise care about. `reject` searches
// from the start of the snippet.

const cases = [
  {
    name: "comments are the only comment there is",
    src: `// a note\nlet x = 1 // trailing\n`,
    expect: [
      ["//", "punctuation.definition.comment.nika"],
      ["a note", "comment.line.double-slash.nika"],
      ["let", "keyword.declaration.variable.nika"],
    ],
  },
  {
    name: "a plain string's braces are braces (ADR-309 D1)",
    src: `print("{ margin: 0 }")\nprint("\\\\d{3}")\n`,
    expect: [
      ['"', "string.quoted.double.nika"],
      ["{ margin", "string.quoted.double.nika"],
    ],
    reject: [["{ margin", "meta.interpolation.nika"]],
  },
  {
    name: "an f-string hole holds a Nikaia expression",
    src: `println(f"{name}: {count}")\n`,
    expect: [
      ["f", "storage.type.string.interpolated.nika"],
      ["{", "punctuation.section.interpolation.begin.nika"],
      ["name", "meta.interpolation.nika"],
      ["}", "punctuation.section.interpolation.end.nika"],
    ],
  },
  {
    name: "a hole's method call and format spec",
    src: `println(f"{a}={s.mean():.1}")\n`,
    expect: [
      ["mean", "entity.name.function.method.nika"],
      [".1", "constant.other.format-spec.nika"],
    ],
  },
  {
    comment:
      "1brc's line. The format spec follows a number here, so a lookbehind that excluded a word character in front of the colon lost it — which is the bug this case was written for.",
    name: "a format spec after an expression that ends in a digit",
    src: `println(f"{a}={s.min as f64 / 10.0:.1}/{s.max:.1}")\n`,
    expect: [
      ["10.0", "constant.numeric.float.nika"],
      [":", "punctuation.separator.format-spec.nika"],
      [".1", "constant.other.format-spec.nika"],
    ],
  },
  {
    name: "a path in a hole keeps its `::` (not a format spec)",
    src: `println(f"{utils::double(21)}")\n`,
    expect: [
      ["utils", "entity.name.namespace.nika"],
      ["::", "punctuation.accessor.nika"],
      ["double", "entity.name.function.path.nika"],
    ],
    reject: [["::", "constant.other.format-spec.nika"]],
  },
  {
    name: "a named argument inside a hole keeps its colon",
    src: `println(f"{Point(x: 1)}")\n`,
    expect: [["x", "variable.parameter.nika"]],
    reject: [["x", "constant.other.format-spec.nika"]],
  },
  {
    name: "`{{` is one brace, and the string closes",
    src: `println(f"{{{body}}}")\nlet after = 1\n`,
    expect: [
      ["{{", "constant.character.escape.brace.nika"],
      ["body", "meta.interpolation.nika"],
      ["}}", "constant.character.escape.brace.nika"],
      ["let", "keyword.declaration.variable.nika"],
    ],
    reject: [["let", "meta.interpolation.nika"]],
  },
  {
    name: "an escape is not a hole",
    src: `println(f"\\u{0041} and \\n")\n`,
    expect: [["\\u{0041}", "constant.character.escape.unicode.nika"]],
    reject: [["\\u{0041}", "meta.interpolation.nika"]],
  },
  {
    name: "an escaped quote inside a hole does not end the string",
    src: `println(f"{emphasised(\\"Ada\\")}")\nlet after = 1\n`,
    expect: [
      ["emphasised", "entity.name.function.call.nika"],
      ["Ada", "string.quoted.double.nika"],
      ["let", "keyword.declaration.variable.nika"],
    ],
    reject: [
      ["let", "meta.interpolation.nika"],
      ["let", "string.quoted.double.interpolated.nika"],
    ],
  },
  {
    name: "the `;` config zone in a call (Part I 5.1)",
    src: `let row = summary(lines, blank; separator: "\\t")\n`,
    expect: [
      ["summary", "entity.name.function.call.nika"],
      [";", "punctuation.separator.config.nika"],
      ["separator", "variable.parameter.nika"],
    ],
  },
  {
    name: "the `;` config zone in a signature, with its default",
    src: `fn request(url: &str; timeout: i32 = 30, method: &str = "GET") { }\n`,
    expect: [
      ["request", "entity.name.function.nika"],
      ["url", "variable.parameter.nika"],
      ["str", "support.type.primitive.nika"],
      [";", "punctuation.separator.config.nika"],
      ["timeout", "variable.parameter.nika"],
      ["30", "constant.numeric.integer.nika"],
    ],
  },
  {
    name: "a statement's `;` is not a config zone",
    src: `fn main() {\n    println("hi");\n}\n`,
    expect: [[";", "punctuation.terminator.statement.nika"]],
  },
  {
    name: "the typed spread (ADR-296 D5)",
    src: `pub fn prepare(&self, statement: &str; ...args: Self::dsl) -> Self::dsl { }\n`,
    expect: [
      ["...", "keyword.operator.spread.nika"],
      ["args", "variable.parameter.nika"],
      ["Self::dsl", "support.type.dsl.nika"],
    ],
  },
  {
    name: "`throws` and `sync` on a signature, on either side of the return type",
    src: `fn mean(&self) -> f64 sync { }\nfn fetch_config() throws -> String { }\n`,
    expect: [
      ["sync", "storage.modifier.sync.nika"],
      ["throws", "storage.modifier.throws.nika"],
    ],
  },
  {
    name: "`catch` is a postfix expression, and `??` a fallback",
    src: `let port = read_port() catch { 8080 }\nlet p = cli::args().nth(1) ?? "x"\n`,
    expect: [
      ["catch", "keyword.control.exception.nika"],
      ["??", "keyword.operator.coalesce.nika"],
    ],
  },
  {
    name: "`throw` raises",
    src: `throw ConfigError::NotFound(path)\n`,
    expect: [
      ["throw", "keyword.control.exception.nika"],
      ["ConfigError", "entity.name.type.nika"],
    ],
  },
  {
    name: "`seq { … }` is a keyword, not a struct literal",
    src: `seq {\n    benachrichtige(kunde)\n}\n`,
    expect: [["seq", "keyword.control.seq.nika"]],
    reject: [["seq", "entity.name.type.nika"]],
  },
  {
    name: "the one lambda form is `fn { … }`, with implicit arguments",
    src: `let ids = users.map fn { a.id }\nlet sum = numbers.reduce(0) fn { a + b }\n`,
    expect: [
      ["map", "entity.name.function.method.nika"],
      ["fn", "keyword.declaration.function.nika"],
    ],
  },
  {
    name: "a chain continues after a lambda's brace",
    src: `Server::new()\n    .route("/x") fn { handler(db) }\n    .listen(":8080")\n`,
    expect: [
      ["route", "entity.name.function.method.nika"],
      ["listen", "entity.name.function.method.nika"],
    ],
  },
  {
    name: "types use square brackets",
    src: `let s: HashMap[&str, Stats] = HashMap::new()\nfn f(rows: Vec[Row]) -> String { }\n`,
    expect: [
      ["HashMap", "support.class.nika"],
      ["[", "punctuation.section.brackets.begin.nika"],
      ["str", "support.type.primitive.nika"],
      ["Stats", "entity.name.type.nika"],
      ["]", "punctuation.section.brackets.end.nika"],
    ],
  },
  {
    name: "`Shared` and `Locked` are the language's own types",
    src: `let data: Shared[Locked[i32]] = x\n`,
    expect: [
      ["Shared", "support.class.nika"],
      ["Locked", "support.class.nika"],
      ["i32", "support.type.primitive.nika"],
    ],
  },
  {
    name: "`access` demands a sync lambda, and is marked as such",
    src: `counter.access fn { a += 1 }\npixels.par_iter().map fn { a }\n`,
    expect: [
      ["access", "support.function.concurrency.nika"],
      ["par_iter", "support.function.concurrency.nika"],
    ],
  },
  {
    name: "`spawn` and a task handle",
    src: `let handle = spawn fn { process_image(img_path) }\nlet r = handle.await\n`,
    expect: [
      ["spawn", "keyword.control.spawn.nika"],
      ["await", "support.function.concurrency.nika"],
    ],
  },
  {
    name: "numbers: an exponent, a tuple index, and no suffix in this language",
    src: `let m = 9.54791938424326609e-04\nlet k = pair.0\nrows.sort_by_key fn { -a.1 }\n`,
    expect: [
      ["9.54791938424326609e-04", "constant.numeric.float.nika"],
      ["0", "constant.numeric.tuple-index.nika"],
      ["1", "constant.numeric.tuple-index.nika"],
    ],
  },
  {
    name: "a range binds looser than its operators",
    src: `for i in 0..n - 1 { }\nfor i in 0..=n { }\n`,
    expect: [
      ["for", "keyword.control.loop.nika"],
      ["in", "keyword.control.loop.nika"],
      ["0", "constant.numeric.integer.nika"],
      ["..", "keyword.operator.range.nika"],
    ],
  },
  {
    comment:
      "ADR-276. A jump is scoped with `return` and not with `while`: " +
      "`keyword.control.loop` opens a loop and these two leave one. And " +
      "neither takes a value or a label, so the word is the whole token.",
    name: "`break` and `continue` are control flow, like `return`",
    src: `for i in 0..n {\n    if i % 2 == 0 { continue }\n    if i > 3 { break }\n}\nreturn 0\n`,
    // In source order: the runner walks the token stream with a cursor.
    expect: [
      ["for", "keyword.control.loop.nika"],
      ["continue", "keyword.control.flow.nika"],
      ["break", "keyword.control.flow.nika"],
      ["return", "keyword.control.flow.nika"],
    ],
  },
  {
    name: "a char is one character",
    src: `match c {\n    'n' => { out.push('\\n') }\n}\n`,
    expect: [
      ["match", "keyword.control.conditional.nika"],
      ["'n'", "string.quoted.single.nika"],
    ],
  },
  {
    name: "`use` paths",
    src: `use std::collections::HashMap\nuse utils\n`,
    expect: [
      ["use", "keyword.control.import.nika"],
      ["std::collections::HashMap", "entity.name.namespace.nika"],
    ],
  },
  {
    name: "items: struct, enum, impl, impl-for, pub",
    src: `pub struct User { pub name: String }\nenum Op { Times, Divide }\nimpl Summarize for User { }\n`,
    expect: [
      ["pub", "storage.modifier.nika"],
      ["struct", "keyword.declaration.struct.nika"],
      ["User", "entity.name.type.struct.nika"],
      ["enum", "keyword.declaration.enum.nika"],
      ["impl", "keyword.declaration.impl.nika"],
      ["for", "keyword.declaration.impl.for.nika"],
    ],
  },
  {
    // `@borrowed` stood here and is gone (ADR-283 D4). `@frame` is the same
    // scope on a word the language still has and a program still writes, so
    // what this holds is the rule - an attribute is coloured as one - rather
    // than a word that left.
    name: "an attribute is a modifier (ADR-296 D9)",
    src: `grammar Log {\n    @frame(boundary: "\\n")\n    rule LINE -> i64 = n:dec[i64](digit+) -> { n }\n}\n`,
    expect: [["@frame", "storage.modifier.attribute.nika"]],
  },
  {
    name: "a grammar's rule names say lexical from syntactic",
    src: `grammar Calc {\n    pub rule expr -> i64 = head:term -> { head }\n    rule NUMBER -> i64 = n:dec[i64](digit+) -> { n }\n}\n`,
    expect: [
      ["grammar", "keyword.other.grammar.nika"],
      ["Calc", "entity.name.type.grammar.nika"],
      ["rule", "keyword.other.grammar.rule.nika"],
      ["expr", "entity.name.function.rule.syntactic.nika"],
      ["NUMBER", "entity.name.function.rule.lexical.nika"],
      ["dec", "support.function.grammar.nika"],
      ["digit", "support.function.grammar.nika"],
    ],
  },
  {
    name: "a commit point, a frame attribute and a rule label",
    src: `grammar M {\n    @frame(boundary: "\\n")\n    rule MEASUREMENT -> Reading =\n        name:NAME ";" => temp:TENTHS frame_end -> { Reading { name, temp } }\n    rule expr -> Expr # "expression" = c:closure -> { c }\n}\n`,
    expect: [
      ["@frame", "storage.modifier.attribute.frame.nika"],
      ["boundary", "variable.parameter.nika"],
      ["name", "variable.parameter.nika"],
      ["NAME", "entity.name.function.rule.nika"],
      ["=>", "keyword.operator.commit.nika"],
      ["frame_end", "support.function.grammar.nika"],
      ["#", "punctuation.definition.label.nika"],
    ],
  },
  {
    name: "a grammar body closes, and the item after it is an item again",
    src: `grammar G {\n    rule A -> i32 = n:digit1 -> { let x = 1\n        x }\n}\npub struct After { a: i32 }\n`,
    expect: [
      ["struct", "keyword.declaration.struct.nika"],
      ["After", "entity.name.type.struct.nika"],
    ],
    reject: [["After", "meta.grammar.nika"]],
  },
  {
    comment:
      "`unchecked` is an assertion you make, like `unsafe`, and the word is there to grep for — so it gets a scope of its own, and only inside a grammar body.",
    name: "`@frame(…, unchecked)` asserts rather than asks",
    src: `grammar M {\n    @frame(boundary: "\\n", unchecked)\n    rule R -> i32 = d:digit frame_end -> { 1 }\n}\n`,
    expect: [["unchecked", "keyword.other.unchecked.nika"]],
  },
  {
    comment:
      "`from` is a keyword only in `dsl <Grammar> from <expr>`. tests/samples/while_loop.nika writes `fn countdown(from: i32)`, and `eod` only closes a DSL block.",
    name: "`from` is not a reserved word",
    src: `fn countdown(from: i32) -> i32 {\n    let mut n = from\n    return n\n}\n`,
    expect: [
      ["from", "variable.parameter.nika"],
      ["from", "variable.other.nika"],
    ],
    reject: [["from", "keyword.control.dsl.nika"]],
  },
  {
    name: "`par_fold` and alternation",
    src: `grammar M {\n    pub rule file -> S = par_fold(M, S::new, fn(acc, m) { acc.record(m) }, S::merge)\n    rule t -> i64 = "+" t:term -> { t } | "-" t:term -> { -t }\n}\n`,
    expect: [
      ["par_fold", "support.function.grammar.nika"],
      ["|", "keyword.operator.alternation.nika"],
    ],
  },
  {
    name: "a dsl html body is markup, and its holes are Nikaia",
    src: `fn t(rows: Vec[Row]) -> String {\n    return dsl html {\n        <tr class="{r.shade}"><td>{r.name}</td></tr>\n    } eod\n}\n`,
    expect: [
      ["dsl", "keyword.control.dsl.nika"],
      ["html", "entity.name.type.dsl-target.nika"],
      ["tr", "entity.name.tag.html.nika"],
      ["class", "entity.other.attribute-name.html.nika"],
      ["shade", "meta.interpolation.nika"],
      ["eod", "keyword.control.dsl.eod.nika"],
    ],
    reject: [["tr", "entity.name.type.nika"]],
  },
  {
    name: "a dsl html body closes, and the rest of the file is Nikaia",
    src: `fn t() -> String {\n    return dsl html {\n        <p>a } brace and a 'quote'</p>\n    } eod\n}\nfn after() { let x = 1 }\n`,
    expect: [
      ["after", "entity.name.function.nika"],
      ["let", "keyword.declaration.variable.nika"],
    ],
    reject: [["after", "meta.embedded.block.html.nika"]],
  },
  {
    name: "a dsl sql body's parameters are `:name`",
    src: `let query = dsl mysql {\n    SELECT name FROM people WHERE id = :id AND active = :active\n} eod\nlet bound = db.prepare(query; id: 501, active: true)\n`,
    expect: [
      ["mysql", "entity.name.type.dsl-target.nika"],
      [":", "punctuation.definition.parameter.dsl.nika"],
      ["id", "variable.parameter.dsl.nika"],
    ],
  },
  {
    name: "an unknown dsl target gets no Nikaia colouring in its body",
    src: `let q = dsl someone_elses {\n    fn struct if 42 "not a string"\n} eod\nlet x = 1\n`,
    expect: [
      ["someone_elses", "entity.name.type.dsl-target.nika"],
      ["fn struct", "meta.embedded.foreign.nika"],
      ["let", "keyword.declaration.variable.nika"],
    ],
    reject: [
      ["fn struct", "keyword.declaration.function.nika"],
      ["42", "constant.numeric.integer.nika"],
      ["not a string", "string.quoted.double.nika"],
    ],
  },
  {
    name: "`dsl <Grammar> from <expr>`",
    src: `let totals = dsl Measurements from data\n`,
    expect: [
      ["dsl", "keyword.control.dsl.nika"],
      ["Measurements", "entity.name.type.grammar.nika"],
      ["from", "keyword.control.dsl.from.nika"],
    ],
  },
  {
    name: "match patterns: a path names a variant, a bare name binds",
    src: `match self {\n    ConfigError::NotFound(p) => "no config"\n    _ => throw error\n}\n`,
    expect: [
      ["self", "variable.language.self.nika"],
      ["ConfigError", "entity.name.type.nika"],
      ["=>", "keyword.operator.arrow.nika"],
    ],
  },
  {
    name: "constructs the specification has and the parser does not are marked",
    src: `trait Summarize { }\nselect { }\nlet x = null\nconst C = 1\nlet y = a?.b\n`,
    expect: [
      ["trait", "keyword.declaration.trait.unimplemented.nika"],
      ["select", "keyword.control.select.unimplemented.nika"],
      ["null", "constant.language.null.unimplemented.nika"],
      ["const", "keyword.other.unimplemented.nika"],
      ["?.", "keyword.operator.safe-navigation.unimplemented.nika"],
    ],
  },
  {
    comment:
      "The parser's `STRING` is `\"` STR_CHAR* `\"` where STR_CHAR is `\\\\ any` or any non-quote, and `any` includes a newline — so a Nikaia string literal really does span lines, and an unterminated one really does run to the next quote. tests/errors/F1.nika is the error corpus's case for it. The grammar follows the parser rather than guessing a line end, and this pins that down so nobody 'fixes' it later.",
    name: "a string literal spans lines, as the parser's does",
    src: `fn main() {\n    let s = "one\n    two"\n}\n`,
    expect: [
      ['"', "string.quoted.double.nika"],
      ["two", "string.quoted.double.nika"],
    ],
  },
];

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

function listNika(dir, acc = []) {
  for (const entry of readdirSync(dir)) {
    // `.claude` holds subagent git worktrees - whole copies of this repository.
    // Walking one counts every corpus file twice and attributes a finding to a
    // path that will not exist tomorrow.
    if (
      entry === ".git" ||
      entry === ".claude" ||
      entry === "target" ||
      entry === "node_modules"
    )
      continue;
    const p = join(dir, entry);
    const st = statSync(p);
    if (st.isDirectory()) listNika(p, acc);
    else if (entry.endsWith(".nika")) acc.push(p);
  }
  return acc;
}

/** Tokens that carry nothing but the root scope. */
function uncovered(tokens) {
  return tokens.filter(
    (t) => t.text.trim().length > 0 && t.scopes.filter((s) => s !== "source.nika").length === 0,
  );
}

async function main() {
  const registry = await makeRegistry(noCatchall);
  const grammar = await registry.loadGrammar("source.nika");
  if (!grammar) throw new Error("grammar did not load");

  let failures = 0;
  let checks = 0;

  for (const c of cases) {
    const tokens = tokenize(grammar, c.src);
    let cursor = 0;
    for (const [needle, scope] of c.expect) {
      // `--no-catchall` removes the rule these assertions are about.
      if (noCatchall && scope === "variable.other.nika") continue;
      checks++;
      const hit = scopesOf(tokens, needle, cursor);
      if (!hit) {
        failures++;
        console.log(`FAIL  ${c.name}\n      no token containing ${JSON.stringify(needle)}`);
        continue;
      }
      cursor = hit.index + 1;
      if (!hit.scopes.includes(scope)) {
        failures++;
        console.log(
          `FAIL  ${c.name}\n      ${JSON.stringify(needle)} wanted ${scope}\n      got   ${hit.scopes.join(" ")}`,
        );
      }
    }
    for (const [needle, scope] of c.reject ?? []) {
      checks++;
      const hit = scopesOf(tokens, needle, 0);
      if (hit && hit.scopes.includes(scope)) {
        failures++;
        console.log(`FAIL  ${c.name}\n      ${JSON.stringify(needle)} must not carry ${scope}`);
      }
    }
  }

  console.log(
    `\n${checks - failures}/${checks} scope assertions pass` +
      (noCatchall ? " (catch-all identifier rule removed)" : ""),
  );

  // --- the corpus ---------------------------------------------------------

  const files = listNika(repo).sort();
  let total = 0;
  let bare = 0;
  const byText = new Map();
  const leaked = [];
  const brokenLeaked = [];

  for (const file of files) {
    const text = readFileSync(file, "utf8");
    const { tokens, depth, rootDepth } = tokenizeWithState(grammar, text);
    const rel = relative(repo, file);
    if (depth !== rootDepth) {
      // tests/errors/ holds files that are deliberately broken — an
      // unterminated string, an unclosed block — so ending inside a region is
      // the right answer there. Everywhere else it is a defect.
      (rel.startsWith("tests/errors/") ? brokenLeaked : leaked).push(`${rel} (depth ${depth})`);
    }
    const real = tokens.filter((t) => t.text.trim().length > 0);
    total += real.length;
    for (const t of uncovered(tokens)) {
      bare++;
      const key = t.text.trim();
      if (!byText.has(key)) byText.set(key, []);
      byText.get(key).push(`${rel}:${t.line}`);
    }
  }

  if (leaked.length > 0) {
    failures += leaked.length;
    console.log(`\nFAIL  ${leaked.length} well-formed file(s) end inside an unclosed region:`);
    for (const f of leaked) console.log(`      ${f}`);
  } else {
    console.log(
      `\nevery well-formed .nika file ends at the root scope ` +
        `(${brokenLeaked.length} in tests/errors/ do not, as they should not: ` +
        `${brokenLeaked.join(", ")})`,
    );
  }

  console.log(
    `\ncorpus: ${files.length} .nika files, ${total} non-whitespace tokens, ` +
      `${bare} with no scope beyond source.nika (${((100 * bare) / total).toFixed(2)}%)`,
  );

  if (byText.size > 0) {
    const rows = [...byText.entries()].sort((a, b) => b[1].length - a[1].length);
    console.log(`${byText.size} distinct unscoped spellings:`);
    for (const [text, where] of rows.slice(0, showUncovered ? rows.length : 20)) {
      console.log(
        `  ${String(where.length).padStart(5)}  ${JSON.stringify(text)}   e.g. ${where[0]}`,
      );
    }
    if (!showUncovered && rows.length > 20) {
      console.log(`  … ${rows.length - 20} more (--uncovered to list)`);
    }
  }

  process.exit(failures === 0 ? 0 : 1);
}

main().catch((e) => {
  console.error(e);
  process.exit(2);
});
