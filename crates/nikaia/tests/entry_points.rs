//! **A function C calls** ([ADR-284](../../../docs/specification/adr/adr-284.md)
//! D4, D5, ADR-324 D6): `pub extern fn` with a body is an entry point, and
//! four shapes are refused at the declaration. The checker knows nothing of
//! the build, so an entry point is `NK1238` here; a library build drops it
//! (`project::check`, `tests/c_library.rs`).

use nikaia::check;
use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::parser::parse_to_ast;

fn found(source: &str) -> Vec<check::Finding> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std's shipped ledger parses");
    check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new())
        .findings
        .into_iter()
        .filter(|f| f.code.starts_with("NK123") || f.code == "NK1240" || f.code == "NK1145")
        .collect()
}

fn codes(source: &str) -> Vec<&'static str> {
    found(source).into_iter().map(|f| f.code).collect()
}

/// `extern` before a body parses, and an entry point C may call is refused
/// only because no library is built.
#[test]
fn an_entry_point_parses_and_waits_for_the_library() {
    let codes = codes(
        "pub struct Counter { n: i64 }\n\
         pub extern fn fine(name: ref String, xs: ref Array[i64], c: Counter, f: fn(i64) -> bool sync) -> String {\n    return name.clone()\n}\n\
         fn main() { }\n",
    );
    assert_eq!(codes, ["NK1238"]);
}

/// **`NK1237`**: an entry point is the package's surface, so it is `pub`.
#[test]
fn an_entry_point_that_is_not_pub_is_refused() {
    assert!(
        codes("extern fn hidden(a: i64) -> i64 {\n    return a\n}\nfn main() { }\n")
            .contains(&"NK1237")
    );
}

/// **`NK1239`**: C has neither generics nor traits.
#[test]
fn a_generic_entry_point_and_a_trait_method_are_refused() {
    assert!(
        codes("pub extern fn first[T](x: T) -> i64 {\n    return 0\n}\nfn main() { }\n")
            .contains(&"NK1239")
    );
    assert!(codes(
        "trait Named {\n    fn name(ref self) -> i64\n}\n\
         pub struct P { n: i64 }\n\
         impl Named for P {\n    pub extern fn name(ref self) -> i64 {\n        return 1\n    }\n}\n\
         fn main() { }\n"
    )
    .contains(&"NK1239"));
}

/// **`NK1240`**: what does not cross is refused, and the help names the
/// shape that does.
#[test]
fn a_signature_that_does_not_cross_is_refused_with_its_shape() {
    let refused: Vec<check::Finding> = found(
        "use std::collections\n\
         pub extern fn bad(s: String, m: collections::HashMap[String, i64], f: fn(i64) -> bool) -> ref String {\n    return \"x\"\n}\n\
         fn main() { }\n",
    )
    .into_iter()
    .filter(|f| f.code == "NK1240")
    .collect();
    let helps: Vec<String> = refused.iter().filter_map(|f| f.help.clone()).collect();
    assert_eq!(refused.len(), 4, "{refused:#?}");
    assert!(helps.iter().any(|h| h.contains("ref String")), "{helps:?}");
    assert!(helps.iter().any(|h| h.contains("map")), "{helps:?}");
    assert!(helps.iter().any(|h| h.contains("sync")), "{helps:?}");
    assert!(
        helps.iter().any(|h| h.contains("Hand back `String`")),
        "{helps:?}"
    );
}

/// **`NK1145`**: an `extern` struct has C's layout, so each field is a C
/// value: a number, `bool`, `scalar`, an enum without payload, another
/// `extern` struct, or a fixed array of these. Its message names the handle.
#[test]
fn an_extern_struct_holds_c_values_only() {
    assert!(
        codes(
            "pub enum Light { Red, Green }\n\
             pub extern struct Point { x: f64, y: f64 }\n\
             pub extern struct Rect { corner: Point, light: Light, sides: Array[f64, 4], on: bool }\n\
             fn main() { }\n"
        )
        .is_empty()
    );
    let refused = found(
        "pub struct Plain { n: i64 }\n\
         pub extern struct Bad { name: String, n: i64?, inner: Plain }\n\
         fn main() { }\n",
    );
    let nk1145: Vec<_> = refused.iter().filter(|f| f.code == "NK1145").collect();
    assert_eq!(nk1145.len(), 3, "{refused:?}");
    assert!(nk1145[0].message.contains("`Bad.name`"), "{:?}", nk1145[0]);
    assert!(
        nk1145[0]
            .help
            .as_deref()
            .is_some_and(|help| help.contains("handle")),
        "{:?}",
        nk1145[0]
    );
    assert!(codes("extern struct Hidden { x: f64 }\nfn main() { }\n").contains(&"NK1237"));
}

/// **`NK1145` for a `pub extern enum`** (ADR-284 D32): a tagged union in C's
/// layout, so each field of each variant is a C value - another `extern` enum
/// among them - and it is `pub` (`NK1237`).
#[test]
fn an_extern_enum_holds_c_values_only() {
    assert!(
        codes(
            "pub enum Light { Red, Green }\n\
             pub extern struct Point { x: f64, y: f64 }\n\
             pub extern enum Shape { Circle(f64), Rect { corner: Point, light: Light }, Empty }\n\
             pub extern struct Framed { shape: Shape, sides: Array[Shape, 2] }\n\
             pub extern enum Nested { One(Shape) }\n\
             fn main() { }\n"
        )
        .is_empty()
    );
    let refused = found(
        "pub struct Plain { n: i64 }\n\
         pub extern enum Bad { Named(String), Held { inner: Plain }, Fine(i64) }\n\
         fn main() { }\n",
    );
    let nk1145: Vec<_> = refused.iter().filter(|f| f.code == "NK1145").collect();
    assert_eq!(nk1145.len(), 2, "{refused:?}");
    assert!(
        nk1145[0].message.contains("`Bad::Named.0`"),
        "{:?}",
        nk1145[0]
    );
    assert!(
        nk1145[1].message.contains("`Bad::Held.inner`"),
        "{:?}",
        nk1145[1]
    );
    assert!(
        nk1145[0]
            .help
            .as_deref()
            .is_some_and(|help| help.contains("`_kind`")),
        "{:?}",
        nk1145[0]
    );
    assert!(codes("extern enum Hidden { A(f64) }\nfn main() { }\n").contains(&"NK1237"));
}
