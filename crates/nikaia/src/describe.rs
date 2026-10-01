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

use crate::contracts::{FnContract, Ledger, Notes, Signature, Sync, TypeContract};
use nikaia_std::tools::crossing;

use nikaia_std::tools::signature;
use nikaia_std::tools::surface;

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
    let mut surface = surface::empty();
    let mut hashes = BTreeMap::new();
    for (relative, text) in &sources.files {
        hashes.insert(
            relative.clone(),
            orchestrator::cache::sha256_hex(text.as_bytes()),
        );
        // **Read by the grammar, offered by `tools/surface.nika`** (ADR-195
        // D3-D4): this drives the two and hands the items between them.
        let items = nikaia_std::tools::rust::file(text)
            .map_err(|error| anyhow::anyhow!("{relative}: {error}"))?;
        surface::read(&mut surface, relative, &items);
    }
    surface::resolve(&mut surface);
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
        let (contract, mentions) = contract_of(function, crate_word, &types);
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
        let bound: Vec<String> = sent_across(function);
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
fn sent_across(function: &surface::Function) -> Vec<String> {
    let names: Vec<String> = function.args.iter().map(|arg| arg.name.clone()).collect();
    let types: Vec<String> = function.args.iter().map(|arg| arg.ty.clone()).collect();
    signature::sent_across(&names, &types, &function.bounds)
}

/// The entry, and the crate types its signature named.
fn contract_of(
    function: &surface::Function,
    crate_word: &str,
    types: &BTreeSet<String>,
) -> (FnContract, BTreeSet<String>) {
    let mut mentioned = BTreeSet::new();
    let mut keeps = Vec::new();
    let mut params = Vec::new();
    for arg in &function.args {
        let translated = signature::translate(
            &arg.ty,
            crate_word,
            &function.parameters,
            types,
            &mut mentioned,
        );
        if translated.kept {
            keeps.push(arg.name.clone());
        }
        params.push((arg.name.clone(), translated.ty));
    }
    let mut throws = Vec::new();
    let written = Some(&function.result).filter(|text| !text.is_empty());
    let result = written.map(|text| {
        let resulted = signature::result_of(
            text,
            crate_word,
            &function.parameters,
            types,
            &mut mentioned,
        );
        if let Some(error) = resulted.fails {
            throws.push(error);
        }
        resulted.ty
    });
    let contract = FnContract {
        public: true,
        // **D3's row, and the default is the strict one**: a plain `fn`
        // cannot pause, and an `async fn` can. Nothing between them.
        sync_claim: match function.pauses {
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
