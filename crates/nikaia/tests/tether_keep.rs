//! **A view that outlives its buffer is tethered**, and the compiler decides
//! where the buffer lives ([ADR-209](../../../docs/specification/adr/adr-209.md)).
//!
//! Eight programs, one per shape the design was checked against before it was
//! built, each compiled and **run** at both settings of `user_parallelism`
//! against real files - the plan, the lowering and the language below have to
//! agree, and only running the program says they do:
//!
//! | program | shape | where the buffer lives |
//! | :--- | :--- | :--- |
//! | `loader` | a function hands back the settings it read | the caller's frame (D2) |
//! | `chain` | a view travels two frames up, and the reader pauses after reading | forwarded twice (D2) |
//! | `many` | a loop over files keeps every list | one keep for the loop (D2) |
//! | `merge` | recursive includes, one map, kept through a `mut` parameter | one keep, many buffers (D2) |
//! | `task` | the settings go into a task | a handle the task carries (D3) |
//! | `cache` | a loop keeps a map that drops entries | a handle per view (D4) |
//! | `no_word` | the same loader with nothing written above it | the caller's frame - no word is asked for (D5) |
//! | `mapped` | one line of a mapped file is handed back | the caller's frame, the mapping included |
//!
//! Not one of them writes anything about lifetimes, buffers or keeps.

mod common;

use std::process::Command;

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

const APP_CONF: &str = "# settings\nhost = example.org\nport = 8080\ninclude = extra.conf\n";
const EXTRA_CONF: &str = "mode = fast\nhost = override.org\n";

fn lowered(source: &str, how: Build) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, how).expect("the source lowers").rust
}

/// Compile the lowering and run it in a directory holding the two files.
fn ran(purpose: &str, source: &str, how: Build) -> String {
    let rust = lowered(source, how);
    let dir = common::scratch_dir(&format!("tether-{purpose}"));
    std::fs::write(dir.join("app.conf"), APP_CONF).expect("write app.conf");
    std::fs::write(dir.join("extra.conf"), EXTRA_CONF).expect("write extra.conf");
    let path = dir.join("program.rs");
    std::fs::write(&path, &rust).expect("write the Rust");
    let binary = dir.join("program");
    let compiled = common::compile(
        &path,
        &[
            "--crate-type",
            "bin",
            "-o",
            binary.to_str().expect("utf-8 path"),
        ],
    );
    assert!(
        compiled.status.success(),
        "{purpose} did not compile:\n{}\n--- emitted ---\n{rust}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let out = Command::new(&binary)
        .current_dir(&dir)
        .output()
        .expect("run it");
    assert!(
        out.status.success(),
        "{purpose} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = String::from_utf8_lossy(&out.stdout).trim().to_string();
    std::fs::remove_dir_all(&dir).ok();
    printed
}

fn findings(source: &str) -> Vec<nikaia::check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = nikaia::contracts::Ledger::infer(&parsed);
    let library =
        nikaia::contracts::Ledger::parse(nikaia::contracts::STD).expect("std ships a ledger");
    nikaia::check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new())
        .findings
        .into_iter()
        .filter(|f| f.severity == nikaia::check::Severity::Error)
        .collect()
}

/// Both settings, the same output, and no refusal on the way.
fn runs(purpose: &str, source: &str, expected: &str) {
    assert!(
        findings(source).is_empty(),
        "{purpose}: {:#?}",
        findings(source)
    );
    for how in [Build::default(), Build::parallel()] {
        assert_eq!(ran(purpose, source, how), expected, "{purpose} at {how:?}");
    }
}

const LOADER: &str = r##"use std::fs

struct Setting {
    key: ref String,
    value: ref String,
}

fn load(path: String) -> Vec[Setting] throws {
    let text = fs::read_to_string(path, fs::Root::Anywhere)
    let mut settings: Vec[Setting] = []
    for line in text.lines() {
        if line.starts_with("#") { continue }
        let parts: Vec[ref String] = line.split("=").collect()
        if parts.len() == 2 {
            settings.push(Setting { key: parts[0].trim(), value: parts[1].trim() })
        }
    }
    return settings
}

fn main() throws {
    for s in load("app.conf") {
        println(f"{s.key} -> {s.value}")
    }
}
"##;

const CHAIN: &str = r##"use std::fs

struct Setting {
    key: ref String,
    value: ref String,
}

fn load(path: String) -> Vec[Setting] throws {
    let text = fs::read_to_string(path, fs::Root::Anywhere)
    let stamp = fs::read_to_string("extra.conf", fs::Root::Anywhere)
    let mut settings: Vec[Setting] = []
    for line in text.lines() {
        let parts: Vec[ref String] = line.split("=").collect()
        if parts.len() == 2 {
            settings.push(Setting { key: parts[0].trim(), value: parts[1].trim() })
        }
    }
    println(f"stamp has {stamp.len()} bytes")
    return settings
}

fn host_of(path: String) -> ref String throws {
    let settings = load(path)
    for s in settings {
        if s.key == "host" { return s.value }
    }
    return "localhost"
}

fn main() throws {
    println(host_of("app.conf"))
}
"##;

const MANY: &str = r##"use std::fs

struct Setting {
    key: ref String,
    value: ref String,
}

fn load(path: String) -> Vec[Setting] throws {
    let text = fs::read_to_string(path, fs::Root::Anywhere)
    let mut settings: Vec[Setting] = []
    for line in text.lines() {
        let parts: Vec[ref String] = line.split("=").collect()
        if parts.len() == 2 {
            settings.push(Setting { key: parts[0].trim(), value: parts[1].trim() })
        }
    }
    return settings
}

fn main() throws {
    let mut all: Vec[Vec[Setting]] = []
    for path in ["app.conf", "extra.conf"] {
        all.push(load(path))
    }
    println(f"{all.len()} files, {all[0].len() + all[1].len()} settings")
}
"##;

const MERGE: &str = r##"use std::fs
use std::collections

struct Setting {
    key: ref String,
    value: ref String,
}

fn load(path: String, mut into: collections::HashMap[ref String, ref String]) throws {
    let text = fs::read_to_string(path, fs::Root::Anywhere)
    for line in text.lines() {
        let parts: Vec[ref String] = line.split("=").collect()
        if parts.len() == 2 {
            let key = parts[0].trim()
            let value = parts[1].trim()
            if key == "include" {
                load(value.clone(), into)
            } else {
                into.insert(key, value)
            }
        }
    }
}

fn main() throws {
    let mut merged: collections::HashMap[ref String, ref String] = collections::HashMap()
    load("app.conf", merged)
    println(f"host = {merged[\"host\"] ?? \"-\"}, mode = {merged[\"mode\"] ?? \"-\"}")
}
"##;

const TASK: &str = r##"use std::fs

struct Setting {
    key: ref String,
    value: ref String,
}

fn load(path: String) -> Vec[Setting] throws {
    let text = fs::read_to_string(path, fs::Root::Anywhere)
    let mut settings: Vec[Setting] = []
    for line in text.lines() {
        let parts: Vec[ref String] = line.split("=").collect()
        if parts.len() == 2 {
            settings.push(Setting { key: parts[0].trim(), value: parts[1].trim() })
        }
    }
    return settings
}

fn main() throws {
    let settings = load("app.conf")
    let worker = spawn fn {
        let mut n = 0
        for s in settings { n = n + s.value.len() }
        n
    }
    println(f"{worker.join()} bytes of values")
}
"##;

const CACHE: &str = r##"use std::fs
use std::collections

fn main() throws {
    let mut last_seen = collections::HashMap()
    let mut round = 0
    while round < 1000 {
        let text = fs::read_to_string("app.conf", fs::Root::Anywhere)
        for line in text.lines() {
            last_seen.insert(line, round)
        }
        if last_seen.len() > 2 { last_seen.clear() }
        round = round + 1
    }
    println(f"{last_seen.len()} lines remembered")
}
"##;

const NO_WORD: &str = r##"use std::fs

struct Setting {
    key: ref String,
    value: ref String,
}

fn load(path: String) -> Vec[Setting] throws {
    let text = fs::read_to_string(path, fs::Root::Anywhere)
    let mut settings: Vec[Setting] = []
    for line in text.lines() {
        let parts: Vec[ref String] = line.split("=").collect()
        if parts.len() == 2 {
            settings.push(Setting { key: parts[0].trim(), value: parts[1].trim() })
        }
    }
    return settings
}

fn main() throws {
    println(f"{load(\"app.conf\").len()}")
}
"##;

const MAPPED: &str = r##"use std::fs

fn first_line(path: String) -> ref String throws {
    let file = fs::map(path, fs::Root::Anywhere)
    for line in file.lines() {
        return line
    }
    return ""
}

fn main() throws {
    println(first_line("app.conf"))
}
"##;

/// The settings outlive the text they point into, and the text lives in a keep beside the call that asked for them.
#[test]
fn loader() {
    runs(
        "loader",
        LOADER,
        "host -> example.org\nport -> 8080\ninclude -> extra.conf",
    );
}

/// Two frames up, and a pause after the buffer exists: the keep is forwarded, and nothing is a closure that could not pause.
#[test]
fn chain() {
    runs("chain", CHAIN, "stamp has 32 bytes\nexample.org");
}

/// A list per file, all kept by a list outside the loop: one keep holds every buffer the loop read.
#[test]
fn many() {
    runs("many", MANY, "2 files, 5 settings");
}

/// Views of every file of a recursive include in one map, reached through a `mut` parameter.
#[test]
fn merge() {
    runs("merge", MERGE, "host = override.org, mode = fast");
}

/// A task takes the settings: no frame outlives it, so the keep travels with the value.
#[test]
fn task() {
    runs("task", TASK, "25 bytes of values");
}

/// A map that drops entries while the loop keeps reading holds each view with a handle of its own - one keep for the loop would keep every buffer it read.
#[test]
fn cache() {
    runs("cache", CACHE, "0 lines remembered");
}

/// The loader again with nothing written above it. It used to be refused; where a buffer lives is not a thing a program is asked to say.
#[test]
fn no_word() {
    runs("no_word", NO_WORD, "3");
}

/// One line of a mapped file handed back keeps the mapping - and its cleanup - alive in the caller.
#[test]
fn mapped() {
    runs("mapped", MAPPED, "# settings");
}

/// **The shape of what is written**, for the three representations: plain
/// borrows of the caller's keep, a packed value for the task, a held view.
#[test]
fn each_representation_is_the_one_the_plan_chose() {
    let loader = lowered(LOADER, Build::default());
    assert!(loader.contains("fn load<'k>("), "{loader}");
    assert!(
        loader.contains("__keep: &'k nikaia_std::tether::Keep"),
        "{loader}"
    );
    assert!(loader.contains("Vec<Setting<'k>>"), "{loader}");
    assert!(loader.contains("__keep.put("), "{loader}");
    assert!(
        !loader.contains("Arc"),
        "a frame keep costs no count: {loader}"
    );

    let task = lowered(TASK, Build::parallel());
    assert!(
        task.contains("let __keep_task = std::sync::Arc::new("),
        "{task}"
    );
    assert!(
        task.contains("_keep: std::sync::Arc::clone(&__keep_task)"),
        "{task}"
    );
    assert!(task.contains("settings.get()"), "{task}");

    let cache = lowered(CACHE, Build::default());
    assert!(
        cache.contains(").hold([std::sync::Arc::clone(&__keep_"),
        "{cache}"
    );
    assert!(cache.contains(".put_viewed("), "{cache}");
    // **No `unsafe` for a held view** (ADR-221 D1): the keep finds the view
    // again by address, so what the plan claims is checked, not trusted.
    assert!(!cache.contains("unsafe"), "{cache}");
}

/// **Nothing is kept that needs no keep**: a buffer whose views stay in its
/// scope is the plain local it always was, and a buffer handed on whole is a
/// move.
#[test]
fn a_buffer_nothing_outlives_is_left_alone() {
    let source = "use std::fs\n\
                  fn count(path: String) -> i64 throws {\n\
                  \x20   let text = fs::read_to_string(path, fs::Root::Anywhere)\n\
                  \x20   let mut n = 0\n\
                  \x20   for line in text.lines() { n = n + line.len() as i64 }\n\
                  \x20   return n\n\
                  }\n\
                  fn whole(path: String) -> String throws {\n\
                  \x20   let text = fs::read_to_string(path, fs::Root::Anywhere)\n\
                  \x20   return text\n\
                  }\n\
                  fn main() throws { println(f\"{count(\\\"app.conf\\\")} {whole(\\\"app.conf\\\").len()}\") }\n";
    let rust = lowered(source, Build::default());
    assert!(!rust.contains("nikaia_std::tether"), "{rust}");
    assert_eq!(ran("left-alone", source, Build::default()), "59 63");
}

// ---------------------------------------------------------------------------
// A container of structs holding views that drops entries in a loop
// ([ADR-221](../../../docs/specification/adr/adr-221.md))
// ---------------------------------------------------------------------------

/// The program `NK2304` refused until ADR-221: records of the lines of files
/// read in a loop, kept in a list that is pruned as it goes, and read back by
/// index, by a `for`, and through a method.
const RECORDS: &str = r##"use std::fs

struct Record {
    name: ref String,
    size: i64,
}

impl Record {
    fn describe(ref self) -> String {
        return f"{self.name}:{self.size}"
    }
}

fn main() throws {
    let mut kept: Vec[Record] = Vec()
    let mut round = 0
    while round < 200 {
        for path in ["app.conf", "extra.conf"] {
            let text = fs::read_to_string(path, fs::Root::Anywhere)
            for line in text.lines() {
                let name = line.trim()
                kept.push(Record { name: name, size: line.len() as i64 })
            }
            while kept.len() > 3 { kept.remove(0) }
        }
        round = round + 1
    }
    for r in kept { println(r.describe()) }
    println(f"{kept.len()} {kept[0].name} {kept[2].describe()}")
    println(kept.last()?.name ?? "-")
}
"##;

#[test]
fn a_list_of_structs_of_views_that_drops_entries_is_held() {
    runs(
        "records",
        RECORDS,
        "include = extra.conf:20\nmode = fast:11\nhost = override.org:19\n\
         3 include = extra.conf host = override.org:19\nhost = override.org",
    );
    let rust = lowered(RECORDS, Build::default());
    assert!(
        rust.contains("Vec<nikaia_std::tether::Holding<__Views_Record, 1>>"),
        "{rust}"
    );
    assert!(rust.contains("enum __Views_Record {}"), "{rust}");
    assert!(rust.contains(".map(|held| held.get())"), "{rust}");
    assert!(!rust.contains("unsafe"), "{rust}");
}

/// **A value whose views point into two buffers carries a handle on each**
/// (ADR-221 D2), and neither buffer is copied.
const PAIRS: &str = r##"use std::fs

struct Pair {
    key: ref String,
    value: ref String,
}

fn main() throws {
    let mut pairs: Vec[Pair] = Vec()
    let mut round = 0
    while round < 100 {
        let a = fs::read_to_string("app.conf", fs::Root::Anywhere)
        let b = fs::read_to_string("extra.conf", fs::Root::Anywhere)
        pairs.push(Pair { key: a.trim(), value: b.trim() })
        if pairs.len() > 2 { pairs.remove(0) }
        round = round + 1
    }
    println(f"{pairs.len()} {pairs[0].key.len()} {pairs[1].value.len()}")
}
"##;

#[test]
fn a_struct_of_views_into_two_buffers_holds_both() {
    runs("pairs", PAIRS, "2 62 31");
    let rust = lowered(PAIRS, Build::default());
    assert!(rust.contains("Holding<__Views_Pair, 2>"), "{rust}");
}

/// **A view of something the element does not hold is carried as a copy**
/// (ADR-221 D2): a literal in a view field points into no buffer, and the
/// keep takes a copy of it rather than a reference it cannot vouch for.
const LITERAL: &str = r##"use std::fs

struct Note {
    key: ref String,
    tag: ref String,
}

fn main() throws {
    let mut notes: Vec[Note] = Vec()
    for path in ["app.conf", "extra.conf", "app.conf"] {
        let text = fs::read_to_string(path, fs::Root::Anywhere)
        for line in text.lines() {
            let tag = if line.starts_with("#") { "comment" } else { "setting" }
            notes.push(Note { key: line, tag: tag })
        }
        if notes.len() > 4 { notes.clear() }
    }
    println(f"{notes.len()} {notes[0].tag} {notes[1].tag}")
}
"##;

#[test]
fn a_view_of_a_literal_in_a_held_struct_is_carried_as_a_copy() {
    runs("literal", LITERAL, "4 comment setting");
}

/// **Dropping entries after the loop is not dropping them while it goes on**:
/// the frame's keep holds the buffers exactly as long, no element carries a
/// handle, and the program is not refused (the case ADR-221 D5 fixes).
const CLEARED_AFTER: &str = r##"use std::fs

struct Record {
    name: ref String,
    size: i64,
}

fn main() throws {
    let mut kept: Vec[Record] = Vec()
    for path in ["app.conf", "extra.conf"] {
        let text = fs::read_to_string(path, fs::Root::Anywhere)
        kept.push(Record { name: text.trim(), size: text.len() as i64 })
    }
    println(f"{kept.len()} {kept[1].size}")
    kept.clear()
    println(f"{kept.len()}")
}
"##;

#[test]
fn entries_dropped_only_after_the_loop_need_no_handle() {
    runs("cleared-after", CLEARED_AFTER, "2 32\n0");
    let rust = lowered(CLEARED_AFTER, Build::default());
    assert!(rust.contains("let __keep_frame"), "{rust}");
    assert!(!rust.contains("Holding"), "{rust}");
}

/// **A map of structs of views is still refused, by name** (ADR-221 §4): a
/// map's reads hand back a value that may be absent, and that is not yet
/// read through the handle.
#[test]
fn a_map_of_structs_of_views_that_drops_entries_is_refused_by_name() {
    let source = r##"use std::fs
use std::collections

struct Record {
    name: ref String,
}

fn main() throws {
    let mut by_path: collections::HashMap[String, Record] = collections::HashMap()
    for path in ["app.conf", "extra.conf"] {
        let text = fs::read_to_string(path, fs::Root::Anywhere)
        by_path.insert(path.clone(), Record { name: text.trim() })
        if by_path.len() > 1 { by_path.clear() }
    }
    println(f"{by_path.len()}")
}
"##;
    let found = findings(source);
    assert!(
        found
            .iter()
            .any(|f| f.code == "NK2304" && f.message.contains("isn't a list")),
        "{found:#?}"
    );
}

/// **An element taken out stays held, and a field is written through the
/// handle** (ADR-221 D3, D4): `old` carries its buffer's keep out of the list,
/// a number is stored as it is, and a view made outside the element is found
/// again in its keeps - or, here, is a literal and is copied into them.
const TAKEN_AND_WRITTEN: &str = r##"use std::fs

struct Record {
    name: ref String,
    size: i64,
}

fn main() throws {
    let mut kept: Vec[Record] = Vec()
    let mut gone = 0
    for path in ["app.conf", "extra.conf", "app.conf"] {
        let text = fs::read_to_string(path, fs::Root::Anywhere)
        kept.push(Record { name: text.trim(), size: text.len() as i64 })
        if kept.len() > 2 {
            let old = kept.remove(0)
            gone = gone + old.size
            println(old.name.len())
        }
    }
    kept[0].size = 7
    kept[1].size += 1
    kept[1].name = "renamed"
    println(f"{gone} {kept[0].size} {kept[1].size} {kept[1].name}")
}
"##;

#[test]
fn an_element_taken_out_stays_held_and_a_field_is_written_through_the_handle() {
    runs(
        "taken-and-written",
        TAKEN_AND_WRITTEN,
        "62\n63 7 64 renamed",
    );
    let rust = lowered(TAKEN_AND_WRITTEN, Build::default());
    assert!(rust.contains("old.get()"), "{rust}");
    // A number is stored as it is; a view is found again in the keeps.
    assert!(rust.contains(".with_mut(|held, _| held.size = "), "{rust}");
    assert!(
        rust.contains(".with_mut(|held, keeps| held.name = "),
        "{rust}"
    );
    assert!(!rust.contains("unsafe"), "{rust}");
}
