// crates/nikaia/src/describe.rs
//
// `nikaia describe <crate>` — a draft ledger for a Rust crate's boundary
// ([ADR-104](../../docs/specification/adr/adr-104.md) D2, D3, D4).
//
// ## What it is for
//
// D1 refuses a call into a crate nothing describes and names this command. What
// the command writes is `contracts/<crate>.contracts`, the file every analysis
// then reads at that boundary — the crossing verdict, `keeps`, `sync` and
// `throws` stop failing closed on a call whose signature answers plainly.
//
// ## What it reads, and what it cannot
//
// **The crate's sources**, because D4's better half is not available: rustdoc's
// JSON is unstable, and [ADR-001](../../docs/specification/adr/adr-001.md) D1
// does not give up a stable-only toolchain for it.
//
// **They are read by a grammar written in Nikaia**
// ([ADR-195](../../docs/specification/adr/adr-195.md) D3):
// `crates/nikaia-std/src/tools/rust.nika`, lowered ahead of time and reached
// from here as an ordinary Rust module — `nikaia_std::tools::rust`, a call and
// nothing else ([ADR-196](../../docs/specification/adr/adr-196.md) D1). What
// stood here before was a hand-written character scanner that matched `pub fn`
// at the start of a line and counted braces without knowing what a brace is;
// on one crate it wrote entries for **four functions that do not exist**
// (`crates/nikaia/tests/describing.rs`).
//
// What is left that it cannot do:
//
//   * an item a **macro** generates is not in the text and is not found. That
//     limit is [ADR-001](../../docs/specification/adr/adr-001.md) D1's and not
//     the parser's: it survives `syn` too, for the reason rustdoc's JSON is out
//     of reach;
//   * a signature this cannot translate is written `?`, which is the absence of
//     a claim and never a guess (D4's own sentence);
//   * a **method** is not described: what an `impl`'s `pub fn` is at a foreign
//     boundary is D4's own question and nothing here asks it. The parser reads
//     them; this does not write them down.
//   * a **macro** is still the only one. The `mod`-path limit is **closed**: a
//     module is a block or a file, `src/foo/bar.rs` is `foo::bar`, and whether
//     a caller may write it is read from the `pub mod foo;` in its parent. A
//     module nothing declares is **not** offered, which is fail-closed and
//     [ADR-010](../../docs/specification/adr/adr-010.md) D1's polarity — the
//     absence is *nobody said this is public*.
//
// Each of those is D5's case: the draft is **committed and reviewed like code**,
// and a `?` in it is a person's to fill. A describer that guessed would put a
// claim in a file nobody wrote, which is the one thing a boundary description
// may not do.
//
// **And the direction that matters more than dropping what is not there**: a
// `pub use` is read, and what it carries out of a private `mod` is offered
// under the name it gives. Refusing a call a crate really answers is
// [Part III C.4](../../docs/specification/30-nikaia-tooling.md), and the
// scanner refused every one of them.
//
// ## Which way it errs
//
// Fail-closed, everywhere the signature is silent: no `touches` (which reads as
// *touches everything*), no `locks` (that column's third answer), and no
// `crosses` (the absence is *nobody said*, never *it may not*). A Rust
// signature cannot tell anybody any of the three, and reading silence as
// "nothing" is the polarity [ADR-010](../../docs/specification/adr/adr-010.md)
// D1 forbids.

use crate::contracts::LedgerOps;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::contracts::ty::TyOps;
use crate::contracts::{FnContract, Ledger, Notes, Signature, Sync, TypeContract, ty::Ty};
use nikaia_std::tools::crossing;
use nikaia_std::tools::paths::{self, Export};

/// What a run of the command did, for the line it prints.
#[derive(Debug)]
pub struct Described {
    /// Where the draft was written — empty from [`draft`], which writes
    /// nothing.
    pub path: PathBuf,
    /// The crate's version, as the manifest declares it or `"?"`.
    pub version: String,
    /// How many functions and types the draft carries.
    pub functions: usize,
    pub types: usize,
    /// The names the program writes that no `pub` signature answered — D5's
    /// `?`, named so a reviewer knows what to fill rather than what to find.
    pub unanswered: Vec<String>,
    /// What the describer **saw and did not claim**
    /// ([ADR-193](../../docs/specification/adr/adr-193.md) D3, D5), written
    /// into the file as comments.
    pub notes: Notes,
}

/// Write `contracts/<crate>.contracts` for the crate the program calls.
pub fn describe(root: &Path, crate_word: &str) -> Result<Described> {
    let (ledger, mut written) = draft(root, crate_word)?;
    let directory = root.join(crate::project::CONTRACTS);
    std::fs::create_dir_all(&directory).with_context(|| format!("{}", directory.display()))?;
    let path = directory.join(format!("{crate_word}.contracts"));
    std::fs::write(
        &path,
        ledger.render_description(crate_word, &written.version, &written.notes),
    )
    .with_context(|| format!("{}", path.display()))?;
    // **And what it was written from, beside it** (ADR-251 D1): the crate's
    // sources by hash, which `sources_that_moved` compares a later build with.
    let derived = crate::contracts::derived_path(&path);
    std::fs::write(&derived, ledger.render_derived())
        .with_context(|| format!("{}", derived.display()))?;
    written.path = path;
    Ok(written)
}

/// The same draft, **without writing it**.
///
/// Split out because the interesting assertion is the file's *contents* and
/// every test that made one would otherwise have to write into a tree and take
/// it back out again — including the one that matters most, which reads the
/// repository's own `examples/foreign-runtime/shim` and would have to put the
/// reviewed file back afterwards.
pub fn draft(root: &Path, crate_word: &str) -> Result<(Ledger, Described)> {
    let manifest = crate::manifest::Manifest::read(&root.join("nikaia.toml"))
        .with_context(|| format!("{}/nikaia.toml", root.display()))?;
    let (declared, value) = rust_dependency(&manifest, crate_word)?;
    let sources = crate_sources(root, &value, crate_word, &declared)?;

    let wanted = names_the_program_writes(root, crate_word)?;
    let mut surface = Surface::default();
    let mut hashes = BTreeMap::new();
    for (relative, text) in &sources.files {
        hashes.insert(
            relative.clone(),
            orchestrator::cache::sha256_hex(text.as_bytes()),
        );
        surface.read(relative, text)?;
    }
    surface.resolve();
    let types = surface.types.clone();
    let fields = surface.fields.clone();
    let derives = surface.derives.clone();

    let mut ledger = Ledger::empty();
    ledger.inference = "described-from-signatures".to_string();
    ledger.sources = hashes;
    let mut unanswered = Vec::new();
    let mut named_types: BTreeSet<String> = BTreeSet::new();
    for name in &wanted {
        // **The path a caller writes, resolved through the `pub use` items**
        // ([ADR-196](../../docs/specification/adr/adr-196.md) D2's own reason
        // for reading them): a `pub fn` inside a private `mod` is reachable
        // after all when one says so, and refusing such a call would be
        // [Part III C.4](../../docs/specification/30-nikaia-tooling.md).
        let Some(function) = surface
            .reachable
            .get(name)
            .and_then(|at| surface.functions.get(at))
        else {
            unanswered.push(format!("{crate_word}::{name}"));
            continue;
        };
        let (contract, mentions) = function.contract(crate_word, &types);
        named_types.extend(mentions);
        ledger
            .functions
            .insert(format!("{crate_word}::{name}"), contract);
    }
    // **A written type is a reach across the boundary too** (D1), and a type a
    // described signature *names* is one the caller's compiler will look up. So
    // both sets get an entry: what the program wrote, and what the entries
    // mention.
    for name in wanted.iter().filter(|name| types.contains(*name)) {
        named_types.insert(name.clone());
    }
    for name in named_types {
        ledger.types.insert(
            format!("{crate_word}::{name}"),
            TypeContract {
                public: true,
                // **The one claim that comes from a field**
                // ([ADR-123](../../docs/specification/adr/adr-123.md) D2), and
                // the one thing here a Rust *signature* could never say.
                crosses: crossing::crosses(fields.get(&name).unwrap_or(&Vec::new())),
                // **What the type derives is what it promises**
                // ([ADR-252](../../docs/specification/adr/adr-252.md) D4.3):
                // `PartialEq` is `==`, and `Copy` is a copy where a move would
                // be. Read from the source and written for review, as the rest.
                compares: derives.get(&name).is_some_and(|d| d.contains("PartialEq")),
                copies: derives.get(&name).is_some_and(|d| d.contains("Copy")),
                ..TypeContract::empty()
            },
        );
    }

    // **What the describer saw and did not claim**
    // ([ADR-193](../../docs/specification/adr/adr-193.md) D3, D5), gathered
    // after the entries because a proposal is written above the one it is
    // about.
    let mut notes = Notes::empty();
    if !surface.promises.is_empty() {
        notes.about_the_crate.push(
            "**This crate makes promises the toolchain cannot check** (ADR-193 D5). A tool"
                .to_string(),
        );
        notes.about_the_crate.push(
            "can see that the promise was made; it cannot see whether it is true - which is"
                .to_string(),
        );
        notes.about_the_crate.push(
            "the line between a rule the toolchain enforces and one it inherits:".to_string(),
        );
        for promise in &surface.promises {
            notes.about_the_crate.push(format!("  {promise}"));
        }
    }
    // What each of the crate's functions calls, by its path: what
    // `crossing::reaches_a_thread` follows.
    let calls: BTreeMap<String, Vec<String>> = surface
        .functions
        .iter()
        .map(|(path, function)| (path.clone(), function.calls.clone()))
        .collect();
    for name in &wanted {
        let Some(function) = surface
            .reachable
            .get(name)
            .and_then(|at| surface.functions.get(at))
        else {
            continue;
        };
        let mut said = Vec::new();

        // **The bound**, which carries the safe half on its own (D4): every
        // safe way of reaching another thread demands it of the caller.
        let bound: Vec<String> = function.sent_across().collect();
        if !bound.is_empty() {
            said.push(format!(
                "`{name}`: {} is bound `Send`.",
                crossing::a_list(&bound, "the parameter", "the parameters")
            ));
            said.push(
                "  Seen and not claimed (ADR-193 D3): a `Send` bound says the callee **may**"
                    .to_string(),
            );
            said.push(
                "  send it, which is usually `spawn` and is sometimes an API keeping a door open."
                    .to_string(),
            );
        }

        // **And the other row**: a sink reached through the crate's own calls.
        // `across_a_thread_unchecked` has no bound, because an
        // `unsafe impl Send` took it away, and only following the calls reaches
        // the `spawn` (D4).
        let at = surface.reachable.get(name).cloned().unwrap_or_default();
        if let Some(path) = crossing::reaches_a_thread(&at, &calls) {
            let (sink, through) = path.split_last().expect("a path ends at its sink");
            said.push(match through.is_empty() {
                true => format!("`{name}`: calls `{sink}`."),
                false => format!(
                    "`{name}`: reaches `{sink}` through {}.",
                    through
                        .iter()
                        .map(|step| format!("`{step}`"))
                        .collect::<Vec<_>>()
                        .join(" -> ")
                ),
            });
            said.push(
                "  A sink reached through a call says *this function threads something*,"
                    .to_string(),
            );
            said.push(
                "  never *this function threads your argument* (ADR-193 D4) - connecting"
                    .to_string(),
            );
            said.push(
                "  those is dataflow through a closure capture and is not built.".to_string(),
            );
        }

        if said.is_empty() {
            continue;
        }
        said.push("  Does this put it on a thread?  -> threads = true | false".to_string());
        notes
            .about_a_function
            .insert(format!("{crate_word}::{name}"), said);
    }

    let described = Described {
        path: PathBuf::new(),
        version: sources.version,
        functions: ledger.functions.len(),
        types: ledger.types.len(),
        unanswered,
        notes,
    };
    Ok((ledger, described))
}

/// The manifest's declaration for this crate, under the name a program writes.
///
/// The key is the manifest's and the word is Cargo's, which is the translation
/// D1 already makes: `hyper-shim` in the manifest is `hyper_shim` in a program.
fn rust_dependency(
    manifest: &crate::manifest::Manifest,
    crate_word: &str,
) -> Result<(String, toml::Value)> {
    for (key, declared) in manifest.dependencies() {
        if key.replace('-', "_") != crate_word {
            continue;
        }
        let crate::manifest::Dependency::Rust(value) = declared else {
            bail!(
                "`{key}` is a Nikaia package, not a Rust crate. `nikaia describe` is for \
                 `[dependencies]` entries with `type = \"rust\"`."
            );
        };
        return Ok((key.clone(), value.clone()));
    }
    bail!(
        "`{crate_word}` isn't in this project's `[dependencies]`. Add it to `nikaia.toml` \
         first: only crates the project depends on can be described."
    )
}

/// The crate's `.rs` files, by their path inside the crate.
struct Sources {
    version: String,
    files: BTreeMap<String, String>,
}

/// Where the crate's sources are, and every `.rs` under its `src/`.
///
/// **A `path` dependency only**, and that is this step's honest scope rather
/// than a gap: a version dependency's sources are in Cargo's registry cache,
/// under a layout this compiler does not resolve — [ADR-002](../../docs/specification/adr/adr-002.md)
/// D1 hands versions to Cargo and never resolves one itself, and a describer
/// that guessed at that cache's shape would be resolving one.
///
/// The path is **relative to `nikaia.toml`**
/// ([ADR-197](../../docs/specification/adr/adr-197.md) D1), as a Nikaia
/// package's is and as anybody would guess. It used to be relative to the
/// generated manifest, which put this reader and Cargo in two different
/// directories for one crate's sources — and only this one had a test.
fn crate_sources(root: &Path, value: &toml::Value, crate_word: &str, key: &str) -> Result<Sources> {
    let version = value
        .get("version")
        .and_then(toml::Value::as_str)
        .unwrap_or("?")
        .to_string();
    let Some(declared) = value.get("path").and_then(toml::Value::as_str) else {
        bail!(
            "`{key}` is a dependency by version, and `nikaia describe` can only read \
             crates given by `path`. Use a `path` dependency, or write \
             `contracts/{crate_word}.contracts` by hand."
        );
    };
    // **Resolved lexically and not by the filesystem**: a crate may perfectly
    // well be described before the project has ever been built - which is the
    // order `NK2504` puts a reader in. `canonicalize` on a directory that is
    // not there yet fails, and what it would have answered is a `..` this can
    // walk off itself.
    let crate_root = without_dots(&root.join(declared));
    if !crate_root.is_dir() {
        bail!(
            "The sources of `{key}` aren't at {}. (A `path` in `nikaia.toml` is relative \
             to `nikaia.toml`.)",
            crate_root.display()
        );
    }
    let version = match version.as_str() {
        "?" => cargo_version(&crate_root).unwrap_or_else(|| "?".to_string()),
        given => given.to_string(),
    };

    // **The walk is Nikaia's** (ADR-195 D4, ADR-196 D4): `tools/sources.nika`
    // reads every `.rs` under `src` by `fs::walk`, and this drives it to its
    // end. A crate with no `src` has no `.rs` file, which the next lines say.
    let src = crate_root.join("src");
    let files = match src.is_dir() {
        false => BTreeMap::new(),
        true => nikaia_std::rt::exec::block_on(nikaia_std::tools::sources::rust_sources(
            &src.to_string_lossy(),
        ))
        .map_err(|e| anyhow::anyhow!("{e}"))
        .with_context(|| format!("reading the sources of `{key}`"))?,
    };
    if files.is_empty() {
        bail!(
            "There's no `.rs` file under {}, and `nikaia describe` reads the crate's \
             sources.",
            src.display()
        );
    }
    Ok(Sources { version, files })
}

/// **Every description this build reads, as one digest**, for the cache key
/// ([ADR-104](../../docs/specification/adr/adr-104.md) D5,
/// [ADR-021](../../docs/specification/adr/adr-021.md) D7).
///
/// Empty where the project describes nothing, which is most of them. The files
/// are read in sorted order and their *paths* hash beside their contents, so a
/// description that is deleted misses the key as surely as one that is edited.
///
/// **Why this exists at all**: the file is hand-edited on purpose — D5 says the
/// draft is committed and reviewed like code — and until this it did not reach
/// the cache key, so a reviewer's edit took effect only after the build
/// directory was thrown away. It failed **open**: a description that *dropped*
/// a claim was seen, because a refused build records nothing, while one that
/// *added* a claim hit an entry recorded before the word was there.
pub fn descriptions_digest(root: &Path) -> String {
    let directory = root.join(crate::project::CONTRACTS);
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return String::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("contracts"))
        .collect();
    if files.is_empty() {
        return String::new();
    }
    // ADR-005 D8: a directory listing is never consumed in filesystem order
    // where the output depends on it, and this output is a cache key.
    files.sort();
    let mut all = String::new();
    for path in files {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        all.push_str(&name);
        all.push('\0');
        all.push_str(&orchestrator::cache::sha256_hex(text.as_bytes()));
        all.push('\0');
    }
    orchestrator::cache::sha256_hex(all.as_bytes())
}

/// **Where a described crate's sources are**, for a reader other than the
/// describer: the hash rule compares what a description recorded against what
/// is there now, and *where is there* is this one question.
///
/// `None` for anything this cannot answer — a crate the manifest does not
/// declare, one declared by version, one whose directory is not there. Each of
/// those is an absence rather than a difference, and a refusal may not rest on
/// one ([ADR-169](../../docs/specification/adr/adr-169.md) D1).
pub fn crate_root(root: &Path, crate_word: &str) -> Option<PathBuf> {
    let manifest = crate::manifest::Manifest::read(&root.join("nikaia.toml")).ok()?;
    let (_, value) = rust_dependency(&manifest, crate_word).ok()?;
    let declared = value.get("path").and_then(toml::Value::as_str)?;
    let at = without_dots(&root.join(declared));
    at.is_dir().then_some(at)
}

/// A path with its `.` and `..` components walked off, without asking the
/// filesystem whether any of it exists.
fn without_dots(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// The crate's own version, from its `Cargo.toml`.
fn cargo_version(crate_root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(crate_root.join("Cargo.toml")).ok()?;
    let document: toml::Value = toml::from_str(&text).ok()?;
    Some(
        document
            .get("package")?
            .get("version")?
            .as_str()?
            .to_string(),
    )
}

/// The names of this crate every `.nika` of the project writes, without the
/// crate word in front.
///
/// [`crate::foreign::qualified_names`] is the walk, which is the one `NK2504`
/// runs: what the refusal asks is which crates a program reaches into, and this
/// asks which names of one — the same walk, read one segment further.
fn names_the_program_writes(root: &Path, crate_word: &str) -> Result<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // `target/` is the build's own and holds nothing a program
                // wrote; `contracts/` holds this command's own output.
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name != "target" && name != crate::project::CONTRACTS {
                    pending.push(path);
                }
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("nika") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(parsed) = crate::parser::parse_to_ast(&text) else {
                // A file that does not parse is the build's to report, not
                // this command's: it would be the same message twice.
                continue;
            };
            for name in crate::foreign::qualified_names(&parsed).keys() {
                let Some(rest) = name.strip_prefix(&format!("{crate_word}::")) else {
                    continue;
                };
                out.insert(rest.to_string());
            }
        }
    }
    Ok(out)
}

/// What a crate's sources say, before anything is asked of them.
///
/// **Read by a grammar and not by a scanner**
/// ([ADR-195](../../docs/specification/adr/adr-195.md) D3,
/// [ADR-196](../../docs/specification/adr/adr-196.md) D1): the parser is
/// `crates/nikaia-std/src/tools/rust.nika`, written in Nikaia, lowered ahead of
/// time and reached from here as `nikaia_std::tools::rust` — an ordinary Rust
/// module and an ordinary call.
///
/// What that buys, measured on one file: the scanner this replaced reported
/// **four functions that do not exist** — one inside a block comment, one on
/// the second line of a string literal, two inside a private `mod` — and put a
/// fifth at the crate root rather than under its module.
#[derive(Default)]
struct Surface {
    /// Every `fn`, by the path it is **defined** at — a private `mod`'s, because
    /// a `pub use` may reach one, and a private `fn`'s, because the call graph
    /// goes through it ([ADR-193](../../docs/specification/adr/adr-193.md) D4).
    functions: BTreeMap<String, Function>,
    /// The subset of [`Self::functions`] the crate writes `pub`. A path is
    /// offered only if its function is one of these **and** every module on the
    /// way to it is `pub`.
    offers: BTreeSet<String>,
    /// Every `pub struct`'s field types, by the path the type is defined at.
    /// A `pub enum` is a type without an entry here, which is *nobody looked*
    /// and not *it holds nothing* — the difference [`crosses`] rests on.
    fields: BTreeMap<String, Vec<String>>,
    /// Every `pub struct` and `pub enum`, by the path it is defined at.
    types: BTreeSet<String>,
    /// What each of those derives, by the trait's last segment: `Copy`,
    /// `PartialEq` ([ADR-252](../../docs/specification/adr/adr-252.md) D4.3).
    derives: BTreeMap<String, BTreeSet<String>>,
    /// Every `mod` declaration found, by the module's path, and whether it was
    /// written `pub`.
    ///
    /// **A module nothing declares is not offered**, which is fail-closed and
    /// [ADR-010](../../docs/specification/adr/adr-010.md) D1's polarity: what
    /// is missing here is *nobody said this module is public*, and reading that
    /// as *it is* would put a claim in the draft that no source supports. The
    /// name reaches the reviewer as a `?` instead
    /// ([ADR-104](../../docs/specification/adr/adr-104.md) D4, D5).
    modules: BTreeMap<String, bool>,
    /// A path a **caller** may write, and the path it resolves to. An item in a
    /// public module is here under its own path; a re-exported one is here
    /// under the name the re-export gives it. Filled by [`Surface::resolve`],
    /// because whether a path is offered takes every file to answer.
    reachable: BTreeMap<String, String>,
    /// `pub use` items, kept until every file has been read: one may name
    /// something another file declares.
    exports: Vec<Export>,
    /// Every `unsafe impl Trait for Type` the crate writes, as the sentence a
    /// reader wants ([ADR-193](../../docs/specification/adr/adr-193.md) D5).
    ///
    /// **Whatever the type's visibility.** The promise is the crate's, and the
    /// hole `examples/foreign-runtime/shim` opens on purpose is a `Smuggled<T>`
    /// nothing else can see.
    promises: Vec<String>,
}

impl Surface {
    /// Read one file's items into this, under the module the **file** is.
    ///
    /// `src/lib.rs` is the crate root, `src/foo.rs` and `src/foo/mod.rs` are
    /// `foo`, `src/foo/bar.rs` is `foo::bar`. A binary's root and anything
    /// under `src/bin/` are not modules of the library at all and are skipped —
    /// a program that calls into this crate cannot reach them.
    ///
    /// **Whether any of it is offered is not decided here**: that takes the
    /// `mod foo;` in the parent, which may be in another file, so it is
    /// [`Surface::resolve`]'s.
    fn read(&mut self, relative: &str, text: &str) -> Result<()> {
        let Some(at) = paths::module_of(relative) else {
            return Ok(());
        };
        let items = nikaia_std::tools::rust::file(text)
            .map_err(|error| anyhow::anyhow!("{relative}: {error}"))?;
        // **The file's `use` items first**, because a call is resolved against
        // them and one may be written below the function that needs it
        // ([ADR-193](../../docs/specification/adr/adr-193.md) D4).
        let mut imports = BTreeMap::new();
        imported(&items, &mut imports);
        self.walk(&items, &at, &imports);
        Ok(())
    }

    fn walk(
        &mut self,
        items: &[nikaia_std::tools::rust::Item<'_>],
        at: &str,
        imports: &BTreeMap<String, String>,
    ) {
        use nikaia_std::tools::rust::Item;
        for item in items {
            match item {
                Item::Fun(f) => {
                    let path = paths::joined(at, f.name);
                    self.offers.insert(path.clone());
                    self.functions.insert(path, Function::of(f, imports));
                }
                // **On the way to an entry rather than one**
                // ([ADR-193](../../docs/specification/adr/adr-193.md) D4):
                // nothing outside the crate can call it, and the call graph
                // goes through it.
                Item::Hidden(f) => {
                    self.functions
                        .insert(paths::joined(at, f.name), Function::of(f, imports));
                }
                Item::Rec(r) => {
                    let path = paths::joined(at, r.name);
                    if r.what == "struct" {
                        self.fields.insert(
                            path.clone(),
                            r.parts.iter().map(|p| p.ty.to_string()).collect(),
                        );
                    }
                    let derived: BTreeSet<String> = r
                        .derives
                        .iter()
                        .flat_map(|list| list.split(','))
                        .map(|name| name.trim().rsplit("::").next().unwrap_or("").to_string())
                        .filter(|name| !name.is_empty())
                        .collect();
                    self.derives.insert(path.clone(), derived);
                    self.types.insert(path);
                }
                Item::Export(text) => self.exports.extend(paths::exported(at, text)),
                // Read before this walk, into the table every call above was
                // resolved against.
                Item::Used(_) => {}
                Item::Group(g) if g.what == "mod" => {
                    let path = paths::joined(at, g.name);
                    self.modules.insert(path.clone(), g.visible);
                    self.walk(&g.items, &path, imports);
                }
                // **An `unsafe impl` is a promise the toolchain cannot check**
                // ([ADR-193](../../docs/specification/adr/adr-193.md) D5), and
                // one syntactic pattern is all it takes to see that it was
                // made. Sound in the only sense that matters here: the item is
                // in the text or it is not.
                Item::Group(g) if g.what == "unsafe impl" && !g.via.is_empty() => {
                    self.promises
                        .push(format!("unsafe impl {} for {}", g.via, g.name));
                }
                // Every other `impl` and every `trait`: not described yet. What
                // a method is at a foreign boundary is
                // [ADR-104](../../docs/specification/adr/adr-104.md) D4's own
                // question and nothing here asks it.
                Item::Group(_) => {}
            }
        }
    }

    /// Whether a caller outside the crate may write this path: every module on
    /// the way to it was declared `pub`.
    fn offered(&self, path: &str) -> bool {
        let mut at = String::new();
        let mut parts: Vec<&str> = path.split("::").collect();
        parts.pop();
        for part in parts {
            at = paths::joined(&at, part);
            if self.modules.get(&at) != Some(&true) {
                return false;
            }
        }
        true
    }

    /// Follow the `pub use` items until nothing new becomes reachable.
    ///
    /// **A re-export may name a re-export**, so this runs to a fixed point
    /// rather than once — bounded, because a crate that re-exports in a cycle
    /// does not compile and this is not the place to say so.
    fn resolve(&mut self) {
        // Every item in a module chain that is `pub` all the way, under its own
        // path. This is what the scanner did for **every** item it found, which
        // is how four functions that do not exist reached a draft.
        let direct: Vec<String> = self
            .offers
            .iter()
            .chain(self.types.iter())
            .filter(|path| self.offered(path))
            .cloned()
            .collect();
        for path in direct {
            self.reachable.insert(path.clone(), path);
        }
        // A `pub use` written in a module nobody outside can reach offers
        // nothing to anybody outside.
        let mut exports = std::mem::take(&mut self.exports);
        exports.retain(|export| self.offered(&paths::joined(&export.at, "x")));
        self.exports = exports;
        for _ in 0..8 {
            let mut added = false;
            let exports = std::mem::take(&mut self.exports);
            for export in &exports {
                match &export.name {
                    Some(paths::Named { name, alias }) => {
                        let offered = paths::joined(&export.at, alias);
                        for candidate in self.candidates(export, name) {
                            // **`self.functions` and not `self.offers`**: a
                            // `pub use` may carry a function that is not `pub`
                            // where it was written, and refusing to resolve one
                            // would refuse a call the crate answers
                            // ([Part III C.4](../../docs/specification/30-nikaia-tooling.md)).
                            if !self.functions.contains_key(&candidate)
                                && !self.types.contains(&candidate)
                            {
                                continue;
                            }
                            added |= self.reachable.insert(offered, candidate).is_none();
                            break;
                        }
                    }
                    // A glob offers everything **directly** under the prefix,
                    // which is what `::*` means: a module's own items and not
                    // its submodules'.
                    None => {
                        for base in paths::bases(export) {
                            let under = format!("{base}::");
                            let names: Vec<String> = self
                                .offers
                                .iter()
                                .chain(self.types.iter())
                                .filter_map(|path| path.strip_prefix(&under))
                                .filter(|rest| !rest.contains("::"))
                                .map(str::to_string)
                                .collect();
                            for name in names {
                                let offered = paths::joined(&export.at, &name);
                                let target = format!("{base}::{name}");
                                added |= self.reachable.insert(offered, target).is_none();
                            }
                        }
                    }
                }
            }
            self.exports = exports;
            if !added {
                break;
            }
        }
    }

    /// Where a `use` path could resolve, most specific first.
    ///
    /// Rust 2018's uniform paths mean a bare first segment is a name in scope
    /// where the `use` was written *or* at the crate root, and this cannot tell
    /// which without a name table — so it tries both and takes the first that
    /// names something. An item neither names is not resolved, which is the
    /// absence of a claim ([ADR-104](../../docs/specification/adr/adr-104.md)
    /// D4) and reaches the draft as a `?` for a reviewer.
    fn candidates(&self, export: &Export, name: &str) -> Vec<String> {
        paths::bases(export)
            .into_iter()
            .map(|base| match base.is_empty() {
                true => name.to_string(),
                false => format!("{base}::{name}"),
            })
            .collect()
    }
}

/// **What a name in one file means**, from its `use` items
/// ([ADR-193](../../docs/specification/adr/adr-193.md) D4).
///
/// `use tokio::spawn;` makes `spawn` and `tokio::spawn` one function written
/// two ways, and a reader of a body's calls that did not have this table would
/// miss every crate that imports what it calls.
///
/// **Flat over the file** rather than per module. A `use` inside a `mod` block
/// reaches only that block, and treating it as the file's can only make a name
/// resolve where it would not have — which for a *note* naming a path is a
/// wrong sentence to a reviewer rather than a wrong claim in a file. The
/// precise version wants a scope table, and what it would buy is not this
/// step's.
fn imported(items: &[nikaia_std::tools::rust::Item<'_>], out: &mut BTreeMap<String, String>) {
    use nikaia_std::tools::rust::Item;
    for item in items {
        match item {
            // A `pub use` is an import here too: it brings the name into this
            // file exactly as a plain one does, and offers it onward besides.
            Item::Used(text) | Item::Export(text) => {
                for one in paths::exported("", text) {
                    let Some(paths::Named { name, alias }) = &one.name else {
                        continue;
                    };
                    let full = match one.prefix.is_empty() {
                        true => name.clone(),
                        false => format!("{}::{name}", one.prefix),
                    };
                    out.insert(alias.clone(), full);
                }
            }
            Item::Group(g) => imported(&g.items, out),
            _ => {}
        }
    }
}

/// One `pub fn`, as its signature reads.
///
/// **No name**: an entry is keyed by the path a caller writes, which
/// [`Surface`] holds and a signature does not.
struct Function {
    /// The crate's own type parameters, which the ledger spells `$T`.
    parameters: Vec<String>,
    /// `name: Type` pairs, in order, as text.
    args: Vec<(String, String)>,
    /// The text after `->`, where there is one.
    result: Option<String>,
    /// The type parameters' bounds, as they were written: the `<…>` list and
    /// the `where` clause, joined.
    ///
    /// Read for one thing only — whether a parameter is bound `Send`
    /// ([ADR-193](../../docs/specification/adr/adr-193.md) D4). Every safe way
    /// of reaching another thread carries that bound, so a crate that takes a
    /// value across a thread boundary in safe Rust demands it of its caller and
    /// says so here.
    bounds: String,
    /// Every path this body calls, with the file's `use` items applied
    /// ([ADR-193](../../docs/specification/adr/adr-193.md) D4).
    ///
    /// **The other row's evidence.** A bound is what a function asks of its
    /// caller; a call is what it does — and `across_a_thread_unchecked` has no
    /// bound, because an `unsafe impl Send` took it away.
    calls: Vec<String>,
    /// `async fn` — a plain `fn` is `sync` (D3).
    pauses: bool,
}

impl Function {
    /// One `pub fn` as the grammar read it.
    ///
    /// **The three text splits left here are lists and not syntax.** A type
    /// parameter list, a `where` bound's head, a receiver — the grammar hands
    /// over what was written and this takes the pieces, which is the same
    /// division `Item::Export` is read under and for the same reason.
    fn of(f: &nikaia_std::tools::rust::Fun<'_>, imports: &BTreeMap<String, String>) -> Function {
        Function {
            parameters: paths::split_top_level(f.generics)
                .into_iter()
                // A lifetime is not a type parameter, and a bound written
                // inline (`T: Send`) names the parameter before the colon.
                .filter(|p| !p.starts_with('\''))
                .map(|p| p.split(':').next().unwrap_or(&p).trim().to_string())
                .filter(|p| !p.is_empty())
                .collect(),
            args: f
                .parts
                .iter()
                // The receiver is not an argument a caller writes.
                .filter(|p| p.name != "self")
                .map(|p| (p.name.to_string(), p.ty.to_string()))
                .collect(),
            result: match f.result.is_empty() {
                true => None,
                false => Some(f.result.to_string()),
            },
            bounds: format!("{}, {}", f.generics, f.wheres),
            calls: f
                .calls
                .iter()
                .map(|call| match imports.get(*call) {
                    Some(full) => full.clone(),
                    None => call.to_string(),
                })
                .collect(),
            pauses: f.pauses,
        }
    }

    /// **The parameters a `Send` bound reaches**, in declaration order
    /// ([ADR-193](../../docs/specification/adr/adr-193.md) D4).
    ///
    /// Two shapes, and both are the same evidence: a parameter whose type is a
    /// type variable the bounds send, and one written `impl … Send …` at the
    /// parameter itself. Rust's own type system does the propagation, and the
    /// answer surfaces in the signature — which is why this is not a heuristic
    /// and why the shim's `across_a_thread_unchecked` is correctly silent: an
    /// `unsafe impl Send` took its bound away, and only following the calls
    /// reaches the `spawn`.
    fn sent_across(&self) -> impl Iterator<Item = String> + '_ {
        let sent = self.sends();
        self.args.iter().filter_map(move |(name, ty)| {
            let ty = ty.trim();
            let named = ty.trim_start_matches(['&', ' ']).trim();
            let named = named.strip_prefix("mut ").unwrap_or(named);
            let reached = sent.iter().any(|p| p == named) || crossing::bounds_send(ty);
            reached.then(|| name.clone())
        })
    }

    /// The type parameters this signature binds `Send`.
    fn sends(&self) -> BTreeSet<String> {
        paths::split_top_level(&self.bounds)
            .into_iter()
            .filter_map(|one| {
                let (name, bound) = one.split_once(':')?;
                crossing::bounds_send(bound).then(|| name.trim().to_string())
            })
            .filter(|name| !name.is_empty() && !name.starts_with('\''))
            .collect()
    }

    /// The entry, and the crate types its signature named.
    fn contract(
        &self,
        crate_word: &str,
        types: &BTreeSet<String>,
    ) -> (FnContract, BTreeSet<String>) {
        let mut mentioned = BTreeSet::new();
        let mut keeps = Vec::new();
        let mut params = Vec::new();
        for (name, text) in &self.args {
            let (ty, kept) = self.translate(text, crate_word, types, &mut mentioned);
            if kept {
                keeps.push(name.clone());
            }
            params.push((name.clone(), ty));
        }
        let mut throws = Vec::new();
        let result = self.result.as_ref().map(|text| {
            let (ty, failing) = self.result_of(text, crate_word, types, &mut mentioned);
            if let Some(error) = failing {
                throws.push(error);
            }
            ty
        });
        let contract = FnContract {
            public: true,
            // **D3's row, and the default is the strict one**: a plain `fn`
            // cannot pause, and an `async fn` can. Nothing between them.
            sync_claim: match self.pauses {
                true => Sync::No,
                false => Sync::Asserted,
            },
            fails_with: throws,
            keeps,
            signature: Some(Signature {
                params,
                result,
                ..Signature::empty()
            }),
            ..FnContract::empty()
        };
        (contract, mentioned)
    }

    /// `Result<T, E>` in the result position: the `T` is the type and the `E`
    /// is a `throws`, named where this can name it and `"?"` where it cannot.
    fn result_of(
        &self,
        text: &str,
        crate_word: &str,
        types: &BTreeSet<String>,
        mentioned: &mut BTreeSet<String>,
    ) -> (Ty, Option<String>) {
        if let Some(inner) = crossing::generic_of(text, "Result") {
            let parts = paths::split_top_level(&inner);
            let ok = parts.first().cloned().unwrap_or_default();
            let error = parts.get(1).cloned();
            let (ty, _) = self.translate(&ok, crate_word, types, mentioned);
            // The error type where 15.2 can name it (D3). A crate's own error
            // is that crate's type; anything else is the absence of a name,
            // which the ledger spells `"?"`.
            let named = error.map(|error| match types.contains(error.trim()) {
                true => format!("{crate_word}::{}", error.trim()),
                false => "?".to_string(),
            });
            return (ty, Some(named.unwrap_or_else(|| "?".to_string())));
        }
        let (ty, _) = self.translate(text, crate_word, types, mentioned);
        (ty, None)
    }

    /// One Rust type as the ledger's, and whether a value of it is **kept**.
    ///
    /// D3's first row: `&T` is a view and `T` by value is kept — the crate
    /// takes ownership, so the caller may not lend it. A number is not kept in
    /// any sense a caller can act on, so the column names what a caller could
    /// otherwise have gone on using.
    fn translate(
        &self,
        text: &str,
        crate_word: &str,
        types: &BTreeSet<String>,
        mentioned: &mut BTreeSet<String>,
    ) -> (Ty, bool) {
        let text = text.trim();
        if let Some(rest) = text.strip_prefix("&mut ") {
            // `&mut T` is *changed in place*, which the ledger says with
            // `mutates` on the entry rather than on the type — and this
            // scraper has no place to put it. `?` is the absence of a claim,
            // which is the fail-closed direction (D4).
            let _ = rest;
            return (Ty::Unknown, false);
        }
        if let Some(rest) = text.strip_prefix('&') {
            let (ty, _) = self.translate(rest, crate_word, types, mentioned);
            return (view_of(ty), false);
        }
        if let Some(inner) = crossing::generic_of(text, "Option") {
            let (ty, kept) = self.translate(&inner, crate_word, types, mentioned);
            return (Ty::Nullable(Box::new(ty)), kept);
        }
        if let Some(inner) = crossing::generic_of(text, "Vec") {
            let (ty, _) = self.translate(&inner, crate_word, types, mentioned);
            return (
                Ty::Named {
                    name: "Vec".to_string(),
                    args: vec![ty],
                    view: false,
                },
                true,
            );
        }
        // A type parameter of this function is the ledger's variable.
        if self.parameters.iter().any(|p| p == text) {
            return (
                Ty::Var {
                    name: text.to_string(),
                    view: false,
                },
                true,
            );
        }
        if crossing::is_plain(text) {
            return (Ty::named(text), false);
        }
        if text == "String" {
            return (Ty::named("String"), true);
        }
        if types.contains(text) {
            mentioned.insert(text.to_string());
            return (Ty::named(format!("{crate_word}::{text}")), true);
        }
        // Anything else is a name this scraper cannot account for — another
        // crate's type, a trait object, a path with segments. `?` is the
        // absence of a claim and D5's own `?`, for a reviewer to fill.
        (Ty::Unknown, true)
    }
}

/// A view of a type, which the ledger's language writes with the `&` on the
/// name.
fn view_of(ty: Ty) -> Ty {
    match ty {
        Ty::Named { name, args, .. } => Ty::Named {
            name,
            args,
            view: true,
        },
        Ty::Var { name, .. } => Ty::Var { name, view: true },
        other => other,
    }
}
