//! The prover's logic layer as its tests ask it: the names `nikaia-logic` had,
//! over the layer that is now Nikaia (`tools/logic_*.nika`, ADR-294, #125).

#![allow(dead_code, unused_imports)]

use nikaia_std::tools::logic_alethe::lg_alethe_write;
use nikaia_std::tools::logic_lia::LgBudget;
use nikaia_std::tools::logic_normal::{
    LgNormal, lg_certificate_of, lg_certificate_text, lg_model_of, lg_model_text, lg_normal_of,
};
use nikaia_std::tools::logic_search::{lg_check, lg_verify, lg_verify_model};
use nikaia_std::tools::logic_smtlib::{LgRead, lg_smtlib_read, lg_smtlib_write};
use nikaia_std::tools::prover_arena::TermArena;

pub use nikaia_std::tools::logic_lia::{
    LgAnswer as Answer, LgCertificate as Certificate, LgModel as Model, LgRefutation as Refutation,
    LgRejected as Rejected, LgStep as Step, LgUnknown as Unknown,
};
pub use nikaia_std::tools::logic_smtlib::LgReadError as ReadError;

pub type TermId = i64;

#[derive(Debug, Clone)]
pub struct Arena(pub TermArena);

impl Arena {
    pub fn new() -> Arena {
        Arena(TermArena::empty())
    }
    pub fn bool(&mut self, v: bool) -> TermId {
        self.0.boolean(v)
    }
    pub fn int(&mut self, v: i64) -> TermId {
        self.0.int(v)
    }
    pub fn var(&mut self, name: &str) -> TermId {
        self.0.var(name)
    }
    pub fn add(&mut self, a: TermId, b: TermId) -> TermId {
        self.0.add(a, b)
    }
    pub fn sub(&mut self, a: TermId, b: TermId) -> TermId {
        self.0.sub(a, b)
    }
    pub fn neg(&mut self, a: TermId) -> TermId {
        self.0.neg(a)
    }
    pub fn mul(&mut self, a: TermId, b: TermId) -> TermId {
        self.0.mul(a, b)
    }
    pub fn le(&mut self, a: TermId, b: TermId) -> TermId {
        self.0.le(a, b)
    }
    pub fn lt(&mut self, a: TermId, b: TermId) -> TermId {
        self.0.lt(a, b)
    }
    pub fn ge(&mut self, a: TermId, b: TermId) -> TermId {
        self.0.ge(a, b)
    }
    pub fn gt(&mut self, a: TermId, b: TermId) -> TermId {
        self.0.gt(a, b)
    }
    pub fn eq(&mut self, a: TermId, b: TermId) -> TermId {
        self.0.equal(a, b)
    }
    pub fn ne(&mut self, a: TermId, b: TermId) -> TermId {
        self.0.unequal(a, b)
    }
    pub fn and(&mut self, parts: Vec<TermId>) -> TermId {
        self.0.and(parts)
    }
    pub fn or(&mut self, parts: Vec<TermId>) -> TermId {
        self.0.or(parts)
    }
    pub fn not(&mut self, a: TermId) -> TermId {
        self.0.not(a)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Query<'a> {
    pub arena: &'a Arena,
    pub facts: &'a [TermId],
    pub goal: TermId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub cases: usize,
    pub bounds: usize,
}

impl Default for Budget {
    fn default() -> Budget {
        Budget {
            cases: 1024,
            bounds: 4096,
        }
    }
}

pub trait Solver {
    fn check(&self, query: &Query<'_>, budget: &Budget) -> Answer;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct FourierMotzkin;

impl Solver for FourierMotzkin {
    fn check(&self, query: &Query<'_>, budget: &Budget) -> Answer {
        lg_check(
            &query.arena.0,
            query.facts,
            query.goal,
            &LgBudget {
                cases: budget.cases as i64,
                bounds: budget.bounds as i64,
            },
        )
    }
}

pub fn verify(query: &Query<'_>, certificate: &Certificate) -> Result<(), Rejected> {
    match lg_verify(&query.arena.0, query.facts, query.goal, certificate) {
        None => Ok(()),
        Some(why) => Err(why),
    }
}

pub fn verify_model(query: &Query<'_>, model: &Model) -> bool {
    lg_verify_model(&query.arena.0, query.facts, query.goal, model)
}

pub fn certificate_text(certificate: &Certificate) -> String {
    lg_certificate_text(certificate)
}

pub fn certificate_of(text: &str) -> Option<Certificate> {
    lg_certificate_of(text)
}

pub fn model_text(model: &Model) -> String {
    lg_model_text(model)
}

pub fn model_of(text: &str) -> Option<Model> {
    lg_model_of(text)
}

pub struct Normal {
    inner: LgNormal,
    arena: Arena,
}

impl Normal {
    pub fn of(query: &Query<'_>) -> Normal {
        let inner = lg_normal_of(&query.arena.0, query.facts, query.goal);
        let arena = Arena(inner.arena.clone());
        Normal { inner, arena }
    }
    pub fn query(&self) -> Query<'_> {
        Query {
            arena: &self.arena,
            facts: &self.inner.facts,
            goal: self.inner.goal,
        }
    }
    pub fn text(&self) -> String {
        self.inner.text()
    }
    pub fn named(&self, model: &Model) -> Model {
        self.inner.named(model)
    }
}

pub mod smtlib {
    use super::*;
    pub use nikaia_std::tools::logic_smtlib::LgReadError as ReadError;

    pub struct Script {
        pub arena: Arena,
        pub assertions: Vec<TermId>,
    }

    pub fn write(query: &Query<'_>) -> String {
        lg_smtlib_write(&query.arena.0, query.facts, query.goal)
    }

    pub fn read(text: &str) -> Result<Script, ReadError> {
        match lg_smtlib_read(text) {
            LgRead::Script(script) => Ok(Script {
                arena: Arena(script.arena),
                assertions: script.assertions,
            }),
            LgRead::Failed(why) => Err(why),
        }
    }
}

pub mod alethe {
    use super::*;

    pub fn write(query: &Query<'_>) -> Option<String> {
        lg_alethe_write(&query.arena.0, query.facts, query.goal)
    }
}
