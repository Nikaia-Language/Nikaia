// crates/nikaia-logic/src/term.rs
//
// Terms by id in an arena (ADR-265 D2). A term never changes once built, so an
// arena handed to a solver can be read from any number of threads without a
// lock. Whether two equal terms share one id (hash-consing) is a
// representation choice ADR-265 D2 leaves to a measurement; nothing here
// depends on it, and today they do not.

/// A term, named by its place in an [`Arena`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TermId(u32);

/// One term. Sorts are `Bool` and `Int` (unbounded integers); a variable is an
/// `Int`. Each further theory adds its sorts and terms by a record of its own
/// (ADR-265 D2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Term {
    Bool(bool),
    Int(i128),
    /// An integer variable, by name.
    Var(String),
    Add(TermId, TermId),
    Sub(TermId, TermId),
    Neg(TermId),
    Mul(TermId, TermId),
    Le(TermId, TermId),
    Lt(TermId, TermId),
    Ge(TermId, TermId),
    Gt(TermId, TermId),
    Eq(TermId, TermId),
    Ne(TermId, TermId),
    And(Vec<TermId>),
    Or(Vec<TermId>),
    Not(TermId),
}

/// The terms of one or more queries. Appended to, never changed.
#[derive(Debug, Default, Clone)]
pub struct Arena {
    terms: Vec<Term>,
}

impl Arena {
    pub fn new() -> Arena {
        Arena::default()
    }

    pub fn get(&self, id: TermId) -> &Term {
        &self.terms[id.0 as usize]
    }

    pub fn len(&self) -> usize {
        self.terms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    fn push(&mut self, term: Term) -> TermId {
        let id = u32::try_from(self.terms.len()).expect("an arena holds fewer than 2^32 terms");
        self.terms.push(term);
        TermId(id)
    }

    pub fn bool(&mut self, value: bool) -> TermId {
        self.push(Term::Bool(value))
    }

    pub fn int(&mut self, value: i128) -> TermId {
        self.push(Term::Int(value))
    }

    pub fn var(&mut self, name: &str) -> TermId {
        self.push(Term::Var(name.to_string()))
    }

    pub fn add(&mut self, a: TermId, b: TermId) -> TermId {
        self.push(Term::Add(a, b))
    }

    pub fn sub(&mut self, a: TermId, b: TermId) -> TermId {
        self.push(Term::Sub(a, b))
    }

    pub fn neg(&mut self, a: TermId) -> TermId {
        self.push(Term::Neg(a))
    }

    pub fn mul(&mut self, a: TermId, b: TermId) -> TermId {
        self.push(Term::Mul(a, b))
    }

    pub fn le(&mut self, a: TermId, b: TermId) -> TermId {
        self.push(Term::Le(a, b))
    }

    pub fn lt(&mut self, a: TermId, b: TermId) -> TermId {
        self.push(Term::Lt(a, b))
    }

    pub fn ge(&mut self, a: TermId, b: TermId) -> TermId {
        self.push(Term::Ge(a, b))
    }

    pub fn gt(&mut self, a: TermId, b: TermId) -> TermId {
        self.push(Term::Gt(a, b))
    }

    pub fn eq(&mut self, a: TermId, b: TermId) -> TermId {
        self.push(Term::Eq(a, b))
    }

    pub fn ne(&mut self, a: TermId, b: TermId) -> TermId {
        self.push(Term::Ne(a, b))
    }

    pub fn and(&mut self, parts: Vec<TermId>) -> TermId {
        self.push(Term::And(parts))
    }

    pub fn or(&mut self, parts: Vec<TermId>) -> TermId {
        self.push(Term::Or(parts))
    }

    pub fn not(&mut self, a: TermId) -> TermId {
        self.push(Term::Not(a))
    }

    /// Whether `id` reads the variable `name`.
    pub fn mentions(&self, id: TermId, name: &str) -> bool {
        match self.get(id) {
            Term::Bool(_) | Term::Int(_) => false,
            Term::Var(n) => n == name,
            Term::Neg(a) | Term::Not(a) => self.mentions(*a, name),
            Term::Add(a, b)
            | Term::Sub(a, b)
            | Term::Mul(a, b)
            | Term::Le(a, b)
            | Term::Lt(a, b)
            | Term::Ge(a, b)
            | Term::Gt(a, b)
            | Term::Eq(a, b)
            | Term::Ne(a, b) => self.mentions(*a, name) || self.mentions(*b, name),
            Term::And(parts) | Term::Or(parts) => parts.iter().any(|p| self.mentions(*p, name)),
        }
    }

    /// The value of an integer term that reads no variable, where it fits.
    pub fn constant(&self, id: TermId) -> Option<i128> {
        match self.get(id) {
            Term::Int(n) => Some(*n),
            Term::Neg(a) => self.constant(*a)?.checked_neg(),
            Term::Add(a, b) => self.constant(*a)?.checked_add(self.constant(*b)?),
            Term::Sub(a, b) => self.constant(*a)?.checked_sub(self.constant(*b)?),
            Term::Mul(a, b) => self.constant(*a)?.checked_mul(self.constant(*b)?),
            _ => None,
        }
    }

    /// Every variable `id` reads, added to `names`.
    pub fn variables(&self, id: TermId, names: &mut std::collections::BTreeSet<String>) {
        match self.get(id) {
            Term::Var(name) => {
                names.insert(name.clone());
            }
            Term::Bool(_) | Term::Int(_) => {}
            Term::Neg(a) | Term::Not(a) => self.variables(*a, names),
            Term::Add(a, b)
            | Term::Sub(a, b)
            | Term::Mul(a, b)
            | Term::Le(a, b)
            | Term::Lt(a, b)
            | Term::Ge(a, b)
            | Term::Gt(a, b)
            | Term::Eq(a, b)
            | Term::Ne(a, b) => {
                self.variables(*a, names);
                self.variables(*b, names);
            }
            Term::And(parts) | Term::Or(parts) => {
                for p in parts {
                    self.variables(*p, names);
                }
            }
        }
    }

    /// An integer term's value under `values`, where it has one: every
    /// variable it reads is given, and nothing leaves `i128`.
    pub fn int_value(
        &self,
        id: TermId,
        values: &std::collections::BTreeMap<String, i128>,
    ) -> Option<i128> {
        let v = |id: &TermId| self.int_value(*id, values);
        match self.get(id) {
            Term::Int(n) => Some(*n),
            Term::Var(name) => values.get(name).copied(),
            Term::Neg(a) => v(a)?.checked_neg(),
            Term::Add(a, b) => v(a)?.checked_add(v(b)?),
            Term::Sub(a, b) => v(a)?.checked_sub(v(b)?),
            Term::Mul(a, b) => v(a)?.checked_mul(v(b)?),
            _ => None,
        }
    }

    /// A Boolean term's value under `values`, where it has one.
    pub fn bool_value(
        &self,
        id: TermId,
        values: &std::collections::BTreeMap<String, i128>,
    ) -> Option<bool> {
        let i = |id: &TermId| self.int_value(*id, values);
        let b = |id: &TermId| self.bool_value(*id, values);
        match self.get(id) {
            Term::Bool(x) => Some(*x),
            Term::Not(a) => Some(!b(a)?),
            Term::And(parts) => parts.iter().try_fold(true, |acc, p| Some(acc & b(p)?)),
            Term::Or(parts) => parts.iter().try_fold(false, |acc, p| Some(acc | b(p)?)),
            Term::Le(x, y) => Some(i(x)? <= i(y)?),
            Term::Lt(x, y) => Some(i(x)? < i(y)?),
            Term::Ge(x, y) => Some(i(x)? >= i(y)?),
            Term::Gt(x, y) => Some(i(x)? > i(y)?),
            Term::Eq(x, y) => Some(i(x)? == i(y)?),
            Term::Ne(x, y) => Some(i(x)? != i(y)?),
            _ => None,
        }
    }

    /// `id` with each variable `with` names replaced by its term; a variable
    /// it does not name stays. Built anew where anything changed, so the
    /// terms `id` was made of are untouched.
    pub fn substitute(
        &mut self,
        id: TermId,
        with: &std::collections::BTreeMap<String, TermId>,
    ) -> TermId {
        let term = self.get(id).clone();
        let mut s = |a: TermId| self.substitute(a, with);
        let rebuilt = match term {
            Term::Var(name) => return with.get(&name).copied().unwrap_or(id),
            Term::Bool(_) | Term::Int(_) => return id,
            Term::Neg(a) => Term::Neg(s(a)),
            Term::Not(a) => Term::Not(s(a)),
            Term::Add(a, b) => Term::Add(s(a), s(b)),
            Term::Sub(a, b) => Term::Sub(s(a), s(b)),
            Term::Mul(a, b) => Term::Mul(s(a), s(b)),
            Term::Le(a, b) => Term::Le(s(a), s(b)),
            Term::Lt(a, b) => Term::Lt(s(a), s(b)),
            Term::Ge(a, b) => Term::Ge(s(a), s(b)),
            Term::Gt(a, b) => Term::Gt(s(a), s(b)),
            Term::Eq(a, b) => Term::Eq(s(a), s(b)),
            Term::Ne(a, b) => Term::Ne(s(a), s(b)),
            Term::And(parts) => Term::And(parts.into_iter().map(&mut s).collect()),
            Term::Or(parts) => Term::Or(parts.into_iter().map(&mut s).collect()),
        };
        self.push(rebuilt)
    }

    /// How many terms `id` is made of, counting a shared one each time.
    pub fn size(&self, id: TermId) -> usize {
        1 + match self.get(id) {
            Term::Bool(_) | Term::Int(_) | Term::Var(_) => 0,
            Term::Neg(a) | Term::Not(a) => self.size(*a),
            Term::Add(a, b)
            | Term::Sub(a, b)
            | Term::Mul(a, b)
            | Term::Le(a, b)
            | Term::Lt(a, b)
            | Term::Ge(a, b)
            | Term::Gt(a, b)
            | Term::Eq(a, b)
            | Term::Ne(a, b) => self.size(*a) + self.size(*b),
            Term::And(parts) | Term::Or(parts) => parts.iter().map(|p| self.size(*p)).sum(),
        }
    }
}
