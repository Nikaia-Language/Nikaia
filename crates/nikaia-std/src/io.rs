//! `std::io` - standard input.
//!
//! Part III 17.1 and [ADR-019](../../../docs/specification/adr/adr-019.md). A
//! stream is not a file, and the surface says so: no map, no seek, no length,
//! and no second read of the same bytes. `std::fs`'s zero-copy story rests on
//! pages that exist before the program asks for them; stdin's bytes do not
//! exist until they are read, so a program that wants views into its input owns
//! the buffer first.
//!
//! Neither function is `sync`: a read from a pipe is a suspension point, which
//! is exactly what the `sync` rule (Part II, 12.1) exists to keep out of a
//! `par_iter` body.
//!
//! **Whole-of-input reads suspend; a line at a time does not yet**
//! ([ADR-121](../../../docs/specification/adr/adr-121.md) D4).
//!
//! Since [ADR-055](../../../docs/specification/adr/adr-055.md) §6 step 3 these
//! signatures have said a read from here may pause, which is what a caller's
//! compiler reads and what makes the enclosing function `async`. The bodies did
//! not: the blocking read ran on the program's own thread, because a worker's
//! reply could not wake an executor parked on the ring and a future fed from one
//! had nowhere to come back to.
//!
//! ADR-121 D1 put the bell on the ring, so [`read`] and [`read_to_string`] are
//! now the read they always were, performed on an I/O worker and **awaited**.
//! There is still no second read shape and no ring path for a stream with no
//! size to `stat`, which is D4's own sentence: what changed is the park.
//!
//! **And [`lines`] has moved too**
//! ([ADR-172](../../../docs/specification/adr/adr-172.md) D4), which took a
//! decision about the *language* rather than a change to the runtime. A step of
//! it was an `Iterator::next`, and a suspension point inside one is
//! `while let Some(x) = s.next().await` below - so D1 decided that a `for` may
//! iterate something whose step pauses and writes that form, and [`Lines`] has
//! an inherent `async fn next` for it to write. The chunking is what keeps it
//! from costing more than it saves: a hop to a worker per *line* would be
//! slower than the blocking reader, so a buffer comes back at a time.

/// What a `std` call fails with ([ADR-280](../../../docs/specification/adr/adr-280.md) D5).
///
/// Part I 7.1's error type, for the library: an `enum` with payload, `impl
/// Error`, and variants a `catch` can tell apart. Until this existed every
/// `std` entry wrote `throws = ["?"]` — *something this compiler cannot name* —
/// which is the absence of an answer standing in for one, and it made
/// [ADR-023](../../../docs/specification/adr/adr-023.md) D1's set unusable for
/// every program that reads a file.
///
/// **It lives in `io` and is written out.** `use std::io` and
/// `io::IoError::NotFound(…)` is what a program that matches on it writes, which
/// is [ADR-313](../../../docs/specification/adr/adr-313.md) D3's rule applied
/// straight: what needs no prefix is Part I 1.3's list, and this is not on it.
/// `fs::read` throwing an `io` type is the cost, accepted: a program that only
/// *propagates* the failure names nothing, and one that takes it apart writes
/// the second `use`.
///
/// **Why these four.** D4 of [ADR-023](../../../docs/specification/adr/adr-023.md)
/// makes the variants of one error type **closed**, so each is a lasting
/// commitment and the list is short on purpose:
///
/// * `NotFound` is the specification's own — Appendix A.1 calls *a missing
///   file* the example of a recoverable error, and D6's worked output writes
///   `IoError::NotFound`;
/// * `PermissionDenied` is the other environmental failure a program acts on
///   **differently** rather than reports;
/// * `NotText` is `std`'s own rather than the operating system's: three places
///   here check UTF-8 and report `InvalidData`, and Part III 17.1 specifies the
///   check;
/// * `Other` is D4's **named residue** — *exhaustiveness degrades to "and
///   anything else"* — carrying what the operating system said.
///
/// **The payload is owned text and not a view.**
/// [ADR-023](../../../docs/specification/adr/adr-023.md) D10 says a path that
/// came from a buffer should travel as a view; that is the tether
/// ([ADR-283](../../../docs/specification/adr/adr-283.md)), which is not built,
/// and a lifetime on this type would reach every `std` signature. One
/// allocation on a path that is already failing is the honest price until then.
/// What an [`IoError`] from this module was about: a stream has no path, and
/// the name is what a message can say instead.
const STDIN: &str = "standard input";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IoError {
    /// It is not there. The payload is what was looked for.
    NotFound(String),
    /// It is there and this program may not have it.
    PermissionDenied(String),
    /// The bytes are not text, and text in this language is UTF-8.
    NotText(String),
    /// **The name would leave the root it was given**
    /// ([ADR-108](../../../docs/specification/adr/adr-108.md) D3), and this is
    /// a refusal rather than a rewritten name: a program that quietly serves a
    /// *different* file than the one asked for is a worse bug than one that
    /// serves nothing.
    ///
    /// The payload is what was asked for, as it was asked for — the resolved
    /// name is not in it, because a message that printed where the name landed
    /// would tell whoever sent it what is on the machine.
    ///
    /// A case beside `NotFound` and not an error type of its own, which is what
    /// D3's *one case beside not found among the errors the entry already names*
    /// asks for: every path-taking entry already `throws = ["io::IoError"]`, so
    /// the root check adds no member to any program's failure set.
    Outside(String),
    /// Everything else, as the operating system said it.
    Other(String),
}

impl IoError {
    /// One of the language below's failures, told apart and given what it was
    /// about.
    ///
    /// `what` is the path, or the name of the stream: an operating system's
    /// error does not carry it, and *not found* without the thing that was not
    /// found is the message D3 calls a round trip to the user.
    pub fn of(error: std::io::Error, what: &str) -> IoError {
        match error.kind() {
            std::io::ErrorKind::NotFound => IoError::NotFound(what.to_string()),
            std::io::ErrorKind::PermissionDenied => IoError::PermissionDenied(what.to_string()),
            std::io::ErrorKind::InvalidData => IoError::NotText(what.to_string()),
            _ => IoError::Other(format!("{what}: {error}")),
        }
    }

    /// What was looked for, where the variant names one.
    pub fn what(&self) -> &str {
        match self {
            IoError::NotFound(what)
            | IoError::PermissionDenied(what)
            | IoError::NotText(what)
            | IoError::Outside(what)
            | IoError::Other(what) => what,
        }
    }
}

impl std::fmt::Display for IoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IoError::NotFound(what) => write!(f, "no such file or directory: {what}"),
            IoError::PermissionDenied(what) => write!(f, "permission denied: {what}"),
            IoError::NotText(what) => write!(f, "not valid UTF-8: {what}"),
            IoError::Outside(what) => {
                write!(f, "the name leaves the root it was given: {what}")
            }
            IoError::Other(what) => f.write_str(what),
        }
    }
}

impl std::error::Error for IoError {}

/// All of standard input, as text.
///
/// UTF-8 validated, for the reason `fs::read_to_string` validates: a parser
/// handed bytes that are not text would find that out one view at a time, and a
/// frame boundary is a string.
///
/// Reading it a second time yields what the operating system says, which is
/// nothing.
pub async fn read_to_string() -> Result<String, IoError> {
    let bytes = crate::rt::io::stdin_whole()
        .await
        .map_err(|e| IoError::of(e, STDIN))?;
    // The same failure `std`'s own `read_to_string` reports, under the name
    // this library gives it: a stream that is not text is `NotText` and not a
    // lossy string.
    String::from_utf8(bytes).map_err(|_| IoError::NotText(STDIN.to_string()))
}

/// All of standard input, as bytes.
///
/// The half for input that is not text - and the one that says what
/// `read_to_string` is doing, since the only difference is the check.
pub async fn read() -> Result<Vec<u8>, IoError> {
    crate::rt::io::stdin_whole()
        .await
        .map_err(|e| IoError::of(e, STDIN))
}

/// Standard input, one line at a time.
///
/// Part III 17.1, and the first thing here that reads an unbounded input in
/// **constant memory**: nothing holds the whole stream, so a pipe that never
/// ends is a program that never grows.
///
/// The lines are **owned**, and 17.1 says why: a file's line can be a view
/// because the file is still there to point at, and a stream's bytes are gone
/// once consumed. Keeping them would be `read_to_string` with extra steps.
/// That is also what keeps this expressible at all - an iterator may hand out
/// views into a buffer it does not own, and never into one it does
/// ([ADR-025](../../../docs/specification/adr/adr-025.md), Wall B).
///
/// **A step can fail**, and the item says so rather than pretending a failed
/// read is the end of the stream - which would turn a truncated input into a
/// shorter one, silently, and is the bug class Part I 6.4 refuses by name. What
/// a Nikaia program writes has none of that in it:
///
/// ```nika
/// fn count() -> i64 throws {
///     let mut n = 0
///     for line in io::lines() { n += 1 }
///     return n
/// }
/// ```
///
/// The failure travels out of the loop and out of the function, and the
/// compiler is what made `throws` be there (`NK2701`). `std.contracts` records
/// `iterates = "throws"` on `Lines`, which is how a compiler that cannot see
/// this body knows.
///
/// The trailing newline is not part of a line, and a final line without one is
/// still a line.
///
/// **This one does not suspend**, and the module comment says why: a step is an
/// `Iterator::next`, and a suspension point inside one needs a `for` over a
/// stream that neither this language nor the one below has decided.
/// [`read_to_string`] is the entry that does, for an input a program is willing
/// to hold whole.
pub async fn lines() -> Lines {
    Lines {
        held: Vec::new(),
        at: 0,
        ended: false,
    }
}

/// What [`lines`] hands back.
///
/// A named type rather than `impl Iterator`, because the ledger names types and
/// a caller's compiler looks this one up by name (ADR-020).
///
/// **Its step pauses** ([ADR-172](../../../docs/specification/adr/adr-172.md)
/// D4), and the buffer is why that costs nothing: a hop to an I/O worker and
/// back is a wake-up, and paying one per *line* would make this slower than
/// the blocking reader it replaces. So a chunk comes back at a time and the
/// line endings are found here — one hop per buffer, and a program that reads
/// a million lines pays as many hops as it has buffers.
///
/// **Not an `Iterator`**, and that is the decision rather than an omission.
/// `Iterator::next` has no `await` in it, which is the whole reason D1 exists;
/// the inherent `async fn next` below is the shape a `for` over a pausing
/// sequence lowers to. The trait a second such producer wanted is
/// `nikaia_std::seq::Step` ([ADR-233](../../../docs/specification/adr/adr-233.md)
/// D1), and this is one: its item is the line or the failure.
pub struct Lines {
    /// What the last chunks brought and what has not been handed out yet.
    held: Vec<u8>,
    /// How far into `held` the lines already handed out reach.
    at: usize,
    /// The stream is over: no further chunk is asked for, and what is left in
    /// `held` is the last line if it is not empty.
    ended: bool,
}

/// How much of standard input one hop to a worker asks for.
///
/// The size `BufReader` uses by default, and for the reason it does: big
/// enough that the hops are rare, small enough that a program holding one per
/// stream is holding nothing worth counting.
const CHUNK: usize = 8 * 1024;

impl Lines {
    /// The next line, **pausing** while the stream has nothing to give
    /// ([ADR-172](../../../docs/specification/adr/adr-172.md) D4).
    ///
    /// **`IoError` and not the language below's own**
    /// ([ADR-280](../../../docs/specification/adr/adr-280.md) D5): a step of
    /// this fails, so the function around the `for` says `throws` — and what it
    /// throws is what `std` says it throws, here as everywhere else. This was
    /// the one surface that record missed, and it showed the day a program
    /// whose whole set was `io::IoError` got a **named** channel
    /// ([ADR-280](../../../docs/specification/adr/adr-280.md) D9): the `?` the
    /// loop's step takes had a raw `std::io::Error` on the left of it and a
    /// named one on the right.
    ///
    /// The trailing newline is not part of a line, a `\r` before it is not
    /// either, and a final line without one is still a line — which is
    /// `BufRead::lines`' own rule, kept because programs were written against
    /// it.
    pub async fn next(&mut self) -> Option<Result<String, IoError>> {
        loop {
            if let Some(line) = self.take_a_line() {
                return Some(line);
            }
            if self.ended {
                return None;
            }
            match crate::rt::io::stdin_chunk(CHUNK).await {
                Err(e) => {
                    // The stream is over either way: a reader that kept asking
                    // after a failure would loop on it.
                    self.ended = true;
                    return Some(Err(IoError::of(e, STDIN)));
                }
                Ok(chunk) if chunk.is_empty() => self.ended = true,
                Ok(chunk) => {
                    // What has been handed out is dropped before the new bytes
                    // go in, so the buffer is the size of what is *unread*
                    // rather than of everything the stream ever carried —
                    // which is what keeps a pipe larger than memory a program
                    // that does not grow (Part III, 17.1).
                    self.held.drain(..self.at);
                    self.at = 0;
                    self.held.extend_from_slice(&chunk);
                }
            }
        }
    }

    /// Everything the sequence produces, as a list — `Seq::collect` over a
    /// sequence whose step pauses
    /// ([ADR-172](../../../docs/specification/adr/adr-172.md) D5).
    ///
    /// **The eager walks**: what `collect`, `count`, `nth` and `join` hand back
    /// is a value, so each is a loop around [`Lines::next`] and nothing more.
    /// The lazy ones hand back another sequence whose step pauses and can
    /// fail, `nikaia_std::seq::Failing`
    /// ([ADR-234](../../../docs/specification/adr/adr-234.md)).
    ///
    /// **The first failure ends the walk.** A `collect` that swallowed one
    /// would turn a truncated stream into a shorter list, which is the bug
    /// class Part I 6.4 refuses by name — and is what this entry did until
    /// D5, when `Lines` was an `Iterator` over `Result` and the ledger said
    /// the result was a list of strings.
    pub async fn collect(mut self) -> Result<Vec<String>, IoError> {
        let mut out = Vec::new();
        while let Some(line) = self.next().await {
            out.push(line?);
        }
        Ok(out)
    }

    /// How many lines the sequence produces — `Seq::count`.
    ///
    /// It walks the whole of it to answer, which is what separates this from a
    /// list's `len()`, and it holds one line at a time while it does.
    pub async fn count(mut self) -> Result<i64, IoError> {
        let mut n = 0_i64;
        while let Some(line) = self.next().await {
            line?;
            n += 1;
        }
        Ok(n)
    }

    /// The line at a position, or nothing where the stream is shorter than that
    /// — `Seq::nth`. It walks up to that position to answer.
    pub async fn nth(mut self, at: i64) -> Result<Option<String>, IoError> {
        if at < 0 {
            return Ok(None);
        }
        let mut seen = 0_i64;
        while let Some(line) = self.next().await {
            let line = line?;
            if seen == at {
                return Ok(Some(line));
            }
            seen += 1;
        }
        Ok(None)
    }

    /// Every line written one after another with this text between them —
    /// `Seq::join`.
    pub async fn join(mut self, separator: &str) -> Result<String, IoError> {
        let mut out = String::new();
        let mut first = true;
        while let Some(line) = self.next().await {
            if !first {
                out.push_str(separator);
            }
            out.push_str(&line?);
            first = false;
        }
        Ok(out)
    }

    /// Each line through a lambda — `Seq::map`, lazy: nothing is read until
    /// something walks the result
    /// ([ADR-234](../../../docs/specification/adr/adr-234.md) D1). A failure
    /// passes through to that walk, which is where the program's `?` is.
    pub fn map<U, F: FnMut(String) -> U>(
        self,
        f: F,
    ) -> crate::seq::Failing<crate::seq::OkMap<Self, F>> {
        crate::seq::Failing(self).map(f)
    }

    /// Only the lines a lambda says yes to — `Seq::filter`, lazy.
    pub fn filter<F: FnMut(&String) -> bool>(
        self,
        f: F,
    ) -> crate::seq::Failing<crate::seq::OkFilter<Self, F>> {
        crate::seq::Failing(self).filter(f)
    }

    /// Each line through a lambda that pauses
    /// ([ADR-233](../../../docs/specification/adr/adr-233.md) D1).
    pub fn then<U, F: AsyncFnMut(String) -> U>(
        self,
        f: F,
    ) -> crate::seq::Failing<crate::seq::OkThen<Self, F>> {
        crate::seq::Failing(self).then(f)
    }

    /// Only the lines a lambda that pauses says yes to.
    pub fn then_filter<F: AsyncFnMut(&String) -> bool>(
        self,
        f: F,
    ) -> crate::seq::Failing<crate::seq::OkThenFilter<Self, F>> {
        crate::seq::Failing(self).then_filter(f)
    }

    /// The first `n` lines — `Seq::take`, lazy.
    pub fn take(self, n: usize) -> crate::seq::Failing<crate::seq::Take<Self>> {
        crate::seq::Failing(self).take(n)
    }

    /// Everything after the first `n` lines — `Seq::skip`, lazy.
    pub fn skip(self, n: usize) -> crate::seq::Failing<crate::seq::Skip<Self>> {
        crate::seq::Failing(self).skip(n)
    }

    /// The first line, and then every `step`-th after it — `Seq::step_by`.
    pub fn step_by(self, step: usize) -> crate::seq::Failing<crate::seq::StepBy<Self>> {
        crate::seq::Failing(self).step_by(step)
    }

    /// Pairs, one line and one item of `other` — `Seq::zip`, lazy.
    pub fn zip<J: IntoIterator>(
        self,
        other: J,
    ) -> crate::seq::Failing<crate::seq::OkZip<Self, J::IntoIter>> {
        crate::seq::Failing(self).zip(other)
    }

    /// One line out of what is already held, where there is one.
    ///
    /// A whole line is one with its newline in hand; at the end of the stream
    /// the remainder is a line too, and `None` there means there is nothing
    /// left at all.
    fn take_a_line(&mut self) -> Option<Result<String, IoError>> {
        let rest = &self.held[self.at..];
        let (line, step) = match rest.iter().position(|b| *b == b'\n') {
            Some(at) => (&rest[..at], at + 1),
            None if self.ended && !rest.is_empty() => (rest, rest.len()),
            None => return None,
        };
        let line = match line.strip_suffix(b"\r") {
            Some(shorter) => shorter,
            None => line,
        };
        let line = String::from_utf8(line.to_vec());
        self.at += step;
        Some(line.map_err(|e| {
            IoError::of(
                std::io::Error::new(std::io::ErrorKind::InvalidData, e),
                STDIN,
            )
        }))
    }
}

#[cfg(test)]
mod lines_tests {
    //! Where a line **ends**, which is the half of
    //! [ADR-172](../../../docs/specification/adr/adr-172.md) D4 that a chunk
    //! boundary can get wrong and a whole-stream read never could.
    //!
    //! Asked of the buffer directly rather than of standard input: there is one
    //! of those per process, a test may not consume it, and what is in question
    //! here is not the read but what is done with what came back. The read
    //! itself is asserted by `examples/tally/src/main.nika`, which runs end to end.

    use super::Lines;

    /// A `Lines` that has been handed these bytes and told whether more are
    /// coming — the state one hop to a worker leaves behind.
    fn holding(bytes: &[u8], ended: bool) -> Lines {
        Lines {
            held: bytes.to_vec(),
            at: 0,
            ended,
        }
    }

    fn drain(lines: &mut Lines) -> Vec<String> {
        let mut out = Vec::new();
        while let Some(line) = lines.take_a_line() {
            out.push(line.expect("the bytes are text"));
        }
        out
    }

    /// The rule `BufRead::lines` has and this keeps, because programs were
    /// written against it: the newline is not part of the line, a `\r` before
    /// it is not either, and a blank line is a line.
    #[test]
    fn a_newline_ends_a_line_and_is_not_part_of_it() {
        let mut lines = holding(b"one\n\nthree\r\n", true);
        assert_eq!(drain(&mut lines), ["one", "", "three"]);
    }

    /// **A final line without a newline is still a line** — but only once the
    /// stream is over. Before that the same bytes are a line that has not
    /// finished arriving, and handing them out would split a line at whatever
    /// byte the chunk happened to end on.
    #[test]
    fn the_last_line_needs_no_newline_and_a_partial_one_waits() {
        assert_eq!(drain(&mut holding(b"one\ntwo", true)), ["one", "two"]);

        let mut waiting = holding(b"one\ntwo", false);
        assert_eq!(drain(&mut waiting), ["one"]);
        // …and the rest is still held, which is what the next chunk is
        // appended to.
        assert_eq!(&waiting.held[waiting.at..], b"two");
    }

    /// A chunk that ends mid-line is the case this exists for: the two halves
    /// meet, and nothing is lost or repeated.
    #[test]
    fn a_line_split_across_two_chunks_is_one_line() {
        let mut lines = holding(b"one\ntw", false);
        assert_eq!(drain(&mut lines), ["one"]);

        // What `next` does with a chunk: drop what was handed out, append.
        lines.held.drain(..lines.at);
        lines.at = 0;
        lines.held.extend_from_slice(b"o\nthree\n");
        assert_eq!(drain(&mut lines), ["two", "three"]);
    }

    /// An empty stream is no lines, and a stream of one newline is one empty
    /// line — the difference a `for` over this can see.
    #[test]
    fn an_empty_stream_is_not_one_empty_line() {
        assert!(drain(&mut holding(b"", true)).is_empty());
        assert_eq!(drain(&mut holding(b"\n", true)), [""]);
    }

    /// Bytes that are not text are a **step that failed**, which is what the
    /// `throws` on the sequence is for — not the end of the stream, which is
    /// the bug class Part I 6.4 refuses by name.
    #[test]
    fn a_line_that_is_not_text_is_a_failed_step() {
        let mut lines = holding(b"fine\n\xff\xfe\n", true);
        assert_eq!(lines.take_a_line().expect("a line").expect("text"), "fine");
        assert!(lines.take_a_line().expect("a step").is_err());
        assert!(lines.take_a_line().is_none());
    }
}

#[cfg(test)]
mod tests {
    use std::io::BufRead;

    /// **Standard input is read on a worker, and the read is awaited**
    /// ([ADR-121](../../../docs/specification/adr/adr-121.md) D4).
    ///
    /// It has to be a *process*. There is one standard input per process, a test
    /// may not consume it, and what is being asserted is that the executor
    /// **parks** for the read and is woken by the worker's reply - which needs a
    /// program whose standard input is a pipe somebody else is writing to.
    ///
    /// **The child bounds itself**, which is D3's *a hang is the failure this
    /// may not have*. `exec::block_on`'s park is unbounded while `main` is
    /// running, so a bell that is not heard would be a child that never exits
    /// and a test that never finishes; the watchdog turns that into a status the
    /// parent reads and a sentence that says what happened.
    #[test]
    fn standard_input_is_read_on_a_worker_and_awaited() {
        use std::io::Write;

        const NAME: &str = "standard_input_is_read_on_a_worker_and_awaited";
        const CHILD: &str = "NIKAIA_STDIN_CHILD";
        /// What the watchdog exits with. Not 70 and not 101, so that a hang is
        /// told apart from an expired cleanup and from a panic.
        const DEAF: i32 = 99;

        if std::env::var_os(CHILD).is_some() {
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(10));
                eprintln!(
                    "the child never got its input: the executor parked for a worker's \
                     reply and was not woken (ADR-121 D1)"
                );
                std::process::exit(DEAF);
            });
            let text = crate::rt::exec::block_on(super::read_to_string()).expect("the input");
            print!("READ[{text}]");
            std::io::stdout().flush().expect("flushed");
            return;
        }

        let mut child = std::process::Command::new(std::env::current_exe().expect("this binary"))
            .args([NAME, "--nocapture"])
            .env(CHILD, "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("the child started");

        // Written after a pause, so the child reaches its park with nothing to
        // read - which is the whole of what this is about. The pipe is then
        // closed, because a whole-of-input read ends at the end of the input.
        let mut input = child.stdin.take().expect("the child's standard input");
        let writing = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            input.write_all(b"eins\nzwei\n").expect("the parent writes");
        });

        let ran = child.wait_with_output().expect("the child ran");
        let _ = writing.join();
        let said = String::from_utf8_lossy(&ran.stderr).into_owned();
        let out = String::from_utf8_lossy(&ran.stdout).into_owned();
        assert_ne!(
            ran.status.code(),
            Some(DEAF),
            "the read never came back.\nstderr:\n{said}"
        );
        assert!(
            out.contains("READ[eins\nzwei\n]"),
            "the whole of the input came back through the worker.\nstdout:\n{out}\nstderr:\n{said}"
        );
    }

    /// The three cases a step has, on a reader that is not standard input -
    /// there is one stdin per process and a test may not consume it.
    ///
    /// This is the shape `Lines` wraps, and pinning it here is what says the
    /// wrapper adds nothing: a line, the end, and a failure that arrives *as a
    /// step* rather than as the end.
    #[test]
    fn a_step_is_a_line_an_end_or_a_failure() {
        let text: &[u8] = b"one\ntwo\n";
        let read: Vec<String> = text.lines().map(|l| l.expect("valid")).collect();
        assert_eq!(read, ["one", "two"], "the newline is not part of a line");

        // A last line without a newline is still a line.
        let text: &[u8] = b"one\ntwo";
        assert_eq!(text.lines().count(), 2);

        // And a byte that is not UTF-8 is a *failing step*, not an end: the
        // count is two, and the second one is the failure.
        let text: &[u8] = b"one\n\xFF\n";
        let steps: Vec<_> = text.lines().collect();
        assert_eq!(steps.len(), 2);
        assert!(steps[0].is_ok());
        assert!(
            steps[1].is_err(),
            "a failed read must not be indistinguishable from the end of the stream"
        );
    }
}

impl crate::seq::Step for Lines {
    type Item = Result<String, IoError>;
    async fn step(&mut self) -> Option<Result<String, IoError>> {
        self.next().await
    }
}
