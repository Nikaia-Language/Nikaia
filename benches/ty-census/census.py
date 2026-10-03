#!/usr/bin/env python3
"""ADR-294 D16's census build: counts, never costs.

Rewrites `contracts/ty.rs` in a copy of the repository so that `Clone` and
`PartialEq` for `Ty` are written out, `#[inline(never)]`, with a counter, and
`fits` counts its calls; `main.rs` prints the counts to stderr at exit when
`NIKAIA_TY_CENSUS` is set. Apply it to a scratch copy, never to the tree
that is committed:

    cp -r crates <copy>/crates ... ; census.py <copy>
"""
import re
import sys

root = sys.argv[1]
ty = f"{root}/crates/nikaia/src/contracts/ty.rs"
s = open(ty).read()
s = s.replace("#[derive(Debug, Clone, PartialEq, Eq)]\npub enum Ty {", "#[derive(Debug, Eq)]\npub enum Ty {", 1)
s += r'''

pub mod census {
    use std::cell::{Cell, RefCell};
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    pub static CLONED_NODES: AtomicU64 = AtomicU64::new(0);
    pub static CLONES: AtomicU64 = AtomicU64::new(0);
    pub static EQ_NODES: AtomicU64 = AtomicU64::new(0);
    pub static EQS: AtomicU64 = AtomicU64::new(0);
    pub static FITS: AtomicU64 = AtomicU64::new(0);
    thread_local! {
        pub static DEPTH: Cell<u32> = const { Cell::new(0) };
        pub static EQ_DEPTH: Cell<u32> = const { Cell::new(0) };
        pub static DISTINCT: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    }
    pub fn report() {
        if std::env::var_os("NIKAIA_TY_CENSUS").is_none() {
            return;
        }
        let distinct = DISTINCT.with(|d| d.borrow().len());
        eprintln!(
            "ty-census clones={} cloned_nodes={} eqs={} eq_nodes={} fits={} distinct_cloned={}",
            CLONES.load(Relaxed),
            CLONED_NODES.load(Relaxed),
            EQS.load(Relaxed),
            EQ_NODES.load(Relaxed),
            FITS.load(Relaxed),
            distinct
        );
    }
}

impl Clone for Ty {
    #[inline(never)]
    fn clone(&self) -> Ty {
        use std::sync::atomic::Ordering::Relaxed;
        census::CLONED_NODES.fetch_add(1, Relaxed);
        let top = census::DEPTH.with(|d| {
            let v = d.get();
            d.set(v + 1);
            v == 0
        });
        if top {
            census::CLONES.fetch_add(1, Relaxed);
            census::DISTINCT.with(|d| {
                d.borrow_mut().insert(format!("{self:?}"));
            });
        }
        let out = match self {
            Ty::Unknown => Ty::Unknown,
            Ty::Named { name, args, view } => Ty::Named { name: name.clone(), args: args.clone(), view: *view },
            Ty::Tuple(parts) => Ty::Tuple(parts.clone()),
            Ty::Pointed { item, slice, mutable } => Ty::Pointed { item: item.clone(), slice: *slice, mutable: *mutable },
            Ty::Count(n) => Ty::Count(*n),
            Ty::Var { name, view } => Ty::Var { name: name.clone(), view: *view },
            Ty::Fn { params, result, is_sync, throws } => Ty::Fn { params: params.clone(), result: result.clone(), is_sync: *is_sync, throws: *throws },
            Ty::Nullable(inner) => Ty::Nullable(inner.clone()),
            Ty::Seq { item, is_sync, pauses, throws, parallel, shape } => Ty::Seq { item: item.clone(), is_sync: *is_sync, pauses: *pauses, throws: *throws, parallel: *parallel, shape: *shape },
        };
        census::DEPTH.with(|d| d.set(d.get() - 1));
        out
    }
}

impl PartialEq for Ty {
    #[inline(never)]
    fn eq(&self, other: &Ty) -> bool {
        use std::sync::atomic::Ordering::Relaxed;
        census::EQ_NODES.fetch_add(1, Relaxed);
        let top = census::EQ_DEPTH.with(|d| {
            let v = d.get();
            d.set(v + 1);
            v == 0
        });
        if top {
            census::EQS.fetch_add(1, Relaxed);
        }
        let out = match (self, other) {
            (Ty::Unknown, Ty::Unknown) => true,
            (Ty::Named { name: a, args: b, view: c }, Ty::Named { name: x, args: y, view: z }) => a == x && b == y && c == z,
            (Ty::Tuple(a), Ty::Tuple(b)) => a == b,
            (Ty::Pointed { item: a, slice: b, mutable: c }, Ty::Pointed { item: x, slice: y, mutable: z }) => a == x && b == y && c == z,
            (Ty::Count(a), Ty::Count(b)) => a == b,
            (Ty::Var { name: a, view: b }, Ty::Var { name: x, view: y }) => a == x && b == y,
            (Ty::Fn { params: a, result: b, is_sync: c, throws: d }, Ty::Fn { params: w, result: x, is_sync: y, throws: z }) => a == w && b == x && c == y && d == z,
            (Ty::Nullable(a), Ty::Nullable(b)) => a == b,
            (Ty::Seq { item: a, is_sync: b, pauses: c, throws: d, parallel: e, shape: f }, Ty::Seq { item: u, is_sync: v, pauses: w, throws: x, parallel: y, shape: z }) => a == u && b == v && c == w && d == x && e == y && f == z,
            _ => false,
        };
        census::EQ_DEPTH.with(|d| d.set(d.get() - 1));
        out
    }
}
'''
s = s.replace("    pub fn fits(&self, expected: &Ty) -> bool {\n",
              "    pub fn fits(&self, expected: &Ty) -> bool {\n        census::FITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);\n", 1)
open(ty, "w").write(s)

main = f"{root}/crates/nikaia/src/main.rs"
m = open(main).read()
m = m.replace("pub fn main() -> std::process::ExitCode {\n",
              "pub fn main() -> std::process::ExitCode {\n    struct Report;\n    impl Drop for Report {\n        fn drop(&mut self) {\n            nikaia::contracts::ty::census::report();\n        }\n    }\n    let _report = Report;\n", 1)
m = m.replace("std::process::exit(code);", "{ nikaia::contracts::ty::census::report(); std::process::exit(code); }")
open(main, "w").write(m)
