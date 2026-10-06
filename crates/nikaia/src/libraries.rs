//! **The system libraries a package links** (Part III 13.4,
//! [ADR-324](../../../docs/specification/adr/adr-324.md)).
//!
//! An `extern(library: "x")` block names a library, and the package's manifest
//! says how it is found: `[library.x] pkg-config = "…"`. Both are required and
//! must agree (`NK1226`). The library is found through the machine's
//! `pkg-config` and nothing else (`NK1224`), and only `-l`, `-L`, `-pthread`,
//! and on macOS `-framework` and `-F`, reach the linker from what it says
//! (`NK1225`). There is no build script: nothing of a package runs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;

use crate::ast::Item;

/// The libraries the `extern` blocks of these sources name, each with the
/// first file that names it.
pub fn declared(sources: &[PathBuf]) -> Result<BTreeMap<String, PathBuf>> {
    let mut out = BTreeMap::new();
    for source in sources {
        let Ok(text) = std::fs::read_to_string(source) else {
            continue;
        };
        // **A file that does not parse names nothing here**: the lowering of
        // the same file is what says why, in its own words.
        let Ok(parsed) = crate::parser::parse_to_ast(&text) else {
            continue;
        };
        for item in &parsed.program.items {
            if let Item::Extern {
                library: Some(name),
                ..
            } = &item.node
            {
                out.entry(name.clone()).or_insert_with(|| source.clone());
            }
        }
    }
    Ok(out)
}

/// **`NK1226`, both ways** (ADR-324 D2): every library a block names has a
/// `[library.<name>]` entry, and every entry is named by a block.
pub fn named_both_ways(
    package: &str,
    manifest_path: &Path,
    entries: &BTreeMap<String, String>,
    declared: &BTreeMap<String, PathBuf>,
) -> Result<()> {
    for (name, file) in declared {
        if !entries.contains_key(name) {
            crate::refuse!(
                "error[NK1226]: `{}` names the library `{name}`, and {} has no \
                 `[library.{name}]` for it.\n  = help: Add\n\n    [library.{name}]\n    \
                 pkg-config = \"{name}\"\n\n  to package `{package}`'s manifest, with the name \
                 `pkg-config` knows the library by.",
                file.display(),
                manifest_path.display()
            );
        }
    }
    for name in entries.keys() {
        if !declared.contains_key(name) {
            crate::refuse!(
                "error[NK1226]: {} has `[library.{name}]`, and no `extern(library: \"{name}\")` \
                 block of package `{package}` names it.\n  = help: Remove the entry, or name \
                 the library on the `extern` block whose functions come from it.",
                manifest_path.display()
            );
        }
    }
    Ok(())
}

/// **What the linker is handed for these libraries** (ADR-324 D3, D4):
/// `pkg-config --libs` for each, filtered. `NK1224` where `pkg-config` is not
/// there or does not find a library, `NK1225` for a flag outside the list.
pub fn link_flags(package: &str, entries: &BTreeMap<String, String>) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for (name, found_by) in entries {
        let said = Command::new("pkg-config")
            .args(["--libs", found_by.as_str()])
            .output();
        let said = match said {
            Ok(said) if said.status.success() => said,
            Ok(said) => crate::refuse!(
                "error[NK1224]: package `{package}` needs the system library `{name}`, and \
                 `pkg-config` does not find `{found_by}`.\n  = note: {}\n  = help: Install the \
                 library's development files, or set `PKG_CONFIG_PATH` to the directory of its \
                 `{found_by}.pc`.",
                String::from_utf8_lossy(&said.stderr).trim()
            ),
            Err(_) => crate::refuse!(
                "error[NK1224]: package `{package}` needs the system library `{name}`, and this \
                 machine has no `pkg-config` to find `{found_by}` with.\n  = help: Install \
                 `pkg-config`: a library is found only through it (Part III 13.4)."
            ),
        };
        for flag in filtered(package, found_by, &String::from_utf8_lossy(&said.stdout))? {
            if !out.contains(&flag) {
                out.push(flag);
            }
        }
    }
    Ok(out)
}

/// **D4's list**, over what `pkg-config` printed: `-l…`, `-L…`, `-pthread`, and
/// on macOS `-framework <name>` and `-F…`. Anything else is `NK1225`, because a
/// linker flag can load a plugin or run a script, and the library's `.pc` file
/// is not the program's to trust.
pub fn filtered(package: &str, found_by: &str, said: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut words = said.split_whitespace();
    while let Some(word) = words.next() {
        let allowed = (word.starts_with("-l") && word.len() > 2)
            || (word.starts_with("-L") && word.len() > 2)
            || word == "-pthread"
            || (cfg!(target_os = "macos") && word.starts_with("-F") && word.len() > 2);
        if allowed {
            out.push(word.to_string());
            continue;
        }
        if cfg!(target_os = "macos")
            && word == "-framework"
            && let Some(name) = words.next()
        {
            out.push(format!("-framework {name}"));
            continue;
        }
        crate::refuse!(
            "error[NK1225]: `pkg-config --libs {found_by}` for package `{package}` says \
             `{word}`, which is not a flag the build hands the linker.\n  = note: Only `-l`, \
             `-L`, `-pthread`, and on macOS `-framework` and `-F`, are taken (Part III 13.4).\n  \
             = help: Check `{found_by}.pc`: a library's linker flags may not run anything."
        );
    }
    Ok(out)
}

/// **The flags as `rustc` takes them**: `-L dir` becomes `-L native=dir`,
/// and `-l name`, `-pthread` and a framework go to the linker as they are.
///
/// **`-l` as a linker argument**, because `rustc` writes those after every
/// crate (#499): a library a dependency package names is called from that
/// package's crate, and a linker that reads a static archive once, in order -
/// GNU `ld`, the default on `aarch64` - found nothing calling it yet where
/// `rustc` puts its own `-l`, before the crates.
pub fn as_rustc_args(flags: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for flag in flags {
        if flag.starts_with("-l") {
            out.push(format!("-Clink-arg={flag}"));
        } else if let Some(dir) = flag.strip_prefix("-L") {
            out.push("-L".to_string());
            out.push(format!("native={dir}"));
        } else if let Some(dir) = flag.strip_prefix("-F") {
            out.push("-L".to_string());
            out.push(format!("framework={dir}"));
        } else if let Some(name) = flag.strip_prefix("-framework ") {
            out.push("-l".to_string());
            out.push(format!("framework={name}"));
        } else {
            out.push(format!("-Clink-arg={flag}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_listed_flags_pass_and_nothing_else() {
        let flags = filtered("p", "x", "-L/opt/x/lib -lx -pthread\n").expect("all listed");
        assert_eq!(flags, ["-L/opt/x/lib", "-lx", "-pthread"]);
        let refused = format!(
            "{:#}",
            filtered("p", "x", "-lx -Wl,-plugin,/x.so").expect_err("a plugin is refused")
        );
        assert!(refused.contains("NK1225"), "{refused}");
        assert!(refused.contains("-Wl,-plugin,/x.so"), "{refused}");
    }

    #[test]
    fn rustc_is_told_the_flags_in_its_own_words() {
        let args = as_rustc_args(&["-L/opt/x/lib".into(), "-lx".into(), "-pthread".into()]);
        assert_eq!(
            args,
            [
                "-L",
                "native=/opt/x/lib",
                "-Clink-arg=-lx",
                "-Clink-arg=-pthread"
            ]
        );
    }
}
