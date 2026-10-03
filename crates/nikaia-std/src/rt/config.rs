//! The runtime configuration file, read at startup
//! ([ADR-038](../../../../docs/specification/adr/adr-038.md) D5).
//!
//! Four settings and no fifth. They are read when the program starts, by the
//! person running it on the machine it runs on - which is why they are *not*
//! `nikaia.toml` keys: a build-time value cannot be tuned by an operator, who
//! is not the person who compiled it. `nikaia.toml` keeps what the compiler
//! must know (`target` and `user_parallelism`,
//! [ADR-037](../../../../docs/specification/adr/adr-037.md) D5) and nothing
//! else.
//!
//! | key | what it decides | default |
//! | :--- | :--- | :--- |
//! | `io-workers` | how many I/O threads run | `1` |
//! | `user-pool` | how large the pool for user code is, at `user_parallelism = yes` | `0` = as many as the machine has |
//! | `io-method` | which mechanism serves a file: `auto`, `uring`, `blocking` | `auto` |
//! | `cleanup-deadline` | how long shutdown drains before cancelling the rest | `30s` |
//!
//! `io-method`'s default is `auto` and it can be pinned - the same shape
//! `--ordering strict` has: the default decides, and a person may overrule it
//! to rule something out in the field. Pinning `uring` on a machine that has
//! no `io_uring` is a *failure to start*, not a silent fallback, because the
//! whole point of pinning is to find out.
//!
//! `cleanup-deadline` arrives here from `nikaia.toml`, where Part III 13.3 had
//! put it. How long a program waits at exit for pending cleanup
//! ([ADR-297](../../../../docs/specification/adr/adr-297.md) D6) is an
//! operating property.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where the runtime looks for its configuration, unless the environment says
/// otherwise.
///
/// **Not decided by ADR-038.** D5 says "a runtime configuration file, read at
/// startup" and names the four settings; it does not name the file or its
/// search path. This is one constant and one function so that naming it is a
/// one-line change when the decision is made.
pub const FILE: &str = "nikaia-runtime.toml";

/// The environment variable that names the file outright, for the operator who
/// has one binary and several deployments of it.
pub const PATH_VAR: &str = "NIKAIA_RUNTIME_CONFIG";

/// Which mechanism serves a file operation (D3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Method {
    /// Feature-detect at startup: completion where the machine has it,
    /// blocking where it does not. **Never a compile-time decision** - a
    /// binary built on a machine with `io_uring` still runs on one without.
    #[default]
    Auto,
    /// Pinned to completion. Refuses to start where there is none.
    Uring,
    /// Pinned to the blocking path, whatever the machine offers.
    Blocking,
}

impl Method {
    fn parse(word: &str) -> Result<Method, String> {
        match word {
            "auto" => Ok(Method::Auto),
            "uring" => Ok(Method::Uring),
            "blocking" => Ok(Method::Blocking),
            other => Err(format!(
                "`io-method` is `{other}`, which names no mechanism \
                 (expected auto, uring or blocking)"
            )),
        }
    }

    /// The word an operator wrote, for a report that has to name it back.
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Auto => "auto",
            Method::Uring => "uring",
            Method::Blocking => "blocking",
        }
    }
}

/// The four settings, resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// How many I/O threads run. At least one: D4 says one always starts.
    pub io_workers: usize,
    /// How large the pool for user code is at `user_parallelism = yes`. `0`
    /// means "as many as the machine has", which is what `rayon` does when
    /// nobody says - and is not the same as a count somebody typed.
    ///
    /// It is read at every setting and ignored at `no`, because a
    /// configuration file that silently drops a key the operator wrote is
    /// worse than one that reads a key it will not use.
    pub user_pool: usize,
    /// Which mechanism serves a file.
    pub io_method: Method,
    /// How long shutdown drains (ADR-297 D6). `0` disables draining.
    pub cleanup_deadline: Duration,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            io_workers: 1,
            user_pool: 0,
            io_method: Method::Auto,
            // ADR-297 D6's "generous default", unchanged by the move.
            cleanup_deadline: Duration::from_secs(30),
        }
    }
}

/// The keys the file may carry. A key outside this set is a typo until proven
/// otherwise, and saying so beats a setting that silently stayed at its
/// default - the same rule `nikaia.toml`'s `[build]` follows, for the same
/// reason.
const KNOWN: &[&str] = &["io-workers", "user-pool", "io-method", "cleanup-deadline"];

impl Config {
    /// Read the file, wherever it is. Absent is not an error: a program with no
    /// configuration file runs on the defaults, and littering one into an
    /// operator's directory to make that work would be the wrong trade.
    pub fn load() -> Result<Config, String> {
        match Self::path() {
            Some(path) => {
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| format!("reading {}: {e}", path.display()))?;
                Config::parse(&text).map_err(|e| format!("in {}: {e}", path.display()))
            }
            None => Ok(Config::default()),
        }
    }

    /// The file this process will read, if there is one.
    ///
    /// The variable first, because it is what one binary in several
    /// deployments needs; then the working directory, which is where an
    /// operator running `./server` puts it.
    pub fn path() -> Option<PathBuf> {
        if let Some(named) = std::env::var_os(PATH_VAR) {
            return Some(PathBuf::from(named));
        }
        let here = Path::new(FILE);
        here.is_file().then(|| here.to_path_buf())
    }

    /// The four settings, out of the text.
    ///
    /// A hand-written reader rather than a TOML crate, and that is a decision:
    /// `nikaia-std` is linked into **every** generated program, and four
    /// `key = value` lines are not worth a parser and its dependency tree in
    /// every binary this compiler produces. The shape it accepts is the shape
    /// the documented file has - comments, blank lines, and `key = value` with
    /// an optionally quoted value.
    pub fn parse(text: &str) -> Result<Config, String> {
        let mut config = Config::default();
        for (number, line) in text.lines().enumerate() {
            let line = match line.split_once('#') {
                Some((before, _)) => before.trim(),
                None => line.trim(),
            };
            if line.is_empty() {
                continue;
            }
            let at = |message: String| format!("line {}: {message}", number + 1);
            let Some((key, value)) = line.split_once('=') else {
                return Err(at(format!(
                    "`{line}` is not `key = value` (the file has four keys: {})",
                    KNOWN.join(", ")
                )));
            };
            let key = key.trim();
            let value = value.trim().trim_matches('"');
            if !KNOWN.contains(&key) {
                return Err(at(format!(
                    "unknown key `{key}` (expected one of: {})",
                    KNOWN.join(", ")
                )));
            }
            match key {
                "io-workers" => {
                    let n = count(key, value).map_err(at)?;
                    if n == 0 {
                        return Err(at(
                            "`io-workers` is 0, and one I/O thread always runs (ADR-038 D4)"
                                .to_string(),
                        ));
                    }
                    config.io_workers = n;
                }
                "user-pool" => config.user_pool = count(key, value).map_err(at)?,
                "io-method" => config.io_method = Method::parse(value).map_err(at)?,
                "cleanup-deadline" => config.cleanup_deadline = deadline(value).map_err(at)?,
                _ => unreachable!("KNOWN and this match are the same four keys"),
            }
        }
        Ok(config)
    }
}

fn count(key: &str, value: &str) -> Result<usize, String> {
    value
        .parse()
        .map_err(|_| format!("`{key}` is `{value}`, which is not a count"))
}

/// `"30s"`, `"500ms"`, `"2m"` or a bare number of seconds. `"0"` disables
/// draining, which is ADR-297 D6's own word for it.
fn deadline(value: &str) -> Result<Duration, String> {
    let unwell = || {
        format!(
            "`cleanup-deadline` is `{value}`, which is not a duration \
             (expected `30s`, `500ms`, `2m` or `0`)"
        )
    };
    let (digits, scale) = if let Some(rest) = value.strip_suffix("ms") {
        (rest, 1)
    } else if let Some(rest) = value.strip_suffix('s') {
        (rest, 1_000)
    } else if let Some(rest) = value.strip_suffix('m') {
        (rest, 60_000)
    } else {
        (value, 1_000)
    };
    let n: u64 = digits.trim().parse().map_err(|_| unwell())?;
    Ok(Duration::from_millis(n * scale))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file nobody wrote is the defaults, and the defaults are the four the
    /// documentation names.
    #[test]
    fn nothing_written_is_the_documented_defaults() {
        let config = Config::parse("").expect("empty parses");
        assert_eq!(config, Config::default());
        assert_eq!(config.io_workers, 1, "D4: one I/O thread always");
        assert_eq!(config.user_pool, 0, "as many as the machine has");
        assert_eq!(config.io_method, Method::Auto);
        assert_eq!(config.cleanup_deadline, Duration::from_secs(30));
    }

    #[test]
    fn all_four_settings_are_read() {
        let config = Config::parse(
            "# the runtime, on this machine\n\
             io-workers = 2\n\
             user-pool = 8\n\
             io-method = \"blocking\"   # rule the ring out\n\
             cleanup-deadline = \"500ms\"\n",
        )
        .expect("parses");
        assert_eq!(config.io_workers, 2);
        assert_eq!(config.user_pool, 8);
        assert_eq!(config.io_method, Method::Blocking);
        assert_eq!(config.cleanup_deadline, Duration::from_millis(500));
    }

    /// The mistake this exists for: a setting that silently stayed at its
    /// default leaves an operator believing they changed something.
    #[test]
    fn an_unknown_key_is_named_rather_than_ignored() {
        let error = Config::parse("io_workers = 2\n").expect_err("refused");
        assert!(error.contains("io_workers"), "{error}");
        assert!(error.contains("io-workers"), "{error}");
        assert!(error.contains("line 1"), "{error}");
    }

    /// There is no fifth setting, and a key that would be one is a typo.
    #[test]
    fn there_is_no_fifth_setting() {
        assert_eq!(KNOWN.len(), 4);
        assert!(Config::parse("ordering = \"strict\"\n").is_err());
        assert!(
            Config::parse("user-parallelism = \"yes\"\n").is_err(),
            "a build switch is not an operating property (ADR-037 D5)"
        );
    }

    #[test]
    fn a_duration_takes_the_units_adr_006_writes() {
        for (written, expected) in [
            ("30s", Duration::from_secs(30)),
            ("0", Duration::ZERO),
            ("250ms", Duration::from_millis(250)),
            ("2m", Duration::from_secs(120)),
            ("45", Duration::from_secs(45)),
        ] {
            let config =
                Config::parse(&format!("cleanup-deadline = \"{written}\"\n")).expect("parses");
            assert_eq!(config.cleanup_deadline, expected, "{written}");
        }
        assert!(Config::parse("cleanup-deadline = \"soon\"\n").is_err());
    }

    /// Zero I/O workers would contradict D4 outright, so it is refused rather
    /// than rounded up to the one that always runs.
    #[test]
    fn zero_io_workers_is_refused_by_name() {
        let error = Config::parse("io-workers = 0\n").expect_err("refused");
        assert!(error.contains("ADR-038 D4"), "{error}");
    }

    #[test]
    fn a_method_that_names_no_mechanism_is_refused() {
        let error = Config::parse("io-method = \"epoll\"\n").expect_err("refused");
        assert!(error.contains("auto"), "{error}");
    }

    #[test]
    fn a_line_that_is_not_a_setting_is_refused_by_line_number() {
        let error = Config::parse("io-workers = 1\nnonsense\n").expect_err("refused");
        assert!(error.contains("line 2"), "{error}");
    }
}
