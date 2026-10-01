// crates/nikaia/src/interpreter/mod.rs
use crate::ast::{Block, Expr, Item, Stmt};
use crate::parser::Parsed;
use winnow_grammar::{InternerContext, Symbol};

pub struct Interpreter {
    /// Identifiers in the AST are interned handles; the interner turns them
    /// back into text.
    interner: InternerContext,
}

impl Interpreter {
    pub fn new(interner: InternerContext) -> Self {
        Self { interner }
    }

    fn text(&self, sym: &Symbol) -> &str {
        self.interner.resolve(*sym)
    }

    pub fn run(&self, parsed: &Parsed) {
        let program = &parsed.program;
        println!("[Nikaia Kernel] Interpreter Init...");
        // Entry point lookup: find 'main' function
        for item in &program.items {
            if let Item::Fn {
                name: Some(name),
                body,
                ..
            } = &item.node
                && self.text(name) == "main"
            {
                println!("[Nikaia Kernel] Executing 'main'...");
                self.eval_block(body);
                return;
            }
        }
        println!("[Nikaia Kernel] No main function found.");
    }

    fn eval_block(&self, block: &Block) {
        for stmt in &block.stmts {
            self.eval_stmt(&stmt.node);
        }
    }

    fn eval_stmt(&self, stmt: &Stmt) {
        match stmt {
            Stmt::Let { names, value, .. } => {
                // In a real implementation, we would store the result in a scope map.
                // For now, we just print the binding.
                let bound: Vec<&str> = names.iter().map(|n| self.text(n)).collect();
                println!("[Nikaia Runtime] Bind: {} = <evaluated>", bound.join(", "));
                self.eval_expr(value);
            }
            Stmt::Comptime { name, value, .. } => {
                println!("[Nikaia Runtime] Bind: {} = <evaluated>", self.text(name));
                self.eval_expr(value);
            }
            Stmt::Expr(expr) => {
                self.eval_expr(expr);
            }
            Stmt::Assign { .. } => {
                println!("[Nikaia Runtime] Assignment (Skipped)");
            }
            Stmt::Return(_) => {
                println!("[Nikaia Runtime] Return (Skipped)");
            }
            Stmt::Break => {
                println!("[Nikaia Runtime] Break (Skipped)");
            }
            Stmt::Continue => {
                println!("[Nikaia Runtime] Continue (Skipped)");
            }
            Stmt::For { body, .. } => {
                // No iteration yet: the loop body is walked once so that calls
                // inside it are still visible.
                println!("[Nikaia Runtime] For loop (single pass)");
                self.eval_block(body);
            }
            Stmt::While { body, .. } => {
                // The same, and for the same reason: nothing here evaluates a
                // condition, so repeating the body would repeat it forever.
                println!("[Nikaia Runtime] While loop (single pass)");
                self.eval_block(body);
            }
        }
    }

    fn eval_expr(&self, expr: &Expr) {
        match expr {
            Expr::Call { func, args, .. } => {
                // Simplified function resolution
                if let Expr::Variable(name) = &**func {
                    let name_str = self.text(name);
                    if name_str == "println" {
                        self.builtin_println(args);
                        return;
                    }
                    if name_str == "log" {
                        self.builtin_log(args);
                        return;
                    }
                }
                println!("[Nikaia Runtime] Call to unknown function");
            }
            Expr::Spawn { body, .. } => {
                println!("[Nikaia Runtime] Spawning Task (Async -> Sync Simulation)...");
                // In Stage 1, this will use Tokio. For now, we execute inline.
                // The body is usually an Expr::Block because of `spawn({ ... })` syntax.
                if let Expr::Block(block) = &**body {
                    self.eval_block(block);
                } else {
                    // Fallback for single expression spawn(expr)
                    self.eval_expr(body);
                }
            }
            Expr::LitStr { .. } | Expr::LitInterpolated { .. } => {
                // Literals evaluate to themselves.
            }
            // A `seq` block runs its statements in the order they were written,
            // which is the only order this interpreter has ever had (ADR-033
            // D7): nothing here overlaps anything, so `seq` asks for what it
            // already does.
            Expr::Block(b) => self.eval_block(b),
            Expr::Dsl { target, .. } => {
                println!(
                    "[Nikaia Runtime] DSL Block '{}' (Skipped)",
                    self.text(target)
                );
            }
            _ => println!("[Nikaia Runtime] Eval: {:?}", expr),
        }
    }

    fn builtin_println(&self, args: &[Expr]) {
        for arg in args {
            if let Expr::LitStr { text: s, .. } = arg {
                println!("{}", s);
            } else if let Expr::LitInterpolated { parts } = arg {
                println!("{}", crate::emit::format_of(parts));
            } else {
                println!("<expression>");
            }
        }
    }

    fn builtin_log(&self, args: &[Expr]) {
        for arg in args {
            if let Expr::LitStr { text: s, .. } = arg {
                println!("[LOG] {}", s);
            } else if let Expr::LitInterpolated { parts } = arg {
                println!("[LOG] {}", crate::emit::format_of(parts));
            }
        }
    }
}
