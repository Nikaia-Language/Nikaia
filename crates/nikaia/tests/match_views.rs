//! **A `match` over a place reads its parts and leaves it whole**
//! ([ADR-291](../../../docs/specification/adr/adr-291.md), issue #168
//! issue #168, Part I 6.5). An arm that bound a part of a name
//! took the part by value; the name was gone afterwards, and a second read of
//! it was `rustc`'s *use of moved value*. A part that does not copy and that
//! the arm only reads is now bound as a view; one the arm keeps, returns or
//! hands back as its value is taken as before, and a number is copied as
//! before. What the programs compute, at both settings of `user_parallelism`,
//! is `tests/language/src/match_views.nika`; here stays how a part is bound.

use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn lowered(source: &str, how: Build) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, how).expect("the source lowers").rust
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
/// struct matched with the struct read afterwards - the part a view, the
/// number copied.
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
    let rust = lowered(&source, Build::default());
    assert!(rust.contains("Shape::Named(ref n)"), "{rust}");
    // **A number is copied, as it was**: `r` binds by value, so an arm that
    // compares it with a number compiles as before.
    assert!(rust.contains("Shape::Round { ref label, r }"), "{rust}");
}
