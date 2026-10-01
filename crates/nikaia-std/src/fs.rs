//! `std::fs` - files.

use std::path::Path;

/// **Where a path may go** ([ADR-108](../../../docs/specification/adr/adr-108.md)
/// D2): the directory the name is resolved under, or the word that says the
/// program answers for the name itself.
///
/// Every path-taking entry of this module takes one, right after the path, with
/// **no default** (D1). That is Part I 5.1's rule doing the work: an option has a
/// default, and a default here is the hole — a call that could leave the root out
/// is a call nobody can be sure remembered it. A call that leaves it out is
/// `NK1101`, the same refusal as any other missing argument.
///
/// Two variants and no shorthand (D2). Not a bare `String` where a `Root` is
/// wanted, which would be a conversion rule this language does not have; and not
/// a second spelling beside the variant, which would be two ways to write one
/// thing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Root {
    /// The name is resolved under this directory and may not leave it.
    ///
    /// A `String` and not a `PathBuf`, because the caller is a Nikaia program
    /// and text in this language is one type: `fs::Root::Dir(store)` hands over
    /// what the program has.
    Dir(String),
    /// No check — the program answers for the name.
    ///
    /// It does not say *checked*. It says *I answer for this*, and it is visible
    /// three times (D4): at the line, in the ledger, and in `nikaia --trust`.
    Anywhere,
}

/// The name this call hands the operating system, or the refusal that it leaves
/// its root.
///
/// **Resolved and compared by component, never as a string prefix** (D3),
/// because `/data` is a prefix of `/data2`. Both sides are canonicalised —
/// symlinks followed, `..` applied — which is what makes a symlink out of the
/// directory the same answer as a `..` out of it.
///
/// **What comes back is the name joined under the root, and not the resolved
/// one.** The resolution is for the *comparison*; what the operating system is
/// given is what the program asked for, under the directory it was given. D3's
/// *never a rewritten name* is the reason, and the case that shows it is
/// `sub/../ok.txt` where `sub` does not exist: resolved it lands on `ok.txt`,
/// and handing that over would serve a file a POSIX `open` of the written name
/// says is not there. So the check answers *inside*, and the operating system
/// answers *not found* — each about the thing it knows.
///
/// **A name that does not exist yet is checked all the same.** `fs::write`
/// creates, so canonicalising the whole name would fail on exactly the call that
/// most needs the check; the walk below canonicalises every component that
/// exists and applies the rest on top.
///
/// **`Anywhere` resolves nothing**, which is the whole of what it costs at run
/// time: the name goes to the operating system exactly as the program wrote it.
fn resolve(path: &Path, root: &Root) -> Result<std::path::PathBuf, crate::io::IoError> {
    let Root::Dir(dir) = root else {
        return Ok(path.to_path_buf());
    };
    let asked = path.display().to_string();
    let outside = || crate::io::IoError::Outside(asked.clone());

    // The root itself has to resolve. One that does not is not a directory a
    // name may be used under, and answering `Outside` for it is the fail-closed
    // direction ([ADR-010](../../../docs/specification/adr/adr-010.md) D1).
    let base = std::fs::canonicalize(Path::new(dir)).map_err(|_| outside())?;

    // **An absolute name gets the same treatment**: resolved, compared, and fine
    // where it lies inside. A relative one is joined under the root, which is
    // what makes the root this call's working directory and closes D1's
    // *whoever controls the working directory has changed what the name means*.
    let joined = match path.is_absolute() {
        true => path.to_path_buf(),
        false => base.join(path),
    };
    let landed = climb(&joined).ok_or_else(outside)?;
    match landed.starts_with(&base) {
        true => Ok(joined),
        false => Err(outside()),
    }
}

/// `path` with every symlink followed and every `..` applied, for a name whose
/// tail may not exist yet.
///
/// `std::fs::canonicalize` answers only for a name that is there, and a write
/// creates. So this walks the components: each ordinary one is pushed and the
/// result canonicalised **where it exists**, so a symlink is followed the moment
/// it can be; a `.` is dropped; and a `..` pops.
///
/// **A `..` with nothing to pop is `None`**, because a name that climbs above the
/// filesystem root has left every directory it could have been joined to. The
/// caller reads that as *outside*.
fn climb(path: &Path) -> Option<std::path::PathBuf> {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            other => {
                out.push(other);
                if let Ok(real) = std::fs::canonicalize(&out) {
                    out = real;
                }
            }
        }
    }
    Some(out)
}

/// A file's bytes, addressable as text for as long as the value lives.
///
/// Part III, 17.1: a memory mapping. The pages *are* the buffer, so a 13 GB
/// input costs no copy and every view a parser yields points straight into the
/// file. The text is validated as UTF-8 once, when the map is made; after that
/// a view of it is a view of the pages.
///
/// ADR-009 D7 allows a large read-only mapping to be left to the OS at process
/// exit. Nothing here does that yet: the mapping is released when the value is
/// dropped, and the drop is what `main` returning costs.
pub struct Mapped {
    map: Backing,
}

enum Backing {
    Pages(checked_text::CheckedText<checked_text::Mmap>),
    /// A zero-length file cannot be mapped, and an empty input is still an
    /// input.
    Empty,
}

impl std::ops::Deref for Mapped {
    type Target = str;

    fn deref(&self) -> &str {
        match &self.map {
            Backing::Pages(pages) => pages,
            Backing::Empty => "",
        }
    }
}

impl AsRef<str> for Mapped {
    fn as_ref(&self) -> &str {
        self
    }
}

/// A mapping is a buffer views of text are cut from, so a view of it kept by a
/// container that drops entries is found in it again (ADR-221 D2).
impl crate::tether::Viewed for Mapped {
    fn bytes(&self) -> &[u8] {
        let text: &str = self;
        text.as_bytes()
    }
}

/// Map a file and make its contents addressable as text.
///
/// Fails as the file system does, and additionally when the file is not
/// UTF-8 - a parser handed such a mapping would find that out one byte at a
/// time, and the boundary of a frame is a string, not a byte.
///
/// **`async` with nothing awaited inside it, and that costs nothing.** A
/// mapping is an `open`, a `stat` and an `mmap` - a path walk and a page-table
/// change, with no data transfer to complete and so nothing to suspend on
/// (`rt::uring`'s own note about what is not completed there). It is `async`
/// because the ledger says it does I/O and may pause, which is a claim about
/// the *surface* - and an `async fn` that never awaits finishes on its first
/// poll ([ADR-055](../../../docs/specification/adr/adr-055.md) D1). A page
/// fault later is not a suspension point this language can see, and pretending
/// otherwise would be a promise nothing keeps.
pub async fn map(path: impl AsRef<Path>, root: &Root) -> Result<Mapped, crate::io::IoError> {
    // **The path, held**: an operating system's error does not carry what was
    // asked for, and `NotFound` without it is the round trip to the user
    // [ADR-023](../../../docs/specification/adr/adr-023.md) D3 names. What is
    // *held* is what the caller asked for and what is *used* is what the root
    // resolved it to, so a refusal names the name the program wrote.
    let asked = path.as_ref().display().to_string();
    let path = resolve(path.as_ref(), root)?;
    let path = path.as_path();
    let named = |e| crate::io::IoError::of(e, &asked);
    let file = std::fs::File::open(path).map_err(named)?;
    if file.metadata().map_err(named)?.len() == 0 {
        return Ok(Mapped {
            map: Backing::Empty,
        });
    }

    // Mapped and checked as text in one step, in the crate that holds what
    // that takes (ADR-218), which says what a map cannot rule out.
    let pages = match checked_text::map(&file).map_err(named)? {
        Ok(pages) => pages,
        Err(at) => {
            return Err(crate::io::IoError::NotText(format!(
                "{asked} (at byte {at})"
            )));
        }
    };

    Ok(Mapped {
        map: Backing::Pages(pages),
    })
}

/// A whole file, as text.
///
/// Part III, 17.1. The other half of `map`, and the one to reach for when the
/// file is small or has to outlive the parse: this copies the bytes into a
/// `String` the caller owns, where `map` hands back pages it does not.
///
/// Fails as the file system does, and additionally when the file is not
/// UTF-8 - the same rule `map` follows, for the same reason.
///
/// **The read goes through the runtime**
/// ([ADR-038](../../../docs/specification/adr/adr-038.md) D3): the kernel
/// completes it where the machine has a completion queue, and an I/O worker
/// performs it where it does not. Which one is invisible from here and
/// invisible from a `.nika` file - that is what "one `std` surface" means, and
/// it is why the next change of mechanism is a `std` change rather than a
/// compiler change (ADR-033 §8.4 gave the same reason for the overlap
/// vehicle).
///
/// The read is `rt::io`'s rather than [`read`]'s, and deliberately: `read`
/// hands back a shared buffer and this wants the bytes themselves, so going
/// through it would buy a handle only to copy out of it.
pub async fn read_to_string(
    path: impl AsRef<Path>,
    root: &Root,
) -> Result<String, crate::io::IoError> {
    let what = path.as_ref().display().to_string();
    let path = resolve(path.as_ref(), root)?;
    let bytes = crate::rt::io::reading(&path)
        .await
        .map_err(|e| crate::io::IoError::of(e, &what))?;
    text(bytes, &what)
}

/// Bytes as text, or the failure `read_to_string` reports for bytes that are
/// not.
///
/// The half of [`read_to_string`] that is not the read, so that a read
/// performed somewhere else - one half of a pair the runtime put in flight
/// ([`crate::task::as_text`]) - is finished by exactly the same check rather
/// than by a second copy of it. One place, for the reason ADR-033 §8.4 gives
/// about the vehicle: the next change belongs in `std`.
pub(crate) fn text(bytes: Vec<u8>, what: &str) -> Result<String, crate::io::IoError> {
    // The same check `map` makes, and the same reason: a parser handed bytes
    // that are not text would find that out one view at a time.
    match checked_text::CheckedText::check(bytes) {
        Ok(text) => Ok(text.into_string()),
        Err(at) => Err(crate::io::IoError::NotText(format!(
            "{what} (at byte {at})"
        ))),
    }
}

/// Two files, **both in flight at once**, answered in the order they were
/// asked for.
///
/// ADR-033 §8.5's prediction, as a function: two operations that meet on
/// nothing and do not wait for each other, with **no thread started or woken
/// for the pair** - the cost §8.4 measured at ~46 µs for a pair on the pool and
/// could not remove with any user-space vehicle.
///
/// It is available at `user_parallelism = no`, and that is not a loophole: the
/// two reads are `std`'s own operations and nothing the *user* wrote runs
/// concurrently ([ADR-037](../../../docs/specification/adr/adr-037.md) D2,
/// [ADR-016](../../../docs/specification/adr/adr-016.md) D3).
///
/// **It always overlaps**, because a program asked for it - on the completion
/// path for nothing, and on the blocking fallback for the ~38 µs a worker's
/// wake-up costs, which above a quarter megabyte a pair it earns back
/// (ADR-038 §4.3). There used to be a quieter twin for the pairs the
/// *compiler* put together, which overlapped only where that was free; the
/// automatic grouping is withdrawn ([ADR-050](../../../docs/specification/adr/adr-050.md)
/// D1) and the twin went with it, so this is the only pair vehicle left and
/// every use of it is a written one.
pub fn read_both(
    a: impl AsRef<Path>,
    b: impl AsRef<Path>,
) -> (
    Result<Vec<u8>, std::io::Error>,
    Result<Vec<u8>, std::io::Error>,
) {
    crate::rt::io::read_both(a.as_ref(), b.as_ref())
}

/// A whole file, as bytes.
///
/// Part III 17.1. The half of `read_to_string` that does not check: an input
/// that is not text is not a failure here, because nothing downstream is going
/// to cut a `&str` out of it. Reach for this where the bytes are the point -
/// an image, a checksum, a format with a length prefix.
///
/// **`Bytes` and not a `Vec[u8]`**, which is what Part III 17.2 has always
/// said and [ADR-156](../../../docs/specification/adr/adr-156.md) D3 makes
/// true: one shared buffer, so handing the file on costs a count rather than a
/// copy of it.
pub async fn read(
    path: impl AsRef<Path>,
    root: &Root,
) -> Result<crate::bytes::Bytes, crate::io::IoError> {
    let asked = path.as_ref().display().to_string();
    let path = resolve(path.as_ref(), root)?;
    crate::rt::io::reading(&path)
        .await
        .map(crate::bytes::Bytes::from)
        .map_err(|e| crate::io::IoError::of(e, &asked))
}

/// A whole file, written.
///
/// Part III 17.1. The file is created if it is not there and **truncated if it
/// is**, which is what `write` means everywhere and is why the specification
/// gives it `create: bool = true` as the default rather than as a decision at
/// the call.
///
/// A whole file, written.
///
/// Part III 17.1, in full:
///
/// ```nika
/// fs::write(path, data)                        // create or truncate
/// fs::write(path, data; append: true)          // add to the end
/// fs::write(path, data; create: false)         // refuse to make a new file
/// ```
///
/// `append` and `create` are Kap 5.1 **options** - after the `;`, named at the
/// call, never positional. The lowering makes them ordinary parameters in
/// declaration order and fills in the defaults a caller left out, so this
/// signature is what a Nikaia call expands to rather than what it looks like.
///
/// The defaults are the ones the specification names, and they are what `write`
/// means everywhere: create the file if it is not there, and truncate it if it
/// is. `append` keeps what is there and adds; `create: false` refuses to make a
/// file that does not exist, which is how a program says it means to overwrite
/// something in particular.
///
/// Takes anything that is bytes, so a Nikaia program hands it a `String`, a
/// `&str` or a buffer without saying which it meant.
///
/// It is not `sync` (Part II, 12.1) and it is not a source (ADR-010 D2): it
/// does I/O, and the bytes travel out of the program rather than in.
pub async fn write(
    path: impl AsRef<Path>,
    root: &Root,
    data: impl AsRef<[u8]>,
    append: bool,
    create: bool,
) -> Result<(), crate::io::IoError> {
    // Through the runtime, like the reads (ADR-038 D3). The bytes travel as a
    // borrowed slice and the runtime owns the copy only where the *kernel*
    // needs one to outlive the submission - which is the completion path, and
    // is `rt::uring`'s soundness rule rather than a convenience.
    let asked = path.as_ref().display().to_string();
    let path = resolve(path.as_ref(), root)?;
    crate::rt::io::writing(&path, data.as_ref(), append, create)
        .await
        .map(|_| ())
        .map_err(|e| crate::io::IoError::of(e, &asked))
}

/// Where `nikaia test` puts a test's own directory, for [`scratch`]
/// ([ADR-247](../../../docs/specification/adr/adr-247.md) D5).
pub const TEST_DIR_VAR: &str = "NIKAIA_TEST_DIR";

/// **A fresh, empty directory, as a [`Root`]**
/// ([ADR-247](../../../docs/specification/adr/adr-247.md) D5): what a test
/// hands a function that writes, so that what it writes is checked to stay
/// inside a place of the test's own (ADR-108 D3).
///
/// Under `nikaia test` it is made inside the test's own directory, which the
/// runner removes when the test's process ends. Anywhere else it is a new
/// directory under the system's temporary directory, left to the operating
/// system as any temporary file is. Each call is a new directory: two tests,
/// or two calls in one, never share one.
pub fn scratch() -> Result<Root, crate::io::IoError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CALLS: AtomicU64 = AtomicU64::new(0);
    let under = match std::env::var_os(TEST_DIR_VAR) {
        Some(dir) => std::path::PathBuf::from(dir),
        None => std::env::temp_dir(),
    };
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let name = format!(
        "nikaia-scratch-{}-{}-{nanos}",
        std::process::id(),
        CALLS.fetch_add(1, Ordering::Relaxed)
    );
    let dir = under.join(name);
    std::fs::create_dir_all(&dir)
        .map_err(|e| crate::io::IoError::of(e, &dir.display().to_string()))?;
    Ok(Root::Dir(dir.display().to_string()))
}

/// **A file written through a buffer, flushed when it is done with**
/// ([ADR-239](../../../docs/specification/adr/adr-239.md) D6): what `create`
/// hands back, and the first type in `std` whose cleanup needs I/O.
///
/// `write` adds to the buffer and does no I/O; `flush` writes what is held; and
/// a writer nobody flushed is flushed by its cleanup, at the end of the scope
/// that owned it last. That write can fail, so the function that owns one can
/// fail with it — the decades-old `stdio` data-loss bug, told the truth about
/// (D3). `close()` does it at a named moment and hands the failure back
/// as that call's.
pub type Writer = crate::cleanup::CleanedSend<Buffered>;

/// What a [`Writer`] holds: where the bytes go, and those not written yet.
pub struct Buffered {
    path: std::path::PathBuf,
    shown: String,
    held: Vec<u8>,
}

impl Buffered {
    /// Add text to what the file will hold. No I/O.
    pub fn write(&mut self, data: impl AsRef<[u8]>) {
        self.held.extend_from_slice(data.as_ref());
    }

    /// Write what is held to the file, pausing while it is written.
    pub async fn flush(&mut self) -> Result<(), crate::io::IoError> {
        if self.held.is_empty() {
            return Ok(());
        }
        crate::rt::io::writing(&self.path, &self.held, true, true)
            .await
            .map_err(|e| crate::io::IoError::of(e, &self.shown))?;
        self.held.clear();
        Ok(())
    }
}

impl crate::cleanup::CleanupSend for Buffered {
    fn cleanup(
        &mut self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + '_>> {
        Box::pin(async move { self.flush().await.map_err(|e| e.to_string()) })
    }

    fn describe(&self) -> String {
        format!("the file `{}`", self.shown)
    }
}

/// **A file to write, empty**: made where it is not there, emptied where it
/// is. What is written to it is held until it is flushed, closed, or done
/// with.
pub async fn create(path: impl AsRef<Path>, root: &Root) -> Result<Writer, crate::io::IoError> {
    let shown = path.as_ref().display().to_string();
    let path = resolve(path.as_ref(), root)?;
    crate::rt::io::writing(&path, b"", false, true)
        .await
        .map_err(|e| crate::io::IoError::of(e, &shown))?;
    Ok(crate::cleanup::CleanedSend::new(Buffered {
        path,
        shown,
        held: Vec::new(),
    }))
}

/// **Every file under a directory**, as names relative to it, `/` between the
/// parts, in sorted order ([ADR-195](../../../docs/specification/adr/adr-195.md)
/// D4).
///
/// What a tool that reads a tree needs, and the shape `nikaia describe` reads a
/// crate's sources in: the files themselves, recursively, with the directories
/// left out, because a directory is never what a caller then reads. Sorted, so
/// two runs over the same tree hand back the same list — an order the operating
/// system happens to keep is not one a program may rest on.
///
/// **The root holds for every name the walk finds, and not only for the one it
/// was asked for** ([ADR-108](../../../docs/specification/adr/adr-108.md) D3).
/// A symlink inside the directory may point out of it; under `Root::Dir` such an
/// entry is *not listed*, rather than failing the walk, because the program did
/// not ask for that name and a tree with one stray link in it is still a tree
/// worth reading. `Root::Anywhere` follows it, as it follows everything.
///
/// **A directory is visited once**, by its resolved name, so a link that points
/// back up the tree ends the walk instead of looping it.
///
/// It fails where the directory itself cannot be read, and where a name under it
/// is not UTF-8 — text in this language is one type, and a name handed back
/// changed would be a name that opens a different file.
///
/// `async` with nothing awaited, for the reason [`map`] gives: the ledger says
/// it does I/O and may pause.
pub async fn walk(path: impl AsRef<Path>, root: &Root) -> Result<Vec<String>, crate::io::IoError> {
    let asked = path.as_ref().display().to_string();
    let start = resolve(path.as_ref(), root)?;
    let base = match root {
        Root::Dir(dir) => Some(
            std::fs::canonicalize(Path::new(dir))
                .map_err(|_| crate::io::IoError::Outside(asked.clone()))?,
        ),
        Root::Anywhere => None,
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut found = Vec::new();
    // The directory asked for has to be readable; one under it that is not is
    // left out, as a name outside the root is.
    std::fs::read_dir(&start).map_err(|e| crate::io::IoError::of(e, &asked))?;
    let mut pending = vec![(start, String::new())];
    while let Some((dir, prefix)) = pending.pop() {
        let Ok(real) = std::fs::canonicalize(&dir) else {
            continue;
        };
        if !seen.insert(real) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return Err(crate::io::IoError::NotText(format!(
                    "{asked}/{prefix}{}",
                    name.to_string_lossy()
                )));
            };
            let at = entry.path();
            let Ok(real) = std::fs::canonicalize(&at) else {
                continue;
            };
            if base.as_ref().is_some_and(|base| !real.starts_with(base)) {
                continue;
            }
            let relative = format!("{prefix}{name}");
            match real.is_dir() {
                true => pending.push((at, format!("{relative}/"))),
                false => found.push(relative),
            }
        }
    }
    found.sort();
    Ok(found)
}

#[cfg(test)]
mod writer {
    use super::*;

    /// **What a writer holds reaches the file when it dies and its task
    /// settles**, and not before.
    #[test]
    fn a_writer_is_flushed_by_its_cleanup() {
        let dir = std::env::temp_dir().join(format!("nikaia-writer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        let file = dir.join("out.txt");
        crate::rt::exec::block_on(async {
            {
                let mut w = create(&file, &Root::Anywhere).await.expect("created");
                w.write("eins\n");
                w.write("zwei\n");
            }
            assert_eq!(std::fs::read_to_string(&file).expect("there"), "");
            crate::cleanup::settle::<crate::cleanup::Local>()
                .await
                .expect("flushed");
        });
        assert_eq!(
            std::fs::read_to_string(&file).expect("there"),
            "eins\nzwei\n"
        );

        crate::rt::exec::block_on(async {
            let mut w = create(&file, &Root::Anywhere).await.expect("created");
            w.write("drei");
            w.close().await.expect("closed");
        });
        assert_eq!(std::fs::read_to_string(&file).expect("there"), "drei");
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// Half of a pair finished by `task::as_text` is finished exactly as
    /// `read_to_string` would have finished it (ADR-033 D10).
    ///
    /// The lowering performs the read somewhere else and hands the bytes back,
    /// so this is the join: same value on the happy path, and the *same
    /// failure* on the other one. A pair whose halves reported a different
    /// error from the sequential program would be D1 broken by a vehicle, which
    /// is the one thing the overlap may never do.
    #[test]
    fn a_half_of_a_pair_is_finished_the_way_read_to_string_finishes_it() {
        let dir = std::env::temp_dir().join(format!("nikaia-as-text-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");

        let text = dir.join("text");
        std::fs::write(&text, "Hamburg;12.0\n").expect("write");
        // Driven rather than called, since ADR-055 §6 step 3: a read is a
        // future now, and `block_on` is what a program's `main` drives it with
        // (ADR-038 D4) - so a test that awaited it any other way would be
        // testing something no program does.
        let (mine, theirs) = crate::rt::exec::block_on(async {
            (
                crate::task::as_text(
                    crate::rt::io::reading(&text).await,
                    &text.display().to_string(),
                ),
                super::read_to_string(&text, &super::Root::Anywhere).await,
            )
        });
        assert_eq!(mine.expect("text"), theirs.expect("text"));

        // `0xff` is not UTF-8 anywhere, so both routes have to refuse it - and
        // refuse it with the same words and the same byte offset.
        let bytes = dir.join("bytes");
        std::fs::write(&bytes, [b'a', 0xff]).expect("write");
        let (one, other) = crate::rt::exec::block_on(async {
            (
                crate::task::as_text(
                    crate::rt::io::reading(&bytes).await,
                    &bytes.display().to_string(),
                ),
                super::read_to_string(&bytes, &super::Root::Anywhere).await,
            )
        });
        let one = one.expect_err("not text");
        let other = other.expect_err("not text");
        // **The values and not a `kind()` beside them**: since
        // [ADR-158](../../../docs/specification/adr/adr-158.md) D1 the variant
        // *is* the kind and it carries what it was about, so comparing the two
        // errors says what this used to need two assertions to say.
        assert_eq!(one, other);
        assert_eq!(one.to_string(), other.to_string());
        assert!(one.to_string().contains("byte 1"), "{one}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A written file reads back byte for byte, and a second write replaces
    /// what the first one left rather than adding to it.
    #[test]
    fn a_file_is_written_whole_and_replaced_whole() {
        let path = std::env::temp_dir().join(format!("nikaia-write-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);

        // One `block_on` for the whole test, which is what a program is: the
        // executor drives `main` and every read and write inside it (ADR-055 §6
        // step 2 and step 3).
        crate::rt::exec::block_on(async {
            super::write(&path, &super::Root::Anywhere, "Hamburg;12.0\n", false, true)
                .await
                .expect("write");
            assert_eq!(
                super::read_to_string(&path, &super::Root::Anywhere)
                    .await
                    .expect("read back"),
                "Hamburg;12.0\n"
            );

            super::write(&path, &super::Root::Anywhere, "Bremen;9.5\n", false, true)
                .await
                .expect("write again");
            assert_eq!(
                super::read_to_string(&path, &super::Root::Anywhere)
                    .await
                    .expect("read back"),
                "Bremen;9.5\n",
                "a second write truncates rather than appends"
            );

            // Bytes are bytes: what `read` hands back is what `write` was
            // given, with no check in between - which is the difference from
            // `read_to_string` and the reason both exist.
            super::write(
                &path,
                &super::Root::Anywhere,
                [0xFFu8, 0x00, 0xFE],
                false,
                true,
            )
            .await
            .expect("write bytes");
            assert_eq!(
                super::read(&path, &super::Root::Anywhere)
                    .await
                    .expect("read bytes")
                    .as_slice(),
                [0xFF, 0x00, 0xFE]
            );
            assert!(
                super::read_to_string(&path, &super::Root::Anywhere)
                    .await
                    .is_err(),
                "the same bytes are not text, and the text half says so"
            );
        });

        std::fs::remove_file(&path).expect("clean up");
    }

    /// Failing to write is an ordinary failure, not a panic: a path whose
    /// parent is not there is the commonest one there is.
    #[test]
    fn writing_where_nothing_can_be_written_fails() {
        let path = std::env::temp_dir()
            .join("nikaia-no-such-directory")
            .join("report.html");
        assert!(
            crate::rt::exec::block_on(super::write(
                &path,
                &super::Root::Anywhere,
                "x",
                false,
                true
            ))
            .is_err()
        );
    }

    /// **Many threads reading and writing at once each get their own answer.**
    ///
    /// A finished operation's handle used to give its ring slot back when it
    /// was replaced by its answer - a `Drop` running on the assignment - and by
    /// then another thread could already hold that slot. Its operation was
    /// marked as nobody's, reclaimed before it was read, and answered *the
    /// runtime lost a file operation's slot*: a CI failure of
    /// `appending_adds_and_create_false_refuses_a_new_file`, once in a dozen
    /// runs, with other tests running beside it.
    #[test]
    fn many_threads_reading_and_writing_at_once_each_get_their_own_answer() {
        let dir = std::env::temp_dir().join(format!("nikaia-many-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        let threads: Vec<_> = (0..8)
            .map(|t| {
                let dir = dir.clone();
                std::thread::spawn(move || {
                    let path = dir.join(format!("file-{t}"));
                    crate::rt::exec::block_on(async {
                        for round in 0..300 {
                            let text = format!("{t}:{round}\n");
                            super::write(&path, &super::Root::Anywhere, &text, false, true)
                                .await
                                .expect("write");
                            let back = super::read_to_string(&path, &super::Root::Anywhere)
                                .await
                                .expect("read back");
                            assert_eq!(back, text);
                        }
                    });
                })
            })
            .collect();
        for thread in threads {
            thread.join().expect("a thread's reads and writes");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The two options Part III 17.1 names, doing what it says they do.
    #[test]
    fn appending_adds_and_create_false_refuses_a_new_file() {
        let path = std::env::temp_dir().join(format!("nikaia-options-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let missing = std::env::temp_dir().join(format!("nikaia-absent-{}", std::process::id()));
        let _ = std::fs::remove_file(&missing);

        crate::rt::exec::block_on(async {
            super::write(&path, &super::Root::Anywhere, "one\n", false, true)
                .await
                .expect("write");
            super::write(&path, &super::Root::Anywhere, "two\n", true, true)
                .await
                .expect("append");
            assert_eq!(
                super::read_to_string(&path, &super::Root::Anywhere)
                    .await
                    .expect("read back"),
                "one\ntwo\n",
                "appending kept what was there"
            );

            // …and `create: false` is how a program says it means to overwrite
            // something in particular, rather than to make a file.
            assert!(
                super::write(&missing, &super::Root::Anywhere, "x", false, false)
                    .await
                    .is_err()
            );
            assert!(!missing.exists(), "`create: false` made the file anyway");

            // On a file that *is* there it writes, and truncates as `write`
            // does.
            super::write(&path, &super::Root::Anywhere, "three\n", false, false)
                .await
                .expect("overwrite");
            assert_eq!(
                super::read_to_string(&path, &super::Root::Anywhere)
                    .await
                    .expect("read back"),
                "three\n"
            );
        });

        std::fs::remove_file(&path).expect("clean up");
    }

    /// **[ADR-108](../../../docs/specification/adr/adr-108.md) D3, case by
    /// case**, on a real directory with a real symlink in it.
    ///
    /// The comparison is what the record is about, so it is measured rather than
    /// reasoned about: a sibling whose name begins with the root's (`/data` and
    /// `/data2`) is the case a string prefix gets wrong, and it is in here.
    #[test]
    fn a_name_is_checked_against_the_root_it_was_given() {
        let dir = std::env::temp_dir().join(format!("nikaia-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = dir.join("store");
        // `store2` begins with `store`, which is the whole reason the comparison
        // is by component: as a string, `store` is a prefix of `store2`.
        let sibling = dir.join("store2");
        std::fs::create_dir_all(store.join("sub")).expect("scratch");
        std::fs::create_dir_all(&sibling).expect("scratch");
        std::fs::write(store.join("ok.txt"), "inside").expect("write");
        std::fs::write(dir.join("outside.txt"), "secret").expect("write");
        std::fs::write(sibling.join("near.txt"), "near").expect("write");

        let root = super::Root::Dir(store.display().to_string());
        let inside = |name: &str| super::resolve(Path::new(name), &root).is_ok();

        assert!(inside("ok.txt"), "a plain name under the root");
        assert!(inside("./ok.txt"), "a `.` is not a way out");
        assert!(inside("sub/deep.txt"), "a name that does not exist yet");
        assert!(inside("sub/../ok.txt"), "a `..` that stays inside");
        assert!(
            inside(store.join("ok.txt").to_str().expect("utf-8")),
            "an absolute name that lies inside is fine"
        );

        assert!(!inside("../outside.txt"), "a `..` out of the root");
        assert!(!inside("sub/../../outside.txt"), "and a longer climb");
        assert!(
            !inside(dir.join("outside.txt").to_str().expect("utf-8")),
            "an absolute name outside"
        );
        assert!(
            !inside(sibling.join("near.txt").to_str().expect("utf-8")),
            "`store2` is not under `store`, and a string prefix would have said it was"
        );
        assert!(
            !inside("../store2/near.txt"),
            "the same, reached relatively"
        );

        // **A symlink is followed before the comparison**, which is what makes it
        // the same answer as a `..`: the name inside the root is polite and what
        // it points at is not.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.join("outside.txt"), store.join("link.txt"))
                .expect("symlink");
            assert!(!inside("link.txt"), "a symlink out of the root");
        }

        // **What comes back is the name joined under the root, not the resolved
        // one** — D3's *never a rewritten name*. `sub/../ok.txt` resolves onto
        // `ok.txt`, and handing that to the operating system would serve a file
        // that an `open` of the written name does not find.
        let handed = super::resolve(Path::new("sub/../ok.txt"), &root).expect("inside");
        assert_eq!(handed, store.join("sub/../ok.txt"));

        // A root that does not resolve is not a directory a name may be used
        // under, and the answer is the fail-closed one (ADR-010 D1).
        let nowhere = super::Root::Dir(dir.join("no-such-store").display().to_string());
        assert!(super::resolve(Path::new("ok.txt"), &nowhere).is_err());

        // **`Anywhere` resolves nothing**: the name goes over exactly as written.
        let free = super::resolve(Path::new("../outside.txt"), &super::Root::Anywhere)
            .expect("`Anywhere` checks nothing");
        assert_eq!(free, Path::new("../outside.txt"));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// And the refusal travels as a failure of the call, with the name the
    /// program asked for in it.
    #[test]
    fn a_read_and_a_write_outside_the_root_fail_with_the_name_that_was_asked_for() {
        let dir = std::env::temp_dir().join(format!("nikaia-root-io-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = dir.join("store");
        std::fs::create_dir_all(&store).expect("scratch");
        std::fs::write(dir.join("outside.txt"), "secret").expect("write");
        let root = super::Root::Dir(store.display().to_string());

        crate::rt::exec::block_on(async {
            let refused = super::read_to_string("../outside.txt", &root)
                .await
                .expect_err("the name leaves the root");
            assert_eq!(refused.what(), "../outside.txt");
            assert!(refused.to_string().contains("leaves the root"), "{refused}");

            // A write is checked before it creates anything, which is the half a
            // canonicalising check would have got wrong.
            assert!(
                super::write("../made.txt", &root, "x", false, true)
                    .await
                    .is_err()
            );
            assert!(
                !dir.join("made.txt").exists(),
                "the write created it anyway"
            );

            // And a name inside the root is written where the root says, not
            // where the working directory does.
            super::write("made.txt", &root, "x", false, true)
                .await
                .expect("inside");
            assert_eq!(
                super::read_to_string("made.txt", &root)
                    .await
                    .expect("read"),
                "x"
            );
            assert!(store.join("made.txt").exists(), "under the root");

            // `map` takes the same root, and refuses on the same comparison.
            assert!(super::map("../outside.txt", &root).await.is_err());
            assert!(super::read("../outside.txt", &root).await.is_err());
        });

        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod walking {
    use super::*;

    /// **Files only, recursively, relative and sorted** — and a link out of a
    /// `Root::Dir` is not listed, while a link back up the tree ends the walk.
    #[test]
    fn a_walk_lists_the_files_under_a_directory_and_stays_inside_its_root() {
        let dir = std::env::temp_dir().join(format!("nikaia-walk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let tree = dir.join("tree");
        std::fs::create_dir_all(tree.join("src/deep")).expect("scratch");
        std::fs::create_dir_all(dir.join("away")).expect("scratch");
        std::fs::write(tree.join("b.rs"), "").expect("write");
        std::fs::write(tree.join("src/a.rs"), "").expect("write");
        std::fs::write(tree.join("src/deep/c.txt"), "").expect("write");
        std::fs::write(dir.join("away/secret"), "").expect("write");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.join("away"), tree.join("out")).expect("link");
            std::os::unix::fs::symlink(&tree, tree.join("src/up")).expect("link");
        }

        let inside = Root::Dir(tree.display().to_string());
        let found = crate::rt::exec::block_on(walk(".", &inside)).expect("walked");
        assert_eq!(found, ["b.rs", "src/a.rs", "src/deep/c.txt"]);

        // From `src`, the link up reaches the rest of the tree once, and its
        // way back into `src` is a directory already seen.
        #[cfg(unix)]
        {
            let under = crate::rt::exec::block_on(walk("src", &inside)).expect("walked");
            assert_eq!(under, ["a.rs", "deep/c.txt", "up/b.rs"]);
        }

        #[cfg(unix)]
        {
            let anywhere = crate::rt::exec::block_on(walk(&tree, &Root::Anywhere)).expect("walked");
            assert_eq!(
                anywhere,
                ["b.rs", "out/secret", "src/a.rs", "src/deep/c.txt"]
            );
        }

        let refused = crate::rt::exec::block_on(walk("..", &inside));
        assert!(matches!(refused, Err(crate::io::IoError::Outside(_))));
        let missing = crate::rt::exec::block_on(walk("nope", &inside));
        assert!(matches!(missing, Err(crate::io::IoError::NotFound(_))));
        std::fs::remove_dir_all(&dir).ok();
    }
}
