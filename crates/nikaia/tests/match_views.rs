//! **A `match` over a place reads its parts and leaves it whole**
//! ([ADR-242](../../../docs/specification/adr/adr-242.md), issue #168
//! issue #168, Part I 6.5). An arm that bound a part of a name
//! took the part by value; the name was gone afterwards, and a second read of
//! it was `rustc`'s *use of moved value*. A part that does not copy and that
//! the arm only reads is now bound as a view; one the arm keeps, returns or
//! hands back as its value is taken as before, and a number is copied as
//! before. Each program prints the same at both settings of `user_parallelism`.

mod common;

use std::process::Command;

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str, how: Build) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, how).expect("the source lowers").rust
}

fn ran(purpose: &str, source: &str, how: Build) -> String {
    let rust = lowered(source, how);
    let dir = common::scratch_dir(&format!("match-views-{purpose}"));
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
    let out = Command::new(&binary).output().expect("run it");
    assert!(
        out.status.success(),
        "{purpose} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::remove_dir_all(&dir).ok();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

const SHAPE: &str = "enum Shape {\n\
    \x20   Named(String),\n\
    \x20   Round { label: String, r: i64 },\n\
    \x20   Empty,\n\
    }\n\
    \n\
    struct Holder {\n\
    \x20   shape: Shape,\n\
    \x20   id: i64,\n\
    }\n";

/// **The report's program**: the same name matched twice, and a field of a
/// struct matched with the struct read afterwards.
#[test]
fn a_name_matched_twice_is_still_there() {
    let source = format!(
        "{SHAPE}fn main() {{\n\
         \x20   let s = Shape::Named(f\"box\")\n\
         \x20   match s {{\n\
         \x20       Shape::Named(n) => println(f\"named {{n}}\")\n\
         \x20       Shape::Round {{ label, r }} => println(f\"{{label}} {{r}}\")\n\
         \x20       Shape::Empty => println(\"empty\")\n\
         \x20   }}\n\
         \x20   match s {{\n\
         \x20       Shape::Named(n) => println(f\"again {{n}}\")\n\
         \x20       else => println(\"other\")\n\
         \x20   }}\n\
         \x20   let h = Holder {{ shape: Shape::Round {{ label: f\"ring\", r: 3 }}, id: 7 }}\n\
         \x20   match h.shape {{\n\
         \x20       Shape::Round {{ label, r }} => println(f\"{{label}} {{r}}\")\n\
         \x20       else => println(\"other\")\n\
         \x20   }}\n\
         \x20   println(f\"{{h.id}}\")\n\
         }}\n"
    );
    for how in [Build::default(), Build::parallel()] {
        assert_eq!(
            ran("twice", &source, how),
            "named box\nagain box\nring 3\n7",
            "at {how:?}"
        );
    }
    let rust = lowered(&source, Build::default());
    assert!(rust.contains("Shape::Named(ref n)"), "{rust}");
    // **A number is copied, as it was**: `r` binds by value, so an arm that
    // compares it with a number compiles as before.
    assert!(rust.contains("Shape::Round { ref label, r }"), "{rust}");
}

/// **What an arm keeps is taken, as before**: pushed into a list, handed back
/// as the arm's value, or returned.
#[test]
fn a_part_the_arm_keeps_is_taken() {
    let source = format!(
        "{SHAPE}fn label(s: Shape) -> String {{\n\
         \x20   match s {{\n\
         \x20       Shape::Named(n) => return n\n\
         \x20       else => return f\"none\"\n\
         \x20   }}\n\
         }}\n\
         \n\
         fn main() {{\n\
         \x20   let mut kept: Vec[String] = []\n\
         \x20   let s = Shape::Named(f\"a\")\n\
         \x20   match s {{\n\
         \x20       Shape::Named(n) => kept.push(n)\n\
         \x20       else => println(\"other\")\n\
         \x20   }}\n\
         \x20   let t = Shape::Named(f\"b\")\n\
         \x20   let got = match t {{\n\
         \x20       Shape::Named(n) => n\n\
         \x20       else => f\"\"\n\
         \x20   }}\n\
         \x20   let big = Shape::Round {{ label: f\"c\", r: 5 }}\n\
         \x20   match big {{\n\
         \x20       Shape::Round {{ label, r }} => {{\n\
         \x20           if r > 4 {{\n\
         \x20               println(f\"big {{label}}\")\n\
         \x20           }}\n\
         \x20       }}\n\
         \x20       else => println(\"other\")\n\
         \x20   }}\n\
         \x20   let d = label(Shape::Named(f\"d\"))\n\
         \x20   println(f\"{{kept.len()}} {{got}} {{d}}\")\n\
         }}\n"
    );
    for how in [Build::default(), Build::parallel()] {
        assert_eq!(ran("kept", &source, how), "big c\n1 b d", "at {how:?}");
    }
}

/// **The same in a handler**: `match error` takes the error apart by variant,
/// and `{error}` afterwards is still the error.
#[test]
fn a_handler_matches_the_error_and_still_has_it() {
    let source = "enum ConfigError {\n\
        \x20   NotFound(String),\n\
        }\n\
        \n\
        impl Error for ConfigError {\n\
        \x20   fn message(ref self) -> String {\n\
        \x20       match self {\n\
        \x20           ConfigError::NotFound(p) => f\"no config at {p}\"\n\
        \x20       }\n\
        \x20   }\n\
        }\n\
        \n\
        fn load() -> i64 throws {\n\
        \x20   throw ConfigError::NotFound(f\"etc\")\n\
        }\n\
        \n\
        fn main() {\n\
        \x20   let port = load() catch {\n\
        \x20       match error {\n\
        \x20           ConfigError::NotFound(p) => println(f\"missing {p}\")\n\
        \x20       }\n\
        \x20       println(f\"{error}\")\n\
        \x20       8080\n\
        \x20   }\n\
        \x20   println(f\"{port}\")\n\
        }\n";
    for how in [Build::default(), Build::parallel()] {
        assert_eq!(
            ran("handler", source, how),
            "missing etc\nno config at etc\n8080",
            "at {how:?}"
        );
    }
}
