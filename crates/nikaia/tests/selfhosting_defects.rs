//! Defects a move of the toolchain into Nikaia found (ADR-294 D3, #125).
//!
//! Each program here was accepted by the checker and lowered to Rust that
//! `rustc` refused, so the test is the whole path: lower, compile, run, and
//! compare what it printed.

mod common;

use std::process::Command;

/// Lowers `source` as a one-file program, compiles the Rust it came to and
/// returns what the program printed.
fn printed(purpose: &str, source: &str) -> String {
    let dir = common::scratch_dir(&format!("selfhosting-{purpose}"));
    std::fs::write(dir.join("p.nika"), source).expect("write the source");
    let lowered = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .current_dir(&dir)
        .args(["lower", "p.nika", "--output", "p.rs"])
        .output()
        .expect("the nikaia binary runs");
    assert!(
        lowered.status.success(),
        "{purpose} did not lower:\n{}",
        String::from_utf8_lossy(&lowered.stderr)
    );
    let binary = dir.join("p");
    let compiled = common::compile(
        &dir.join("p.rs"),
        &["--crate-type", "bin", "-o", &binary.to_string_lossy()],
    );
    assert!(
        compiled.status.success(),
        "{purpose} did not compile:\n{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let out = Command::new(&binary).output().expect("run it");
    assert!(out.status.success(), "{purpose} did not run");
    std::fs::remove_dir_all(&dir).ok();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// #559: a view cut from a call's result is used after the statement, so the
/// result must live as long as the view.
#[test]
fn a_view_of_a_call_result_outlives_the_statement() {
    let said = printed(
        "view-of-a-call",
        r#"
fn f() -> String { return "  abc,d  " }
fn main() {
    let v = f().trim_start()
    let w = f().trim().split(",").collect()
    let n = f().trim_end().len()
    println(f"[{v}] {w.len()} {n}")
}
"#,
    );
    assert_eq!(said.trim(), "[abc,d  ] 2 7");
}

/// #560: `Wrapped(x, at)` is the constructor of a struct another file of the
/// package declares, as it is of one this file declares.
#[test]
fn a_constructor_call_of_a_struct_in_another_file_is_the_new_it_has() {
    let dir = common::scratch_dir("selfhosting-constructor-beside");
    std::fs::write(
        dir.join("a.nika"),
        "pub struct Wrapped[T] {\n    pub node: T,\n    pub at: i64,\n}\n\n\
         impl Wrapped[T] {\n    pub fn new(node: T, at: i64) -> Wrapped[T] {\n        \
         return Wrapped { node: node, at: at }\n    }\n}\n",
    )
    .expect("write a");
    std::fs::write(
        dir.join("b.nika"),
        "pub fn wrap(name: String, at: i64) -> Wrapped[String] {\n    return Wrapped(name, at)\n}\n\n\
         pub fn wrap_written(name: String, at: i64) -> Wrapped[String] {\n    \
         return Wrapped { node: name, at: at }\n}\n",
    )
    .expect("write b");
    let lowered = nikaia::sysroot::lower_tools(&dir).expect("the package lowers");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        lowered.contains("Wrapped::new(name, at)"),
        "the call spelling reaches the `new`:\n{lowered}"
    );
    assert!(
        lowered.contains("Wrapped { node: name, at }"),
        "and the struct spelling stays a struct:\n{lowered}"
    );
}

/// #565: the lowered tools package allows the two rustc lints that valid
/// Nikaia trips (`let mut s = init` then branches that always assign `s`, and
/// a `while true { … return … }` then a `return`). Measured once by adding both
/// shapes to a tools file and running `cargo check -p nikaia-std`: two
/// warnings without the attribute, none with it.
#[test]
fn the_tools_package_allows_what_valid_nikaia_trips() {
    let lib = include_str!("../../nikaia-std/src/lib.rs");
    let before_the_module = lib
        .split("pub mod tools {")
        .next()
        .expect("the tools module is declared");
    let attribute = before_the_module
        .rsplit("#[allow(")
        .next()
        .expect("an allow stands over it");
    for lint in ["unused_assignments", "unreachable_code"] {
        assert!(attribute.contains(lint), "{lint} is allowed over `tools`");
    }
}

/// #563: a `String` local reassigned from a view of itself, in a loop, is a
/// mixed text; the view is copied before it replaces the buffer it points
/// into. The tools package also needs `IntoEither` in scope for it, which
/// `nikaia-std`'s `tools` module brings (measured with a probe in a tools
/// file: E0599 without the import, none with it).
#[test]
fn a_string_reassigned_from_a_view_of_itself_in_a_loop() {
    let said = printed(
        "string-from-its-own-view",
        r#"
fn bare(lines: ref Vec[String]) -> Vec[String] {
    let mut out: Vec[String] = []
    for line in lines {
        let mut rest: String = line.clone()
        let mut again = true
        while again {
            again = false
            if rest.starts_with(">") {
                rest = rest.strip_prefix(">") ?? ""
                again = true
            }
        }
        out.push(rest.clone())
    }
    return out
}
fn main() {
    let v: Vec[String] = [">>a", "b"]
    println(f"{bare(v).join(",")}")
}
"#,
    );
    assert_eq!(said.trim(), "a,b");
    let lib = include_str!("../../nikaia-std/src/lib.rs");
    assert!(
        lib.contains("use crate::either_text::{IntoEither, IntoEitherMaybe};"),
        "the tools module has the conversions in scope"
    );
}

/// #564: `drain()` over a `mut` parameter takes the elements out of the
/// caller's list as owned values.
#[test]
fn drain_over_a_mut_parameter_empties_the_callers_list() {
    let said = printed(
        "drain-a-mut-parameter",
        r#"
fn take_all(mut from: Vec[String], mut into: Vec[String]) -> i64 {
    let mut count = 0
    for x in from.drain() {
        into.push(x)
        count += 1
    }
    return count
}
fn main() {
    let mut source: Vec[String] = ["a", "b"]
    let mut kept: Vec[String] = []
    let n = take_all(source, kept)
    println(f"{n} {source.len()} {kept.len()}")
}
"#,
    );
    assert_eq!(said.trim(), "2 0 2");
}

/// #561: a `??` whose fallback is an index read names a value, not a reference.
#[test]
fn a_fallback_read_at_an_index_is_a_value() {
    let said = printed(
        "fallback-at-an-index",
        r#"
fn word(maybe: String?, list: ref Vec[String]) -> String {
    let x = maybe ?? list[0]
    return x
}
fn number(maybe: i64?, list: ref Vec[i64]) -> i64 {
    let x = maybe ?? list[0]
    return x
}
fn main() {
    let words: Vec[String] = ["z"]
    let numbers: Vec[i64] = [7]
    println(f"{word(null, words)} {word("a", words)} {number(null, numbers)} {number(3, numbers)}")
}
"#,
    );
    assert_eq!(said.trim(), "z a 7 3");
}

/// #576: a `mut` parameter is a `&mut T` below, so assigning it whole writes
/// through it and reading a number out of it reads the number - `n = n + 1`
/// was `n = n + 1;` against a `&mut i64`, which `rustc` refused, on exactly
/// the program `NK1138` asks for. Handed to another `mut` parameter it is
/// handed on as it is.
#[test]
fn a_mut_parameter_is_assigned_and_read_through() {
    let said = printed(
        "mut-parameter-assigned",
        r#"
struct P {
    x: i64,
}

fn bump(mut n: i64) {
    n = n + 1
    n += 2
    if n == 4 {
        n = n * 10
    }
}

fn twice(mut n: i64) {
    bump(n)
    bump(n)
}

fn step(mut n: i64, xs: ref Vec[i64]) {
    n = xs[n] + n.abs() - (-n)
}

fn flip(mut b: bool) {
    b = !b
}

fn reset(mut p: P) {
    p = P { x: 0 }
}

fn refill(mut xs: Vec[i64]) {
    xs = [7, 8]
    xs.push(9)
}

fn main() {
    let mut n = 1
    bump(n)
    let mut m = 0
    twice(m)
    let mut k = 1
    step(k, [10, 20, 30])
    let mut b = false
    flip(b)
    let mut p = P { x: 5 }
    reset(p)
    let mut xs: Vec[i64] = []
    refill(xs)
    println(f"{n} {m} {k} {b} {p.x} {xs.len()}")
}
"#,
    );
    assert_eq!(said.trim(), "40 6 22 true 0 3");
}

/// #576, the open shape of `a_mut_parameter_is_assigned_and_read_through`: a
/// `match` over a `mut` parameter whose arm takes the parts and assigns the
/// whole (ADR-094 D7). The arm works on the value taken out of the place and
/// writes the whole back; an arm that takes nothing reads a number it binds
/// as the number, and the place can be a field of the parameter.
#[test]
fn an_arm_that_takes_parts_of_a_mut_parameter_writes_the_whole_back() {
    let said = printed(
        "mut-parameter-parts",
        r#"
enum E {
    Num(i64),
    Add(E, E),
    Wrap(E),
}

fn wrap_nums(mut e: E) {
    match e {
        E::Num(n) => { e = E::Wrap(E::Num(n)) }
        E::Add(a, b) => {
            let mut l = a
            let mut r = b
            wrap_nums(l)
            wrap_nums(r)
            e = E::Add(l, r)
        }
        E::Wrap(x) => {}
    }
}

fn swap_all(mut e: E) {
    match e {
        E::Num(n) => {}
        E::Add(a, b) => {
            let mut l = a
            let mut r = b
            swap_all(l)
            swap_all(r)
            e = E::Add(r, l)
        }
        E::Wrap(x) => { e = x }
    }
}

fn bump(mut e: E) {
    match e {
        E::Num(n) => { e = E::Num(n + 1) }
        E::Add(a, b) => {}
        E::Wrap(x) => {}
    }
}

struct Holder {
    root: E,
    count: i64,
}

fn rewrap(mut h: Holder) {
    match h.root {
        E::Num(n) => {}
        E::Add(a, b) => { h.root = E::Wrap(a) }
        E::Wrap(x) => { h.root = x }
    }
    h.count += 1
}

fn show(e: ref E) -> String {
    match e {
        E::Num(n) => f"{n}",
        E::Add(a, b) => f"({show(a)}+{show(b)})",
        E::Wrap(x) => f"[{show(x)}]",
    }
}

fn main() {
    let mut e = E::Add(E::Num(1), E::Add(E::Num(2), E::Num(3)))
    wrap_nums(e)
    println(show(e))
    swap_all(e)
    println(show(e))
    bump(e)
    println(show(e))
    let mut one = E::Num(1)
    bump(one)
    println(show(one))
    let mut h = Holder { root: E::Add(E::Num(7), E::Num(8)), count: 0 }
    rewrap(h)
    rewrap(h)
    println(f"{show(h.root)} {h.count}")
}
"#,
    );
    assert_eq!(said.trim(), "([1]+([2]+[3]))\n((3+2)+1)\n((3+2)+1)\n2\n7 2");
}

/// #567: a `match` over a `let` over a place, over a field and through a nested `match` binds a number as the number.
#[test]
fn a_number_bound_under_a_reference_is_a_number() {
    let said = printed(
        "number-under-a-reference",
        r#"
enum V {
    Case(i64, i64),
    Pair(V, i64),
    Nothing,
}

struct H {
    kind: V,
    list: Vec[i64],
}

impl H {
    fn one(ref self) -> i64 {
        match self.kind {
            V::Case(a, b) => self.list[a] + b * 2
            else => 0
        }
    }

    fn two(ref self, vs: ref Vec[V]) -> i64 {
        let mut t = 0
        for v in vs {
            match v {
                V::Case(a, b) => { t += self.list[a] + b }
                V::Pair(inner, k) => {
                    match inner {
                        V::Case(c, d) => { t += self.list[c] + d + k }
                        else => {}
                    }
                }
                V::Nothing => {}
            }
        }
        return t
    }
}

fn three(vs: ref Vec[V], list: ref Vec[i64]) -> i64 {
    let v = vs[0]
    match v {
        V::Case(a, b) => list[a] + b
        else => 0
    }
}


fn five(h: ref H) -> i64 {
    match h.kind {
        V::Pair(inner, k) => {
            match inner {
                V::Case(c, d) => h.list[c] + d + k
                else => 0
            }
        }
        else => 0
    }
}

fn main() {
    let h = H { kind: V::Case(1, 5), list: [10, 20, 30] }
    let vs = [V::Case(1, 5), V::Pair(V::Case(2, 1), 4)]
    println(f"{h.one()} {h.two(vs)} {three(vs, h.list)} {five(h)}")
}
"#,
    );
    assert_eq!(said.trim(), "30 60 25 0");
}

/// #570: `v[v.len() - 1] = x` and `+=`, on a list, a list of lists and a field.
#[test]
fn an_index_that_reads_the_target_is_worked_out_before_the_write() {
    let said = printed(
        "index-reads-the-target",
        r#"
struct Holder {
    items: Vec[i64],
}

fn last_set(mut v: Vec[i64], x: i64) {
    v[v.len() - 1] = x
}

fn last_add(mut v: Vec[i64]) {
    v[v.len() - 1] += 1
}

fn nested(mut vv: Vec[Vec[i64]]) {
    vv[vv.len() - 1][0] = 9
    vv[0][vv[0].len() - 1] += 2
}

fn field(mut h: Holder) {
    h.items[h.items.len() - 1] = 7
}

fn main() {
    let mut v: Vec[i64] = [1, 2, 3]
    last_set(v, 8)
    last_add(v)
    let mut vv: Vec[Vec[i64]] = [[1, 2], [3, 4]]
    nested(vv)
    let mut h = Holder { items: [5, 6] }
    field(h)
    let mut w: Vec[i64] = [1, 2, 3]
    w[w.len() - 1] = 5
    w[w.len() - 1] += 1
    println(f"{v[2]} {vv[1][0]} {vv[0][1]} {h.items[1]} {w[2]}")
}
"#,
    );
    assert_eq!(said.trim(), "9 9 4 7 6");
}

/// #574: `x ?? BinaryOp::Add` for an `x: ref BinaryOp?`, with a struct variant as the fallback.
#[test]
fn a_value_beside_a_view_of_an_option_is_lent_with_it() {
    let said = printed(
        "fallback-beside-a-view",
        r#"
enum BinaryOp {
    Add,
    Sub,
    Mul,
}

enum Shape {
    Square(i64),
    Rect { w: i64, h: i64 },
}


fn name(op: ref BinaryOp) -> String {
    match op {
        BinaryOp::Add => "add".clone()
        BinaryOp::Sub => "sub".clone()
        BinaryOp::Mul => "mul".clone()
    }
}

fn pick(x: ref BinaryOp?) -> String {
    let op = x ?? BinaryOp::Add
    return name(op)
}

fn pick_const(x: ref BinaryOp?) -> String {
    let op = x ?? BinaryOp::Mul
    return name(op)
}

fn area(s: ref Shape?) -> i64 {
    let shape = s ?? Shape::Rect { w: 2, h: 3 }
    match shape {
        Shape::Square(n) => n * n
        Shape::Rect { w, h } => w * h
    }
}

fn main() {
    let some: BinaryOp? = BinaryOp::Sub
    let none: BinaryOp? = null
    let shape: Shape? = null
    println(f"{pick(some)} {pick(none)} {pick_const(none)} {area(shape)}")
}
"#,
    );
    assert_eq!(said.trim(), "sub add mul 6");
}

/// #566: `return ("text", n)` for a `(String, i64)`, a tail, a struct field, a list element and a nested tuple (no longer reproduces; kept as the test).
#[test]
fn a_literal_in_a_tuple_is_text_where_the_result_says_text() {
    let said = printed(
        "literal-in-a-tuple",
        r#"
struct Row {
    name: String,
    n: i64,
}

fn pair(n: i64) -> (String, i64) {
    return ("text", n)
}

fn tail(n: i64) -> (String, i64) {
    ("tail", n)
}

fn built(n: i64) -> Row {
    return Row { name: "row", n: n }
}

fn rows(n: i64) -> Vec[(String, i64)] {
    return [("a", n), ("b", n + 1)]
}

fn nested(n: i64) -> ((String, i64), String) {
    return (("deep", n), "outer")
}

fn main() {
    let (s, n) = pair(1)
    let (t, m) = tail(2)
    let r = built(3)
    let xs = rows(4)
    let q = nested(5)
    println(f"{s}{n} {t}{m} {r.name}{r.n} {xs.len()} {q.0.0}{q.0.1}{q.1}")
}
"#,
    );
    assert_eq!(said.trim(), "text1 tail2 row3 2 deep5outer");
}

/// #568: mutually recursive `throws` functions, one throwing `make_error(x)` (no longer reproduces; kept as the test).
#[test]
fn a_group_that_throws_a_called_error_keeps_its_channel() {
    let said = printed(
        "throw-a-call",
        r#"
enum Fault {
    Odd { at: i64 },
    Deep(i64),
}

impl Error for Fault {
    fn message(ref self) -> String {
        match self {
            Fault::Odd { at } => f"odd {at}"
            Fault::Deep(n) => f"deep {n}"
        }
    }
}

fn make_error(x: i64) -> Fault {
    return Fault::Odd { at: x }
}

fn even(x: i64) -> i64 throws {
    if x == 0 {
        return 0
    }
    if x == 7 {
        throw make_error(x)
    }
    return odd(x - 1)
}

fn odd(x: i64) -> i64 throws {
    if x == 0 {
        throw Fault::Deep(x)
    }
    return even(x - 1)
}

fn main() {
    let r = even(4) catch { -1 }
    let q = odd(8) catch { -2 }
    let z = even(7) catch { -3 }
    println(f"{r} {q} {z}")
}
"#,
    );
    assert_eq!(said.trim(), "0 -2 -3");
}

/// #569: a `Copy` and a non-`Copy` enum handed on from a `ref` binding and an owned one (no longer reproduces; kept as the test).
#[test]
fn an_enum_is_passed_by_value_from_a_view_and_an_owner() {
    let said = printed(
        "enum-by-value",
        r#"
enum Op {
    Add,
    Sub,
}

enum Tree {
    Leaf(i64),
    Node(Op, Tree, Tree),
}

fn apply(op: Op, a: i64, b: i64) -> i64 {
    match op {
        Op::Add => a + b
        Op::Sub => a - b
    }
}

fn describe(op: Op) -> String {
    match op {
        Op::Add => "add".clone()
        Op::Sub => "sub".clone()
    }
}

fn eval(t: ref Tree) -> i64 {
    match t {
        Tree::Leaf(n) => n
        Tree::Node(op, l, r) => apply(op, eval(l), eval(r))
    }
}

fn names(ops: ref Vec[Op]) -> Vec[String] {
    let mut out: Vec[String] = []
    for op in ops {
        out.push(describe(op))
    }
    return out
}

fn first(ops: ref Vec[Op]) -> String {
    let op = ops[0]
    return describe(op)
}

fn main() {
    let t = Tree::Node(Op::Add, Tree::Leaf(2), Tree::Node(Op::Sub, Tree::Leaf(9), Tree::Leaf(4)))
    let ops = [Op::Add, Op::Sub]
    println(f"{eval(t)} {names(ops).len()} {first(ops)}")
}
"#,
    );
    assert_eq!(said.trim(), "7 2 add");
}

/// #571: `let n = opt ?? return` then `m.get(n)`, `contains_key(n)` and `n.len()` for `ref String?` and `String?` (no longer reproduces; kept as the test).
#[test]
fn an_option_opened_past_a_jump_is_not_borrowed_again() {
    let said = printed(
        "option-past-a-jump",
        r#"
use std::collections

fn lookup(opt: ref String?, m: ref collections::HashMap[String, i64]) -> i64 {
    let n = opt ?? return 0
    let a = m.get(n) ?? 0
    let b = if m.contains_key(n) { 1 } else { 0 }
    return a + b + n.len()
}

fn lookup_owned(opt: String?, m: ref collections::HashMap[String, i64]) -> i64 {
    let n = opt ?? return 0
    let a = m.get(n) ?? 0
    let b = if m.contains_key(n) { 1 } else { 0 }
    return a + b + n.len()
}

fn main() {
    let mut m: collections::HashMap[String, i64] = collections::HashMap()
    m["ab"] = 5
    let some: String? = "ab"
    let none: String? = null
    let other: String? = "ab"
    println(f"{lookup(some, m)} {lookup(none, m)} {lookup_owned(other, m)}")
}
"#,
    );
    assert_eq!(said.trim(), "8 0 8");
}

/// #572: a struct variant holding an enum, matched twice through a `ref` parameter (no longer reproduces; kept as the test).
#[test]
fn a_second_match_under_a_view_writes_no_ref() {
    let said = printed(
        "match-twice",
        r#"
enum Kind {
    Num(i64),
    Neg,
}

enum Term {
    Atom { kind: Kind, name: String },
    Group { inner: Kind, count: i64 },
}

fn show(t: ref Term) -> String {
    match t {
        Term::Atom { kind, name } => {
            match kind {
                Kind::Num(n) => f"{name}={n}"
                Kind::Neg => f"-{name}"
            }
        }
        Term::Group { inner, count } => {
            match inner {
                Kind::Num(n) => f"g{count}:{n}"
                Kind::Neg => f"g{count}"
            }
        }
    }
}

fn main() {
    let a = Term::Atom { kind: Kind::Num(3), name: "x" }
    let b = Term::Group { inner: Kind::Neg, count: 2 }
    println(f"{show(a)} {show(b)}")
}
"#,
    );
    assert_eq!(said.trim(), "x=3 g2");
}
