//! **A number's methods are `std`'s** (#438, Part III C.1): `i64::min`,
//! `i64::max` and `i64::div_euclid` are in its ledger, and so is the `abs` of
//! both signed types that Part I 2.2 names, and a method of a number
//! the ledger does not describe is refused here (`NK1210`) rather than taken as
//! *nothing is known* - which made the function around it `async` and handed
//! the call to `rustc` as written.
//!
//! What the methods compute is `tests/language/src/number_methods.nika`.

use nikaia::contracts::{Ledger, LedgerOps, STD};
use nikaia::emit::{Build, emit_program};
use nikaia::parser::parse_to_ast;

fn codes(source: &str) -> Vec<String> {
    let parsed = parse_to_ast(source).expect("the source parses");
    let own = Ledger::infer(&parsed);
    let library = Ledger::parse(STD).expect("std ships a ledger");
    nikaia::check::check_program(&parsed, &own, &library, &std::collections::BTreeSet::new())
        .findings
        .into_iter()
        .map(|f| f.code.to_string())
        .collect()
}

fn lowered(source: &str) -> String {
    let parsed = parse_to_ast(source).expect("the source parses");
    emit_program(&parsed, Build::default())
        .expect("the source lowers")
        .rust
}

const DESCRIBED: &str = "fn lo(a: i64, b: i64) -> i64 {\n\
     \x20   return a.min(b)\n\
     }\n\
     \n\
     fn hi(a: i64, b: i64) -> i64 {\n\
     \x20   return a.max(b)\n\
     }\n\
     \n\
     fn floor_half(a: i64) -> i64 {\n\
     \x20   return a.div_euclid(2)\n\
     }\n\
     \n\
     fn size(a: i64) -> i64 {\n\
     \x20   return a.abs()\n\
     }\n\
     \n\
     fn main() {\n\
     \x20   println(f\"{lo(7, 3)} {hi(7, 3)} {floor_half(-7)} {size(-5)}\")\n\
     }\n";

/// The issue's three methods are described, so the functions that call them
/// are accepted and stay plain functions.
#[test]
fn min_max_and_div_euclid_are_described_and_do_not_pause() {
    assert!(codes(DESCRIBED).is_empty(), "{:?}", codes(DESCRIBED));
    let rust = lowered(DESCRIBED);
    for name in ["lo", "hi", "floor_half", "size"] {
        assert!(
            rust.contains(&format!("\nfn {name}(")),
            "`{name}` calls only described methods, so it is not `async`:\n{rust}"
        );
    }
}

/// A method no ledger describes on a number is refused in this compiler's words.
#[test]
fn a_method_of_a_number_nothing_describes_is_refused() {
    let source = "fn f(a: i64) -> i64 {\n    return a.frobnicate(2)\n}\n\nfn main() {}\n";
    assert_eq!(codes(source), vec!["NK1210".to_string()]);
    let float = "fn g(a: f64) -> f64 {\n    return a.frobnicate()\n}\n\nfn main() {}\n";
    assert_eq!(codes(float), vec!["NK1210".to_string()]);
}

/// **The methods a program reaches for first are described** (#443): since
/// `NK1210`, a method missing from the ledger is a program refused, so text,
/// powers, the smaller of two and a float's floor have entries - and the
/// function that calls them stays a plain one. What they compute is
/// `tests/language/src/number_methods.nika`.
#[test]
fn the_common_methods_of_a_number_are_described() {
    let source = "fn calc(a: i64, c: i32, d: i32, u: u64, f: f64, g: f64) -> String {\n\
         \x20   let parts = [a.to_string(), a.pow(2).to_string(), (-a).rem_euclid(3).to_string(), (-a).signum().to_string(), c.min(d).to_string(), u.count_ones().to_string(), f.floor().to_string(), f.round().to_string(), f.max(g).to_string(), f.powi(2).to_string(), f.is_nan().to_string()]\n\
         \x20   return parts.join(\" \")\n\
         }\n\
         \n\
         fn main() {\n\
         \x20   println(calc(7, 4, 2, 5, 1.5, 2.5))\n\
         }\n";
    assert!(codes(source).is_empty(), "{:?}", codes(source));
    let rust = lowered(source);
    assert!(
        rust.contains("\nfn calc("),
        "`calc` does not pause:\n{rust}"
    );
}
