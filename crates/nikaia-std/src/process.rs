// crates/nikaia-std/src/process.rs
//
// `std::process`: start another program and wait for what it says
// ([ADR-243](../../../docs/specification/adr/adr-243.md), on
// [ADR-195](../../../docs/specification/adr/adr-195.md) D4's decision that
// `std` gains a subprocess).
//
// **One call, run to the end.** `run` starts a program, hands it its
// arguments, waits for it to finish and hands back its exit code and its two
// output streams as text. That is the shape every caller this tree has wants -
// `cargo metadata` for `nikaia describe`, `rustc` for a compiler written in
// Nikaia - and it is the one that needs no handle a program has to close.
//
// **A non-zero exit is an answer, not a failure.** What fails is not being
// able to start the program, or output that is not text; how the program
// ended is `code` and `ok`, and the caller decides what a `1` means. A tool's
// refusal is information the tool printed, and a `catch` would lose it.
//
// **The wait gives the thread up** (D3). The child is waited for on a thread
// of its own and the calling task parks on the runtime's bell, so a program at
// `user_parallelism = no` keeps serving its other tasks while a build runs.

use std::process::Command;

use crate::io::IoError;

/// What a program that ran to its end left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// The exit code, or `-1` where the program was ended by a signal and has
    /// none (D2).
    pub code: i64,
    /// Whether the program says it succeeded: an exit code of `0`.
    pub ok: bool,
    /// Everything it wrote to standard output.
    pub stdout: String,
    /// Everything it wrote to standard error.
    pub stderr: String,
}

/// Start `program` with `args` in the directory `dir`, wait for it to finish,
/// and hand back what it said.
///
/// `program` is looked for on `PATH` where it has no `/` in it, as a shell
/// would. It fails with `IoError::NotFound(program)` where nothing is there,
/// `IoError::PermissionDenied(program)` where it may not be run, and
/// `IoError::NotText` where either stream is not UTF-8.
pub async fn run(program: &str, args: Vec<String>, dir: &str) -> Result<Output, IoError> {
    let program = program.to_string();
    let dir = dir.to_string();
    let named = program.clone();
    let finished = crate::rt::io::beside(move || {
        Command::new(&program)
            .args(&args)
            .current_dir(&dir)
            .stdin(std::process::Stdio::null())
            .output()
    })
    .await
    .map_err(|e| IoError::of(e, &named))?;
    let stdout = String::from_utf8(finished.stdout)
        .map_err(|_| IoError::NotText(format!("{named}'s standard output")))?;
    let stderr = String::from_utf8(finished.stderr)
        .map_err(|_| IoError::NotText(format!("{named}'s standard error")))?;
    Ok(Output {
        code: finished.status.code().map(i64::from).unwrap_or(-1),
        ok: finished.status.success(),
        stdout,
        stderr,
    })
}
