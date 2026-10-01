// crates/nikaia/src/modules.rs
//
// A program is more than one file, and **a package is a directory**
// (Part I, 9.1; ADR-047 D1).
//
// The files of a package see one another with no `use` at all: they share one
// namespace, so a name declared in any of them can be written in any other. What
// the package offers outward is whatever says `pub`, in whichever file it is
// declared - which is what keeps a library's internal file layout from being its
// public surface.
//
// What this does is **resolution**, and resolution is the one thing ADR-011 D2
// said Stage 0 does not do: the lowering is name for name, and nothing looks a
// name up. That stays true of the *emitter* - what changes is that the compiler
// knows which files take part, and hands the emitter each of them in turn.
//
// The shape it produces is the plainest one the language below has: **one crate
// root**, with every file's items in it. No `mod`, no `use super::*`, nothing to
// resolve - because one namespace in this language is one namespace in that one.
// Two files declaring the same name is refused here rather than left to `rustc`,
// which would report it about a file nobody wrote (Part III, C.1).
//
// **Which files** is the directory and not the `use` lines. A `use` names another
// package (ADR-046 D1), and depending on one is not built (ADR-047 §5), so a
// `use` that is not `std`'s is refused with what to do instead.

use crate::contracts::LedgerOps;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::ast::Item;
use crate::parser::{self, Parsed};

/// One file of a package.
pub struct Unit {
    /// The package this file belongs to, as a **consumer** would write it -
    /// and `None` for every file of the program being built, whose names are
    /// the program's own.
    ///
    /// It is not the file stem any more. A file is not a unit of naming
    /// (ADR-047 D1), so nothing about a name says which file it came from; the
    /// field stays because a package that arrives by path will fill it in
    /// (ADR-047 D2), and that is the same qualification one level up.
    pub package: Option<String>,
    /// The package names **inside this file** that are another word for a
    /// package this build already has a word for - see
    /// [`Dependency::renames`]. Empty for every file of the program itself,
    /// whose words are this build's by definition.
    pub renames: BTreeMap<String, String>,
    pub path: PathBuf,
    pub source: String,
    pub parsed: Parsed,
}

impl Unit {
    /// How a consumer writes a name from this file: `http::serve` for another
    /// package, and plain `serve` inside the program's own.
    pub fn qualify(&self, name: &str) -> String {
        match &self.package {
            Some(package) => format!("{package}::{name}"),
            None => name.to_string(),
        }
    }
}

/// A package this program depends on: the name a `use` writes, and where it is
/// ([ADR-047](../../../docs/specification/adr/adr-047.md) D2).
///
/// The name is the **manifest key** and nothing inside the package, which is D2
/// rule 1: a package does not name itself, so two libraries that both want to be
/// `http` are the consumer's to name apart.
#[derive(Debug, Clone)]
pub struct Dependency {
    pub name: String,
    /// The package's own directory - the one its `src/` is in.
    pub root: PathBuf,
    /// The names **it** may reach - its own manifest keys. Its files are read
    /// here so that this program's checks know its public surface, and they are
    /// checked as it would check them
    /// ([ADR-053](../../../docs/specification/adr/adr-053.md) D3): a package
    /// that depends on a package is a package, not a rule broken.
    pub reachable: BTreeSet<String>,
    /// How **its** package keys are read in this build: its own word for a
    /// package, mapped to the one word this build uses for that directory
    /// ([ADR-053](../../../docs/specification/adr/adr-053.md) D2, and
    /// `project::renames_in` computes it).
    ///
    /// Empty for a dependency that depends on nothing, which is most of them,
    /// and empty for every key it already spells the way this build does.
    pub renames: BTreeMap<String, String>,
}

/// Every file of the package the entry belongs to, entry first.
///
/// **The directory decides, not the `use` lines** (ADR-047 D1). Every `.nika`
/// beside the entry takes part, whether or not anything names it - which is what
/// makes moving a declaration from one file to another housekeeping rather than a
/// change to the package's surface.
///
/// Entry first because the entry is where `fn main` may be written, and the rest
/// sorted by file name: the order files are emitted in has to be a function of
/// the source tree alone (Part III 13.5's determinism), and a directory listing
/// is not sorted anywhere.
pub fn collect(entry: &Path) -> Result<Vec<Unit>> {
    collect_with(entry, &[])
}

/// The same, with the packages this program depends on
/// ([ADR-047](../../../docs/specification/adr/adr-047.md) D2).
///
/// A dependency's files come **after** the program's own, in the order the
/// manifest sorted its names, so the emitted file stays a function of the source
/// tree (Part III 13.5). Each is read from the package's `src/`, because a
/// dependency is a project and that is where a project keeps its files
/// (Part III, 13.1).
pub fn collect_with(entry: &Path, dependencies: &[Dependency]) -> Result<Vec<Unit>> {
    let names: BTreeSet<String> = dependencies.iter().map(|d| d.name.clone()).collect();
    let mut units = package_at(entry, None, &names, &BTreeMap::new())?;

    // **Read, and not emitted** ([ADR-053](../../../docs/specification/adr/adr-053.md)
    // D1). A dependency is its own crate now, so its files are Cargo's to
    // compile - but its *surface* is this program's to check against, and a
    // package's surface is what its files declare. So they arrive here tagged
    // with the name this program reaches them by, the ledger and the checks see
    // them, and `Program::emit` writes none of them.
    //
    // **And its own `use` lines are checked against its own manifest keys**,
    // not against nothing: a package that depends on a package is a package
    // (D3), and the level below it is its business rather than a rule it broke.
    for dependency in dependencies {
        let src = dependency.root.join(SRC);
        if !src.is_dir() {
            return Err(crate::diagnostics::refuse(format!(
                "`{}` is a dependency by path, but {} doesn't exist.\n\
                 A package is a directory with a `{SRC}/` folder in it.",
                dependency.name,
                src.display()
            )));
        }
        units.extend(package_at(
            &src.join(ENTRY),
            Some(&dependency.name),
            &dependency.reachable,
            &dependency.renames,
        )?);
    }
    Ok(units)
}

/// One package: the directory an entry sits in, checked as a namespace of its
/// own.
fn package_at(
    entry: &Path,
    package: Option<&str>,
    reachable: &BTreeSet<String>,
    renames: &BTreeMap<String, String>,
) -> Result<Vec<Unit>> {
    let base = entry
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let mut beside: Vec<PathBuf> = std::fs::read_dir(&base)
        .with_context(|| format!("cannot read {}", base.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "nika"))
        .filter(|path| path != entry)
        .collect();
    beside.sort();

    // A library needs no `main.nika`: what a package offers is its public
    // surface, and an entry point is what a *program* has.
    let mut units = match entry.is_file() {
        true => vec![read_unit(entry, package, reachable, renames)?],
        false => Vec::new(),
    };
    for path in beside {
        units.push(read_unit(&path, package, reachable, renames)?);
    }
    one_namespace(&units)?;
    Ok(units)
}

/// Where a project keeps its sources and what its entry point is called
/// (Part III, 13.1).
const SRC: &str = "src";
const ENTRY: &str = "main.nika";

/// The entry and nothing beside it - a `.nika` file compiled on its own.
///
/// `--input` outside a project is not a package: a directory of loose examples is
/// a directory of programs, and compiling one of them must not pull in the other
/// ten. A package is a directory **of a project**, which is what a `nikaia.toml`
/// declares.
pub fn collect_one(entry: &Path) -> Result<Vec<Unit>> {
    Ok(vec![read_unit(
        entry,
        None,
        &BTreeSet::new(),
        &BTreeMap::new(),
    )?])
}

fn read_unit(
    path: &Path,
    package: Option<&str>,
    reachable: &BTreeSet<String>,
    renames: &BTreeMap<String, String>,
) -> Result<Unit> {
    let source =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    // `with_context` and not `anyhow!("{e}")`: formatting the error into a string
    // loses its type, and the type is what says this is a refusal of the program
    // rather than a failure of this compiler (`diagnostics::Refused`).
    // A parse error is a finding, rendered here where the path is known; any
    // other failure keeps the path as its context.
    let parsed =
        parser::parse_to_ast(&source).map_err(
            |error| match crate::diagnostics::refused_finding(&error) {
                Some(finding) => crate::diagnostics::refuse(
                    crate::diagnostics::render_finding(
                        finding,
                        &path.display().to_string(),
                        &source,
                    )
                    .trim_end()
                    .to_string(),
                ),
                None => error.context(path.display().to_string()),
            },
        )?;
    check_imports(&parsed, path, reachable)?;
    Ok(Unit {
        package: package.map(str::to_string),
        renames: renames.clone(),
        path: path.to_path_buf(),
        source,
        parsed,
    })
}

/// **Two files of one package may not declare the same name** (ADR-047 D1).
///
/// One namespace, so two `Row`s in it is an error rather than a rule about which
/// of them a line means. Refused here and not left to `rustc`: it would report a
/// duplicate definition against the generated file, which is exactly what
/// Part III C.1 forbids - and it would name a line the user never wrote.
///
/// **Two files and not one**
/// ([ADR-144](../../../docs/specification/adr/adr-144.md) D3). A name declared
/// twice in *one* file is `NK1148` in the checker, which every build runs -
/// where this runs only for a build that has a manifest. This kept the one-file
/// case for as long as it was the only rule there was, and served it badly:
/// a sentence about two files, naming one of them twice.
fn one_namespace(units: &[Unit]) -> Result<()> {
    let mut seen: BTreeMap<String, PathBuf> = BTreeMap::new();
    for unit in units {
        for name in declared_names(&unit.parsed) {
            if let Some(first) = seen.get(&name) {
                if first == &unit.path {
                    continue;
                }
                return Err(crate::diagnostics::refuse(format!(
                    "`{name}` is declared twice in this package: in {} and in {}.\n\
                     All files of a package share their names. Rename one of the two, or \
                     keep one declaration and use it from the other file.",
                    first.display(),
                    unit.path.display()
                )));
            }
            seen.insert(name, unit.path.clone());
        }
    }
    Ok(())
}

/// The names a file declares: what `one_namespace` counts.
///
/// A method is not among them - it belongs to its type and two types may each
/// have a `len`. An `impl` block declares nothing of its own, and neither does a
/// **rule** of a grammar: it belongs to its grammar and is reached as
/// `Json::value` ([ADR-140](../../../docs/specification/adr/adr-140.md) D3).
///
/// **A `trait` and a `grammar` are among them**
/// ([ADR-144](../../../docs/specification/adr/adr-144.md) D2), and were not:
/// `trait Foo` beside `struct Foo` was accepted through every path, including
/// this one, and `rustc` answered about the generated file.
fn declared_names(parsed: &Parsed) -> Vec<String> {
    parsed
        .program
        .items
        .iter()
        .filter_map(|item| match &item.node {
            Item::Fn { name, .. } => name.map(|name| parsed.text(name).to_string()),
            Item::Struct { name, .. } | Item::Enum { name, .. } | Item::Trait { name, .. } => {
                Some(parsed.text(*name).to_string())
            }
            Item::Grammar(def) => Some(parsed.text(def.name).to_string()),
            _ => None,
        })
        .collect()
}

/// **Every `use` in a file, against the packages it may name**
/// ([ADR-046](../../../docs/specification/adr/adr-046.md) D2, D4, D5).
///
/// `use` makes a package reachable and brings **no name** in, so there are four
/// things to say no to and one thing to accept. Each message names the way out,
/// because a rule a reader cannot act on is an obstacle (Part III, C.2):
///
/// * a **path** other than `std`'s — `use net::http` — which is either a nested
///   package (there is no such thing) or an attempt to import a name;
/// * a name that is a **file beside this one**, the shape that worked before
///   ADR-047 D1 made a package a directory;
/// * a name **no dependency declares**, which is D4 read from the other side: a
///   prefix must be introduced, and this is the introduction that names nothing;
/// * the **same name twice**, which is D5 — two packages under one name is an
///   error rather than a rule about which of them wins.
///
/// The braced and glob forms (`use pool::{Conn}`, `use pool::*`) are parse errors
/// at the brace and the star, so D2's sentence about them is not reachable from
/// here; that is [ADR-046](../../../docs/specification/adr/adr-046.md) §5's
/// remaining piece and it belongs in the grammar.
fn check_imports(parsed: &Parsed, at: &Path, reachable: &BTreeSet<String>) -> Result<()> {
    let here = at.display();

    // What each `use` names, and what this file calls it. The two differ exactly
    // where an alias is written (ADR-046 D3).
    let mut named: Vec<(&str, &str)> = Vec::new();

    // A path first, and every one of them, because it is the only shape that is
    // wrong about *itself* rather than about what the project declares.
    for item in &parsed.program.items {
        let Item::Import { path, alias } = &item.node else {
            continue;
        };
        let segments: Vec<&str> = path.iter().map(|s| parsed.text(*s)).collect();
        if matches!(segments.as_slice(), ["std", ..]) {
            continue;
        }
        if segments.len() > 1 {
            return Err(crate::diagnostics::refuse(format!(
                "`use {}` in {here}: `use` takes a package name only, not a path into it. \
                 Write `use {}`, and `{}` where you need it.",
                segments.join("::"),
                segments[0],
                segments.join("::")
            )));
        }
        let package = segments[0];
        named.push((package, alias.map_or(package, |a| parsed.text(a))));
    }

    // **D5 before D4**, because a name written twice is a fact about this file and
    // says nothing about whether either of them resolves: checking reachability
    // first would answer a file with two unknown names by naming one of them.
    //
    // And it counts the name **this file introduces**, which is the alias where
    // there is one: `use http as h` beside `use https as h` is the collision D3
    // exists to let a consumer fix, so it has to be the collision D5 catches.
    let mut once: BTreeSet<&str> = BTreeSet::new();
    for (_, here_name) in &named {
        if !once.insert(here_name) {
            return Err(crate::diagnostics::refuse(format!(
                "`{here_name}` is used twice in {here}. Two packages can't have the same \
                 name in one file. Write `use … as …` to give one of them another name."
            )));
        }
    }

    for (package, _) in named {
        if reachable.contains(package) {
            continue;
        }
        let beside = at
            .parent()
            .map(|dir| dir.join(format!("{package}.nika")))
            .is_some_and(|path| path.is_file());
        return Err(crate::diagnostics::refuse(match beside {
            true => format!(
                "`use {package}` in {here}: the files of a package already see each other, \
                 so there's nothing to bring in. Remove the line and use the name directly."
            ),
            false => format!(
                "`use {package}` in {here}: there's no dependency called `{package}`.\n\
                 Add it to the project's `[dependencies]` under that name:\n\
                 \x20   [dependencies]\n\
                 \x20   {package} = {{ path = \"../{package}\" }}"
            ),
        }));
    }
    Ok(())
}

/// The units of one package, by file name and the SHA-256 of their bytes
/// ([ADR-100](../../../docs/specification/adr/adr-100.md) D3).
///
/// **The same digest the cache key uses** ([ADR-021](../../../docs/specification/adr/adr-021.md)),
/// because the record says so and because a build that hashed the same file
/// twice with two digests would be paying twice to disagree with itself.
fn sources_of<'a>(units: impl IntoIterator<Item = &'a Unit>) -> BTreeMap<String, String> {
    units
        .into_iter()
        .map(|unit| {
            let name = unit
                .path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                // A unit always comes from a file; a path that ends in nothing
                // is not one this compiler read, and naming it by its whole
                // spelling is better than dropping it out of the table.
                .unwrap_or_else(|| unit.path.to_string_lossy().to_string());
            (
                name,
                orchestrator::cache::sha256_hex(unit.source.as_bytes()),
            )
        })
        .collect()
}

/// The runs of units that belong to one package, in the order they were read.
///
/// The units arrive grouped (`collect_with`), so this is a walk and not a sort.
fn groups(units: &[Unit]) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < units.len() {
        let from = at;
        while at < units.len() && units[at].package == units[from].package {
            at += 1;
        }
        out.push(from..at);
    }
    out
}

/// One package's contracts: the ledger it ships where that may be believed, and
/// the inference over its own units where it may not
/// ([ADR-100](../../../docs/specification/adr/adr-100.md) D1 and D3).
///
/// **Believed is the fast path and the *correct* one**, not merely the quick
/// one. A package's own build had its own dependencies in view and this one does
/// not (ADR-053 D3), so an answer derived here can only be the same or worse -
/// and two builds that derive the same function differently is the divergence
/// D1 exists to make impossible.
///
/// **And it is never believed against its own sources.** The hashes in the
/// header say which files the entries are an answer about; where one does not
/// match, that package is derived again here. A ledger with no `[sources]` at
/// all is treated as not matching, which is the fail-closed direction
/// ([ADR-010](../../../docs/specification/adr/adr-010.md) D1): *nothing was
/// recorded* must not read as *nothing changed*.
fn package_ledger(
    units: &[Unit],
    dependency: Option<&Dependency>,
    library: &crate::contracts::Ledger,
) -> crate::contracts::Ledger {
    let sources = sources_of(units);

    if let Some(dependency) = dependency
        && let Some(shipped) = shipped_ledger(&dependency.root)
        && shipped.stale_against(&sources).is_empty()
    {
        return shipped.published(&dependency.reachable);
    }

    let parsed: Vec<&Parsed> = units.iter().map(|u| &u.parsed).collect();
    let mut own = crate::contracts::Ledger::infer_package(&parsed, library);
    // **What these entries are an answer about** (D3), recorded here because
    // this is the only place that has both the answer and the files it came
    // from - and recorded for every package, so that the ledger a dependency's
    // own build writes says what it was derived from.
    own.sources = sources;
    own
}

/// A package's committed `nikaia.contracts`, where it has one this compiler can
/// read.
///
/// **A ledger that does not parse is not an error here.** It is a generated file
/// a person may have edited or an older compiler may have written, and the
/// answer to both is the same as to a stale one: derive this package again. A
/// build that failed because a *cache* was unreadable would be a worse build
/// than one that is slower.
fn shipped_ledger(root: &Path) -> Option<crate::contracts::Ledger> {
    crate::contracts::Ledger::read_beside(&root.join("nikaia.contracts"))
}

/// A whole program: one ledger, one Rust file, one source map.
pub struct Program {
    pub units: Vec<Unit>,
    /// **What the manifest's described crates say about their boundary**
    /// ([ADR-104](../../../docs/specification/adr/adr-104.md) D1,
    /// [ADR-237](../../../docs/specification/adr/adr-237.md) D1): read by the
    /// emitter for one question, whether a call into one can fail. Empty
    /// outside a project and for a project that declares no crate.
    pub described: crate::contracts::Ledger,
    /// One ledger for the project (Part III, 13.5), with every module's entries
    /// under the name a caller writes.
    pub contracts: crate::contracts::Ledger,
    /// **Each dependency's ledger as that package itself wrote it**, before
    /// `absorb` qualified its keys and the types inside them.
    ///
    /// `contracts` above is the right ledger for the *program's* files and the
    /// wrong one for a package's own: absorbing turns `Request` into
    /// `http::Request`, in the key and in every signature that names it, and the
    /// package's own file writes the bare word as its author must
    /// ([ADR-046](../../../docs/specification/adr/adr-046.md) D2 gives no import
    /// to write). Checked against the absorbed one, a package's `[H: Handler]`
    /// found no trait and `answer(connection, handler)` was told its own
    /// `fn(Request) -> Response` is not a `fn(http::Request) -> http::Response` —
    /// a correct program refused, twice over
    /// ([Part III C.4](../../../docs/specification/30-nikaia-tooling.md)).
    ///
    /// **Kept rather than reconstructed.** Un-qualifying the absorbed ledger
    /// would be `absorb` run backwards, and an inverse that is nearly right is
    /// worse than none: this is the very ledger the package was inferred with, so
    /// there is nothing to get wrong.
    pub as_its_own: std::collections::BTreeMap<String, crate::contracts::Ledger>,
    /// **The program's `test` blocks**, in the order `nikaia test` numbers
    /// them ([ADR-245](../../../docs/specification/adr/adr-245.md) D1). Empty
    /// in every build but a test build, which is the only one that keeps them.
    pub tests: Vec<TestCase>,
}

/// One `test "…" { … }` of the program, as `nikaia test` names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestCase {
    /// The title as written, escapes and all.
    pub title: String,
    pub path: PathBuf,
    /// The line the block starts on.
    pub line: usize,
}

/// The name the `k`th test's function has in a test build.
fn test_function(k: usize) -> String {
    format!("__nikaia_test_{k}")
}

/// Whether a function is a `test` block a test build turned into one
/// ([`test_function`]): its `assert`s are the test's verdict, not claims the
/// prover holds ([ADR-256](../../docs/specification/adr/adr-256.md) D2).
pub fn is_a_test_function(name: &str) -> bool {
    name.strip_prefix("__nikaia_test_")
        .is_some_and(|k| !k.is_empty() && k.bytes().all(|b| b.is_ascii_digit()))
}

/// The function a test build's entry dispatches from, before it takes `main`'s
/// place.
const DISPATCH: &str = "__nikaia_tests";

impl Program {
    /// Parse every file, and infer the contracts of all of them together.
    ///
    /// Together, and not one at a time: `sync` is a fixpoint over the call
    /// graph (ADR-027), and a call graph that stops at a file boundary would
    /// give a different answer depending on which file was looked at first.
    /// 13.5 makes the ledger a pure function of the source tree, which is a
    /// promise about the tree and not about each file in it.
    pub fn read(entry: &Path) -> Result<Program> {
        Self::of(collect(entry)?, &[])
    }

    /// The same, with the packages this program depends on
    /// ([ADR-047](../../../docs/specification/adr/adr-047.md) D2).
    pub fn read_with(entry: &Path, dependencies: &[Dependency]) -> Result<Program> {
        Self::of(collect_with(entry, dependencies)?, dependencies)
    }

    /// The entry compiled on its own - what `--input` outside a project is
    /// (see [`collect_one`]).
    pub fn read_one(entry: &Path) -> Result<Program> {
        Self::of(collect_one(entry)?, &[])
    }

    /// **The program as `nikaia test` builds it**
    /// ([ADR-245](../../../docs/specification/adr/adr-245.md) D1): every
    /// `test` block of the program's own files is a function, and the entry's
    /// `main` is replaced by one that runs the test whose number it is given
    /// as its first argument - so the program is compiled once and each test
    /// runs in a process of its own. A package that is a dependency keeps no
    /// tests; nor does an entry with no test, which is then the program as
    /// every other build sees it.
    pub fn read_for_tests(
        entry: &Path,
        dependencies: &[Dependency],
        in_project: bool,
    ) -> Result<Program> {
        let units = match in_project {
            true => collect_with(entry, dependencies)?,
            false => collect_one(entry)?,
        };
        let (units, tests) = with_tests_as_functions(units)?;
        let mut program = Self::of(units, dependencies)?;
        program.tests = tests;
        Ok(program)
    }

    fn of(mut units: Vec<Unit>, dependencies: &[Dependency]) -> Result<Program> {
        // **A `test` block is compiled only by `nikaia test`** (ADR-245 D1),
        // which has turned every one it runs into a function by now: what is
        // left is left out.
        for unit in &mut units {
            unit.parsed
                .program
                .items
                .retain(|item| !matches!(item.node, Item::Test { .. }));
        }
        let mut contracts = crate::contracts::Ledger::empty();
        let mut as_its_own: std::collections::BTreeMap<String, crate::contracts::Ledger> =
            std::collections::BTreeMap::new();

        // **A package at a time, not a file at a time.** `absorb` qualifies the
        // types *inside* an entry with the package's name, and it can only do
        // that for the types the ledger it is given declares - so a package whose
        // `Request` is in one file and whose `route(r: Request)` is in another has
        // to arrive as one ledger, or the signature keeps the bare name and a
        // caller writing `http::Request` is told the two are different types.
        // That was the file-level defect (issue #229's cross-file type entry,
        // since closed) one level up.
        //
        // The units arrive grouped (`collect_with`), so this is a walk and not a
        // sort - and the order inside a package is the order they were read in,
        // which Part III 13.5 needs to stay a function of the tree.
        //
        // **And the inference is the package's, not each file's**
        // ([ADR-100](../../../docs/specification/adr/adr-100.md) D2). The group
        // is handed over whole rather than inferred file by file and merged
        // afterwards: merging joins *answers*, and the answer to `sync` is a
        // fixpoint over a call graph, so a graph that stopped at the file
        // boundary had already read a callee in the file next door as one it
        // could not vouch for. That is what `NK1129` was refusing.
        //
        // **The dependencies first, and the program's own package last**
        // (D5). A package's ledger is a build input of everything that depends
        // on it, so it has to exist before its consumer is checked - and what
        // the consumer then resolves `http::ok` against is that *answer*,
        // rather than a worse one derived here from half the graph. That order
        // is the whole of why this is two passes over the groups and not one.
        let mut library = crate::contracts::std_library();

        for group in groups(&units) {
            let Some(package) = units[group.start].package.clone() else {
                continue;
            };
            let renames = units[group.start].renames.clone();
            let own = package_ledger(
                &units[group.clone()],
                dependencies.iter().find(|d| d.name == package),
                // **`std` alone for a dependency's own inference.** Its own
                // dependencies are deliberately invisible here (ADR-053 D3),
                // which is exactly why a ledger derived on this side is the
                // second-best answer and D1 prefers the one it shipped.
                &crate::contracts::std_library(),
            );
            as_its_own.insert(package.clone(), own.clone());
            library.absorb_renaming(Some(&package), &renames, own.clone());
            contracts.absorb_renaming(Some(&package), &renames, own);
        }

        for group in groups(&units) {
            if units[group.start].package.is_some() {
                continue;
            }
            let renames = units[group.start].renames.clone();
            let own = package_ledger(&units[group.clone()], None, &library);
            contracts.sources = own.sources.clone();
            contracts.absorb_renaming(None, &renames, own);
        }

        Ok(Program {
            units,
            described: crate::contracts::Ledger::blank(),
            contracts,
            as_its_own,
            tests: Vec::new(),
        })
    }

    /// The **packages** this program reaches, by the name a `use` writes.
    ///
    /// What it is for is telling a `package::item` call from a `Type::method`
    /// one. Empty for a program of its own files, which is every program today:
    /// the files of a package share one namespace, so nothing in it is qualified
    /// (ADR-047 D1), and depending on another package is not built (D2).
    pub fn package_names(&self) -> std::collections::BTreeSet<String> {
        self.units
            .iter()
            .filter_map(|u| u.package.clone())
            .collect()
    }

    /// Whether this is one file, in which case nothing about the build changes.
    pub fn is_single_file(&self) -> bool {
        self.units.len() == 1
    }

    /// Every source that took part, in the order they are emitted - which is
    /// what the cache key has to cover (Part III, 13.1: "the SHA256 of each
    /// `.nika` source that took part").
    pub fn sources(&self) -> Vec<&str> {
        self.units.iter().map(|u| u.source.as_str()).collect()
    }

    /// The whole package as one Rust file.
    ///
    /// **One crate root per package** (ADR-047 D1). One namespace in this
    /// language is one namespace in the language below, so the program's own
    /// files need no `mod`, no `use super::*` and nothing for a reader to
    /// resolve. `pub` becomes `pub`, which is what publishes a name out of the
    /// package - the same move ADR-017 D2 made with `html::Render`: put the rule
    /// where the language below can act on it rather than re-implementing it
    /// here.
    ///
    /// **A dependency is a `mod` at that root** (D2), named by the manifest key,
    /// so `http::serve()` in Nikaia is `http::serve()` in Rust and nothing
    /// resolved it (ADR-011 D2). Each opens with `use super::*` under an
    /// `#[allow]`, which brings in the preamble the root wrote - the allow is
    /// because the import is **ours**, and a module that happens to use nothing
    /// from the preamble would otherwise produce a warning about a line no Nikaia
    /// source maps to (Part III, C.1).
    ///
    /// The program's own files go **first**, the entry first among them, because
    /// the entry is the file that may declare `main` and a reader opens the
    /// generated file at the top.
    pub fn emit(&self, build: crate::emit::Build) -> Result<crate::emit::Lowered> {
        self.emit_reading(build, &crate::assets::Reads::none())
    }

    /// The same, told what this build may read while it builds
    /// ([ADR-072](../../../docs/specification/adr/adr-072.md)).
    ///
    /// **A second entry point and not a field on `Build`**: `Build` is the
    /// machine and the switches, copied freely, and what a build may read is a
    /// fact about the invocation with a lifetime on it. A caller with nothing
    /// to say passes [`crate::assets::Reads::none`], which is D1.
    pub fn emit_reading(
        &self,
        build: crate::emit::Build,
        reads: &crate::assets::Reads,
    ) -> Result<crate::emit::Lowered> {
        use crate::emit::{Lowered, Needs, SourceMap};

        let trust = crate::contracts::trust::analyse(&self.units[0].parsed, std_ledger());
        let needs = self.units.iter().fold(Needs::default(), |acc, u| {
            acc.join(Needs::of(&u.parsed, build))
        });
        // **Every file, for every file.** What a `comptime` came to is the
        // checker's answer and the emitter writes it, so the two have to be
        // asked the same question — a unit lowered against fewer files than it
        // was checked against would refuse an item the check accepted.
        let beside: Vec<&crate::parser::Parsed> =
            self.units.iter().map(|unit| &unit.parsed).collect();

        let mut rust = String::new();
        rust.push_str("// Generated by the Nikaia bootstrap compiler (Stage 0).\n");
        rust.push_str("// Edit the .nika source, not this file.\n\n");
        rust.push_str(&needs.preamble());
        rust.push('\n');

        let mut map = SourceMap::default();
        for (at, unit) in self.units.iter().enumerate() {
            // **A dependency's files are read and not written** (ADR-053 D1).
            // They are here so the ledger and the checks know what the package
            // offers; the crate that offers it is generated beside this one, and
            // a qualified name needs no help from us - `c::thing()` is emitted
            // as written and Rust resolves `c` to the crate the manifest names.
            //
            // The index is still `self.units`', because a diagnostic about a
            // dependency's file has to name that file.
            if unit.package.is_some() {
                continue;
            }
            let body = crate::emit::emit_module_body_at(
                &unit.parsed,
                &beside,
                build,
                trust.provenance,
                &self.contracts,
                &self.described,
                // The entry is the only file ADR-038 D4's generated `fn main`
                // may be written from.
                at == 0,
                reads,
            )
            // **A refusal from the lowering gets its line here**
            // ([ADR-171](../../../docs/specification/adr/adr-171.md) D2), which
            // is the one place that has both the byte and the file: the
            // lowering knows the statement and not the path, and whoever
            // catches the error at the top knows neither.
            .map_err(|error| match crate::diagnostics::refusal_at(&error) {
                Some((byte, message)) => {
                    crate::diagnostics::refuse(crate::diagnostics::render_refusal(
                        &message,
                        byte,
                        &unit.path.display().to_string(),
                        &unit.source,
                    ))
                }
                None => error,
            })?;

            map.extend(body.map.placed(rust.len(), at));
            rust.push_str(&body.rust);
            rust.push('\n');
        }
        // **Last, because it names lines** (ADR-044 D1). Only where the program
        // has an entry point: the table is read by the hook `fn main` installs,
        // and a package with no `main` is a library whose consumer has one.
        if self.units[0].parsed.program.items.iter().any(|item| {
            matches!(&item.node, crate::ast::Item::Fn { name: Some(name), .. }
                if self.units[0].parsed.text(*name) == "main")
        }) {
            let paths: Vec<String> = self
                .units
                .iter()
                .map(|unit| unit.path.display().to_string())
                .collect();
            let sources: Vec<&str> = self.units.iter().map(|u| u.source.as_str()).collect();
            rust.push_str(&crate::emit::abort_table(&rust, &map, &paths, &sources));
        }

        Ok(Lowered { rust, map })
    }
}

/// **Every `test` block of the program's own files, as a function**, and the
/// entry's `main` replaced by the one that runs them by number
/// ([ADR-245](../../../docs/specification/adr/adr-245.md) D1).
///
/// The dispatcher is Nikaia, appended to the entry's text and parsed with it,
/// so it goes through every check and the one lowering a program does: a test
/// body may pause and may fail, and `main` is where both are already answered
/// (ADR-038 D4). A test's function `throws`, so a failure that leaves its body
/// leaves `main` - which prints it and ends the process unsuccessfully.
fn with_tests_as_functions(mut units: Vec<Unit>) -> Result<(Vec<Unit>, Vec<TestCase>)> {
    let own: Vec<usize> = (0..units.len())
        .filter(|&at| units[at].package.is_none())
        .collect();
    let count: usize = own
        .iter()
        .map(|&at| {
            units[at]
                .parsed
                .program
                .items
                .iter()
                .filter(|item| matches!(item.node, Item::Test { .. }))
                .count()
        })
        .sum();
    if count == 0 || units.is_empty() || units[0].package.is_some() {
        return Ok((units, Vec::new()));
    }

    // The entry first, because it is parsed again with the dispatcher in it.
    let entry = &units[0];
    let cli = entry
        .parsed
        .program
        .items
        .iter()
        .find_map(|item| match &item.node {
            Item::Import { path, alias }
                if path.len() == 2
                    && entry.parsed.text(path[0]) == "std"
                    && entry.parsed.text(path[1]) == "cli" =>
            {
                Some(alias.map_or("cli", |a| entry.parsed.text(a)).to_string())
            }
            _ => None,
        });
    let mut text = entry.source.clone();
    text.push('\n');
    if cli.is_none() {
        text.push_str("use std::cli\n");
    }
    let cli = cli.unwrap_or_else(|| "cli".to_string());
    text.push_str(&format!(
        "fn {DISPATCH}() throws {{\n    let which = {cli}::args().nth(1) ?? \"\"\n"
    ));
    for k in 0..count {
        text.push_str(&format!(
            "    if which == \"{k}\" {{\n        {}()\n        return\n    }}\n",
            test_function(k)
        ));
    }
    text.push_str("    panic(f\"this build has no test numbered `{which}`\")\n}\n");
    let mut parsed = parser::parse_to_ast(&text)
        .with_context(|| format!("{} with its tests", entry.path.display()))?;
    // `main` gives its name to the dispatcher and goes.
    let mut main = None;
    let interner = parsed.interner.clone();
    parsed.program.items.retain(|item| match &item.node {
        Item::Fn {
            name: Some(name), ..
        } if interner.resolve(*name) == "main" => {
            main = Some(*name);
            false
        }
        _ => true,
    });
    let main = match main {
        Some(name) => name,
        None => match parser::parse_expression(&parsed.interner, "main")? {
            crate::ast::Expr::Variable(name) => name,
            _ => anyhow::bail!("`main` did not parse as a name"),
        },
    };
    for item in &mut parsed.program.items {
        if let Item::Fn {
            name: Some(name), ..
        } = &mut item.node
            && parsed.interner.resolve(*name) == DISPATCH
        {
            *name = main;
        }
    }
    units[0].parsed = parsed;

    let mut tests = Vec::new();
    for at in own {
        let unit = &mut units[at];
        for item in &mut unit.parsed.program.items {
            let Item::Test { name: title, body } = &item.node else {
                continue;
            };
            let name =
                match parser::parse_expression(&unit.parsed.interner, &test_function(tests.len()))?
                {
                    crate::ast::Expr::Variable(name) => name,
                    _ => anyhow::bail!("a test's function name did not parse as a name"),
                };
            tests.push(TestCase {
                title: title.clone(),
                path: unit.path.clone(),
                line: unit.source[..item.span.at().min(unit.source.len())]
                    .matches('\n')
                    .count()
                    + 1,
            });
            item.node = Item::Fn {
                name: Some(name),
                generics: Vec::new(),
                receiver: None,
                args: Vec::new(),
                config: Vec::new(),
                spread: None,
                ret_type: None,
                body: body.clone(),
                is_sync: false,
                sync_by: Vec::new(),
                is_public: false,
                can_throw: true,
            };
        }
    }
    Ok((units, tests))
}

fn std_ledger() -> &'static crate::contracts::Ledger {
    crate::contracts::std_ledger()
}
