// crates/nikaia/src/interpreter/mod.rs
//
// **The Stage 0 interpreter is Nikaia** (ADR-294, #125):
// `nikaia-std/src/tools/interpreter.nika` walks `main` and hands back what it
// would say. What stays here is the printing, and the two texts a Nikaia module
// cannot write - a node's `Debug` and an `f"…"`'s format string.
use crate::parser::Parsed;
use winnow_grammar::InternerContext;

pub struct Interpreter {
    /// Identifiers in the AST are interned handles; the interner turns them
    /// back into text.
    interner: InternerContext,
}

impl Interpreter {
    pub fn new(interner: InternerContext) -> Self {
        Self { interner }
    }

    pub fn run(&self, parsed: &Parsed) {
        let lines = nikaia_std::tools::interpreter::run(
            &parsed.program.items,
            &self.interner,
            &|expr| format!("{expr:?}"),
            &|parts| crate::emit::format_of(parts),
        );
        for line in lines {
            println!("{line}");
        }
    }
}
