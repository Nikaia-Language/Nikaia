// crates/nikaia/src/describe.rs
//
// `nikaia describe <crate>` — a draft ledger for a Rust crate's boundary
// ([ADR-290](../../docs/specification/adr/adr-290.md) D2-D4).
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
// ([ADR-290](../../docs/specification/adr/adr-290.md) D13):
// `crates/nikaia-std/src/tools/rust.nika`, lowered ahead of time and reached
// from here as an ordinary Rust module — `nikaia_std::tools::rust`, a call and
// nothing else ([ADR-290](../../docs/specification/adr/adr-290.md) D16). What
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

use crate::contracts::{Ledger, Notes};

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
    /// ([ADR-290](../../docs/specification/adr/adr-290.md) D8, D10), written
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
        // **Read by the grammar, offered by `tools/surface.nika`** (ADR-290
        // D3-D4): this drives the two and hands the items between them.
        let items = nikaia_std::tools::rust::file(text)
            .map_err(|error| anyhow::anyhow!("{relative}: {error}"))?;
        surface::read_items(&mut surface, relative, &items);
    }
    surface::resolve(&mut surface);

    // **What the description holds is Nikaia's** (`tools/describe.nika`,
    // #125): the entries, the types they name and what the describer saw and
    // did not claim ([ADR-290](../../docs/specification/adr/adr-290.md) D8,
    // D10). The header is this file's.
    let drafted = nikaia_std::tools::describe::drafted(&surface, crate_word, &wanted);
    let mut ledger = Ledger::empty();
    ledger.inference = "described-from-signatures".to_string();
    ledger.sources = hashes;
    ledger.functions = drafted.functions;
    ledger.types = drafted.types;
    let (unanswered, notes) = (drafted.unanswered, drafted.notes);

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
/// ([ADR-286](../../docs/specification/adr/adr-286.md) D16), as a Nikaia
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

    // **The walk is Nikaia's** (ADR-290 D14, ADR-290 D19): `tools/sources.nika`
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
/// ([ADR-290](../../docs/specification/adr/adr-290.md) D5,
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
/// one ([ADR-281](../../docs/specification/adr/adr-281.md) D34).
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
/// [`crate::foreign::qualified_names`] is the walk of each tree, which is the
/// one `NK2504` runs: what the refusal asks is which crates a program reaches
/// into, and this asks which names of one — the same walk, read one segment
/// further. **The walk of the directory is Nikaia's** (#124):
/// `tools/sources.nika`'s `program_sources` finds the files by `fs::walk`,
/// leaves out `target/` and `contracts/`, and reads them; parsing them is the
/// compiler's.
fn names_the_program_writes(root: &Path, crate_word: &str) -> Result<BTreeSet<String>> {
    let files = nikaia_std::rt::exec::block_on(nikaia_std::tools::sources::program_sources(
        &root.to_string_lossy(),
    ))
    .map_err(|e| anyhow::anyhow!("{e}"))
    .with_context(|| format!("reading the `.nika` files under {}", root.display()))?;
    let prefix = format!("{crate_word}::");
    let mut out = BTreeSet::new();
    for text in files.values() {
        let Ok(parsed) = crate::parser::parse_to_ast(text) else {
            // A file that does not parse is the build's to report, not
            // this command's: it would be the same message twice.
            continue;
        };
        for name in crate::foreign::qualified_names(&parsed).keys() {
            if let Some(rest) = name.strip_prefix(&prefix) {
                out.insert(rest.to_string());
            }
        }
    }
    Ok(out)
}
