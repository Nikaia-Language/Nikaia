# Nikaia Language Specification
**Part I: The Language Core**
**Version:** 0.0.645 (Draft)
**Date:** 2026-10-07

---

## Chapter 1: Introduction and Principles

### 1.1. What Nikaia is
Nikaia is a statically typed systems programming language. Its properties are
predictable execution, memory safety and concise source. Memory management,
concurrency constraints and representation details are enforced by the
compiler. The compiler translates Nikaia source to Rust and drives the Rust
toolchain, which is called the **backend** throughout this specification.

Four weights decide a question about the language
([ADR-258](adr/adr-258.md)). **The special case carries the cost**, not the
common one. **The compiler works for the developer**, not the developer for
the compiler: what can be worked out is not written. **The common case has
zero overhead**: it costs what the same program costs written by hand in the
backend. **The language is consistent with itself**: a rule reads the same
wherever its shape appears.

### 1.2. One language, and the build options
There is one Nikaia. The same source compiles for every target and under every
build option, and the resulting program prints the same bytes. A build option
decides **how** a program is built, never **what** it computes.

> A build decides how, never what.

The build options are named below. Each lives under `[build]` in
`nikaia.toml`; `--target` and `--user-parallelism` override the first two for a
single build. The specification does not state their number. Statements run in
the order they are written; a program that wants overlap writes
`overlap { … }` (8.1.2).

#### `target` — the machine
`target` names the machine a program is built for. The target decides what the
standard library offers on it (Part III 17.2) and what a `panic` does: the
stack unwinds where the machine unwinds, and the program traps where the
machine traps (Part III, Appendix A). The targets are `x86_64-linux`,
`aarch64-linux` and `wasm32-unknown`; the default is the machine the compiler
runs on.

#### `user_parallelism` — whether user code may run concurrently
`user_parallelism` is a permission, not a thread count.

* `no` (the default): no two pieces of user code are ever in flight at the
  same time. A data race in user code is impossible under this option.
* `yes`: user code may run concurrently. The compiler enforces the rules of
  Part II chapter 12 that keep shared data intact.

The option applies to user code only. The compiler and the runtime may use
additional threads for internal work, provided that no user code executes
concurrently on them. Such work does not change a byte of what the program
prints. How many threads serve a `yes` is the runtime's decision.

> The compiler may be concurrent even when the program is not.

#### The re-entrancy check — whether a broken rule is noticed
Taking a lock while a lock is held is refused with `NK2203` when the program is
compiled (Part II 12.3). The re-entrancy check is a build option that decides
whether the program also carries the runtime check that notices such a nesting
if one occurs. It is on by default and may be declined.

For every program that obeys the nesting rule, both builds behave identically.
The check cannot fire in a correct compiler. If it fires, the compiler has a
defect; without the check, the same defect appears as a silent hang.

The key is `reentrancy-check` in `[build]`, and it takes `yes` or `no`. It has
no command-line override. Declining it removes the check from the crossing
shape of `std` only; the single-threaded shape keeps its own borrow check.

### 1.3. The prelude
Some names need no `use`. They are these, and there are no others:

* the containers a program cannot do without: **`Vec`**, **`String`**,
  **`Bytes`**;
* the printing functions: **`println`**, **`print`**, **`eprintln`**,
  **`eprint`**;
* **`assert`** and **`panic`**;
* the doors over several locks: **`access_all`** and **`update_all`**
  (Part II, 12.3);
* the shared types: **`Shared`**, **`SharedMut`** and **`Locked`** (6.2);
* the numeric conversions 2.2 already offers.

Nothing in the prelude does I/O but printing, and nothing in it pauses. The
prelude is not cut down on a machine with no filesystem (Part III 17.2). `HashMap` is outside it:
a program writes `collections::HashMap` after `use std::collections`.

The compiler decides the shape of a `SharedMut[T]` per value. A program writes `SharedMut(0)` with no import
(6.2, Part II 12.2).

`eprint` and `eprintln`, like `panic` and `assert`, are diagnosis; what they do
depends on the target, and the compiler decides it. `access_all` and
`update_all` take the locks in one order however the program wrote them, so a
deadlock cycle cannot form. `Bytes` is one shared buffer: what `fs::read`
hands back, passed on by a count. `panic` ends the
program with the program's own words, at the Nikaia line.

Everything else in the standard library is reached the way a package is
reached: `use std::fs` at the top of the file, and
`fs::read_to_string(path, fs::Root::Anywhere)` where it is used (9.1). A name
that lives in a `std` module is refused without its prefix — `NK1117` for a
function, `NK1135` for a type — and a prefix is refused without its `use`, each
with the line to add. A name on this list written with a `std` module in front of it, such as `io::println`, is refused too (`NK1163`).

---

## Chapter 2: Variables and Data Types

A **comment** begins with `//` and runs to the end of the line, or begins with
`/*` and runs to the matching `*/`. A block comment may span lines and may
stand anywhere whitespace may stand. Block comments **nest**: `/* a /* b */ c */`
is one comment. An unclosed `/*` is reported where it opened. A block comment
is a comment wherever it stands, including `/** … */` and `/*! … */`.

**A run of `///` lines immediately before an item is that item's
documentation.** An item is a `fn`, a `struct`, an `enum`, a `trait`, a field or
a variant. Anywhere else `///` is an ordinary comment. An ordinary comment
standing between the run and the item does not end it; a statement does. A doc
comment is prose: the compiler reads no directive, no `@param` and no link out
of it. The compiler does not copy it into
`nikaia.contracts` (Part III, 13.5; [ADR-251](adr/adr-251.md) D2).

### 2.1. Variables and Assignment
A **variable** is a named storage location that holds a value. A variable is
declared with `let`.

**Immutability**
A variable is **immutable** unless it is declared otherwise. Once a value is
assigned to an immutable name, the name cannot be assigned again.

```nika
let x = 10
// x = 20  <-- This would cause a Compiler Error
```

**Mutability**
A variable that may change is declared **mutable** with the keyword `mut`.

```nika
let mut y = 10
y = 20     // This is allowed
```

**Taking a tuple apart.** `let (a, b) = pair()` binds each name to the part at its position. A `let` takes one name or a flat tuple of names; a nested tuple is refused, and so is a written type (`NK1136`), because one type cannot say which name it is about.

**Reserved words**

A reserved word means one thing wherever it appears. A name may not be a
reserved word. The reserved words are:

```text
as        break     catch     comptime  continue  dsl       else      enum
extern    false     fn        for       grammar   if        impl      in
let       match     mut       null      overlap   pub       ref       return
select    self      spawn     struct    sync      test      throw     throws
trait     true      unsafe    use       while     with
```

A name that is a reserved word does not parse, and the parse error names the
word and says it is reserved.

**`ref` is reserved for the view it writes.** `ref(x)` is a borrow of `(x)`.

**`comptime` is reserved for its construct**: a binding where an item stands or a statement inside a function
body (Part II, 10.2). It promises a time of evaluation, not constancy.

**`_` is not a name; it is the ignore pattern.** It stands where a name would be
bound and says that the value is ignored on purpose: a position of a
destructured tuple (`let (name, _) = pair()`), a parameter a shape dictates
(`fn handle(event: Event, _: Context)`, `fn(_, value) { … }`), and a part of a
`match` pattern (`(0, _)`, `Shape::Circle(_)`). The arm taken when nothing else
matched is `else`, not `_` (3.4). `let _ = expr` is refused: a call made for its effect is written as the
call, and a resource is closed by name. `_` is never a value. What `_` ignores
is not moved, so a `let` over a place stays a view of it (6.5).

**`with` is reserved for its construct**: a copy of a value with named fields
changed, `p with { x: 1 }` (4.2).

**`extern` and `unsafe` are reserved for their constructs**: an `extern`
block, and the `unsafe { … }` a call into one is written in (Part III 15.1).

**`select` is reserved for its construct**: the block that races several
waits and takes the first to finish (Part II, 12.4).

**`break` and `continue` are reserved for their constructs** (3.3).

**`test` is reserved for its construct**: a `test "name" { … }` block of the
package it stands in (Part III 14.1).

**`loop`, `const`, `macro`, `quote` and `from` are ordinary names.** A declared
`loop`, `quote` or `from` is a name like any other. Where nothing declares one
of them, the refusal every undeclared name meets carries a hint: `loop { … }` is
answered with *write `while true`*, `const X = …` with *write `comptime`*, and
`macro` and `quote` with *Nikaia has no macros*.

**`seq` is an ordinary name.**

Three further rules about the list:

* **A `grammar` block has its own vocabulary**, and it is not on the list:
  `rule`, `boundary`, `fold`, `par_fold` and `unchecked` are keywords *inside*
  a `grammar` (Part II, 10.1) and ordinary names everywhere else.
* **After a `::` or a `.`, a reserved word is a name.** A segment follows a `::`
  and a member follows a `.`, and no construct begins in either position, so
  `Self::dsl` (Part II, 10.5) and `scope.spawn fn { … }` (Part II, 12.5) are
  what they look like.
* **`self` is on the list and is also a name**: the receiver of a method.
  A method body uses it; user code may not declare it. Declaring `self` at a
  `let`, a `for` binding, a lambda's argument or a struct field is refused with
  `NK1119`; a parameter named `self` does not parse, and the message says so.

### 2.2. Primitive Data Types
Nikaia provides basic types to represent simple values.

* **Integers:** whole numbers without fractions.
    * `i64`: a 64-bit integer. It is the type of most numbers, and the type
      of a **length**: `xs.len()` hands back an `i64`, and an index is one.
    * `i32`: a 32-bit integer. It is written where the layout matters: a
      struct that has to be small, a wire format, a C header. An `i32` that
      meets an `i64` becomes one (below).
    * `u8`: one byte. It has the same conversion and arithmetic names as the
      other integer types. A file read whole is a `Bytes` (`fs::read`): one
      shared buffer of them, handed on by a count and not copied, and read
      as a run of bytes (2.6). `text.bytes` is a text's UTF-8 as `u8`s.
    * `u64` and `u32`: unsigned, 64 and 32 bits. They are written where the
      bits are the point: a hash, a mask, a wire format's field
      ([ADR-285](adr/adr-285.md)).
* **Floats:** numbers with decimal points.
    * `f64`: a double-precision floating-point number. A literal may carry an
      **exponent**, always written `e`: `1.5e-4`, `2e3`,
      `9.54791938424326609e-04`. `1E5` is refused.
* **Booleans:** logic values.
    * `bool`: `true` or `false`.
* **Text:**
    * `String`: text, and always valid UTF-8: bytes that are not are refused
      where they enter the program, with `NotText`. A file name need not be
      text, and is an `fs::Path` (Part III 17.1). Whether a value of it is a view into text that is
      already there, or text of its own, is the compiler's decision per use
      (6.6). A literal is a view of the program's own text; where the use
      keeps it as a `String` it is constructed there, and where the use only
      reads it nothing is allocated. Wherever a program declares `String` -
      a field, a result, a parameter, a `let`, the element of a list, the key
      or value of a map - it is text of its own where only text of its own is
      put there, a view where only views are, and either, per value, where
      both are; a published field, parameter or result is never the third.
    * `ref String`: the same text with a promise attached: *this is a borrowed
      view, no copy and no handle*. The compiler holds the program to the
      promise. It is written where allocating would be a mistake.
    * `scalar`: one Unicode scalar value - a code point that is not a
      surrogate - and not a byte. It is written between single quotes, with
      the escapes a string uses: `'a'`, `'ä'`, `'\n'` (2.6). Walking a text's
      scalars yields it (`for c in name.scalars`), and a `match` over one
      compares against it (3.4). There is no `char`: what one character is
      depends on the unit, and 2.6 names the three.
* **A run of elements:**
    * `ref Array[T]`: a **view** of a run of `T`, with the promise
      `ref String` carries — *this points at elements somebody else keeps, no
      copy and no handle*. It is read with `.len()`, `xs[i]` and `for`, and it
      is what a `Vec[T]` **crosses as** when a build hands one to the program
      (10.2): a `comptime` declared `ref Array[T]` is a `const` the program
      reads and nothing allocates. `ref Array[u8]` is the byte buffer.
    * A value a body **built** does not go into one. Where the program builds
      the run while it runs, the type is `Vec[T]`; a **parameter** is the one
      place the view is the compiler's to write, so `total(xs)` for a
      `Vec[i64]` needs no word. `NK1179` says which of the two a line is.
    * `Array[T]`: an array of **any** length, fixed for each use. It is a
      **parameter's** type and the call says which length:
      `fn total(xs: Array[i64])` takes an array of three and an array of two,
      and two arrays in one signature are two lengths. A **field** or a
      **result** has no call, so `Array[T]` there is `NK1182` and the message
      names all three ways out — `Vec[T]` to own and grow, `ref Array[T]` to
      view, `Array[T, N]` to write the length down.

**A number may carry a digit separator, a radix prefix or a scale, and nothing
else.** `1_000_000`, `0xFF`, `0b1010` and `0o17` are the four forms, beside the
exponent above. A decimal whole number may end in a **scale**: `K`, `M`, `G`,
`T` and `P` are 10³ to 10¹⁵, `Ki`, `Mi`, `Gi`, `Ti` and `Pi` are 2¹⁰ to 2⁵⁰
([ADR-322](adr/adr-322.md)). `1G` is `1000000000` and `4Gi` is `4294967296`. A
radix-prefixed number and a float take no scale. An underscore stands **between** digits and nowhere else. A
float takes the separator (`1_000.5`) and no prefix. A digit the radix does not
have (`0b1210`, `0o19`) is refused. A **leading** underscore is not one of the
four forms: `_000` is a name.

**The radix is a spelling, and a literal is a value.** `0xFF` is `255`, and it
takes the first type that holds it exactly as `255` does. So `let mask = 0xFF`
is an `i32` and `let mask: u8 = 0xFF` is a `u8`. The width is never read off
the digits. The separator and the scale are not part of the value: `1_000` is `1000` and
`1K` is `1000` to the checker, to the ledger and to a diagnostic's text.

**There is no type suffix.** `1i64` is a number beside a name, and the name
beside it is refused with `NK1117` because nothing declares it (Part III, C.3).
A word that stands on its own is read as a name, so `assert c` is refused the
same way.

**A number too wide for a `u64` is refused** (Part III C.1), and one above
an `i64` is a `u64`'s: `let basis: u64 = 0xcbf29ce484222325`.

**These five are the integer types a program writes:** `i64`, `i32`, `u8`,
`u64` and `u32`. The specification does not offer others (`usize`, `i8`). A **length** is an `i64`:
`for i in 0..<xs.len()` gives an `i64`, `xs[i]` takes one, and neither
conversion is written, because the compiler emits both. A negative index
reports as an access out of bounds (Part III, A.2).

**An integer that does not fit aborts, at every build.** An `i32` holds what an
`i32` holds. An arithmetic result that does not fit is an inconsistent program
state, like an index past the end of a list or a division by zero (Part III,
A.2), and the program stops. The rule is the same at both values of
`user_parallelism` and in every build (1.2).

Three cases abort although the code looks harmless:

* `-x` and `x.abs()` on the **smallest** value of a signed type, because there is
  no matching positive one;
* the smallest value divided by `-1`, for the same reason;
* shifting by more places than the type is wide.

**Where a program means to wrap or to stop at the limit, it says so by name.**

```nika
let h = h.wrapping_mul(31).wrapping_add(c)        // a hash, wrapping on purpose
let volume = level.saturating_add(increase)       // stops at the maximum
let n = a + b                                     // aborts if it does not fit
```

The names are `wrapping_add`, `wrapping_sub`, `wrapping_mul`, `wrapping_div`,
`wrapping_neg`, `wrapping_abs`, `wrapping_shl`, `wrapping_shr`, and the same
names with `saturating_`, except that there is no `saturating_shl` and no
`saturating_shr`. They exist for `i32`, `i64`, `u32` and `u64`; an unsigned
type has no `_neg` and no `_abs`. There is no operator for either.

**The bits of an integer are reached with operators**
([ADR-285](adr/adr-285.md)): `&`, `|`, `^`, `<<` and `>>`, and `!`, which on
an integer flips every bit. Both sides are one integer type, except a shift's
count, which may be any. A shift loses the bits it moves out, and a count as
large as the type is wide aborts; `>>` fills with zeros on an unsigned type and
with the sign on a signed one. They bind tighter than a comparison, and one
written beside a comparison is written in parentheses (`NK1197`), because C
reads `a & mask == 0` the other way:

```nika
fn fnv(key: ref String) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325
    for b in key.bytes() {
        hash ^= b as u64
        hash = hash.wrapping_mul(0x100000001b3)
    }
    return hash
}

fn main() {
    let h = fnv("hello")
    println(f"{h >> 32} {(h & 0xff) == 0x0b}")
}
```

On a `bool` they are refused (`NK1198`): `&&` and `||` join two `bool`s.

**One integer type becomes another without `as` where no value can be lost**
([ADR-285](adr/adr-285.md) D32). Two integer types have a **common type** when
one type holds every value of both:

| | `u8` | `u32` | `u64` | `i32` | `i64` |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `u8` | `u8` | `u32` | `u64` | `i32` | `i64` |
| `u32` | | `u32` | `u64` | `i64` | `i64` |
| `u64` | | | `u64` | none | none |
| `i32` | | | | `i32` | `i64` |
| `i64` | | | | | `i64` |

* A place whose type is stated takes a value of a type it holds: a `let` with
  an annotation, an argument, a `return` or a body's last expression, a field
  in a struct literal, an assignment, an element of a list whose element type
  is stated.
* An operator over two integer types computes in their common type, and a list
  literal of two integer types holds their common type. Where there is none,
  the operator is refused (`NK1199`) and so is the list (`NK1154`).
* A comparison of two integer types compares the numbers, for every pair.
* A list that already exists is not converted: a `Vec[u32]` is not a
  `Vec[i64]`.

```nika
fn area(x: i32, y: i32) -> i64 {
    x * y                       // computed in i64
}

let total = count + offset      // u32 + i64 is an i64
let n: u64 = xs.len()           // a length is never negative
```

**A stated type reaches through `+`, `-`, `*`, `/`, `%` and a negation to the
operands**, each widened before the operation: `x * y` above is computed in
`i64`, and does not stop where an `i32` would. It does not reach into what a
name was given, into a call's arguments, a method's receiver or an index. It
stops at `&`, `|`, `^`, `!`, `<<` and `>>` and at the `wrapping_`,
`saturating_` and `truncating_` names, whose result depends on the width: a
computation of them in a narrower type, put where a wider one is stated, is
refused (`NK1215`), and the program writes `(a as i64) << 3` or
`(a << 3) as i64`.

**A value the compiler shows is not negative goes into an unsigned type
without `as`**: a length, and a value after `if k >= 0` or `assert k >= 0`.
Where nothing shows it, the conversion is `k as u64`, which aborts on a
negative value.

**Widening plays no part in deciding a type.** A number without an annotation
(2.4) and a type parameter (4.6) take the type their uses ask for, and two uses
asking two types are refused (`NK1200`) though one would widen into the other.
`pick(a, b)` for `fn pick[T](a: T, b: T)` over an `i32` and an `i64` is
refused; `pick(a as i64, b)` is not.

**A conversion is written `as`, and one that may not fit aborts too.**

```nika
let average = (total as f64) / (count as f64)     // widening, always fits
let small = big as i32                            // aborts if `big` does not fit
```

The check does not depend on a build option: `5000000000 as i32` aborts in
every build.

**Where a program means to keep only the low digits, it says so by name**, as
it does for wrapping. The name carries the type it converts to:

```nika
let small = big.truncating_i32()                  // keep the low digits, on purpose
let floored = measurement.truncating_i32()        // a float, toward zero, clamped
let n = text.bytes.len().truncating_i32()         // a count that may not fit
```

The names are `truncating_i32`, `truncating_i64`, `truncating_u32` and
`truncating_u64`. Each converts out of a wider integer and out of an `f64`.

**Where a program means to ask whether it fits, it says so by name too**
([ADR-315](adr/adr-315.md)), and the answer is optional:

```nika
let sum = a.checked_add(b) ?? return null         // null where it does not fit
let small = big.checked_i32() ?? 0                // null where an i32 does not hold it
```

The arithmetic names are `checked_add`, `checked_sub`, `checked_mul`,
`checked_div`, `checked_rem`, `checked_neg` and `checked_abs`, for the four
types the wrapping names have and without `_neg` and `_abs` on an unsigned one;
a division by zero is `null` too. The conversions are `checked_i32`,
`checked_i64`, `checked_u32` and `checked_u64`, out of an integer type that has
values the destination lacks.

Two conversions are checked or unchecked in a way the code does not show:

* **An `f64` to an integer** aborts where the value does not fit: `1e20 as i32`,
  `-1e20 as i32`, and a value that is not a number all abort.
* **An integer to an `f64`** is **not** checked. Digits are lost at large values
  without anything overflowing: `9007199254740993` through an `f64` comes back
  `9007199254740992`. This is a limit of the language, not an abort.

**An `as` names one of the types above and nothing else.** `n as u128` and
`n as usize` are refused with `NK1122`, naming the type.

**Where the backend wants a machine-width number, the compiler writes the
conversion.** A length comes back as an `i64` and an index goes in as one. A
**count** goes in as one too, so `"  ".repeat(indent)` is written with no
conversion. A negative count aborts with *"a count cannot be negative"*.

**A literal that does not fit its type is a compile error, not an abort.**
`let x: i32 = 3000000000` is refused with `NK1116`, and so is a sum of literals
that cannot fit. `NK1116` fires wherever a type stands beside the constant: an
annotated `let`, a `return` against a declared result, or an argument whose
parameter says what it takes.

The compile-time check reaches a **sum**. `let b = a + 1`, where `a` is a
constant an annotation declared an `i32`, is folded and refused with no
annotation on the `let` line: the operand's declaration gives the arithmetic
its type. `+ - * / %` and a negation fold, through any number of immutable
`let`s, in a magnitude and a sign (65 bits), each step held to the type a declared operand gives it, so that the message can name what
the expression comes to. A `mut` local, a parameter, a `for` binding and a cast
stop the fold. An expression that does not fold is never refused.

A division whose divisor is a constant zero is refused with `NK1118`. Every
other division by zero is unrecoverable at run time and names its line
(Part III A.2).

A literal that nothing constrains, `let big = 3000000000` on its own, is not
refused: it takes the first type that holds it (2.4).

### 2.3. Nullable Types (Null Safety)
A type is **non-nullable** unless it says otherwise. A variable of type
`String` always holds a string and is never `null`. A type that admits the
absence of a value is written with a trailing question mark `?`.

```nika
let strictly_string: String = "Hello"
// strictly_string = null // Error!

let mut maybe_string: ref String? = null // Valid
maybe_string = "World"             // Valid (`mut`, as in 2.1)
```

A literal is a **view** of text the program was compiled with (6.6). A literal
stands wherever a `String` is wanted: where the use **keeps** it — a field, an
annotated `let`, a `return`, an argument the callee keeps — it is constructed
there, as `[1, 2]` is a `Vec` where one is wanted; where the use only **reads**
it, nothing is allocated. A **view** of text the program *has* is not a literal,
and where it is kept `.clone()` makes the copy, written where it happens. A view
in a published `String` slot that text of its own also flows into is refused without it (`NK1106`); any other `String` slot takes the view (2.2). **`.clone()` is the one
word for a copy**, of text and of everything else: a copy of text is text of its
own. `.to_owned()` is refused naming it (`NK1189`); `.to_string()` is the text
form of a value, and the text form of text is the text itself: a view stays a
view, and nothing is copied.

**`T?` lowers to the backend's `Option<T>`**, the mapping Part III 15.2 writes
the other way round, and `null` is a reserved word (2.1) that lowers to `None`.
The `?` comes last, after the type's arguments: `Vec[i64]?` is a nullable list
and `Vec[i64?]` is a list of nullables. `(A, B)?` is not written.

**A plain value stands where a nullable one is wanted.** That is the one
widening the language has, and the compiler writes the constructor. There is no
`Some` in Nikaia. The constructor is written wherever a plain value meets a
nullable slot: an annotated `let`, an assignment, a `return`, a struct-literal
field, and a call argument. Where the compiler cannot work out the value's type,
it writes a conversion instead, which is right whether or not the value is
already nullable.

`let m = null` with nothing beside it is not refused by the compiler:
`let mut m = null` followed by `m = "hi"` is a correct program. Where nothing
ever says the type, the backend asks for the annotation.

### 2.4. Type Inference
Nikaia is **statically typed**: the type of every variable is known at compile
time. A type is rarely written. The compiler uses **type inference** to deduce
the type from the value.

```nika
let name = "Nikaia"  // Compiler knows this is a ref String - a view of static text
let count = 42       // an i32, because nothing here asks for anything else
```

**A number takes the type its use asks for. Where nothing asks, it takes the
first type that holds it** — `i32`, and `i64` where an `i32` is too small:

```nika
let count = 42          // an i32: nothing asks, and an i32 holds it
let big = 3000000000    // an i64: nothing asks, and an i32 does not
let m = 3000000000      // also an i64 - here because the line below asks
println(f"{wide(m)}")   // fn wide(n: i64) -> i64
let small = 42
println(f"{wide(small)}")  // an i64 here: the use decides, and 42 holds in one
```

The use is asked first and the size second. So `small` may still become an
`i64`, and `big` never has to be annotated to be one. An integer `comptime` is
such a number whatever computed it, and each use may take it in its own type
(Part II 10.2). A number too large for an
`i64` is refused unless a use asks for a `u64`.

**A use is anything the function does with the name**: the parameter it is
handed to, a typed value it meets in an operation or a comparison, an annotated
`let`, a field or a `return` it goes into, a value assigned to it or from it,
and an index, which asks for an `i64` only where nothing else asks. The compiler
reads every use in the function and writes down the type it decides, among all
five integer types. Two uses that ask for two types are refused (`NK1200`), with
the annotation to write. Where no use asks, what the name is given decides:
`let mut n = 1` followed by `n = 3000000000` is an `i64`, and a value it is
given that the decided type does not hold is `NK1116`, naming the use.

**A sum of numbers is a number**, so the same rule decides it:

```nika
let c = 2000000000 + 2000000000   // an i64: nothing asks, and an i32 does not hold it
let small = 2 + 3                 // an i32, the same as any number that fits
```

**A name is where the widening stops.** A name already took a type, and
arithmetic happens in the type of its operands where no wider type is stated
for it (2.2):

```nika
let a = 2000000000        // an i32 - the first type that holds it
let c = a + a             // refused: this comes to 4000000000, which an i32 does not hold
let b: i64 = 2000000000   // say so, and the sum is an i64
let d = b + b             // 4000000000
```

The way out is the annotation on the third line.

The question is about the **value**, not the digits: `-2147483648` is an `i32`.
`NK1116` answers the case where a type **stands beside** the literal (an
annotated `let`, a `return` against a declared result, an argument whose
parameter says what it takes) and is silent otherwise (Part III, C.4). An
operation over names whose type is pinned is asked wherever it stands - in an
`f"…"` hole, a condition, a list - and refused once, at the innermost operation
that overflows. A
constant is widened where an `i64` holds it and refused with `NK1116` where no
type does or where a name pinned a narrower one. Two positions take their type
from **where they stand** and are left alone: a sequence index and a repeat
count, both counted by the machine.

### 2.5. Strings, Plain and Interpolated
There are two string literals, and the difference is one character at the front.

```nika
let name = "Nikaia"

println(f"hello, {name} - {name.to_uppercase()}")   // f"…" - the braces are code
println("hello, {name}")                              // "…"  - the braces are braces
```

**`"…"` is text.** A `{` in it is a brace and nothing else, so a program that
writes JSON, CSS or a regular expression writes the braces as they are:

```nika
print("{}")               // prints {}
print("\\d{3}")            // a regular expression, written as one
print("{ margin: 0 }")    // a rule, not a hole
```

**`f"…"` has code in it.** Between `{` and `}` stands an expression: a name, a
field, a call, an index. It is written as code is written anywhere else, so a
string inside a hole has plain quotes: `f"{greet("Ada")}"`. What follows a `:` inside a hole says *how* to write
the value rather than which value. The first colon that is not inside a call or
an index separates the two, so `move(by: 1)` in a hole keeps its own colon.

Inside an `f"…"`, two braces stand for one: `{{` is a literal `{` and `}}` a
literal `}`. An escape is not a hole: the `{` in `"\u{0041}"` belongs to the
escape. A `}` on its own is an error. **A plain string needs none of that**: it
has no holes, so `"{"` is a brace and `"{{"` is two.

The `f` and the quote are **one token**. `f"x"` interpolates; `f "x"` is a
variable named `f` beside a string. Whitespace never decides what a program
means. An `f"…"` is not a constant: it cannot stand as a default, a `match`
pattern or a grammar's literal. Inside a hole, `\"` is refused - a hole is
code, and a string in it has plain quotes.

**A newline is an ordinary character in a literal.** Nothing ends a literal but
the closing `"`. A literal left unterminated runs on to the next `"` in the file
or, if there is none, to the end of the file.

**A `\` introduces an escape, and the set is closed:**

| written | what it is |
| :--- | :--- |
| `\n` `\r` `\t` | newline, carriage return, tab |
| `\0` | the zero character |
| `\\` `\'` `\"` | the backslash and the two quotes, standing for themselves |
| `\xNN` | two hexadecimal digits, naming a byte: below `\x80` in text, any byte in a `'…'` that is a `u8` |
| `\u{…}` | up to six hexadecimal digits, naming any character: `\u{1F600}` |

The same set holds in a `'…'`, in an `f"…"` and in a template. A `\` in front of
anything else is refused (`NK1184`), naming the set; a literal backslash is
written `\\`. A character above `\x7F` is `\u{…}`. The set is the backend's,
and a literal is written into the generated file as it stands.

**The type follows the syntax and the use, never the contents.** `"…"` is a
view of static text and `f"…"` builds a `String`, whether or not it has a hole
in it. Adding a brace to a piece of text cannot change its type. Where the use
asks for an `fs::Path`, an `f"…"` builds one, and its holes may be a path, text
or a number (Part III 17.1); a `let` without a type takes it from the binding's
first use. A path in a hole of an `f"…"` that is text is refused (`NK1201`): the text is
written `p.to_text()` or `p.display()`. Handed to `print` or `println`, a path
in a hole is written as its bytes are.

**A template's holes need no `f`.** `dsl html { <p>{name}</p> } eod` (Part II)
marks the construct as a place where code appears.

### 2.6. Text: Bytes, Scalars and Graphemes

Text is UTF-8, and a position in it is a byte position
([ADR-320](adr/adr-320.md)). A text is read through a **view** that names what
it counts:

| view | an element | reads |
| :--- | :--- | :--- |
| `s.bytes` | `u8` | `s.bytes[i]`, `s.bytes[a..<b]`, `s.bytes.len()`, `s.bytes.find(…)`, `for` |
| `s.scalars` | `scalar` | `take(n)`, `skip(n)`, `count()`, `for` |
| `s.graphemes` | `ref String`, one grapheme | `take(n)`, `skip(n)`, `count()`, `for` |

There is no `s.len()` and no `s[i]`. `s.is_empty()` needs no view. `split`,
`lines`, `trim`, `starts_with`, `ends_with`, `contains`, `==` and `find` work
on the text itself.

```nika
let preview = title.graphemes.take(10).collect()   // ten things a reader sees
let field = record.bytes[0..<8]                     // eight bytes of a fixed format
for c in name.scalars { if c == 'ä' { … } }
```

**A position the text hands out cuts it without a view.** `s.find(needle; from:
i64) -> i64?` gives the byte position of the first match at or after `from`
(default `0`), or `null`. That position, a grammar's match in the text, and
either moved by the byte length of a text literal, cut the text they came from:

```nika
let sep = line.find(";") ?? return
let name = line[..<sep]
let rest = line[sep + 1..]
```

A number the program makes up does not: `s[0..<10]` is refused (`NK1214`) and
says the view - `s.bytes[0..<10].text()` for bytes, `s.graphemes.take(10)` for
what a reader sees. So is a position from one text used on another. A cut inside
a scalar stops the program.

**A `Bytes` is read as a run of bytes:** `b[i] -> u8`, `b[a..<b]`, `b.len()`,
`b.find(byte; from: i64) -> i64?`, and `b.text()`, which checks UTF-8 and is
`NotText` where it fails. An index outside stops the program, as a list's does.

**A `'…'` is a `u8` or a `scalar`, by its use.** One ASCII character is either;
`'\xNN'` above `\x7F` is only a `u8`; one scalar that is not ASCII is only a
`scalar`. A literal its uses do not type is a `scalar`. A surrogate is refused.
Several scalars that normalise (NFC) to one are that one, with the warning
`NK1213`; anything else is refused with `NK1212`, which hands over the text
literal: `"👍🏽"`.

**A grapheme is the standard library's.** `s.graphemes` follows Unicode's
segmentation rules in the version of the `std` the program is built with. A
grapheme is a `ref String`, compared with a text literal.

**`==` compares bytes, and so does a hash.** A precomposed and a decomposed `ä`
differ. `s.normalized(; form: nfc)` - or `nfd`, `nfkc`, `nfkd` - makes a text
that compares as its form says.

---

## Chapter 3: Control Flow

Control flow is the order in which statements and calls are executed.

### 3.1. Expressions and Blocks
Nikaia is an **expression-oriented language**: almost every construct has a
value. A **block** is a group of statements surrounded by curly braces
`{ ... }`. The last line of a block is the value of that block.

```nika
let result = {
    let a = 5
    let b = 10
    a + b  // Returns 15
}
```

A block's last line is the **block's** value; `return` is the **function's**.
A `return` written at the end of a block that is itself a value (a `match` arm,
3.4; an `if` branch whose value is taken, 3.2; a `catch` handler, 7.1) leaves
the enclosing function rather than handing that block a value. The one block
that is its own function is a lambda, whose `return` leaves the lambda (5.3).

### 3.2. Conditional Logic (if / else)
The `if` expression tests a condition of type `bool`. Where the condition is
true it evaluates the first block; otherwise it evaluates the `else` block.
`if` is an expression, so its value may be bound to a variable.

```nika
let age = 18

let status = if age >= 18 {
    "Adult"
} else {
    "Minor"
}
```

After `else`, an `if` may stand where the block would stand: **`else if`**. A
chain is one `if` inside another with the inner braces left out, so every rule
of `if` holds at every link:

```nika
let grade = if score >= 90 {
    "A"
} else if score >= 80 {
    "B"
} else {
    "C"
}
```

**A condition is an ordinary expression.** Every expression the language has
may stand there, with the same operators, the same precedence and the same
associativity as anywhere else. `&&`, `||`, `!`, `??`, `as`, `null`, a tuple and
a range are all conditions.

**One rule concerns the brace.** The `{` after the condition opens the
**body**, so an expression that *starts* with a brace is parenthesised:

```nika
if p == (P { x: 1 }) {          // the parentheses say which `{` is which
    …
}

if (match n { 1 => 10, else => 20 }) > 15 {
    …
}
```

That is the whole of the difference between the head of an `if`, a `while` or a
`for` and any other position. Every expression is reachable.

**A jump may carry its condition after it** ([ADR-276](adr/adr-276.md)).
`return`, `throw`, `break` and `continue` take an `if` on their own line, and
the line means `if cond { jump }`:

```nika
fn limit(speed: i64) -> i64 {
    return 250 if speed > 250
    return speed
}

fn first_even(numbers: Vec[i64]) -> i64 {
    for n in numbers {
        continue if n % 2 != 0
        return n
    }
    return 0 - 1
}
```

`throw TooFast(speed) if speed > 250` reads the same way. Nothing but a jump is
guarded like this, and an `if` that begins a line is a statement of its own.
`return if c { a } else { b }` is still a `return` of an `if` expression.

### 3.3. Loops
A loop repeats code.

**The `while` Loop**
A `while` loop repeats its body as long as its condition is true.

```nika
let mut count = 0
while count < 5 {
    println(f"Count is {count}")
    count += 1
}
```

**The `for` Loop**
A `for` loop iterates over a sequence, such as a range of numbers or a list.

```nika
// Iterates from 0 to 4: `..<` stops before its end.
for i in 0..<5 {
    println(f"Index: {i}")
}
```

**`a..b` includes its end and `a..<b` excludes it.** The spelling is the same
in a `for`, in a slice and in a pattern. `a..=b` is refused, naming the form
that replaces it. A range is an ordinary expression: it may be given a name,
passed, or indexed with. A range binds **looser than every operator in it**, so
`0..<n - 1` is a range ending below `n - 1`, not a range with something
subtracted from it.

**A range is a value, and walking it does not use it up.** A range kept in a
name is walked as often as a program likes, from either end, and in steps:
after `let days = 0..<n`, both `for d in days` and
`for d in days.step_by(7).rev()` walk it, the second every seventh day counted
back from the last. A sequence something **produces** — `keys()`,
`io::lines()`, `xs.iter().map(…)` — is walked once, and a second walk is refused
(`NK2702`, Part III C.3). Whether one can be walked from its back end is known
to the compiler: a range, a list's `iter()`, `scalars` and a `map` over any of
them can; `io::lines()` cannot, and `rev()` on it is refused. A range in a
list's brackets reads a **run** of it, `ref Array[T]`, whether the range is
written there or kept in a name.

A `for` over a list **lends** it: the elements are looked at, and the list is
still there when the loop is over. A binding is a view of its element, so an
element kept past the turn, text in a field for instance, is kept as a copy,
`.clone()`; a number, a `bool` and a `scalar` are the element itself. A map is
walked as pairs, `for (key, value) in scores`. Taking the elements away is written,
`for x in xs.drain()` (6.5). A `ref` written in front of the list is refused with
`NK1137`, because the compiler writes that reference.

**Leaving a loop early: `break` and `continue`**

`break` leaves the loop. `continue` skips the rest of this turn and starts the
next one. Both act on the **innermost** loop around them:

```nika
let mut first_even = 0 - 1
for n in numbers {
    if n % 2 != 0 {
        continue                 // not this one; take the next
    }
    first_even = n
    break                        // found it; stop looking
}
println(f"{first_even}")         // the loop is over, the program is not
```

A `break` leaves the **loop** and a `return` leaves the **function**. After the
`break` above, the `println` runs.

Three rules govern them:

* **Neither takes a value.** A loop is a statement and hands back nothing.
  `break n` is refused rather than read as a `break` followed by a statement
  `n`:

  ```
  error[NK1133]: Nothing after `break` in the same block can run.
  ```

* **There is no label.** `break` acts on the loop it is written in, and there
  is no way to name an outer one. A program that leaves two loops at once
  leaves the inner one and tests outside it, or `return`s where the function
  has nothing left to do.

* **A jump does not leave a function.** A `break` whose loop is outside a
  **lambda**, a **task** (`spawn`), an **`overlap` branch** or a DSL fold's step
  is refused. Each of those is a function of its own, and a jump is a jump to a
  place in the same function:

  ```nika
  for x in xs {
      let each = fn (n) {
          break                  // error[NK1132]: the nearest loop is
      }                          //               outside this lambda
  }
  ```

  Such a lambda hands back a `bool`, and the loop tests it. A `catch` handler
  is **not** a function of its own: a `break` in one leaves the loop around it:

  ```nika
  for p in paths {
      let text = fs::read_to_string(p, fs::Root::Anywhere) catch { break }
      seen += text.bytes.len()
  }
  ```

**A loop can fail.** Some sequences a `for` walks are read *as it goes*, such
as standard input's lines. Getting the next element can fail. When it does, the
loop stops and **the failure leaves the function**, exactly as a failing call
would (Chapter 7). The function declares it:

```nika
use std::io

fn tally() -> i64 throws {           // without `throws`: error[NK2701]
    let mut n = 0
    for line in io::lines() { n += 1 }
    return n
}
```

Nothing marks the loop, as nothing marks a call that can fail. The same rule
applies at the *end* of a block, where a resource's cleanup can fail and the
function that owns it declares it (6.4).

Getting the next element can also **pause**, and nothing marks that either. A
loop over standard input gives its thread up between lines. The library says
which sequences are like that; a program writes `for`.

Most loops cannot fail. A range, a list, a map: nothing is read.

**There is no third loop form.** A loop that does not end on its own is written
`while true { … }`, and it is left by a `break`:

```nika
let mut n = 0
while true {
    n += 1
    if n == 5 {
        break
    }
}
```

There is no `loop` keyword. `break` hands back nothing. A function whose last statement is a `while true` that no `break` leaves needs no `return` after it, whatever type it declares: it never reaches its end ([ADR-276](adr/adr-276.md) D6).

### 3.4. Pattern Matching (`match`)
The `match` expression compares a value against a series of patterns. Every
possible case is handled.

```nika
let value = 2

match value {
    1 => println("One"),
    2 => println("Two"),
    else => println("Something else"), // `else` is the arm taken when nothing above matched
}
```

A pattern is one of six things, and each is read the way it is written:

| pattern | matches |
| :--- | :--- |
| `else` | anything, and binds nothing: the arm taken when none above it matched. `_` in this position is refused: it is the **ignore pattern**, which stands in a tuple position, as a parameter and as a part of a pattern, `(0, _)` |
| `1`, `"text"`, `true`, `'n'` | that value |
| `Op::Times` | that variant |
| `Message::Write(text)` | that variant, binding what it carries |
| `Message::Move { x, y }` | that variant, binding its fields by name |
| `other` | anything, and **binds it** to that name |

The last two rows are one rule: a path with `::` in it names a variant, and a
bare name binds. `Quit` is a variant *of* `Message`, never on its own.

**Six more shapes**:

| pattern | matches |
| :--- | :--- |
| `(0, 0)`, `(0, y)` | a tuple, position by position |
| `(0, y) \| (y, 0)` | either alternative; every alternative binds the **same set of names**, and alternatives that bind different names are refused with `NK1155` |
| `200..299` | a range, **inclusive at both ends**. An exclusive one is written by moving the end; `..<` is never written in a pattern |
| `(x, y) if x == y` | a **guard**: the arm matches only where the condition holds, and the word is `if` |
| `Event::Click(Point { x, .. })` | a pattern inside a pattern, and `..` for the fields this one does not name |

**`..` means two different things**: in a range pattern it is the range, and
in a struct pattern it is *the rest of the fields*. The position says which.

**A `match` reads what it binds.** Over a name the function owns, an arm that
only reads a part it binds takes a view of it (6.5), and the name is whole
after the `match`: it may be matched again, printed, or handed on. An arm that
keeps the part — pushes it into a list, returns it, hands it back as the arm's
value, changes it — takes it, and the name is gone afterwards, as it is after
any hand-over. A number, a `bool` and a `scalar` are copied either way.

**Every case is covered.** A `match` over an **enum** is complete when every
variant is named (an or-pattern names each variant in it), and needs no `else`. A `match` over anything else needs an
`else`, or a bare name, which catches everything. `bool` is the exception: `true` and `false` are two arms and a complete
`match`. An incomplete `match` is refused with `NK1151`, which names what is
missing: the variants, or `else`. A guarded arm covers nothing, so a `match`
whose only catch-all carries an `if` is refused with `NK1151`.

An arm's body is an expression or a block. The expression may be a `throw`, a
`return`, a `break`, a `continue` or a `panic(…)`. Their type is **never**, so
an arm that throws sits beside an arm that hands back a value, and the `match`
is that value's type:

```nika
match step {
    (Op::Times, n)  => { value = value * n }
    (Op::Divide, n) => { value = value / n }
}
```

### 3.5. Null Safety Operators
A member of a nullable type is reached through an operator that handles the
`null` case.

* **Safe navigation (`?.`):** reaches a member only where the receiver is not
  `null`. Where the receiver is `null`, the expression is `null`. A **field**
  and a **method** are both members, and a method call takes its arguments
  there as it does anywhere.
* **Null coalescing (`??`):** supplies a fallback value where an expression is
  `null`. A chain may be written: `a ?? b ?? c` takes the first that has a
  value. Where the last fallback may be `null` too, so may the answer: `a ?? b`
  with a `b: T?` is a `T?`. The fallback is one value - a literal, a name, a
  call, a field, a jump, or an expression in brackets - so `a ?? 0 > 3` is
  refused and `(a ?? 0) > 3` is written. There is no postfix `??`: `a??` is
  refused, naming `a ?? b`, a jump and `a ?? panic("…")`
  ([ADR-279](adr/adr-279.md) D1, D3).

```nika
// If find_user returns null, 'name' becomes null.
let name = repo.find_user(id)?.full_name

// A method is a member, so it is reached the same way.
let greeting = repo.find_user(id)?.greet("Hallo")

// If 'name' is null, "Guest" is assigned.
let display_name = name ?? "Guest"

// And a fallback may have a fallback.
let shown = nickname ?? name ?? "Guest"
```

**The result of a `?.` is a `T?`.** Where the member is itself a `T?`, the
result is flattened: `a?.b?.c` never reaches through a nullable of a nullable.
A read of a map whose values are `T?` is flattened the same way (4.5).
The compiler decides which case applies from the declared type.

**`?.` or `??` after a type that cannot be absent is a warning.** A type that
is not `T?` always has a value: `s?.len()` is `s.len()`, with the member's type
and the warning `NK1217`; `s ?? "b"` is `s`, its fallback never runs, and the
warning is `NK1216`.

**After a list's index they are refused.** A list's index is a `T` (4.5), so
`xs[i] ?? 0` over a `Vec[i64]` is `NK1211` and `users[i]?.name` over a
`Vec[User]` is `NK1121`: one past the end stops the program, and neither
operator guards an index. Over a `Vec[i64?]` the index is an `i64?`, and `??`
replaces a `null` element.

**`?.` takes nothing.** It reaches through a view of its receiver, so `user` is
usable on the line after `user?.name`. What comes out is a copy where the
member copies and a view of the receiver otherwise, as a field read is (6.6).
A receiver that is a call's result is a temporary: a field reached through it
is taken out of it, and a method that hands back a view of it keeps it, to the
end of the block, in a binding the compiler writes on the line before. Where
that would change what runs (on the lazy side of `??`, `&&` or `||`, in a
`match` arm, after another call in the same statement) or where the view is
the block's value or the result, the line is refused, naming the `let` to
write ([ADR-278](adr/adr-278.md) D22).

**`??` takes its left side where the answer is kept, and lends it where the
answer is only read** ([ADR-279](adr/adr-279.md) D5), as an argument is lent or
handed over (6.6). `println(user ?? "Guest")` leaves `user` usable on the next
line; `let shown = user ?? "Guest"` takes `user`, as `let b = a` takes `a`.
The positions that only read are an argument the function only reads, an
`f"…"` hole, a comparison of text, and the receiver of a method that only
reads it. A value that is a word by its kind - a number, a `bool`, a `scalar`, an
enum whose variants hold nothing - is copied at a read rather than lent; every
other type is lent, at every size ([ADR-279](adr/adr-279.md) D8).

**A left side that is itself only borrowed, beside a fallback that jumps, is
a view wherever it stands** ([ADR-279](adr/adr-279.md) D10): over a `c: ref
Contract`, `let s = c.signature ?? return false` binds a view of the field.
Nothing can take a part of a loan, and a `return`, `throw`, `continue` or
`break` has no value of its own, so the answer can only be the left side,
lent. Kept past the loan - returned, stored, pushed - the view is refused, and
`.clone()` is the copy.

**`?.` reaches a method.** `find(1)?.greet("Hallo")` calls the method only
where there is something to call it on; the arguments reach it, and the result
is a `T?` like any other reach. It flattens where the method's own result is
already a `T?`. A method may **pause** and may **fail**, and a `?.` on one
carries both.

**A `?.` guards its own member and no more.** Where the receiver is absent, the
whole expression is `null` and what follows is never reached. A `.` written
*after* the reach is reaching into a `T?`, and is refused where it is written
with `NK1125`: `T?` is a type of its own (2.3), and a member of `T` is not a
member of it. So `a?.b.c` is refused and `a?.b?.c` is the program.

---

## Chapter 4: Data Structures

### 4.1. Structs (Custom Data Types)
A **struct** groups related values under a single name.

**Visibility and Encapsulation**
**Everything is private unless it says `pub`.** That includes a struct and its
fields.
* A struct another package may use is marked `pub`.
* The fields of a public struct stay private unless each is marked `pub`.

```nika
// file: users.nika

// The Struct is public, but fields are private
pub struct User {
    username: String,
    email: String,
    is_active: bool,
}
```

**A field may have a default**, a build-time value written after its type
([ADR-331](adr/adr-331.md) D5, [ADR-318](adr/adr-318.md)):

```nika
pub struct Page {
    pub size: i64 = 50,
    pub cursor: String?,
}
```

A struct literal may leave such a field out, and the field takes its default:
`Page { cursor: null }` has a `size` of 50. A literal that leaves out a field
without a default is refused with `NK1234`. The default is evaluated once,
while the package that declares the struct is built.

### 4.2. Constructors and Instantiation
A struct literal `Type { field: value }` names fields, and a private field
cannot be named from another package. A type another package constructs
provides a public **constructor**.

**The Anonymous Constructor (`pub fn`)**
An `impl` block may declare one function with no name. That function is called
when the type name is invoked like a function: `User(...)`.

* **Inside the package:** code builds the value with the struct literal, because
  it may name the private fields.
* **Outside the package:** code calls the public anonymous constructor.

```nika
// file: users.nika
impl User {
    // The Constructor
    // It accepts positional arguments (Subject Zone)
    pub fn(username: String, email: String) -> User {
        // We can access private fields here because we are inside the module
        return User {
            username: username,
            email: email,
            is_active: true, // Default logic handled internally
        }
    }
}
```

**A struct literal is written with braces, and only with braces.**
`User { username: name, email: address }` is the literal. `User(a, b)` is a
**call**, of the anonymous constructor or of anything else. Where the name is a
type, `Type(field: value)` is refused with `NK1146`, naming the braces.

**A type is constructed by its anonymous constructor**, in `std` as in a
`.nika` file: `Vec()`, `String()`, `HashMap()`, `Stats(first)`. A written
`Type::new` is refused with `NK1149`, in a call and as a value alike; the
constructor handed over as a **value** is the same spelling,
`par_fold(M, Summary, …)`. The ledger writes `Type::new`, the name the lowering
uses.

**A copy with fields changed: `with`.** `with` writes a copy of a value with
named fields changed, without naming the rest:

```nika
let moved = p with { x: p.x + 1 }
let stats = old with { count: old.count + 1, sum: old.sum + t }
```

The braces are the struct literal's, with its field list and its shorthand.
`with` names top-level fields only: a field of a field is
`p with { pos: p.pos with { x: 1 } }`. The fields not named are **moved** from
`p`, never copied unseen. Where one of them is a text or a list and `p` is used
afterwards, the refusal names the copy to write. Across a package, `with` names
`pub` fields only, as a literal does.

A field the type does not have is `NK1107` and a private one `NK1110`, both the
literal's own refusals. A field named twice is `NK1172`, here and in a plain
literal. A `with` that names none is `NK1174`. `NK1173` refuses the operand: an
`enum`, a **view** (there is nothing to move out of one), a type with no
fields, or a value whose type the compiler did not work out.

**Usage Example**
Another package reaches `User` through its constructor.

```nika
// file: main.nika
use users

fn main() {
    // ERROR: Private Fields
    // Direct struct initialization is forbidden because fields are private.
    // let u = users::User { username: "A", email: "a@b.com", is_active: true }

    // OK: Public Factory Constructor
    // Calls the 'pub fn' defined in 'impl User'.
    // Note: Uses positional arguments as per Function Syntax.
    let u = users::User("Alice", "alice@example.com")
}
```

### 4.3. Why No Classes? (Data vs. Behavior)
Nikaia has no **classes**. Data and behavior are declared apart:
1.  A **struct** defines the **data**: what a value is.
2.  An **`impl` block** defines the **behavior**: what a value does.

```nika
// Defining behavior for the User struct
impl User {
    fn login(ref self) {
        println(f"{self.username} logged in.")
    }
}
```

### 4.4. Enums (Algebraic Data Types)
An **enum** is a type whose value is one of several distinct variants.

```nika
enum Message {
    Quit,                        // carries nothing
    Move { x: i32, y: i32 },     // named fields, read by name
    Write(String),               // positional, read by position
}
```

An enum is the type for a value that is **one of a fixed set of things**: two
operators, four directions, the three states a connection can be in. An enum is
read back with `match` (3.4): an arm per variant, and no arm for a case that
cannot happen.

**A type may hold itself.** A syntax tree is an enum whose variants hold more of
it, and a list is a node whose `next` is another node or nothing:

```nika
enum Expr {
    Num(i64),
    Add(Expr, Expr),
}

struct Node {
    value: i64,
    next: Node?,
}
```

The program writes the tree as the tree. A value that held a whole value of its
own type inline would be larger than itself, so the compiler keeps such a field
somewhere else and the program never names where: it is built, read, matched
and assigned as the type it declares (ADR-246). What that costs is one
allocation per such field built. A pattern cannot yet look *inside* such a part
(`Expr::Add(Expr::Num(n), b)`); a name binds it, and a `match` in the arm asks
the question.

### 4.5. Collections
The standard library provides types for groups of values.

* **List (Vector):** an ordered sequence of elements.
    ```nika
    let numbers = [1, 2, 3, 4]
    ```
    A list keeps the order it was given, and that order can be changed:
    `xs.sort()` puts the elements in their natural order, and
    `xs.sort_by_key fn(x) { … }` in the order of what the closure returns. **Both
    are stable**: elements the key does not separate keep the order they had.
    Two passes therefore express a compound order without a comparator:
    ```nika
    names.sort()                                  // by name
    names.sort_by_key fn (name) { -(report[name]?.hits ?? 0) }   // then by hits, descending
    ```
    A map has no order, so a program that prints one says which.
* **Tuple:** a fixed number of values of *different* types, with no name for
    the group and no names for the parts. It is written and read by position:
    ```nika
    let pair = ("*", 3)          // (ref String, i64)
    let op = pair.0
    ```
    A tuple is the type for values that belong together for one step of a
    computation: the element of a grammar rule that yields an operator and its
    operand, or the two halves of a map entry in `for (name, value) in map`.
    A group whose meaning outlives the step is a struct (4.1) with named
    fields.
* **Map (HashMap):** key-value pairs.
    ```nika
    use std::collections

    let mut scores = collections::HashMap()
    scores["Player1"] = 100
    ```
    Reading a map through the brackets gives a `T?`, because a key is data and
    may be absent: `scores["Player1"] ?? 0`, or `scores[name]?.rank ?? 0`.
    `get` gives the same. A list's `xs[i]` stays a `T`, and a wrong index
    aborts (Part III, Appendix A). A `+=` on a map slot is refused with the
    written-out form (`NK1162`).

    The value reached is a **view** of the map where it does not copy, and
    `m[k] ?? 0` on a map of numbers is the number. Kept where a `T?` of its
    own is wanted - `let k: Kind? = m[1]`, or `x = m[k] ?? return` into a
    name that owns its value - a value that copies is copied out, and one
    that does not is written `m[k]?.clone()`.

    **A map of `T?` values reads one `T?`**, as `?.` does (2.3): a key that
    is absent and a key that holds `null` both answer `null`, and
    `m.contains_key(k)` tells them apart. `m[k] = null` **stores** `null` - the
    key is there afterwards - and `m.remove(k)` removes it
    ([ADR-293](adr/adr-293.md)). A map whose keys it
    **owns** — a `HashMap[String, V]`, a `HashMap[i64, V]` — takes a key
    written into it as its own and is lent one it is read with; `m[k] ?? "-"`
    over a map of text is a view of text.

    A map's hash function follows where its keys came from. Keys derived from
    data a remote peer supplied are hashed with a random per-run key. Keys from
    data the program supplied are hashed with the fast function. User code does
    not configure this; Part III, 17.1 names the cases where a program decides
    it. **The iteration order of a map is not guaranteed** and may differ
    between runs.

**A list is written `[1, 2, 3]`.** The elements are expressions, a trailing
comma is allowed, and the type is `Vec[T]` where `T` is what the elements agree
on. Elements that do not agree are refused with `NK1154`. `[]` is the empty
list and **takes its element type from the first use that says one**:
`let xs: Vec[i64] = []`, or a `push`. Where nothing ever says the type, `[]` is
refused with `NK1153`, asking for the type. A `[` at the **start of a line**
begins a literal and never an index of the line above it, so an index is always
written where its subject is. The list *type* is written `Vec[T]`; there is no
second spelling `[User]`.

**A list has no operators**, and neither has a map or a set: `a + b` on two
lists is refused with `NK1191`. One list's elements join another with
`a.extend(b)`, which grows `a` in place ([ADR-253](adr/adr-253.md)).

A `[]` whose only uses cannot give it an element type (`xs.len()` and nothing
else) is not refused by the compiler, and the backend reports it
([Part III C.4](30-nikaia-tooling.md)).

**A fixed-size array is `Array[T, N]`**, in the bracket generic every other
parameterised type is written in; `N` is a number rather than a type, and it is
the one place a type argument is one. It is `N` elements **inline**: in a
struct it is part of the struct, as an argument it is passed as a value, and
nothing is allocated. It is indexed as a list is, an index out of range aborts
(Appendix A), and `len()` is the `N` it was declared with, known while the
program is built.

```nika
struct Vector3 { parts: Array[f64, 3] }

let origin: Array[f64, 3] = [0.0, 0.0, 0.0]
```

The literal is the list literal: **it takes the array type where the use asks
for one**, and a literal with no use to constrain it is a `Vec`. A use is an
annotated `let`, an argument, a declared result or a struct literal's field,
and the answer descends with the literal, so `Vec[Array[f64, 2]]` and
`Array[Array[i64, 2], 2]` take their shape too. The length is part of the type,
so `Array[f64, 3]` and `Array[f64, 4]` are different types and a literal whose
length does not match `N` is refused with `NK1157` naming both numbers.

### 4.6. Generics (Type Parameters)
A **generic** declaration is written once for several types. A type parameter is
declared inside square brackets `[...]`.

```nika
struct Box[T] {
    item: T,
}

impl Box[T] {
    fn get(self) -> T {
        return self.item
    }
}

fn hand[T](x: T) -> T {
    return x
}
```

A type parameter is written, never inferred. **Inside the body it is a type**:
`x` is a `T`, and a `T` is not an `i64`, because the caller picks what `T` is.
**At the call it is filled in from the arguments**: `hand(n)` where `n` is an
`i64` hands back an `i64`, and `Box { item: n }` is a `Box[i64]`. A parameter
list may hold the type itself, `fn schema[T](T)`; the call then writes a type
there, `schema(User)`, and the function needs no value of it (Part II 10.3).
A call may also name a function's type arguments in brackets, `parse[Config](text)`;
the types fill the type parameters in order, and the call is warned with
`NK1233` ([ADR-330](adr/adr-330.md) D9).

**A type may take values as well as types in its brackets** ([ADR-333](adr/adr-333.md)), as
`Array[f64, 3]` takes its length. A value parameter is marked `comptime` and written with its
type; those after a `;` are named at the use and have a default, as a function's options do (5.1):

```nika
pub struct Decimal[comptime precision: i64, comptime scale: i64; comptime rounding: Rounding = Rounding::HalfEven] { … }

let price: Decimal[10, 2] = …
```

An argument is a build-time value (Part II 10.2), refused with the codes a `comptime` is refused
with where it is not one. Two uses name the same type when their arguments are equal. Inside the
declaration a value parameter is a constant: `precision` is an `i64` known while the program is
built.

**A function's parameter may be `comptime` as well.** The call writes its argument as any other,
and the argument must be a build-time value; inside, the parameter is a constant, and the function
is emitted once per value:

```nika
pub fn get[F: Fn](self, comptime path: String, handler: F) -> Server { … }

server.get("/orders/{id}") fn(id: i64) { … }       // "/orders/{id}" is read while building
```

A caller that passes its own parameter on declares it `comptime` too.

A `T` on its own has no members:

```nika
fn shout[T](x: T) -> String {
    return x.to_uppercase()   // error[NK1126]: `T` stands for a type the caller
}                             // picks, and nothing says it has a method
                              // `to_uppercase`
```

A bound, `[T: Summarize]`, is 4.7's subject.

### 4.7. Traits (Defining Behavior)
A **trait** declares a set of methods that different types can share.

```nika
trait Summarize {
    fn summary(ref self) -> String
}

impl Summarize for User {
    fn summary(ref self) -> String {
        return f"User: {self.username}"
    }
}
```

A trait's methods are **signatures** without a body. The declaration says what
a type must have, and the `impl` says what it does.

**A type's own methods are its declarer's.** An `impl` without a trait works
only on a `struct` or an `enum` the program declares. One on a built-in type,
a `std` type or any type written with a module in front is refused with
`NK1209`. To give such a type a method, declare a trait and implement it for
the type:

```nika
use std::time

trait Doubled { fn doubled(self) -> time::Duration }

impl Doubled for time::Duration {           // not `impl time::Duration`
    fn doubled(self) -> time::Duration { return self * 2 }
}
```

A type parameter may be **bound** by a trait. The bound is what gives a generic
body something it may do (4.6):

```nika
trait Summarize {
    fn summary(ref self) -> String
}

fn shout[T: Summarize](x: T) -> String {
    return x.summary()     // `Summarize` says there is one
}
```

The bound answers the whole call: how many arguments `summary` takes, what
they have to be, what it hands back, and whether it can fail or pause all come
from the declaration. Several bounds are written `[T: Named + Aged]`. A bound
may take a path, `[H: http::Handler]`, and the ledger records a trait and each
`impl` where they were written. Whether a type implements a trait is the union
over every ledger the program reads plus its own. A call that passes a type with no `impl` of the bound's trait is refused at the call, with `NK1164`.

**A trait method reads like any signature.** Without `sync` it may pause;
without `throws` it cannot fail. An implementation is checked against the
declaration: a body that pauses under a `sync` declaration is refused, and a
body that does less than the declaration allows is accepted. A trait can
therefore describe I/O, `fn load(ref self) -> String throws`, and a call
through its bound pauses where the declaration says it may.

An `impl` owes its trait the declared methods and no others: a method the trait
does not declare, or one it declares that the `impl` leaves out, is refused
with `NK1130`. A method whose implementation **pauses** where the declaration
says `sync` is refused with `NK1129`. A trait method has no **default body**,
and a trait is not a type: there is no `dyn` and no `fn f(x: Summarize)`.

---

## Chapter 5: Functions & Argument Architecture

### 5.1. The "Subject ; Config" Protocol
A function signature separates the data a function operates on (its subject)
from its configuration options with a **semicolon separator (`;`)**.

**Zone 1: Subject (Positional)**
Parameters *before* the semicolon are the data the function operates on.
* **Syntactic rule:** arguments here are positional.

**Zone 2: Configuration (Named Only)**
Parameters *after* the semicolon are options, flags, or modifiers.
* **Syntactic rule:** arguments here are named. A positional argument in this
  zone is refused.

```nika
// Definition
fn request(url: ref String; timeout: time::Duration = 30.seconds(), method: ref String = "GET") { ... }

// Valid calls
request("https://api.com")                                        // both options defaulted
request("https://api.com"; timeout: 60.seconds())                 // one of them named
request("https://api.com"; method: "POST", timeout: 5.seconds())  // in any order

// Invalid calls
// request("https://api.com", 60.seconds())      // a positional argument in the named zone
// request("https://api.com"; timout: 5.seconds())  // error[NK1109]: no option `timout`
```

**The `;` stands between the two zones, and where one zone is empty it is not
written.** A function whose parameters are all options is declared
`fn execute(target_age: i64 = 0)` and called `execute(target_age: 30)`.
`execute(; target_age: 30)` is refused. A mixed call keeps its `;`, and the `;`
is required there. A **method** call takes the same form. A driver's deferred
parameters (Part II 10.5) write no `;` either, because a receiver stands
outside the parentheses.

**A call's `(` is on the line of what it calls** ([ADR-317](adr/adr-317.md)),
as an index's `[` is: a `(` at the **start of a line** begins what is written
there - a tuple, a parenthesised expression - and never the arguments of the
line above it.

**Every configuration parameter has a default.** A caller may leave an option
out, and the call then takes the default. A parameter that has to be passed
belongs before the `;`. An option without a default is a parse error that says
so.

**A default is a build-time value** ([ADR-318](adr/adr-318.md)): a literal,
or anything a `comptime` may hold (Part II 10.2), such as
`timeout: time::Duration = 30.seconds()`. It is evaluated once, while the
package that declares the function is built, and every call that leaves the
option out receives that value, in the form a `comptime` crosses in. A default
that cannot be evaluated or cannot cross is refused with the codes a `comptime`
is.

**Order is the declaration's**, not the call's: `method` written first above is
still passed second. The ledger records an option's name, type *and* default,
the default as its value (Part III, 13.5).

**Optional Parentheses**
A function declared without parameters may omit the parentheses, matching the
block lambda style.

```nika
fn init { 
    // No args, no parentheses required
}
```

### 5.2. There Is One Lambda Form
A lambda is written `fn { … }`, and 5.3 describes it. There is no second,
shorter form for single-line bodies.

Naming the arguments, `fn(user) { … }` (5.3), is the same form spelled out: the
body is a block either way, and both spellings stand in all the same places.

**The arguments are the ones the lambda names.** A lambda that names none takes
none, so `fn { … }` is a lambda of no arguments. A body that uses `a`, `b` or
`c` without declaring it is refused with `NK1117`.

`fn: expression` is refused with a message that names the block form.

### 5.3. Lambdas (`fn { ... }`)
A block lambda holds any number of statements. Its arguments are the ones it
names.

* **Syntax:** `fn(name) { ... }`, and `fn(first, second) { ... }` for more than one
* **Arguments:** as many as the list says; the list is the only thing that says so
* **A lambda that names none takes none**, so `fn { ... }` is the zero-argument
  form, which is what `.or_insert_with fn { Stats(0) }` takes

```nika
let complex = users.map fn(user) {
    let bonus = calculate_bonus(user)
    // Implicit return of the last line
    user.score + bonus
}

let ids = users.map fn(user) { user.id }
```

**There are no automatic argument names.** A local inside a lambda may be
called anything. A body that uses `a`, `b` or `c` without declaring it names
something nothing declares:

```text
error[NK1117]: `a` isn't declared anywhere.
```

**Trailing Syntax**
A lambda that is the last argument may stand *outside* the parentheses. Where
there are no other arguments, the parentheses are omitted with it:

```nika
let ids = users.map fn(user) { user.id }
let sum = numbers.reduce(0) fn(acc, n) { acc + n }
```

A block ends at its `}`, so a chain continues after it and means what it reads as:

```nika
Server()
    .get("/x") fn { handler(db) }
    .listen(":8080")
```

```nika
// The count comes from the list, so a local may be called anything
users.map fn(user) {
    if user.is_guest() {
        return "Guest"
    }
    return user.name
}
```

A trailing lambda may follow a method call with or without other arguments, a
plain call, or a path: `users.map fn(user) { … }`,
`numbers.reduce(0) fn(acc, n) { … }`, `access_all(a, b) fn(x, y) { … }`
(Part II, 12.3), `task::scope fn(s) { … }` (Part II, 12.7), and the chain
above.

**A lambda carries no effect marker.** A parameter list is followed by the body
and by nothing else; `fn(info) sync { … }` is not one construct (7.2).

### 5.4. Contextual Capture (The Lifecycle Rule)
Whether a lambda borrows or moves the variables it uses is inferred from the
context the lambda is used in. The rule is the same at both values of
`user_parallelism`.

#### A. Immediate Context (`@immediate`)
A function that runs the callback to completion before it returns is an
**immediate context**.
* **Behavior:** implicit borrow (`ref T`).
* **Examples:** `map`, `filter`, `for_each`, `sort_by`.

```nika
let prefix = "User: "
let names = ["Alice", "Bob"]

// 'map' is @immediate. It executes completely within this stack frame.
// 'prefix' is implicitly borrowed.
let formatted = names.map fn (name) { prefix + name }

// 'prefix' is still valid here because it was only borrowed.
println(prefix)
```

#### B. Detached Context (`@detached`)
A function that stores the callback, runs it later, or hands it to another
thread or task is a **detached context**.
* **Behavior:** implicit move (ownership transfer) for ordinary data. A handle
  on a shared value is **duplicated** rather than moved, so the name outside
  stays usable (6.2).
* **Examples:** `spawn`, `defer`, `set_timeout`, `channel.on_receive`.

```nika
let prefix = "Log: "

// 'spawn' is @detached. The lambda might outlive the current function.
// 'prefix' is implicitly moved into the background task to ensure safety.
spawn fn { println(prefix + "System started") }

// Compiler Error: 'prefix' has been moved!
// println(prefix)
```

#### C. Where the Distinction Lives

The context belongs to the **function that takes the lambda**, not to the call.
`map` is immediate for every caller and `spawn` is detached for every caller,
so the capture is decided where the lambda is written.

**A function in user code declares a code parameter with a function type.** The
type is spelled the way a signature is, `handler: fn(Request) -> Response`,
with `sync` or `throws` after the result where a declaration puts them. Without
`sync` the code may pause; without `throws` it cannot fail. A lambda that does
less fits a type that allows more. A pausing lambda handed to a `fn() sync` is
refused.

The context of such a parameter is **inferred**, not written. A parameter the
body only calls is immediate and borrows. A parameter the body keeps (stores,
hands back, gives to a task) is detached and moves. It is the question 6.5 asks
of every parameter, and there is no `@detached` to write. For an immediate
parameter the lambda's `touches` and `throws` count for the callee, while
whether the call may pause, and the shape it is lowered to, follow the
parameter's type. `std`'s own lambda-taking entries, such as `map`, are written
by hand and follow the lambda in everything. For a kept parameter the
callee's promises follow the type, so a `listen` that calls a stored
`fn(Request) -> Response` may pause. The capture at a `spawn` is reported with
`NK2101` (8.3).

**A function type stands wherever a type does**: a struct field, a result, a
`let`, an element of a list. A function value in any of them is **kept**: the
lambda moves what it captures, and a copy of the value shares the one closure.
A field that holds one is called like a method, `button.on_click(4)`, where the
struct has no method of that name. A named function stands where a function
value is wanted. A kept value handed to a parameter that only runs it is lent.
Two values of a type that holds a function are not compared (`NK1188`).

```nika
struct Button { label: String, on_click: fn(i64) -> i64 sync }

fn adder(n: i64) -> fn(i64) -> i64 sync {
    return fn(x) { x + n }
}

fn main() {
    let b = Button { label: "ok", on_click: fn(x) { x * 2 } }
    let add = adder(3)
    println(f"{b.on_click(4)} {add(1)}")
}
```

---

## Chapter 6: Memory and Ownership

Memory is managed by **ownership and borrowing**, which the compiler enforces.
There is no garbage collector, and user code frees no memory.

### 6.1. The Concept of Scope
When a variable goes out of **scope**, at the end of the block `{}` where it
was created, its memory is released. User code frees no memory.

### 6.2. Unified Types
Three shared types carry a value that has more than one owner.

**`Shared[T]` is written in the source; it is not inferred.** The compiler
decides **which owner count each value gets**: one a second thread may safely
touch where the value may reach one, and a cheaper one where it may not.
Whether a `Shared` may be handed to a task (Part II, 11.2) is decided from the
type and from where it is going, at both values of `user_parallelism`. The
count follows that answer.

**Where the compiler proves that a value never leaves the thread that made it,
it uses the cheaper count.** This never changes meaning. **Where nothing proves
it, the value gets the safe count**, and there is no way to ask for the other
one.

**At `user_parallelism = no` every count is the cheap one.** User code runs on
one thread, the runtime's own threads run no user code, and a `Shared` may not
be handed to code nothing written down describes. What a program passes out of
itself is what is **inside** the `Shared`, a view or a copy; a foreign library
that keeps a value puts it in a hull of its own.
`nikaia lower x.nika --sharing` prints which count each value got, why, and
what would have changed it. It changes no decision.

* **`Shared[T]`**: a value several parts of the program own at once and nobody
  changes. The memory is released when the *last* owner is finished.
* **`SharedMut[T]`**: a value several parts own at once and any of them may
  change. The lock that keeps the changes apart is part of the type; there is
  no second wrapper around it. The value is changed through the four doors of
  6.3.
* **`Locked[T]`**: an individually locked field inside a shared structure, one
  lock per field rather than one lock around the whole. It is the same lock as
  the one inside `SharedMut[T]`, opened by the same four doors.

**The second owner arises without a written step.** Where a handle on a
`Shared[T]` or a `SharedMut[T]` is handed on **by value**, passed to a function
that keeps it or used by a task (Part II, 11.2), the handle is **duplicated**.
Each handle is cleaned up at the end of its own block (6.1). No method is
called. A duplicated handle copies no data: one value, one more owner. The rule
is the same for both shared types; a `SharedMut[T]` may go into a task of the program's own, and not into code nothing written down describes
(Part II, 11.2).

**Lending the inner value out duplicates nothing.** `ref` on a shared value is a
view of the value *inside* it, so a function that only uses the value takes an
ordinary view and never mentions sharing:

```nika
fn serve(db: ref Connection) { … }

let db = Shared(postgres::connect("…"))
serve(db)              // a view; no handle is made, and the count is untouched
```

A signature names the shared type only where the function **keeps** the value
past the call: puts it in a structure, gives it to a task, hangs it on
something that outlives the call.

**A `SharedMut[T]` is opened, and the opened value is an ordinary view.** There
is no `ref` straight through a lock. The caller opens the lock with one of the
four doors of 6.3 and passes the borrowed value in. The called function sees a
plain value and obeys the rule for an open lock: it may not pause, and it may
not touch a lock of its own (Part II, 12.2).

```nika
let db = SharedMut(postgres::connect("…"))
db.access fn(open) { serve(open) }    // `serve` must be `sync` and lock-free
```

**The first handle is written in the source.** Each of the three shared types
makes one by being **called with the value that goes in it**:

```nika
let db = Shared(postgres::connect("…"))
let counter = SharedMut(0)
let frei = Locked(0)                    // a field's lock, one per field
```

One rule decides every hull in the language:

> **A hull you cannot see, the compiler writes. A hull you can see, you write.**

The compiler puts a plain value into a `T?` (2.3). A shared type is written
where the value is put into it.

**A constructor stands wherever an expression may:**

```nika
keep(Shared(connect(url)))                      // an argument
let p = Pool { db: Shared(connect(url)) }       // a field of a literal
fn shared(url: String) -> Shared[Connection] {
    return Shared(connect(url))                 // a result
}
```

A plain value where a shared one is wanted is refused with `NK1115`:

```text
error[NK1115]: `serve` takes a shared value, but `db` isn't one.
   --> main.nika:11:5
    |
 11 |     serve(db)
    |     ^^^^^^^^^
    |
    = note: It's declared as `serve(db: Shared[Db])`.
    = help: Wrap it: `Shared(db)`.
```

A shared value may be returned, and a number may be shared: `SharedMut(0)` is a
call, and the call gives the number its type as any other argument does.

**Only the handle is duplicated.** Ordinary data (a string, a number, a struct
of those) is **moved** where it is handed on to something that keeps it (6.5,
8.3). For data, the `.clone()` is written in the source.

**Each handle lives to the end of its own block, whether or not it is used
again.** The duplication is not conditional on a later use, so the value is
cleaned up where the caller's handle ends, which may be later than the task
that holds the other one. A program that wants the cleanup earlier ends the
block earlier (6.1). `--sharing` names each duplication site beside the count
it printed for that value.

**`SharedMut[T]` is one name and two hulls.** Which hull a value gets is
decided per value, as the owner count is. A message, a printed type and a
ledger entry say the name the program wrote. **`Shared[Locked[T]]` is
refused** with `NK1123`, and the message names `SharedMut[T]`.

### 6.3. Changing Shared Data: The Four Doors
A value behind a lock is not changed by assignment. The lock is opened first,
and the shape of the change decides which door opens it:

```nika
let kasse: SharedMut[i32] = ...

let stand = kasse.get()              // take a copy out
kasse.set(hole_neuen_stand())        // replace it; the new value comes from outside
kasse.update fn(mut v) { v += 100 }   // change it, under the lock

// Where the value is large, `update` changes it where it lies, and `access` reads it there.
let protokoll: SharedMut[Log] = ...
protokoll.update fn(mut log) { log.add("gebucht") }
let n = protokoll.access fn(log) { log.len() }
```

* **`get` and `set` take no block.** `get` copies the value out. `set`'s
  argument is computed *before* the call, including any wait for I/O, and the
  lock is open for one store.
* **`update` is handed the value as `mut v` and changes it.** It returns
  nothing (4.3). Whether `v` is a copy of a small value or the address of a
  large one is the compiler's decision. An `update` block may be run more than
  once, so it may not do I/O or take another lock.
* **`access` hands the block the value where it lies, to read.** The block may
  not change it. It is for a value too expensive to copy.

`update` and `access` run user code while the lock is open, so that code runs
straight through: no I/O, and no second lock. The compiler checks both
(Part II, 12.2 and 12.3).

**A value a crashed task left is never read** ([ADR-327](adr/adr-327.md)). A task
that panics while it holds a lock poisons the value. A door to a poisoned value
panics in the task that opens it. A value made with
`SharedMut::supervised(fn { … })` is rebuilt instead: the runtime calls the
function again, puts its result in place, and the holders keep their handle; how
often is bounded as a supervisor's restarts are (Part II 12.8).

**A value taken out of a lock is stamped.** `kasse.get()` is a `Seen[i64]`,
and so is what `access` computes. A `Seen` reads like the value it carries: a
program prints it, compares it, sends it in a response, and hands it to any
function that touches no lock. The stamp goes with it through arithmetic,
calls, struct fields declared `Seen[…]`, and time. A stamped value may not go
back into a lock **blind**: `kasse.set(stand + 100)` is refused wherever
`stand` was read, and so is `if stand > 100 { kasse.set(0) }`. The doors for
what was seen are `update`, which decides inside the lock, and
`set(neu; after: stand)`, which stores only if the lock still holds what was
seen and throws `Overtaken` otherwise. There is no word that removes the stamp.

Three mistakes are refused by name. Assigning to a `SharedMut` directly,
`kasse = 0`, is refused, and the message names `set`. A `set` given a stamped
value, or standing under a stamped condition, is refused, and the message names
`update` and `after:`. An `update` block that assigns to `v` without reading it
is refused as a `set`.

### 6.4. Resource Cleanup (RAII)
Resources are cleaned up deterministically, following the **RAII** principle
(Resource Acquisition Is Initialization). There is no garbage collector.

**Automatic Destruction**
When a variable goes out of scope, at the closing brace `}`, its memory is
released.

**Custom Cleanup (`impl Drop`)**
A struct that manages an external resource (a file handle, a socket, a C
pointer) may implement the `Drop` trait. The `drop` method is called when the
value is destroyed. `drop` is **synchronous**: it runs straight through and
never pauses. It is for teardown that is pure memory work or a cheap native
call.

```nika
struct FileHandle {
    fd: i32
}

impl Drop for FileHandle {
    fn drop(ref mut self) {
        println("Closing file descriptor...")
        // Native close call would go here
    }
}
```

**Cleanup That Needs I/O (`impl Cleanup`)**
A resource whose teardown needs I/O (a buffered file flushing, a transaction
rolling back, a TLS connection closing) implements `Cleanup`. I/O may pause
(Chapter 8), and `drop` cannot pause.

```nika
use std::fs

struct BufferedFile {
    path: String,
    held: String,
}

impl Cleanup for BufferedFile {
    // Pausable teardown. May pause, may fail.
    // Runs where the value dies - on normal exit AND while an error is
    // bubbling up.
    fn cleanup(ref mut self) throws {
        fs::write(self.path.clone(), fs::Root::Anywhere, self.held.clone(); append: true)
    }
}
```

`impl Cleanup` has the one method, `fn cleanup(ref mut self)`, with `throws`
where it can fail. A type that also needs a synchronous last resort writes it
as its own `impl Drop`: `drop` runs after `cleanup`, or alone where `cleanup`
cannot run (below). Anything else written in `impl Cleanup`, a `drop`
included, is refused with `NK2601`, and the message points at `impl Drop`.

User code never calls `cleanup`. **The value dies where ownership says it
does**: at the closing brace of the block that owns it, or in the function it
was handed to when that function keeps it. The compiler runs `cleanup` there:
the end of that block is a place where the function may pause and where a
failure surfaces. Values that die together are cleaned up in the order they
die, the last one bound first. What holds such a value (a struct field, a list
element) has its cleanup too, because the value dies with it.

`std` has one of its own: `fs::create(path, root)` hands back a buffered
`fs::Writer`, whose `write` holds the text and whose cleanup writes what it
holds.

```nika
use std::fs

fn report(lines: Vec[String]) throws {
    let mut out = fs::create("report.txt", fs::Root::Anywhere)
    for line in lines {
        out.write(f"{line}\n")
    }
}   // `out` dies here, and what it holds is written
```

**A cleanup error is an error.** The cleanup is a call the compiler writes, and
it is answered like one. If `cleanup` declares `throws`, the function where the
value dies can fail, and one that does not declare `throws` is refused with
`NK2605`, the code of every call that can fail there. The message names the
resource:

```text
error[NK2605]: This function can fail, because cleaning up `f` can fail.
  --> report.nika:2:5
   |
 2 |     let f = fs::create("report.txt", fs::Root::Anywhere)
   |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
   |
   = note: `f` is a `fs::Writer`. Its cleanup runs at the end of this block and can fail, so the function can fail with it.
   = help: Declare the function with `throws`, or close it yourself where you want to handle the failure: `f.close() catch { … }`.
```

Two refinements:
* If a value dies **while an error is already propagating**, the cleanup error
  does not replace it. It is attached to the original error as a *secondary
  error* (7.1).
* A program that handles the close error specifically calls **`close()`**.
  `close()` consumes the resource, runs its cleanup there and hands the error
  back at the call, and no implicit cleanup runs afterwards.

**Where nothing may pause, nothing with a pausable cleanup may die.** The
cleanup is a pause, and the rules for pauses apply to it as written: a value
that would die inside a door's block is refused with `NK2202` (Part II 12.2),
inside a `par_iter` lambda with `NK2209`, and inside a lambda handed to a
`sync` function type with `NK2206`. The message names the cleanup of the
value; the way out is binding it outside, where it dies after the lock is
released, or closing it there.

**When `cleanup` cannot run.** When a task is *cancelled* (it lost a `select`
race, or a supervisor restarts it), what died in it and was not yet cleaned up
is adopted by the runtime, which finishes it in the background before the
program exits ("parked cleanup"). The runtime configuration `cleanup-deadline`
bounds them (Part III 13.3b). A cleanup the deadline cut off is a failure of
the program: exit status 70, with the resource named on the panic path. A
**panic** runs no pausable cleanup: during panic teardown only the synchronous
`drop` runs, and **on a target that traps rather than unwinds, a panic ends the
process immediately, so no destructors run at all** (Part III, Appendix A). The
**panic hook** (7.2) runs on every panic. A panic is for an unrecoverable bug;
a recoverable failure uses `throws`, where full cleanup is guaranteed.

There is no `defer` keyword. Cleanup happens at the end of the block through
`Drop`. A lock's `.access()` releases the lock; there is no manual
`lock/defer unlock` pair. A `throws` unwinds the stack and runs `drop` for
every variable in scope.

### 6.5. References and Borrowing

A function that only *looks at* a value, and does not own it, takes a
**view** of it. The owner keeps the value, the borrower may read it, and the
loan ends on its own.

**`ref X` is a view of an `X`, and that is the whole rule.** It is one word and
one meaning wherever a type may stand: `ref String` is a view of text,
`ref Array[T]` a view of a run, `ref Reading` a view of a struct, `ref self` a
method's view of its receiver. There is no second noun for the view of a type.

**`ref` is reserved** (2.1). `ref(x)` is a borrow of `(x)`, never a call.

**Nikaia source contains no lifetime annotations.** There is no syntax for
them. Everything described below happens inside the compiler.

> Nikaia source code contains no lifetime annotations. Ever.

Two things are guaranteed:

1.  **Borrowing inside a function**, across I/O. A function may pause at any
    I/O call (Chapter 8), and a borrowed value stays valid across the pause:

    ```nika
use std::fs

    fn report(config: ref Config) throws {
        let name = ref config.name       // borrow
        let data = fs::read("log", fs::Root::Anywhere)    // pauses here (I/O)...
        println(f"{name}: {data}")     // ...and the borrow is still valid.
    }
    ```

2.  **Returning a borrowed value from a function.** `fn first_word(s: ref String) -> ref String`
    needs no annotation. Where the result could come from *several* inputs,
    the compiler infers the connection, across function boundaries and through
    the whole program (6.7).

**A borrow may not outlive its owner** (6.6).

**The declaration writes the `ref`, never the call.** A parameter written with a
plain type is a **view** unless the function's body keeps the value: stores
it, hands it back, gives it to a task, or passes it to something that keeps
it. **Handing back a *part* of it is handing it back**: a field whose type
moves, taken out of the parameter and returned, keeps the parameter; a field
that **copies** leaves the parameter a view. Which of the two it is comes from
the body, is written to the ledger (6.7), and holds for every caller. The
caller writes `serve(db)` and `fs::map(path, root)`, and the compiler writes
the reference the callee asked for, as it writes the pause and the failure a
call carries (7.1, 8.1). A `ref` written at a call, `serve(ref db)`, is refused
with `NK1137`. A `ref` in a parameter type is an assertion, *this is a view*, as
`sync` is (Part II, 12.1).

A parameter the function changes in place says `mut` in the declaration,
`fn fill(mut out: Vec[i64])`, and is passed as `ref mut`; the call shows
nothing, as `xs.push(1)` shows nothing. A parameter the body changes without
`mut` is refused with `NK1138`. A `mut` parameter handed to another `mut`
parameter is passed straight on. `ref self` is written by the author, and that
shape is `NK1131`, as is a `mut` parameter given away whole: it stays the
caller's, so the way out is `.clone()` or taking it without `mut`.

A `for` **lends** its list, so the list is still there after the loop;
iteration that takes the elements away is written `for x in xs.drain()`. A
`let` that annotates a `for` binding with the element's type, `let copy: Row = r`,
is refused with `NK1183`: the binding is a view, so the annotation comes off,
or `.clone()` makes the copy. A
`let` over a place (`config.name`, `totals.stations[name]`) is a view of it
where the value would otherwise have to move.

Nothing here inserts a copy. A value handed to a function that keeps it, and
used again afterwards, is refused with `.clone()` named as the way out (8.3).

Four kinds of argument are not lent and are passed owned: a parameter the body
**keeps**, a value that **copies**, an argument that is **already a view**, and
a **method's** argument.

**A value used after it was handed over is refused** with `NK2105`: an argument
kept, a key or a value written into a container, a field, an element, a `let`
that renames it, an assignment, and, inside a loop or a lambda, the next turn's
hand-over of what is already gone. A second use in the **same** statement is
refused too. A **part** is tracked on its own: after `xs.push(p.name)`, `p.x`
is still there and `p.name` and `p` are not. A part of something only lent is
refused where it is handed over, with `NK2106`. A parameter whose part is
handed over is kept, so its caller hands it over whole.

### 6.6. Escaping References Are Tethered

A borrowed value **escapes** when it is returned past the scope that owns the
buffer, captured by a `@detached` lambda, or put into a collection that lives
longer than the buffer. An escaping view is **tethered**: the buffer stays
alive as long as the view does.

> **Transient = borrow. Escaping = tether. Copying = yours to ask for.**

Every view (`ref String`, `ref Array[u8]`) is in one of three states. The
compiler picks the cheapest one that works, and the states are never written in
the source:

| State | What it is | Cost |
| :--- | :--- | :--- |
| **Borrowed** | a plain reference into the buffer | nothing at all |
| **Tethered** | a plain reference; the buffer has moved into a keep | nothing where a frame keeps it; one count per task, or per buffer where a container drops entries — no copy |
| **Owned** | a `String` of its own | one allocation, **only** where the program wrote `.clone()` |

**The buffer cannot die while anything still points into it.** It is kept alive
deterministically, by the frame that outlives it or, where none does, by a reference count, with no garbage collector.

Two rules keep this cheap on large data:

* **Storing a slice in a struct is not, by itself, an escape.** A struct that
  is built and consumed inside the scope that owns the buffer keeps plain
  references.
* **No handle on every slice.** When a map full of
  slices outlives its buffer, the buffer lives in the keep of the frame that keeps the map, and its keys stay
  plain references.

**Nothing is written for a tether.** Where the buffer lives is the compiler's
decision, like which count a `Shared` gets:

```nika
struct Token {
    text: ref String,   // a view into someone else's buffer
}

fn tokenize(path: String) -> Vec[Token] throws {
    let source = fs::read_to_string(path, fs::Root::Anywhere)
    // The returned tokens outlive this function, so `source` lives on in the
    // caller: no lifetimes, no copies of the text, no dangling references.
    ...
}
```

**A buffer lives in the keep of whatever keeps its views**, and who owns that
keep is read off the program:

* **the caller's frame**, wherever a frame outlives the views — handed back,
  kept in a `mut` parameter, or kept by a list outside a loop. This costs
  nothing, works across any number of calls, and the body may pause;
* **a handle that travels with the value**, where no frame outlives it — a task;
* **a handle per view**, where a container keeps views across a loop and drops
  entries as it goes, so that a buffer is freed when its last view leaves. A
  struct of views kept that way carries a handle on each buffer it points into,
  and is read and written through them; the struct itself is unchanged.

**What a body owns is handed out the same way**, whole or a part: a view of a
local, `fn d() -> ref String { let s = "x".clone(); return ref s }`, puts `s` in
the caller's keep. **A view handed back from a parameter points into the
caller's argument**, which lives as long as the result does; an argument made
in the call itself, `let r = e(name.clone())`, lives in the caller's keep.

A struct built and consumed inside the scope that owns its buffer is borrowed,
and costs nothing:

```nika
struct Reading { name: ref String, temp: i32 }
```

`--tethers` (on `nikaia build` and `nikaia lower`) prints where each buffer lives and why, and changes nothing;
a change is a ledger diff in review.

There is no struct with lifetime parameters in Nikaia.

**An escape whose buffer cannot be shared is refused.** Where a slice escapes
and its buffer lives on the stack or came from a foreign library, no tether is
possible. The compiler refuses the program and names the ways out, `.clone()`
among them. The compiler never inserts that copy. One buffer handed both to a
task and out of the function is refused with `NK2304`, and so is a container of
structs holding views that drops entries inside a loop where the container is
not a list, or a struct's views are not text.

**A parameter written `ref String` may not be kept past its call** unless the
place it is kept names a buffer. A view inside a struct carries the buffer it
points into; a parameter written `ref String` does not. A function that stores
such a parameter where nothing names its buffer is refused with `NK2302`
(Part III, C.3): into a task, into a field of a subject that holds no view, into
a result that may point into another buffer as well. Such a function puts the
view in a struct and takes the struct:

```nika
struct Reading { name: ref String, temp: i32 }

impl Summary {
    fn record(ref mut self, m: Reading) { … }     // and not `name: ref String`
}
```

Handing a view back out of the buffer it came from is **not** this rule:
`fn count(seq: ref String, k: i64) -> HashMap[ref String, Tally]` returns views
of `seq`, and the result points into `seq` and nothing else. The same holds for
a struct literal handed back: `fn make(name: ref String) -> Reading` returns a
`Reading` that points into `name`, where `name` is the only view the function
takes.

Where the destination **already carries a buffer**, there is nothing to refuse.
A method of a struct that holds a view has that struct's buffer in hand, so
storing the parameter into one of its fields is accepted, and the parameter is
a view of *that* buffer. `fn note(ref mut self, name: ref String)` on a
`Summary` holding `label: ref String` compiles, and `name` is a view of the
buffer `label` points into. That narrows what a caller may pass. The same holds
for a field of a struct **parameter** that holds views, where the subject holds
none: `fn relabel(mut s: Summary, name: ref String) { s.label = name }`
compiles, and `name` is a view of the buffer `s` points into. Fields of two such
parameters are refused with `NK2302`. Where the view is handed to a call on the
subject, it is treated as kept, and the parameter is a view of the subject's
buffer.

**A tether keeps the whole buffer alive**, not just the part pointed at.
`--tethers` (on `nikaia build` and `nikaia lower`) names every buffer kept this way and what keeps it;
`.clone()` keeps the part and lets the buffer go.

### 6.7. The Borrow Contract Ledger

For every function whose signature involves a borrow, the compiler infers a
**borrow contract**, such as "the result of `longest(a, b)` borrows from `a`
or `b`." A contract is never written in the source. Contracts are stored in a
generated file, **`nikaia.contracts`**, the ledger, which is committed
alongside `nikaia.lock` (Part III, 13.5).

The ledger has two jobs:

1.  **Cache:** if a contract did not change, none of the function's callers is
    re-checked.
2.  **Explanation:** if an edit to a function *body* changes its contract, the
    compiler compares the old and the new contract. If a caller elsewhere
    breaks, the error names the edit, the change in the contract, the caller
    that is affected, and the way out.

### 6.8. When the Compiler Says No

An ownership rule refuses a program that breaks it. **Every such refusal
explains itself in plain language and names the way out.** Reading a Nikaia
diagnostic requires no knowledge of Rust; a backend error that reaches user
code is a defect of the compiler.

The most common case is changing a collection while looping over it:

```nika
let mut users = load_users()
for u in users {
    if u.is_duplicate() {
        users.remove(u)   // bug: pulling the rug out from under the loop
    }
}
```

```text
error[NK2301]: You're changing `users` while a loop is still reading it.
  --> main.nika:4:9
   |
 2 | for u in users {
   |          ----- the loop reads `users` here
 3 |     if u.is_duplicate() {
 4 |         users.remove(u)
   |         ^^^^^^^^^^^^^^^ changed here
   |
   = note: Removing items mid-loop would lose the loop's place. In most languages that's a crash or a silent bug.
   = help: Use the method that does this safely: `users.retain fn (user) { !user.is_duplicate() }`.
```

For every known pattern of this kind, the standard library provides a safe,
named method (`retain`, `drain`, `entry`, `swap(i, j)`, …), and the diagnostic
names it.

---

## Chapter 7: Error Handling

Nikaia distinguishes two kinds of errors.

### 7.1. Recoverable Errors (`throws`)

A recoverable error is an expected problem: a file is missing, a connection
drops, an input does not fit the format. A function that can fail says so with
`throws`.

```nika
use std::fs
use std::net

fn fetch_config() -> String throws {
    let text = fs::read_to_string("config.txt", fs::Root::Anywhere)   // can fail
    let mut peer = net::connect("127.0.0.1:9000")   // can fail too
    peer.write(ref text)
    return text
}
```

**`throws` stands after the result type**, and `sync` with it:
`fn fetch_config() -> String throws`. A function *type* has the same order. A
declaration with no result type writes the word after the parameters:
`fn tick() sync { … }`. The form before the arrow does not parse; the message
names the order.

**`throws` names no types.** What a function can fail *with* follows from its
body. The compiler infers it whole-program and writes it to `nikaia.contracts`
(Part III, 13.5).

**An error type is an `enum`.**

```nika
enum ConfigError {
    NotFound(fs::Path),
    Unreadable(fs::Path),
    BadSyntax { line: i64, expected: ref String },
}

impl Error for ConfigError {
    fn message(ref self) -> String {
        match self {
            ConfigError::NotFound(p)   => f"no config at {p.display()}"
            ConfigError::Unreadable(p) => f"cannot read {p.display()}"
            ConfigError::BadSyntax { line, expected } => f"line {line}: expected {expected}"
        }
    }
}
```

An error **carries what belongs to it**: "not found" carries the path. A
`match` over an `enum` (4.4) is checked for completeness. What is thrown
implements `Error`, and the `impl` line says so. A number, a `bool`, a
character and text are not errors, and a `throw` of one is refused with
`NK1161`.

**Raising: `throw`.**

```nika
if !fs::exists(path, fs::Root::Anywhere) {
    throw ConfigError::NotFound(path)
}
```

**A `throw` is an expression**, and so are `return`, `break` and `continue`.
Their type is **never**. A `never` fits every expected type without widening
it, so each may stand wherever an expression may:

```nika
let user = find(id) ?? throw NotFound(id)
```

A `throw` leaves the function and makes it `throws`, a `break` needs a loop,
and a statement after a `break` in the same block is refused with `NK1133`
(3.3). What a `return` written inside an expression hands back stands on its
line: `let x = y ?? return` at the end of one returns nothing, and the next
line is the next statement.

**Propagation happens on its own, and nothing marks it.** A call that can fail
stands inside a function that declares `throws`. There is no operator and no
sigil:

```nika
fn load() -> Config throws {
    let text = fetch_config()   // if it fails, `load` fails
    return parse(text)
}
```

Four things leave the control flow of a program without the line showing it: a
call may **pause** (8.1), a block's end may **pause and fail** (6.4), a call may
**fail** (here), and a loop's step may fail.

**The declaration is required.** A call that can fail, in a function that does
not say `throws`, is refused with `NK2605`:

```text
error[NK2605]: This function can fail, because `liest` can fail.
  --> app.nika:2:23
   |
 2 | fn ruft() -> String { return liest() }
   |                       ^^^^^^^^^^^^^^
   |
   = note: `liest` can fail with `io::IoError`.
   = note: A call that can fail looks like any other call, so its failure passes straight through this function unless you handle it.
   = help: Add `throws` to `ruft`, or handle it at the call: `… catch { … }`.
```

for `fn liest() -> String throws { return fs::read_to_string("x.txt", fs::Root::Anywhere) }` one
line above. The shape is Appendix C.4's: the statement is underlined, the
first note says what the call can fail with, as the ledger the call was
resolved against records it (13.5), and the help names the two ways out.

**Handling: `catch`.** The block supplies the replacement value, or it leaves
the function.

```nika
let config = load() catch {
    eprintln(f"{error}")
    return                                // leaves the function
}

let port = read_port() catch { 8080 }     // replacement value
```

Inside the block the error is named **`error`**. A handler that passes the
error on writes `throw error`. A `catch` over an expression that cannot fail
is refused (`NK1134`): the expression is the value, and the handler goes.

The `catch` has the type of the value it guards: `config` above is what
`load()` hands back, and a member of it is reached as on that value.

**Telling failures apart.** `error` is the sum of the errors that can arrive at
this point, as the compiler inferred them. A handler tells them apart with the
patterns of 3.4, where a path with `::` names a variant:

```nika
let config = load() catch {
    match error {
        ConfigError::NotFound(p) => Config::default()
        ConfigError::BadSyntax { line, .. } => {
            eprintln(f"config broken at line {line}")
            return
        }
        else => throw error
    }
}
```

A `match` over `error` that more than one error type reaches needs `else`, because the set of error **types** is open
(`NK1151`). A library's error type arrives as that type: a function that reads
a file hands its failure on as an `io::IoError`. No `throw` in the program
raised it, so it has no site, and `error.full()` says so; the failures that
joined it travel with it through every caller that hands it on.

**Two sets differ.** The **variants of an error type** are closed: a `match`
over them is exhaustive, and adding one is a breaking change. The **set of
error types** arriving at a `catch` is open, and it grows when a callee gains a
failure. When it grows, **every `catch` over that callee is named once in the
build output**, with the new error, the handler it now reaches, and the fact
that the handler takes it as it takes everything (`NK2402`, given at the call
inside the handler). Under `--locked` the build fails until the ledger is
regenerated and committed. The commit is the acknowledgement; nothing is
written at the handler, and a handler that matches on `error` is told the same
as one that does not.

**Every error carries its site and its chain.** The **site** is where the error
was raised. The chain holds every error that joined on the way: a cleanup that
failed while the stack was unwinding is attached to the original as a
*secondary* error rather than replacing it (6.4), and so are the other failing
branches of an `overlap` (8.1.2). The joined errors are kept in the order they
joined, each with its own site, and a log, an uncaught failure and
`nikaia explain` show them under the first. They are a diagnostic, not a value:
a program does not read them, and `throw error` hands them on. A `catch`
catches one error and chooses by its type. The site costs nothing at run time:
the compiler writes it into the binary as text.

**A stack trace is not carried.** `NIKAIA_TRACE=1` asks for one; without it
there is none, and the long form says so.

**Printing it: short is the default.**

```nika
eprintln(f"{error}")           // the message, and nothing else
eprintln(f"{error.full()}")    // the site, the chain, and a trace if one was captured
```

`{error}` is the message the author wrote. A failed HTTP handler answers with a
generic 500 and logs the rest.

> The form you type without thinking is the one you may show a stranger.

`full()` is an ordinary call: a hole holds an expression (2.5), and a method
call is one.

Every error knows the **site that raised it** and has a short form of it a
person can read out. `nikaia explain NK-2C7` leads from `(NK-2C7)` to the line
that threw it, with no log file, also when the working tree has moved on.

**A call the language performs can fail.** Two places perform a call user code
did not write: the end of a block, where a resource is cleaned up (6.4), and a
loop's step over a fallible stream (`NK2701`). In both the enclosing function
declares `throws`, and the compiler names the resource or the loop.

**It is one rule with two codes.** Where a call can fail, written by user code
or performed by the language, the failure fails the enclosing function, the
function declares `throws`, and the compiler names the call that is the
reason. `NK2605` is the written call and the cleanup at a closing brace, which
the message names as the cleanup of the value; `NK2701` is the loop's step.

### 7.2. Unrecoverable Errors (`panic`)
An unrecoverable error is a logic bug, such as reading the tenth item of a
list of five. A panic in a task ends that task, and the program goes on; a panic
in `main` ends the program (Part III A.2). Where the machine cannot unwind, the
process ends.

**The Panic Hook (`std::panic::on_panic`)**
An abort runs no normal cleanup. Before the process dies, or before the crashed
task is isolated where the program survives, the runtime calls one registered
function: the **panic hook**. It is the place for a crash dump, a crash report,
or flushing a diagnostics log.

```nika
use std::panic

fn main() {
    // Global, one per application. Set it early.
    // The hook may not pause — mid-panic there is nothing to pause on — and
    // that promise is on `on_panic`'s parameter type, not on the lambda:
    // a lambda carries no promise of its own.
    panic::on_panic fn(info) {
        // 'info' carries: message, file/line, and the stack trace.
        // Pattern: open crash resources at startup, only WRITE here.
        crash_log.write_report(info)
    }

    run_app()
}
```

The rules:
* **Global, application-only.** There is exactly one hook per program, set by
  the application. A library that calls `on_panic` is refused with `NK2604`. A
  crash-reporting library exports a function that the application's hook
  calls.
* **It runs on every panic, on every target**: before the trap where the
  machine traps, before the task is poisoned where it unwinds. A supervisor
  (Part II, 12.8) receives its crash information from the same `info`.
* **The hook may block, briefly.** Where the program keeps running, the hook
  stays short and hands heavy reporting to something started earlier.
* **Diagnosis, not cleanup.** The hook does not flush buffered files or finish
  transactions. If the hook itself panics, the process aborts immediately.

**The hook may not pause.** That promise is on `on_panic`'s parameter type, not
on the lambda; a lambda carries no promise of its own (5.3).

---

## Chapter 8: Concurrency (Doing things at the same time)

A program performs several tasks concurrently at both values of
`user_parallelism`, such as waiting for a download while answering input. The
mechanism is **asynchronous execution**.

### 8.1. Async by Default
A function that performs I/O, such as reading a file or downloading a URL,
**pauses** without blocking the program. There is no `await` keyword.

Within one task, the order is the written order: `let a = fs::read("x", root)`
pauses, and the line after it does not run until `a` is there. What runs
meanwhile is some *other* task; a pause never forks one. A task exists only
where the program writes one (`spawn`, `par_iter`, `task::scope`), and
`.join()` on a handle is where two tasks meet again. The marker for waiting
stands where something branches, and nowhere else.

### 8.1.1. Statement Order Is the Written Order

Two statements run in the order they are written, whether or not they touch
anything in common. No analysis stands between the source and the schedule. A
program that wants two things to run together says so (8.1.2).

### 8.1.2. Asking for Overlap: `overlap { … }`

**Each statement in the block is a branch.** The block starts every branch, waits
for all of them, and its value is the tuple of their results **in written order**.

```nika
let (user, rights, prefs) = overlap {
    db::load_user(id)
    db::load_rights(id)
    cache::load_prefs(id)
}
```

**An `overlap` block is not a task.** The block ends before the function
continues, so nothing outlives it: nothing is moved, borrowing works as it does
anywhere else, and the crossing rules a `spawn` meets (Part II, 11.2) do not
apply.

**The branches meet on nothing, and the compiler checks it.** A branch pair that
meets on a resource is refused with `NK2104`, and the message names the
resource. A branch that **binds** a name is refused the same way. `--overlaps`
reports on the blocks a program writes.

**A block that joins pauses.** An `overlap` or a `select` hands its branches to the executor and waits there, so a function holding one is not `sync`, whatever its branches do.

**A branch that fails makes the block fail.** Where two branches fail, the
first **in written order** wins. The other failures are attached to the winner
as its `secondary` list, in written order, and a log or `nikaia explain` shows
them under it. Per-branch handling is a `catch` inside the branch. Combining
failures is a `catch` on the block, where the handler has the winner;
re-thrown with `throw error`, the others go with it. A branch cannot see
another branch's failure.

**A branch is an expression.** Several steps in one branch are a block
expression inside it.

**The meaning is the same at both values of `user_parallelism`; the duration
differs.** At `no`, a branch that computes still overlaps with another branch's
*waiting*, because the waiting is not user code. Computation beside I/O
overlaps under both values; computation beside computation overlaps only under
`yes` (1.2).

### 8.2. Spawning Tasks
`spawn` starts a new independent task. It takes a lambda holding the code to
run: the one lambda form (5.3), whose body is a block whether it holds one line
or several.

```nika
spawn fn { println("I am running in the background!") }
```

### 8.3. Data Ownership in Tasks (Implicit Move)
A background task may keep running after the function that started it has
finished, so it does not borrow. Under the contextual capture rules (5.4),
`spawn` is a **detached context**: a variable used inside the task is **moved**
into it. There is no `move` keyword.

```nika
let message = "Hello"

// 'message' is implicitly moved into the task (spawn is @detached)
spawn fn { println(message) }

// Compiler Error: 'message' now belongs to the task.
// println(message)
```

**The type of the value decides between two cases.** For **ordinary data** (a
string, a number, a struct or collection of those) a move is a move. A handle
on a `Shared[T]` is **duplicated** rather than moved (6.2), so the name outside
the task keeps working. `message` here is a string, so the rest of this
section is the **data** case.

A program that needs the value afterwards clones it **before** the task is
built and gives the task the copy. A `.clone()` inside the body clones the
task's own copy and leaves nothing for the parent.

```nika
let message = "Hello"
let copy = message.clone()   // made here, while `message` is still ours
spawn fn { println(copy) }   // the copy is what moves into the task
println(message)             // OK: `message` never left
```

Data moved into a task and used again afterwards is refused with `NK2101`:

```text
error[NK2101]: You're using `message` after a background task took it.
  --> main.nika:4:5
   |
 4 |     println(message)
   |     ^
   |
   = note: A task started with `spawn` may outlive this function, so it takes the variables it uses with it. `message` is a `String`, so it went to the task.
   = note: Numbers, `bool`s, views and `Shared[T]` handles aren't taken: they're copied.
   = help: Make a copy before starting the task and give the task the copy: `let copy = message.clone()`, then use `copy` inside the task.
```

**`NK2101` belongs to the data case only.** A handle on a `Shared[T]` used again
after the task is built is not refused, and no `.clone()` is written.

`NK2101` is raised only where the type is known and a move takes the value
away. A number, a `bool`, a `scalar` and a **view** are copied, so
`let message = "Hello"` is not this case and `let message: String = "Hello"`
is. An **assignment** between the task and the later use clears it. The form
above is the one spelling of a `spawn`, the trailing lambda of 5.3. A lambda
that **names** an argument is refused with `NK2103`: a task is handed nothing.

**A task means the same thing at both values of `user_parallelism`.** At `yes`
a task goes to a pool of futures over the `user-pool` worker count and runs on
a thread of its own, and its future must be `Send`. At `no` every task
interleaves on the one thread (Part II 11.2).

### 8.4. The Runtime Sidecar Model
At `user_parallelism = no`, user code runs on one thread. The runtime uses a
**hidden sidecar** to carry out heavy I/O without blocking that thread.

* **Separation of concerns:** user code runs exclusively on the main thread
  (the event loop). A heavy operation, such as an SQLite query, is offloaded to
  a runtime sidecar: a background thread on a native target, a Web Worker on
  WebAssembly.
* **Safety guarantee:** data is exchanged by message passing (ownership
  transfer). User code never accesses the sidecar's memory, so a **race
  condition** remains impossible.
* **Non-blocking:** a database call is a pause point. The main loop never
  stalls waiting for disk I/O.

**The runtime starts before the first statement of user code**, with one I/O
thread always and a pool for user code only at `user_parallelism = yes`. What
runs on the I/O thread is `std`'s own code and nothing else: the boundary is a
closed list of operations, not a queue of closures. A file read is handed to
the **kernel** where the machine can complete it. A file operation is a
*slot*, on the ring or in an I/O worker's reply; asking whether it has finished
never blocks, so the main loop runs whatever else is ready and parks in the I/O
only when nothing is.

---

## Chapter 9: Project Organization and Visibility

### 9.1. Packages and Files
**A package is a directory.** The files in it see one another with no `use` at
all: they share one namespace, and a name declared in any of them can be written
in any other.

```nika
// file: src/parse.nika      (the same package as src/main.nika)
pub struct Row { pub id: i64 }

fn helper() -> i64 { return 41 }
```

```nika
// file: src/main.nika
fn main() {
    let r = Row { id: helper() + 1 }   // no `use`, and no prefix
    println(f"{r.id}")
}
```

Two files of one package may **not** declare the same name. A `use` naming a
file of the same package is refused.

**A package reaches another package by its name.** `use http` makes the package
`http` reachable and does nothing else. Every name from it is written with its
prefix, at every use. A package is depended on in `[dependencies]` (Part III,
13.3), and a `use` naming no dependency is refused.

```nika
use http
use request_handling as rh

fn handle(r: http::Request) -> rh::Response {
    let body: http::Body = http::read(r)
    return rh::ok(body)
}
```

The prefix falls where a name is **written**, not where a value is used:
`r.path` and `r.header("host")` carry none.

**No name is brought in**: not by a glob, not by a braced list, not one at a
time:

```text
error: `use` can't bring in single names.
  --> main.nika:1:11
   |
 1 | use http::{Request, Response}
   |           ^
   |
   = help: Write `use http`, and `http::Request` where you need it, or `use http as h` for a shorter prefix.
```

`use x as y` shortens a long prefix once, in one place.

**A prefix is introduced before it is used**: `http::Request` without
`use http` is refused. **One name per file**: two packages under the same name
in one file, by alias or by collision, are refused.

**A name denotes one thing.** A second declaration is refused with `NK1148`,
with the caret on the one that arrived. A `fn`, a `struct`, an `enum`, a
`trait` and a `grammar` declare a name. A **method** belongs to its type, so
two types may each have a `len`; a **rule** belongs to its grammar and is
reached as `Json::value`.

`use std::fs` is the one `use` with a path in it, and it names the standard
library rather than a package. **It brings no name in either**: it names a
module and the module's items are reached through it, as a package's prefix
works. A `use` whose last segment is a **type** is refused with `NK1156`. What
needs no `use` is the list in 1.3. `HashMap` is **not** on that list: it is
`use std::collections` and `collections::HashMap`.

A diagnostic names the **package** rather than the alias: the type is
`http::Request` whatever one file calls the package. Outside a project a
`.nika` file is compiled **on its own**: a package is a directory of a project,
and a directory of loose examples is a directory of programs.

### 9.2. Visibility Rules (Privacy)
Visibility is per package.

1.  **Private to its package unless it says otherwise:**
    * A function, a struct, an enum and a `comptime` binding are visible inside
      the **package** that declares them, in every file of that directory, and
      nowhere else.
    * A struct field is the same: visible throughout the package that declares
      the struct.

2.  **The `pub` keyword:**
    * An item **another package** may use is prefixed with `pub`.
    * A field another package may reach is prefixed with `pub`.
    * A **public type may keep its fields private**; 9.3 shows the ordinary
      case.

The boundary is the package and not the file. Moving a declaration from one
file of a package to another changes no visibility.

Reaching a private item or field from another package, by reading a field or
by giving one a value in a struct literal, is refused with `NK1110`, and the
message says which package keeps it:

```text
error[NK1110]: You can't use `secret` from here: it's private to `utils`.
  --> main.nika:4:5
   |
 4 |     let n = utils::secret()
   |     ^^^^^^^^^^^^^^^^^^^^^^^
   |
   = note: Everything is private to its package unless it's declared `pub`.
   = help: Write `pub fn secret` in `utils`, or use something it makes public.
```

### 9.3. Granular Control
`pub` makes an item available to every package. A private field is reached
through a constructor or a method, so a type's invariants hold outside its
package.

```nika
// file: network.nika

// Private: only usable inside its package
struct Config {
    port: i32
}

// Public: Usable by anyone
pub struct Server {
    // Private field: Can only be changed by Server methods
    config: Config,
    
    // Public field: Can be read/written by anyone
    pub name: String 
}
```

