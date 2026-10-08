//! **A package C calls** ([ADR-284](../../../docs/specification/adr/adr-284.md)
//! D4, D5, D7, D8, D12): `artifact = "c-library"` makes a shared and a static
//! library of the package and its header, and a C program links it.
//!
//! What is built is D5's first row - numbers, `bool` and `scalar`, of a
//! function that neither pauses nor throws - with the status, the
//! out-parameter, and a panic caught at the boundary that poisons the library.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const LIBRARY: &str = "\
pub extern fn add(a: i64, b: i64) -> i64 sync {
    return a + b
}

pub extern fn halve(x: f64) -> f64 sync {
    return x / 2.0
}

pub extern fn is_vowel(c: scalar) -> bool sync {
    return c == 'a' || c == 'e' || c == 'i' || c == 'o' || c == 'u'
}

pub extern fn pick(n: i64) -> i64 sync {
    let xs = [10, 20, 30]
    return xs[n]
}

pub extern fn greet(name: ref String) -> String sync {
    return f\"Hello, {name}\"
}

pub extern fn total(xs: ref Array[i64]) -> i64 sync {
    let mut sum = 0
    for x in xs {
        sum = sum + x
    }
    return sum
}

pub extern fn size(b: Bytes) -> i64 sync {
    return b.len()
}

pub enum Light {
    Red,
    Amber,
    Green,
}

pub extern fn after(light: Light) -> Light sync {
    match light {
        Light::Red => Light::Green
        Light::Amber => Light::Red
        Light::Green => Light::Amber
    }
}

pub enum Refusal {
    Negative,
    TooLarge(i64),
}

impl Error for Refusal {
    fn message(ref self) -> String {
        match self {
            Refusal::Negative => \"a negative count\".clone()
            Refusal::TooLarge(n) => f\"{n} is too large\"
        }
    }
}

pub extern fn checked(n: i64) -> i64 sync throws {
    if n < 0 {
        throw Refusal::Negative
    }
    if n > 100 {
        throw Refusal::TooLarge(n)
    }
    return n * 2
}
";

const CALLER: &str = r#"#include <stdio.h>
#include "calc.h"

int main(void) {
    int64_t sum = 0;
    double half = 0;
    bool vowel = false;
    int64_t picked = 0;
    int status = calc_add(2, 40, &sum);
    printf("add %d %lld\n", status, (long long)sum);
    status = calc_halve(5.0, &half);
    printf("halve %d %.1f\n", status, half);
    status = calc_is_vowel('e', &vowel);
    printf("vowel %d %d\n", status, vowel);
    printf("not a scalar %d\n", calc_is_vowel(0xD800, &vowel));
    status = calc_pick(1, &picked);
    printf("pick %d %lld\n", status, (long long)picked);
    printf("past the end %d\n", calc_pick(7, &picked));
    printf("after the panic %d\n", calc_add(1, 1, &sum));
    return CALC_OK;
}
"#;

/// A function that throws (ADR-284 D7): each variant is its own positive
/// status, and `calc_last_error` renders the failure with its site.
const THROWING_CALLER: &str = r#"#include <stdio.h>
#include "calc.h"

int main(void) {
    int64_t n = 0;
    char said[128];
    size_t written = 0;
    int status = calc_checked(21, &n);
    printf("checked %d %lld\n", status, (long long)n);
    status = calc_checked(-1, &n);
    calc_last_error((uint8_t *)said, sizeof said, &written);
    printf("negative %d %.16s\n", status == CALC_E_NEGATIVE, said);
    status = calc_checked(500, &n);
    calc_last_error((uint8_t *)said, sizeof said, &written);
    printf("too large %d %.16s\n", status == CALC_E_TOOLARGE, said);
    return CALC_OK;
}
"#;

/// Text, bytes and a run of numbers (ADR-284 D5, D6): in as an address and a
/// length, out into the caller's buffer.
const TEXT_CALLER: &str = r#"#include <stdio.h>
#include "calc.h"

int main(void) {
    char out[64];
    size_t written = 0;
    int status = calc_greet((const uint8_t *)"Ada", 3, NULL, 0, &written);
    printf("size query %d %zu\n", status, written);
    status = calc_greet((const uint8_t *)"Ada", 3, (uint8_t *)out, 4, &written);
    printf("too small %d %zu\n", status, written);
    status = calc_greet((const uint8_t *)"Ada", 3, (uint8_t *)out, sizeof out, &written);
    printf("greet %d %.*s\n", status, (int)written, out);
    printf("not utf-8 %d\n", calc_greet((const uint8_t *)"\xff", 1, (uint8_t *)out, sizeof out, &written));
    printf("no address %d\n", calc_greet(NULL, 3, (uint8_t *)out, sizeof out, &written));
    int64_t xs[] = {1, 2, 3, 4};
    int64_t sum = 0;
    status = calc_total(xs, 4, &sum);
    printf("total %d %lld\n", status, (long long)sum);
    int64_t n = 0;
    status = calc_size((const uint8_t *)"\x01\x02\x03", 3, &n);
    printf("size %d %lld\n", status, (long long)n);
    calc_Light light = CALC_LIGHT_RED;
    status = calc_after(CALC_LIGHT_GREEN, &light);
    printf("after %d %d\n", status, light == CALC_LIGHT_AMBER);
    printf("no such light %d\n", calc_after((calc_Light)7, &light));
    return CALC_OK;
}
"#;

fn package(purpose: &str, manifest_build: &str, source: &str) -> PathBuf {
    let root = common::scratch_dir(purpose);
    std::fs::create_dir_all(root.join("src")).expect("the package");
    std::fs::write(
        root.join("nikaia.toml"),
        format!("[package]\nname = \"calc\"\nversion = \"0.1.0\"\n\n[build]\n{manifest_build}"),
    )
    .expect("a manifest");
    std::fs::write(root.join("src/main.nika"), source).expect("a source");
    root
}

fn nikaia(root: &Path, subcommand: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .current_dir(root)
        .args([subcommand])
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("the nikaia binary runs")
}

fn said(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// **The library, its header, and a C program that calls both ways in**:
/// values come back through the out-parameter with `OK`, a number that is no
/// scalar is `E_ARGUMENT`, and a panic is `E_PANICKED` and poisons the library
/// for every call after it (D8).
#[test]
fn a_c_program_calls_the_library() {
    if Command::new("cc").arg("--version").output().is_err() {
        eprintln!("skipped: no C compiler");
        return;
    }
    let root = package("c-library", "artifact = \"c-library\"\n", LIBRARY);
    let built = nikaia(&root, "build");
    assert!(built.status.success(), "{}", said(&built));
    let made = root.join("target/nikaia/c-library");
    let header = std::fs::read_to_string(made.join("calc.h")).expect("the header");
    assert!(
        header.contains("int calc_add(int64_t a, int64_t b, int64_t *out);"),
        "{header}"
    );
    assert!(header.contains("#define CALC_E_PANICKED (-4)"), "{header}");
    assert!(header.contains("    CALC_LIGHT_AMBER = 1,"), "{header}");
    assert!(header.contains("ledger: "), "{header}");
    assert!(made.join("libcalc.a").is_file(), "the static library");

    std::fs::write(root.join("use.c"), CALLER).expect("the caller");
    let program = root.join("use");
    let compiled = Command::new("cc")
        .arg("-I")
        .arg(&made)
        .arg(root.join("use.c"))
        .arg("-L")
        .arg(&made)
        .args(["-lcalc", "-o"])
        .arg(&program)
        .output()
        .expect("cc runs");
    assert!(compiled.status.success(), "{}", said(&compiled));
    let ran = Command::new(&program)
        .env("LD_LIBRARY_PATH", &made)
        .output()
        .expect("the caller runs");
    assert!(ran.status.success(), "{}", said(&ran));
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout),
        "add 0 42\nhalve 0 2.5\nvowel 0 1\nnot a scalar -1\npick 0 20\npast the end -4\nafter the panic -4\n"
    );

    // Text, bytes and a run of numbers, from a second program: the first one
    // poisoned its copy of the library.
    std::fs::write(root.join("text.c"), TEXT_CALLER).expect("the caller");
    let program = root.join("text");
    let compiled = Command::new("cc")
        .arg("-I")
        .arg(&made)
        .arg(root.join("text.c"))
        .arg("-L")
        .arg(&made)
        .args(["-lcalc", "-o"])
        .arg(&program)
        .output()
        .expect("cc runs");
    assert!(compiled.status.success(), "{}", said(&compiled));
    let ran = Command::new(&program)
        .env("LD_LIBRARY_PATH", &made)
        .output()
        .expect("the caller runs");
    assert!(ran.status.success(), "{}", said(&ran));
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout),
        "size query 0 10\ntoo small -2 10\ngreet 0 Hello, Ada\nnot utf-8 -1\nno address -1\ntotal 0 10\nsize 0 3\nafter 0 1\nno such light -1\n"
    );

    std::fs::write(root.join("throwing.c"), THROWING_CALLER).expect("the caller");
    let program = root.join("throwing");
    let compiled = Command::new("cc")
        .arg("-I")
        .arg(&made)
        .arg(root.join("throwing.c"))
        .arg("-L")
        .arg(&made)
        .args(["-lcalc", "-o"])
        .arg(&program)
        .output()
        .expect("cc runs");
    assert!(compiled.status.success(), "{}", said(&compiled));
    let ran = Command::new(&program)
        .env("LD_LIBRARY_PATH", &made)
        .output()
        .expect("the caller runs");
    assert!(ran.status.success(), "{}", said(&ran));
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout),
        "checked 0 42\nnegative 1 a negative count\ntoo large 1 500 is too large\n"
    );
}

/// A library has nothing to run, and a library that exports nothing is
/// refused rather than built empty.
#[test]
fn a_library_is_not_run_and_exports_something() {
    let root = package("c-library-run", "artifact = \"c-library\"\n", LIBRARY);
    let ran = nikaia(&root, "run");
    assert!(!ran.status.success());
    assert!(said(&ran).contains("nothing to run"), "{}", said(&ran));

    let root = package(
        "c-library-empty",
        "artifact = \"c-library\"\n",
        "pub fn add(a: i64, b: i64) -> i64 sync {\n    return a + b\n}\n",
    );
    let built = nikaia(&root, "build");
    assert!(!built.status.success());
    assert!(said(&built).contains("exports nothing"), "{}", said(&built));
}

/// `symbol-prefix` must be a C identifier, naming the character that is not.
#[test]
fn a_prefix_that_is_no_c_identifier_is_refused() {
    let root = package(
        "c-library-prefix",
        "artifact = \"c-library\"\nsymbol-prefix = \"my-lib\"\n",
        LIBRARY,
    );
    let built = nikaia(&root, "build");
    assert!(!built.status.success());
    assert!(
        said(&built).contains("`-` cannot stand there"),
        "{}",
        said(&built)
    );
}
