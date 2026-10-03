// crates/nikaia/src/dsl.rs
//
// The shadow type a DSL with deferred parameters generates (ADR-296 D5).
//
// `dsl mysql { … :id … :active } eod` is a statement with two holes that are
// not filled where it is written. The values arrive at the call site, as named
// arguments after the `;` - DSL parameters are *configuration*, and the Subject
// `;` Config protocol applies to metaprogramming without exception.
//
// For that to be checkable rather than hoped for, the body's holes have to
// become a **type**: one struct per parameter list, a field per `:name`, built
// where the parameters are supplied and never on the heap. Without it the
// compiler cannot say that a call forgot `:active`, which is the whole safety
// argument for embedding SQL rather than concatenating strings.
//
// Two rules govern what is here, and both are restrictions:
//
//   * the lowering is **syntactic** (ADR-296 D17). The names come from the body
//     as written; the *types* come from the arguments at the call site, because
//     nothing in the source says what `:id` is. The struct is therefore generic
//     in each field and monomorphised where it is built - the compiler never
//     invents `i32`.
//   * a hole is a hole because the source wrote `:name`, not because anything
//     was inferred about the foreign syntax around it. The statement's text
//     reaches the driver exactly as written, holes included: a deferred
//     parameter is not string interpolation, so the value may not be spliced
//     into the source text (Part III, 15.3).
//
// **Written in Nikaia** (`nikaia-std/src/tools/dsl.nika`): the scan of a body
// for its holes since 0.0.248, and the rest - the shadow types, the drivers,
// and the check of every call that hands a statement its parameters - since
// 0.0.335 (#125). What stays here is the calls, so that the compiler's callers
// name a `dsl` question in `dsl`.

use std::collections::BTreeMap;

use crate::check::Finding;
use crate::parser::Parsed;

/// The deferred parameters of one `dsl … { … } eod` body, in the order the body
/// first names them, and empty where it names none.
pub fn parameters(body: &str) -> Vec<String> {
    nikaia_std::tools::dsl::parameters(body)
}

/// Whether a `dsl <target> { … } eod` body's holes are deferred parameters:
/// for every target but `html`, whose `:name` is an immediate capture
/// (ADR-017, ADR-296 D4).
pub fn is_deferred(target: &str, body: &str) -> bool {
    nikaia_std::tools::dsl::is_deferred(target, body)
}

/// The Rust name of the shadow type for a parameter list, the same whatever
/// order a call writes the names in.
pub fn type_name(parameters: &[String]) -> String {
    nikaia_std::tools::dsl::type_name(parameters)
}

/// Every distinct shadow type a program needs, with the field order of the body
/// that first asked for it.
pub fn shadow_types(parsed: &Parsed) -> BTreeMap<String, Vec<String>> {
    nikaia_std::tools::dsl::shadow_types(&parsed.interner, &parsed.program.items)
}

/// The functions of this program that accept a DSL's parameters, by name.
pub fn drivers(parsed: &Parsed) -> Vec<String> {
    nikaia_std::tools::dsl::drivers(&parsed.interner, &parsed.program.items)
}

/// Every call site that supplies a DSL's parameters and gets them wrong.
pub fn check(parsed: &Parsed) -> Vec<Finding> {
    nikaia_std::tools::dsl::check(&parsed.interner, &parsed.program.items)
        .into_iter()
        .map(crate::traits::from_nikaia)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hole_is_a_colon_and_a_name() {
        assert_eq!(
            parameters("SELECT name FROM users WHERE id = :id AND active = :active"),
            ["id", "active"]
        );
    }

    #[test]
    fn a_path_and_a_time_are_not_holes() {
        assert!(parameters("std::io::write(x)").is_empty());
        assert!(parameters("at 12:30 sharp").is_empty());
    }

    #[test]
    fn a_repeated_hole_is_one_parameter() {
        assert_eq!(parameters("WHERE a = :id OR b = :id"), ["id"]);
    }

    #[test]
    fn the_type_name_does_not_depend_on_the_order_a_call_writes() {
        assert_eq!(
            type_name(&["id".to_string(), "active".to_string()]),
            type_name(&["active".to_string(), "id".to_string()])
        );
    }
}
