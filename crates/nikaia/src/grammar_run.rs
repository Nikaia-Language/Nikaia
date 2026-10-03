//! **Running a grammar while the program is built is not interpretation**
//! (issue #178, Part II 10.2 A).
//!
//! A grammar could be run here by walking the grammar tree this compiler
//! already holds. **It must not be**, and the reason is not the size of the
//! work. `winnow-grammar` is a code **generator**: its model crate parses the
//! grammar language, validates it, and hands the result to a macro that writes
//! a parser. There is no interpreter in it to borrow — so interpreting would be
//! a **second implementation of the same semantics**, and Part II 10.2's
//! promise that one grammar means the same thing at both stages would stop
//! being a property and become a hope. The disagreements would land in the
//! corners — implicit whitespace, repetition bounds, the commit point, frames
//! and resynchronisation, interning, spans — and would present as *this file
//! parsed while the program was built and fails while it runs*, for the same
//! file and the same grammar.
//!
//! So this compiles the **generated** parser and runs it. Then there is one
//! implementation and the agreement is a tautology rather than a claim.
//!
//! **What it costs is a second compilation**, which [ADR-310](../../../docs/specification/adr/adr-310.md)
//! Q4 named. The sub-project is keyed on what went into it, so the cost is paid
//! when the grammar changes rather than on every build. The compiler already
//! emits Rust and already drives Cargo, so the machinery is not new.
//!
//! **And it needs no new security model.** A grammar's action blocks are
//! Nikaia, and [ADR-287](../../../docs/specification/adr/adr-287.md) already
//! says what a build-time body may do; the bytes come from `asset("…")`, which
//! [ADR-310](../../../docs/specification/adr/adr-310.md) already bounds. What
//! is new here is neither the permission nor the input.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::ast::Item;
use crate::contracts::ty::Ty;
use crate::contracts::ty::TyOps;
use crate::parser::Parsed;
use nikaia_std::tools::dump::{FieldShape, Shapes, VariantShape};

/// Why a grammar could not be run, where the reason is this compiler's rather
/// than the input's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wall {
    /// This build has nowhere to compile a parser — a bare `check` with no
    /// project around it, which is every unit test and nothing a person runs.
    NowhereToBuild,
    /// A grammar run inside a grammar run. The sub-project is emitted without
    /// the item-level `comptime`s, so this is a `comptime` inside a *body*, and
    /// it is refused rather than allowed to nest compilers.
    InsideAnother { grammar: String },
    /// The rule's result has no form a `const` can hold
    /// ([ADR-311](../../../docs/specification/adr/adr-311.md) D1).
    NoCrossedForm { ty: String, because: String },
    /// The sub-project did not compile, which is this compiler's fault rather
    /// than the program's — the parser it wrote is the parser the program
    /// links.
    DidNotBuild { detail: String },
    /// The parser ran and refused the input. **The input's own diagnostic**,
    /// in the grammar's vocabulary and against the file the bytes came from.
    Refused { detail: String },
    /// It ran and printed something this could not read, which is the pair of
    /// generated encoder and hand-written decoder disagreeing.
    Unreadable { detail: String },
}

/// One run: which grammar, which rule, and the bytes.
pub struct Ask<'a> {
    pub grammar: &'a str,
    pub rule: &'a str,
    /// The bytes the parser is pointed at.
    pub input: &'a str,
    /// What the rule declares it hands back, for the dump this generates.
    pub result: &'a Ty,
}

/// Where a build compiles the parsers it runs, and the guard against nesting.
///
/// **One per build**, carried with the rest of what a build may do while it
/// builds. A build with no place to put a sub-project is one no person runs:
/// every real build has a `target/`, and a bare `check` in a test has none.
#[derive(Debug, Default)]
pub struct Workshop {
    at: Option<PathBuf>,
    /// The grammars being run, innermost last — a ring rather than a depth,
    /// because the message names what it found.
    running: std::sync::Mutex<Vec<String>>,
}

impl Workshop {
    /// A build with nowhere to compile a parser.
    pub fn none() -> Workshop {
        Workshop::default()
    }

    /// Whether there is anywhere to compile a parser.
    pub fn somewhere(&self) -> bool {
        self.at.is_some()
    }

    /// A build that compiles its parsers under `at`.
    pub fn at(at: impl Into<PathBuf>) -> Workshop {
        Workshop {
            at: Some(at.into()),
            running: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Run `ask`'s rule over `ask`'s file, in a parser compiled from `parsed`.
    ///
    /// `parsed` is the file the **grammar** was declared in, which need not be
    /// the file the `comptime` stands in: a program's files share one namespace
    /// (Part I 9.1) and each owns the interner its symbols resolve in.
    pub fn run(&self, parsed: &Parsed, ask: &Ask<'_>) -> Result<String, Wall> {
        let Some(at) = &self.at else {
            return Err(Wall::NowhereToBuild);
        };
        // **A grammar run inside a grammar run is refused**, because the inner
        // one would compile a compiler. The sub-project carries no item-level
        // `comptime`, so what reaches here is one inside a body.
        {
            let mut running = self.running.lock().map_err(|_| Wall::DidNotBuild {
                detail: "the workshop's lock was poisoned".to_string(),
            })?;
            if let Some(outer) = running.first() {
                return Err(Wall::InsideAnother {
                    grammar: outer.clone(),
                });
            }
            running.push(ask.grammar.to_string());
        }
        let outcome = self.compile_and_run(at, parsed, ask);
        if let Ok(mut running) = self.running.lock() {
            running.pop();
        }
        outcome
    }

    fn compile_and_run(&self, at: &Path, parsed: &Parsed, ask: &Ask<'_>) -> Result<String, Wall> {
        let program = driver(parsed, ask)?;
        // **Keyed on what went into it** — the driver text carries the
        // grammar's lowering, the types it uses and the dump, so a digest of it
        // is a digest of everything this sub-project is.
        let key = crate::assets::digest(program.as_bytes());
        let dir = at.join(&key);
        let source = dir.join("src").join("main.rs");
        write_if_changed(&source, &program)?;
        write_if_changed(&dir.join("Cargo.toml"), &manifest(&key, &program))?;

        let built = Command::new(cargo())
            .arg("build")
            .arg("--quiet")
            .arg("--manifest-path")
            .arg(dir.join("Cargo.toml"))
            .arg("--target-dir")
            .arg(at.join("target"))
            .output()
            .map_err(|error| Wall::DidNotBuild {
                detail: format!("running cargo: {error}"),
            })?;
        if !built.status.success() {
            return Err(Wall::DidNotBuild {
                detail: String::from_utf8_lossy(&built.stderr).to_string(),
            });
        }

        // **The bytes go in a file**, because that is what a parser is pointed
        // at and what the diagnostic's line and column are counted against.
        let input = dir.join("input");
        write_if_changed(&input, ask.input)?;

        let binary = at.join("target").join("debug").join(format!("p{key}"));
        let ran = Command::new(&binary)
            .arg(&input)
            .output()
            .map_err(|error| Wall::DidNotBuild {
                detail: format!("running {}: {error}", binary.display()),
            })?;
        match ran.status.success() {
            true => Ok(String::from_utf8_lossy(&ran.stdout).trim().to_string()),
            false => Err(Wall::Refused {
                detail: String::from_utf8_lossy(&ran.stderr).to_string(),
            }),
        }
    }
}

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string())
}

/// **Written only when it changed**, which is what keeps Cargo's own freshness
/// working: a file rewritten with identical bytes is newer than the fingerprint
/// and rebuilds for ever. The build cache learned this the same way
/// ([ADR-021](../../../docs/specification/adr/adr-021.md)).
fn write_if_changed(path: &Path, text: &str) -> Result<(), Wall> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| Wall::DidNotBuild {
            detail: format!("creating {}: {error}", parent.display()),
        })?;
    }
    if std::fs::read_to_string(path).is_ok_and(|held| held == text) {
        return Ok(());
    }
    std::fs::write(path, text).map_err(|error| Wall::DidNotBuild {
        detail: format!("writing {}: {error}", path.display()),
    })
}

/// The sub-project's manifest.
///
/// **Its own workspace**, because it lives under a `target/` that may be inside
/// one: a member Cargo did not expect is an error about a file nobody wrote.
/// The dependencies are the ones a generated program already gets
/// (`project::runtime_dependencies`), so the parser compiled here is the parser
/// the program links.
fn manifest(key: &str, program: &str) -> String {
    let mut out = String::from(
        "# GENERATED. A parser compiled so that a grammar can run while the program\n\
         # is built (issue #178). Rewritten when the grammar changes.\n\n\
         [workspace]\n\n\
         [package]\n",
    );
    out.push_str(&format!("name = \"p{key}\"\n"));
    out.push_str("version = \"0.0.0\"\nedition = \"2024\"\n\n[[bin]]\n");
    out.push_str(&format!("name = \"p{key}\"\npath = \"src/main.rs\"\n\n"));
    out.push_str("[dependencies]\n");
    let dependencies = crate::project::runtime_dependencies_for(program);
    let traced = dependencies
        .iter()
        .any(|(name, _)| name == "\"winnow-grammar\"");
    for (name, value) in dependencies {
        out.push_str(&format!("{name} = {value}\n"));
    }
    // The same `trace` a program declares (`project::runtime_features`).
    if traced {
        out.push_str("\n[features]\ntrace = [\"winnow-grammar/trace\"]\n");
    }
    out
}

/// The whole sub-program: the file's items without its `fn main` and without
/// its item-level `comptime`s, then the dump, then a `main` that parses.
fn driver(parsed: &Parsed, ask: &Ask<'_>) -> Result<String, Wall> {
    let items = parsed.keeping(|item| match item {
        // **The program's `main` is not this program's** — the driver below is.
        Item::Fn { name, .. } => name.map(|n| parsed.text(n) != "main").unwrap_or(true),
        // **And its constants are not needed**, which is also what stops this
        // recursing: a grammar run happens in a `comptime` initialiser, and the
        // sub-project has none to evaluate.
        Item::Comptime { .. } => false,
        _ => true,
    });
    // **The sub-program owns what the program views**
    // ([ADR-179](../../../docs/specification/adr/adr-179.md) D3): a rule action
    // builds a `Vec`, so a struct field the program declares `&[T]` is a
    // `Vec[T]` here. The dump below is generated from the **program's**
    // declaration and reads a run either way, which is what keeps the two
    // sides one crossing rather than two opinions.
    let items = items.growing();
    let lowered = crate::emit::emit_program(&items, crate::emit::Build::default())
        .map_err(|error| Wall::DidNotBuild {
            detail: format!("lowering the grammar: {error:#}"),
        })?
        .rust;
    // The generated `fn main` and its site table go with it: this program has
    // its own entry point.
    let body = lowered
        .split("\nfn main() {")
        .next()
        .unwrap_or(&lowered)
        .to_string();

    let mut out = body;
    out.push_str(DUMP_HELPERS);
    let dump = dumper(parsed, ask.result)?;
    out.push_str(&format!(
        "\nfn main() {{\n\
         \x20   let path = std::env::args().nth(1).expect(\"the input path\");\n\
         \x20   let text = std::fs::read_to_string(&path).expect(\"reading the input\");\n\
         \x20   let outcome = {{\n\
         \x20       use winnow::Parser;\n\
         \x20       let _source = &*text;\n\
         \x20       let mut stream = winnow_grammar::ParseInput::<()> {{\n\
         \x20           state: winnow_grammar::ParseContext::<()>::default(),\n\
         \x20           input: winnow::stream::LocatingSlice::new(_source),\n\
         \x20       }};\n\
         \x20       {}::parse_{}().parse_next(&mut stream).map_err(|e| e.render(_source))\n\
         \x20   }};\n\
         \x20   match outcome {{\n\
         \x20       Ok(value) => {{\n\
         \x20           let mut out = String::new();\n\
         {}\
         \x20           println!(\"{{out}}\");\n\
         \x20       }}\n\
         \x20       Err(message) => {{\n\
         \x20           eprint!(\"{{message}}\");\n\
         \x20           std::process::exit(2);\n\
         \x20       }}\n\
         \x20   }}\n\
         }}\n",
        ask.grammar, ask.rule, dump
    ));
    Ok(out)
}

/// The encoder's fixed half: text, which is the only shape with an escape.
///
/// **The same escape set the rest of this compiler reads**
/// ([`crate::build_time::decoded`](../build_time/fn.decoded.html)), which is
/// Rust's — so what this writes is what that reads, and the pair is held
/// together by a test that runs a program rather than by care.
const DUMP_HELPERS: &str = r#"
fn __nikaia_text(out: &mut String, s: &str) {
    out.push_str("(s \"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push_str("\")");
}
"#;

/// **The dump, written inline**, from the declaration and not from the value,
/// so a shape a `const` cannot hold is refused *before* a parser is compiled.
/// **Written in Nikaia** (`tools/dump.nika`, #125); what stays here is reading
/// the program's declarations into the shapes it walks.
fn dumper(parsed: &Parsed, result: &Ty) -> Result<String, Wall> {
    let mut out = String::new();
    match nikaia_std::tools::dump::dump(&shapes(parsed), result, "value", 1, &mut out) {
        None => Ok(out),
        Some(refused) => Err(Wall::NoCrossedForm {
            ty: refused.ty,
            because: refused.because,
        }),
    }
}

/// The `enum`s and `struct`s this program declares, each by its name, with
/// each field's name as Rust reads it; the first declaration of a name is the
/// one read.
fn shapes(parsed: &Parsed) -> Shapes {
    let mut shapes = Shapes {
        enums: Default::default(),
        structs: Default::default(),
    };
    for item in &parsed.program.items {
        match &item.node {
            Item::Enum { name, variants, .. } => {
                let variants = variants
                    .iter()
                    .map(|held| {
                        let (carried, named) = match &held.fields {
                            crate::ast::VariantFields::Unit => (Vec::new(), false),
                            crate::ast::VariantFields::Tuple(types) => (
                                types.iter().map(|t| Ty::from_ast(parsed, t)).collect(),
                                false,
                            ),
                            crate::ast::VariantFields::Named(fields) => (
                                fields.iter().map(|f| Ty::from_ast(parsed, &f.ty)).collect(),
                                true,
                            ),
                        };
                        VariantShape {
                            name: parsed.text(held.name).to_string(),
                            carried,
                            named,
                        }
                    })
                    .collect();
                shapes
                    .enums
                    .entry(parsed.text(*name).to_string())
                    .or_insert(variants);
            }
            Item::Struct { name, fields, .. } => {
                let fields = fields
                    .iter()
                    .map(|field| {
                        let name = parsed.text(field.name);
                        FieldShape {
                            name: name.to_string(),
                            spelled: crate::emit::escaped(name).into_owned(),
                            ty: Ty::from_ast(parsed, &field.ty),
                        }
                    })
                    .collect();
                shapes
                    .structs
                    .entry(parsed.text(*name).to_string())
                    .or_insert(fields);
            }
            _ => {}
        }
    }
    shapes
}

/// **Reading back what the sub-program wrote.**
///
/// The encoder is generated above and this reads it — two halves of one format,
/// which is the shape [`crate::fixed`] already carries a warning about. What
/// holds them together is not care but a test that **runs** a program: a
/// decoder that agreed with a generator it never met would prove nothing.
///
/// The form is s-expressions, one per shape a build-time value has:
/// `(i 42)`, `(f 1.5)`, `(b true)`, `(s "…")`, `(l …)`, `(t Name (field …) …)`
/// and `(v Ty Variant …)`. Text carries Rust's escapes, which is the set the
/// rest of this compiler reads.
///
/// **A variant's tag is two words and not one**, which is the whole of what
/// the one defect here was: `(v Shade Odd)` read the type and then read the
/// variant from the *same* position, so the variant came back empty and the
/// payload loop met an `O`. A separator between two words has to be skipped by
/// whoever reads the second one.
pub fn decode(text: &str) -> Result<crate::build_time::Value, Wall> {
    let mut at = text.char_indices().peekable();
    let value = read(text, &mut at)?;
    Ok(value)
}

type Cursor<'a> = std::iter::Peekable<std::str::CharIndices<'a>>;

fn read(text: &str, at: &mut Cursor<'_>) -> Result<crate::build_time::Value, Wall> {
    skip_blanks(at);
    expect(at, '(')?;
    skip_blanks(at);
    let tag = word(text, at);
    skip_blanks(at);
    let value = match tag.as_str() {
        "i" => {
            let digits = word(text, at);
            crate::build_time::Value::Int(digits.parse().map_err(|_| Wall::Unreadable {
                detail: format!("`{digits}` is not a whole number"),
            })?)
        }
        "f" => {
            let written = word(text, at);
            crate::build_time::Value::Float(written.parse().map_err(|_| Wall::Unreadable {
                detail: format!("`{written}` is not a number"),
            })?)
        }
        "b" => crate::build_time::Value::Bool(word(text, at) == "true"),
        "v" => {
            let ty = word(text, at);
            skip_blanks(at);
            let variant = word(text, at);
            let mut payload = Vec::new();
            loop {
                skip_blanks(at);
                match at.peek() {
                    Some((_, ')')) | None => break,
                    _ => payload.push(read(text, at)?),
                }
            }
            crate::build_time::Value::Variant {
                ty,
                variant,
                payload,
            }
        }
        "s" => crate::build_time::Value::Text(quoted(at)?),
        "l" => {
            let mut items = Vec::new();
            loop {
                skip_blanks(at);
                match at.peek() {
                    Some((_, ')')) | None => break,
                    _ => items.push(read(text, at)?),
                }
            }
            crate::build_time::Value::List(items)
        }
        "t" => {
            let name = word(text, at);
            let mut fields = std::collections::BTreeMap::new();
            loop {
                skip_blanks(at);
                match at.peek() {
                    Some((_, '(')) => {
                        at.next();
                        skip_blanks(at);
                        let field = word(text, at);
                        let held = read(text, at)?;
                        skip_blanks(at);
                        expect(at, ')')?;
                        fields.insert(field, held);
                    }
                    _ => break,
                }
            }
            crate::build_time::Value::Struct { name, fields }
        }
        other => {
            return Err(Wall::Unreadable {
                detail: format!("`{other}` is not a shape this reads"),
            });
        }
    };
    skip_blanks(at);
    expect(at, ')')?;
    Ok(value)
}

fn skip_blanks(at: &mut Cursor<'_>) {
    while matches!(at.peek(), Some((_, c)) if c.is_whitespace()) {
        at.next();
    }
}

fn expect(at: &mut Cursor<'_>, want: char) -> Result<(), Wall> {
    match at.next() {
        Some((_, found)) if found == want => Ok(()),
        Some((_, found)) => Err(Wall::Unreadable {
            detail: format!("expected `{want}` and found `{found}`"),
        }),
        None => Err(Wall::Unreadable {
            detail: format!("expected `{want}` and the dump ended"),
        }),
    }
}

/// A run of characters up to a blank, a bracket or the end.
fn word(_text: &str, at: &mut Cursor<'_>) -> String {
    let mut out = String::new();
    while let Some((_, c)) = at.peek() {
        if c.is_whitespace() || *c == '(' || *c == ')' {
            break;
        }
        out.push(*c);
        at.next();
    }
    out
}

/// `"…"` with Rust's escapes, decoded by the one function that reads them
/// ([`crate::build_time::decoded`]).
fn quoted(at: &mut Cursor<'_>) -> Result<String, Wall> {
    expect(at, '"')?;
    let mut written = String::new();
    loop {
        match at.next() {
            Some((_, '"')) => break,
            Some((_, '\\')) => {
                written.push('\\');
                match at.next() {
                    Some((_, c)) => written.push(c),
                    None => {
                        return Err(Wall::Unreadable {
                            detail: "the dump ended inside an escape".to_string(),
                        });
                    }
                }
            }
            Some((_, c)) => written.push(c),
            None => {
                return Err(Wall::Unreadable {
                    detail: "the dump ended inside a string".to_string(),
                });
            }
        }
    }
    crate::build_time::decoded(&written).ok_or(Wall::Unreadable {
        detail: format!("`{written}` contains an escape the compiler can't read"),
    })
}
