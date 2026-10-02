// crates/nikaia-logic/src/smtlib.rs
//
// **SMT-LIB 2 at the edge** (ADR-265 D6): a query written so that another
// solver can be asked the same question, and the part of SMT-LIB 2 this
// crate's sorts cover read back - for comparing answers and for running public
// benchmark sets against the solver. It is not the interface: a solver is
// handed a [`Query`], never text.
//
// A query asks whether its facts imply its goal, which SMT-LIB asks the other
// way round: the facts and the goal's negation are asserted, and *unsat* means
// proved.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use crate::{Arena, Query, Term, TermId};

/// The query as an SMT-LIB 2 script: `unsat` where the facts imply the goal.
pub fn write(query: &Query<'_>) -> String {
    let arena = query.arena;
    let mut names = BTreeSet::new();
    for id in query.facts.iter().chain([&query.goal]) {
        variables(arena, *id, &mut names);
    }
    let mut out = String::from("(set-logic QF_LIA)\n");
    for name in &names {
        let _ = writeln!(out, "(declare-const {} Int)", symbol(name));
    }
    for fact in query.facts {
        let _ = writeln!(out, "(assert {})", term(arena, *fact));
    }
    let _ = writeln!(out, "(assert (not {}))", term(arena, query.goal));
    out.push_str("(check-sat)\n");
    out
}

fn variables(arena: &Arena, id: TermId, names: &mut BTreeSet<String>) {
    match arena.get(id) {
        Term::Var(name) => {
            names.insert(name.clone());
        }
        Term::Bool(_) | Term::Int(_) => {}
        Term::Neg(a) | Term::Not(a) => variables(arena, *a, names),
        Term::Add(a, b)
        | Term::Sub(a, b)
        | Term::Mul(a, b)
        | Term::Le(a, b)
        | Term::Lt(a, b)
        | Term::Ge(a, b)
        | Term::Gt(a, b)
        | Term::Eq(a, b)
        | Term::Ne(a, b) => {
            variables(arena, *a, names);
            variables(arena, *b, names);
        }
        Term::And(parts) | Term::Or(parts) => {
            for p in parts {
                variables(arena, *p, names);
            }
        }
    }
}

/// A name as an SMT-LIB symbol: bare where it may be, quoted otherwise -
/// `xs.len()` is `|xs.len()|`.
fn symbol(name: &str) -> String {
    let simple = !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "~!@$%^&*_-+=<>.?/".contains(c));
    if simple {
        name.to_string()
    } else {
        format!("|{name}|")
    }
}

fn term(arena: &Arena, id: TermId) -> String {
    let t = |id: &TermId| term(arena, *id);
    let list = |op: &str, parts: &[TermId], empty: &str| match parts {
        [] => empty.to_string(),
        [one] => t(one),
        _ => format!(
            "({op} {})",
            parts.iter().map(t).collect::<Vec<_>>().join(" ")
        ),
    };
    match arena.get(id) {
        Term::Bool(b) => b.to_string(),
        Term::Int(n) if *n < 0 => format!("(- {})", n.unsigned_abs()),
        Term::Int(n) => n.to_string(),
        Term::Var(name) => symbol(name),
        Term::Add(a, b) => format!("(+ {} {})", t(a), t(b)),
        Term::Sub(a, b) => format!("(- {} {})", t(a), t(b)),
        Term::Neg(a) => format!("(- {})", t(a)),
        Term::Mul(a, b) => format!("(* {} {})", t(a), t(b)),
        Term::Le(a, b) => format!("(<= {} {})", t(a), t(b)),
        Term::Lt(a, b) => format!("(< {} {})", t(a), t(b)),
        Term::Ge(a, b) => format!("(>= {} {})", t(a), t(b)),
        Term::Gt(a, b) => format!("(> {} {})", t(a), t(b)),
        Term::Eq(a, b) => format!("(= {} {})", t(a), t(b)),
        Term::Ne(a, b) => format!("(distinct {} {})", t(a), t(b)),
        Term::And(parts) => list("and", parts, "true"),
        Term::Or(parts) => list("or", parts, "false"),
        Term::Not(a) => format!("(not {})", t(a)),
    }
}

/// A script read back: its assertions, as terms. Asking whether they imply
/// `false` is asking whether the script is `unsat`.
#[derive(Debug)]
pub struct Script {
    pub arena: Arena,
    pub assertions: Vec<TermId>,
}

/// Why a script could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadError {
    /// The text is not balanced s-expressions.
    Syntax(String),
    /// A command, sort or operator outside what this crate's sorts cover.
    Unsupported(String),
    /// A name used without a `declare-const` or `declare-fun` of no arguments.
    Undeclared(String),
}

/// Read a script in the part of SMT-LIB 2 this crate covers: `set-logic`,
/// `set-info`, `set-option`, `declare-const` and nullary `declare-fun` of sort
/// `Int`, `assert`, `check-sat` and `exit`; the core and integer operators.
pub fn read(text: &str) -> Result<Script, ReadError> {
    let mut reader = Reader {
        arena: Arena::new(),
        declared: BTreeSet::new(),
    };
    let mut assertions = Vec::new();
    for command in parse(text)? {
        let Sexp::List(items) = &command else {
            return Err(ReadError::Syntax("a command is a list".to_string()));
        };
        match items.as_slice() {
            [Sexp::Atom(head), ..]
                if matches!(
                    head.as_str(),
                    "set-logic" | "set-info" | "set-option" | "check-sat" | "exit"
                ) => {}
            [Sexp::Atom(head), Sexp::Atom(name), Sexp::Atom(sort)]
                if head == "declare-const" && sort == "Int" =>
            {
                reader.declared.insert(unquoted(name));
            }
            [
                Sexp::Atom(head),
                Sexp::Atom(name),
                Sexp::List(args),
                Sexp::Atom(sort),
            ] if head == "declare-fun" && args.is_empty() && sort == "Int" => {
                reader.declared.insert(unquoted(name));
            }
            [Sexp::Atom(head), body] if head == "assert" => {
                assertions.push(reader.term(body)?);
            }
            _ => return Err(ReadError::Unsupported(show(&command))),
        }
    }
    Ok(Script {
        arena: reader.arena,
        assertions,
    })
}

#[derive(Debug, Clone)]
enum Sexp {
    Atom(String),
    List(Vec<Sexp>),
}

fn show(sexp: &Sexp) -> String {
    match sexp {
        Sexp::Atom(a) => a.clone(),
        Sexp::List(items) => format!("({})", items.iter().map(show).collect::<Vec<_>>().join(" ")),
    }
}

fn unquoted(name: &str) -> String {
    name.strip_prefix('|')
        .and_then(|n| n.strip_suffix('|'))
        .unwrap_or(name)
        .to_string()
}

fn parse(text: &str) -> Result<Vec<Sexp>, ReadError> {
    let mut stack: Vec<Vec<Sexp>> = vec![Vec::new()];
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ';' => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '(' => stack.push(Vec::new()),
            ')' => {
                let list = stack
                    .pop()
                    .filter(|_| !stack.is_empty())
                    .ok_or_else(|| ReadError::Syntax("a `)` with no `(`".to_string()))?;
                stack
                    .last_mut()
                    .expect("the outermost level is never popped")
                    .push(Sexp::List(list));
            }
            c if c.is_whitespace() => {}
            '|' => {
                let mut atom = String::from("|");
                loop {
                    match chars.next() {
                        Some('|') => break,
                        Some(c) => atom.push(c),
                        None => return Err(ReadError::Syntax("an unclosed `|`".to_string())),
                    }
                }
                atom.push('|');
                stack.last_mut().expect("a level").push(Sexp::Atom(atom));
            }
            '"' => {
                let mut atom = String::from("\"");
                loop {
                    match chars.next() {
                        Some('"') if chars.peek() == Some(&'"') => {
                            chars.next();
                            atom.push_str("\"\"");
                        }
                        Some('"') => break,
                        Some(c) => atom.push(c),
                        None => return Err(ReadError::Syntax("an unclosed string".to_string())),
                    }
                }
                atom.push('"');
                stack.last_mut().expect("a level").push(Sexp::Atom(atom));
            }
            c => {
                let mut atom = String::from(c);
                while let Some(&next) = chars.peek() {
                    if next.is_whitespace() || next == '(' || next == ')' {
                        break;
                    }
                    atom.push(next);
                    chars.next();
                }
                stack.last_mut().expect("a level").push(Sexp::Atom(atom));
            }
        }
    }
    match stack.pop() {
        Some(top) if stack.is_empty() => Ok(top),
        _ => Err(ReadError::Syntax("a `(` with no `)`".to_string())),
    }
}

struct Reader {
    arena: Arena,
    declared: BTreeSet<String>,
}

impl Reader {
    fn term(&mut self, sexp: &Sexp) -> Result<TermId, ReadError> {
        match sexp {
            Sexp::Atom(a) if a == "true" => Ok(self.arena.bool(true)),
            Sexp::Atom(a) if a == "false" => Ok(self.arena.bool(false)),
            Sexp::Atom(a) if a.chars().all(|c| c.is_ascii_digit()) => a
                .parse::<i128>()
                .map(|n| self.arena.int(n))
                .map_err(|_| ReadError::Unsupported(format!("the numeral {a}"))),
            Sexp::Atom(a) => {
                let name = unquoted(a);
                if self.declared.contains(&name) {
                    Ok(self.arena.var(&name))
                } else {
                    Err(ReadError::Undeclared(name))
                }
            }
            Sexp::List(items) => {
                let Some((Sexp::Atom(op), args)) = items.split_first() else {
                    return Err(ReadError::Unsupported(show(sexp)));
                };
                let args = args
                    .iter()
                    .map(|a| self.term(a))
                    .collect::<Result<Vec<_>, _>>()?;
                let a = &mut self.arena;
                let pairwise =
                    |a: &mut Arena,
                     args: &[TermId],
                     make: fn(&mut Arena, TermId, TermId) -> TermId| {
                        let links: Vec<TermId> =
                            args.windows(2).map(|w| make(a, w[0], w[1])).collect();
                        match links.as_slice() {
                            [one] => *one,
                            _ => a.and(links),
                        }
                    };
                let folded =
                    |a: &mut Arena,
                     args: &[TermId],
                     make: fn(&mut Arena, TermId, TermId) -> TermId| {
                        args[1..]
                            .iter()
                            .fold(args[0], |acc, next| make(a, acc, *next))
                    };
                Ok(match (op.as_str(), args.as_slice()) {
                    ("not", [x]) => a.not(*x),
                    ("and", parts) => a.and(parts.to_vec()),
                    ("or", parts) => a.or(parts.to_vec()),
                    ("=>", [x, y]) => {
                        let not_x = a.not(*x);
                        a.or(vec![not_x, *y])
                    }
                    ("-", [x]) => a.neg(*x),
                    ("+", [_, _, ..]) => folded(a, &args, Arena::add),
                    ("-", [_, _, ..]) => folded(a, &args, Arena::sub),
                    ("*", [_, _, ..]) => folded(a, &args, Arena::mul),
                    ("<=", [_, _, ..]) => pairwise(a, &args, Arena::le),
                    ("<", [_, _, ..]) => pairwise(a, &args, Arena::lt),
                    (">=", [_, _, ..]) => pairwise(a, &args, Arena::ge),
                    (">", [_, _, ..]) => pairwise(a, &args, Arena::gt),
                    ("=", [_, _, ..]) => pairwise(a, &args, Arena::eq),
                    ("distinct", [x, y]) => a.ne(*x, *y),
                    _ => return Err(ReadError::Unsupported(show(sexp))),
                })
            }
        }
    }
}
