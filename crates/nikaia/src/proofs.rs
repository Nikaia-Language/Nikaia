// crates/nikaia/src/proofs.rs
//
// **`nikaia.proofs`: what a build uses is the recorded answer**
// ([ADR-270](../../docs/specification/adr/adr-270.md) D4, D19-D21, #448).
//
// Every question the prover (`prove.rs`) and the bounds walk (`bounds.rs`) ask
// goes through [`ask`]. The query is put into normal form (`nikaia_logic::
// Normal`), keyed by the SHA-256 of that form, and answered from the **book**
// a project build opened, where an entry checks (D20): a `proved` entry's
// certificate is replayed against the normal form, a `refuted` entry's model
// evaluated. A missing entry, or one that does not check, is searched for and
// written in. Nothing in the file can make the compiler drop a check that does
// not hold: it needs no more trust than the solver does.
//
// **The book is the thread's**, opened by [`with_book`] around a build's check
// and lowering, which run on one thread. Outside it - a test, `nikaia lower`
// of a loose file - [`ask`] searches as it always did and records nothing.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use nikaia_logic::{
    Answer, Budget, FourierMotzkin, Model, Normal, Query, Solver, certificate_of, certificate_text,
    model_of, model_text, verify, verify_model,
};

/// The file, beside `nikaia.contracts`.
pub const FILE: &str = "nikaia.proofs";

/// Where a certificate longer than [`LONG`] bytes is kept, one file per key
/// (D19).
pub const LONG_DIR: &str = "nikaia.proofs.d";

/// A certificate longer than this is written to [`LONG_DIR`].
const LONG: usize = 4096;

/// The solver an `unknown` was given by (D19): an entry is searched again
/// only under a different one (D21).
pub const SOLVER: &str = "fourier-motzkin-1";

/// What a question came to, in the program's names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asked {
    /// The facts imply the goal; the certificate checked against the normal
    /// form.
    Proved,
    /// Values under which every fact holds and the goal does not, checked.
    Refuted(Model),
    /// Not shown either way.
    Unknown,
    /// The solver's own proof did not check: a fault of the compiler, which
    /// the caller says rather than believes.
    Rejected(String),
}

/// One recorded answer, as the file writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Entry {
    Proved(String),
    Refuted(String),
    Unknown(String),
}

/// The answers a build reads and writes.
#[derive(Debug, Default, Clone)]
pub struct Book {
    entries: BTreeMap<String, Entry>,
    /// Every key a question this build asked had: what the file keeps.
    asked: BTreeSet<String>,
    /// How many questions were searched rather than read.
    pub searched: usize,
}

thread_local! {
    static BOOK: RefCell<Option<Book>> = const { RefCell::new(None) };
}

/// `run` with `book` open for every [`ask`] on this thread, and the book as
/// it stands after.
pub fn with_book<R>(book: Book, run: impl FnOnce() -> R) -> (R, Book) {
    let outer = BOOK.with(|held| held.replace(Some(book)));
    let out = run();
    let book = BOOK
        .with(|held| std::mem::replace(&mut *held.borrow_mut(), outer))
        .unwrap_or_default();
    (out, book)
}

/// Whether the facts imply the goal, and if not, why not.
pub fn ask(query: &Query<'_>) -> Asked {
    let normal = Normal::of(query);
    let text = normal.text();
    let key = orchestrator::cache::sha256_hex(text.as_bytes());
    let recorded = BOOK.with(|held| {
        let mut held = held.borrow_mut();
        let book = held.as_mut()?;
        book.asked.insert(key.clone());
        book.entries.get(&key).cloned()
    });
    if let Some(entry) = recorded
        && let Some(asked) = read(&normal, &entry)
    {
        return asked;
    }
    let (asked, entry) = search(&normal);
    BOOK.with(|held| {
        if let Some(book) = held.borrow_mut().as_mut() {
            book.searched += 1;
            match entry {
                Some(entry) => book.entries.insert(key, entry),
                None => book.entries.remove(&key),
            };
        }
    });
    asked
}

/// A recorded entry, where it checks (D20) - and an `unknown` only from the
/// solver this compiler has, since a newer one is searched again (D21).
fn read(normal: &Normal, entry: &Entry) -> Option<Asked> {
    let query = normal.query();
    match entry {
        Entry::Proved(text) => {
            let certificate = certificate_of(text)?;
            verify(&query, &certificate).ok()?;
            Some(Asked::Proved)
        }
        Entry::Refuted(text) => {
            let model = model_of(text)?;
            verify_model(&query, &model).then(|| Asked::Refuted(normal.named(&model)))
        }
        Entry::Unknown(solver) => (solver == SOLVER).then_some(Asked::Unknown),
    }
}

/// The reference solver's answer to the normal form, and what to record of
/// it. A proof that does not check is not recorded.
fn search(normal: &Normal) -> (Asked, Option<Entry>) {
    let query = normal.query();
    match FourierMotzkin.check(&query, &Budget::default()) {
        Answer::Proved { certificate } => match verify(&query, &certificate) {
            Ok(()) => (
                Asked::Proved,
                Some(Entry::Proved(certificate_text(&certificate))),
            ),
            Err(why) => (
                Asked::Rejected(format!(
                    "the solver's proof did not check ({why:?}), which is a fault of the compiler"
                )),
                None,
            ),
        },
        Answer::Refuted { model } if verify_model(&query, &model) => (
            Asked::Refuted(normal.named(&model)),
            Some(Entry::Refuted(model_text(&model))),
        ),
        Answer::Refuted { .. } => (Asked::Unknown, None),
        Answer::Unknown(_) => (Asked::Unknown, Some(Entry::Unknown(SOLVER.to_string()))),
    }
}

impl Book {
    /// The book `root`'s `nikaia.proofs` holds, or an empty one. A line that
    /// does not read is left out: it is missing, and missing is searched
    /// (D20).
    pub fn read(root: &Path) -> Book {
        let mut book = Book::default();
        let Ok(text) = std::fs::read_to_string(root.join(FILE)) else {
            return book;
        };
        for line in text.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let mut words = line.split(' ');
            let (Some(key), Some(kind), Some(body), None) =
                (words.next(), words.next(), words.next(), words.next())
            else {
                continue;
            };
            let entry = match kind {
                "proved" if body == "@" => {
                    let Ok(long) = std::fs::read_to_string(root.join(LONG_DIR).join(key)) else {
                        continue;
                    };
                    Entry::Proved(long.trim().to_string())
                }
                "proved" => Entry::Proved(body.to_string()),
                "refuted" => Entry::Refuted(body.to_string()),
                "unknown" => Entry::Unknown(body.to_string()),
                _ => continue,
            };
            book.entries.insert(key.to_string(), entry);
        }
        book
    }

    /// The file as this build leaves it: one line per key a question asked,
    /// sorted by key, so that two branches that add entries merge without a
    /// conflict (D19, D21). An entry nothing asked is gone.
    pub fn render(&self) -> (String, BTreeMap<String, String>) {
        let mut out = String::from(
            "# AUTO-GENERATED by `nikaia`. Commit this file like a lockfile.\n\
             # The prover's answers, one per question it asks: the SHA-256 of the\n\
             # question in normal form, and `proved` with its certificate,\n\
             # `refuted` with its model, or `unknown` with the solver that gave up.\n\
             # Every entry is checked when it is read; a wrong one is searched again.\n",
        );
        let mut long = BTreeMap::new();
        for (key, entry) in &self.entries {
            if !self.asked.contains(key) {
                continue;
            }
            let line = match entry {
                Entry::Proved(text) if text.len() > LONG => {
                    long.insert(key.clone(), text.clone());
                    format!("{key} proved @")
                }
                Entry::Proved(text) => format!("{key} proved {text}"),
                Entry::Refuted(text) => format!("{key} refuted {text}"),
                Entry::Unknown(solver) => format!("{key} unknown {solver}"),
            };
            out.push_str(&line);
            out.push('\n');
        }
        (out, long)
    }

    /// Write the file and its long certificates under `root`, where they
    /// changed.
    pub fn write(&self, root: &Path) -> std::io::Result<()> {
        let (text, long) = self.render();
        let path = root.join(FILE);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            std::fs::write(&path, &text)?;
        }
        let dir = root.join(LONG_DIR);
        if !long.is_empty() {
            std::fs::create_dir_all(&dir)?;
        }
        for (key, certificate) in &long {
            std::fs::write(dir.join(key), certificate)?;
        }
        // A long certificate no entry points at any more goes with it.
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !long.contains_key(&name) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
            if long.is_empty() {
                let _ = std::fs::remove_dir(&dir);
            }
        }
        Ok(())
    }

    /// Whether `root` holds exactly the file this book would write, long
    /// certificates included (D22).
    pub fn as_recorded(&self, root: &Path) -> bool {
        let (text, long) = self.render();
        if std::fs::read_to_string(root.join(FILE)).ok().as_deref() != Some(text.as_str()) {
            return false;
        }
        let dir = root.join(LONG_DIR);
        let on_disk = std::fs::read_dir(&dir)
            .map(|entries| entries.flatten().count())
            .unwrap_or(0);
        on_disk == long.len()
            && long.iter().all(|(key, certificate)| {
                std::fs::read_to_string(dir.join(key)).ok().as_deref() == Some(certificate.as_str())
            })
    }

    /// How many entries the file would hold.
    pub fn len(&self) -> usize {
        self.entries
            .keys()
            .filter(|key| self.asked.contains(*key))
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikaia_logic::Arena;

    /// `0 <= i < n <= len` ⊢ `i < len`, and `x >= 3` ⊢ `x > 10`.
    fn ask_two() {
        let mut a = Arena::new();
        let (i, n, len, zero) = (a.var("i"), a.var("n"), a.var("len"), a.int(0));
        let facts = vec![a.ge(i, zero), a.lt(i, n), a.le(n, len)];
        let goal = a.lt(i, len);
        assert_eq!(
            ask(&Query {
                arena: &a,
                facts: &facts,
                goal
            }),
            Asked::Proved
        );
        let (x, three, ten) = (a.var("x"), a.int(3), a.int(10));
        let low = vec![a.ge(x, three)];
        let high = a.gt(x, ten);
        let Asked::Refuted(model) = ask(&Query {
            arena: &a,
            facts: &low,
            goal: high,
        }) else {
            panic!("x = 3 is a model");
        };
        assert_eq!(model.values.get("x"), Some(&3), "named back: {model:?}");
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("nikaia-proofs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// **D21**: what is missing is searched and recorded; **D4**: a second
    /// build with the file searches nothing.
    #[test]
    fn a_recorded_answer_is_read_and_not_searched_again() {
        let ((), first) = with_book(Book::default(), ask_two);
        assert_eq!((first.searched, first.len()), (2, 2));
        let dir = scratch("read");
        first.write(&dir).expect("written");
        let ((), second) = with_book(Book::read(&dir), ask_two);
        assert_eq!(second.searched, 0);
        assert_eq!(second.render(), first.render());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **D20**: an entry that does not check counts as missing - searched
    /// again and written right - and an entry nothing asked is gone.
    #[test]
    fn a_wrong_entry_is_searched_again_and_an_unasked_one_dropped() {
        let ((), first) = with_book(Book::default(), ask_two);
        let dir = scratch("wrong");
        first.write(&dir).expect("written");
        let text = std::fs::read_to_string(dir.join(FILE)).expect("the file");
        let broken = text.replace(" refuted v0=3", " refuted v0=11").replacen(
            " proved R(",
            " proved R(h9,",
            1,
        ) + &format!("{} proved R(h0)\n", "0".repeat(64));
        std::fs::write(dir.join(FILE), broken).expect("rewritten");
        let ((), again) = with_book(Book::read(&dir), ask_two);
        assert_eq!(again.searched, 2);
        assert_eq!(again.render(), first.render());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Outside a build nothing is recorded, and the answer is the same.
    #[test]
    fn without_a_book_a_question_is_searched() {
        ask_two();
    }
}
