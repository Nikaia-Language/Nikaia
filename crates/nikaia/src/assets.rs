//! **The files a build may read while it builds**
//! ([ADR-072](../../../docs/specification/adr/adr-072.md),
//! [ADR-116](../../../docs/specification/adr/adr-116.md) D2).
//!
//! A `comptime` initialiser may write `asset("config.json")`, and what comes
//! back is the file's text. Everything else on this page is about **which**
//! files, and the answer is deliberately hard to give by accident.
//!
//! **A build given no list reads nothing** (D1). That is not a mode a project
//! opts into: it is what happens when nothing is passed, so *this build reads
//! nothing while building* is a fact rather than a claim somebody has to keep
//! true and be believed about. Every other refusal on this page is reached only
//! by a build that already asked for the whole class to be switched on.
//!
//! **A file is named in three places and a read missing any of them is
//! refused** (D3). The flag says a list is in effect; the list says which
//! files; the `asset("…")` literal says which one this line reads. They are
//! deliberately not derivable from one another — the code is where a reader
//! learns what the bytes are *for*, the list is where a reviewer sees the whole
//! set without reading the code, and the flag is where whoever runs the build
//! says the set applies to this invocation. Two of the three are committed, so
//! a change to either is a diff in review; the third is not, which is what lets
//! a build run with the reads switched off without editing anything.
//!
//! **The literal may not be computed** (D4) and **there are no patterns** (D5).
//! Both fall out of D3: if a path could be computed, or stand for files that do
//! not exist yet, *named in the code* would stop being decidable by looking at
//! the line — which is the whole of what the three namings buy. A build that
//! wants ten files lists ten files.
//!
//! **The list binds the whole build, dependencies included** (D6). A package
//! cannot bring its own permission, so a dependency that reads a file makes the
//! consumer write a line for it — which is where the consumer finds out. That
//! friction is the feature.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};

/// The name a `comptime` initialiser writes
/// ([ADR-116](../../../docs/specification/adr/adr-116.md) D2).
///
/// **The compiler's and not `std`'s**, which is why it is a constant here
/// rather than a ledger entry: there is no body to describe, and a program that
/// declares the name is refused rather than shadowing it.
pub const ASSET: &str = "asset";

/// Why a read was refused. Each is a sentence the checker writes, because the
/// reason and the way out are not the same for any two of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Denied {
    /// [D1](../../../docs/specification/adr/adr-072.md): no list is in effect,
    /// so the whole class is off.
    NoList,
    /// [D3](../../../docs/specification/adr/adr-072.md): a list is in effect
    /// and this path is not in it.
    NotListed { list: String },
    /// A path that leaves the project root, by `..` or by being absolute.
    OutsideTheRoot,
    /// The list named it, and the file is not there or cannot be read.
    Unreadable { because: String },
    /// Read, and not text. What crosses from build time to run time is a
    /// `&str` ([ADR-079](../../../docs/specification/adr/adr-079.md) D1), and
    /// bytes that are not UTF-8 are not that.
    NotText,
}

/// The list itself: a file that names files.
#[derive(Debug, Clone)]
pub struct Allowlist {
    /// Where the list was read from, for the sentence a refusal writes.
    pub path: PathBuf,
    /// SHA-256 of the list's own bytes
    /// ([D7](../../../docs/specification/adr/adr-072.md)): it is a file the
    /// build read, so it belongs in the key with the files it names.
    pub digest: String,
    allowed: BTreeSet<String>,
}

impl Allowlist {
    /// One path per line; `#` begins a comment; blank lines are nothing (D2).
    pub fn read(path: &Path) -> Result<Allowlist> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading the allowlist `{}`", path.display()))?;
        Ok(Allowlist {
            path: path.to_path_buf(),
            digest: digest(text.as_bytes()),
            allowed: Allowlist::entries(&text),
        })
    }

    /// The same, from the text — which is what the tests read and what keeps
    /// the parsing one function rather than two.
    pub fn entries(text: &str) -> BTreeSet<String> {
        text.lines()
            .map(|line| match line.split_once('#') {
                Some((before, _)) => before,
                None => line,
            })
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect()
    }
}

/// What this build may read, and what it read.
///
/// **One value for the whole build** (D6), carried beside the program's other
/// files: a dependency's `asset("…")` is checked against the list of the build
/// that is running, never against anything the dependency ships.
#[derive(Debug, Default)]
pub struct Reads {
    /// Every path is resolved under this, and one that leaves it is refused.
    root: PathBuf,
    /// `None` is D1, and D1 is the default.
    list: Option<Allowlist>,
    /// **Where this build compiles the parsers it runs**
    /// ([`crate::grammar_run`], issue #178
    /// issue #178).
    ///
    /// Here rather than threaded a second time through the same six
    /// signatures, and it belongs with the reads for the reason they belong
    /// with each other: both are facts about the **invocation** that no ledger
    /// can carry, both are wanted by the evaluator, and both are off for a
    /// caller that says nothing. A grammar run's bytes come through
    /// `asset("…")`, so the two arrive together in practice as well.
    workshop: crate::grammar_run::Workshop,
    /// Path (as the literal wrote it) to the SHA-256 of what was read.
    ///
    /// **Behind a lock because the evaluator holds a `&Reads`** and the cache
    /// wants the answer afterwards: what a build read is an output of the check
    /// and an input to the key ([ADR-021](../../../docs/specification/adr/adr-021.md)
    /// D13), and the walk that produces it hands back findings rather than
    /// files.
    taken: Mutex<BTreeMap<String, String>>,
}

impl Reads {
    /// **A build given no list reads nothing** (D1), which is what a caller
    /// that passes nothing gets — every test, every `nikaia lower` without the flag,
    /// and every build of a project that never asked.
    pub fn none() -> Reads {
        Reads::default()
    }

    /// A build with a list in effect.
    pub fn with(root: impl Into<PathBuf>, list: Allowlist) -> Reads {
        Reads {
            root: root.into(),
            list: Some(list),
            ..Reads::default()
        }
    }

    /// A build with a root and **no** list: it reads nothing (D1), and it has
    /// somewhere to resolve a path against. What it can still do is run a
    /// grammar over text the program wrote down.
    pub fn at(root: impl Into<PathBuf>) -> Reads {
        Reads {
            root: root.into(),
            ..Reads::default()
        }
    }

    /// The same, told where to compile the parsers it runs.
    ///
    /// **Not under the source tree**, which is the rule the build cache already
    /// keeps: a loose `.nika` file outside a project has nothing written beside
    /// it, so this is handed the cache's own neighbour rather than the root.
    pub fn building_in(self, at: impl Into<PathBuf>) -> Reads {
        Reads {
            workshop: crate::grammar_run::Workshop::at(at),
            ..self
        }
    }

    /// Where a parser is compiled, for the evaluator that runs one.
    pub fn workshop(&self) -> &crate::grammar_run::Workshop {
        &self.workshop
    }

    /// Whether a list is in effect at all — D1 read as a question.
    pub fn switched_on(&self) -> bool {
        self.list.is_some()
    }

    /// **The read itself**, and every one of D1 to D5 on the way through.
    ///
    /// The path is the literal as the line wrote it, which is what the list is
    /// compared against (D3) and what is recorded for the key.
    pub fn read(&self, path: &str) -> Result<String, Denied> {
        let Some(list) = &self.list else {
            return Err(Denied::NoList);
        };
        if !list.allowed.contains(path) {
            return Err(Denied::NotListed {
                list: list.path.display().to_string(),
            });
        }
        let whole = self.under_the_root(path).ok_or(Denied::OutsideTheRoot)?;
        let bytes = std::fs::read(&whole).map_err(|error| Denied::Unreadable {
            because: error.to_string(),
        })?;
        let text = String::from_utf8(bytes).map_err(|_| Denied::NotText)?;
        if let Ok(mut taken) = self.taken.lock() {
            taken.insert(path.to_string(), digest(text.as_bytes()));
        }
        Ok(text)
    }

    /// The path under the root, or nothing where it would leave.
    ///
    /// **Lexical and not `canonicalize`**, for the reason D4 gives about the
    /// literal: what a reader can decide by looking at the line is the property
    /// this is protecting, and a symlink resolved behind their back is the
    /// opposite of that. A `..` and an absolute path are the two shapes that
    /// leave, and both are visible in the literal.
    fn under_the_root(&self, path: &str) -> Option<PathBuf> {
        let written = Path::new(path);
        if written.is_absolute() {
            return None;
        }
        if written
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return None;
        }
        Some(self.root.join(written))
    }

    /// What this build read, for the cache's asset dimension
    /// ([ADR-021](../../../docs/specification/adr/adr-021.md) D13,
    /// [ADR-072](../../../docs/specification/adr/adr-072.md) D7).
    pub fn taken(&self) -> BTreeMap<String, String> {
        self.taken.lock().map(|it| it.clone()).unwrap_or_default()
    }

    /// The list's own digest, which belongs in the key beside the files it
    /// names (D7): a build that stops reading a file must not keep the old
    /// answer.
    pub fn list_digest(&self) -> Option<&str> {
        self.list.as_ref().map(|list| list.digest.as_str())
    }

    /// **An entry nothing read** (D8).
    ///
    /// A list that may hold names nothing uses decays into *everything we ever
    /// needed*, which is how an allowlist stops being read. Naming the unused
    /// entry keeps it the size of the truth.
    pub fn unused(&self) -> Vec<String> {
        let Some(list) = &self.list else {
            return Vec::new();
        };
        let taken = self.taken();
        list.allowed
            .iter()
            .filter(|entry| !taken.contains_key(*entry))
            .cloned()
            .collect()
    }
}

/// SHA-256, hexadecimal — **the cache's own function** and not a second one.
///
/// What this hashes ends up in `nikaia.lock` beside what the cache hashed, and
/// two spellings of one digest is the shape that lets a build agree with itself
/// and disagree with the file it wrote.
pub fn digest(bytes: &[u8]) -> String {
    orchestrator::cache::sha256_hex(bytes)
}
