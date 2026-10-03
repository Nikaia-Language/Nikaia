# A Tour of the Syntax

This page is for readers who already program in another language (Python, JavaScript,
TypeScript, Go, Java, C#, Kotlin or Swift) and want to see what Nikaia looks like before
they read the specification. It covers the parts you will write every day, one short
example each, and says where Nikaia behaves differently from what you probably expect.

Every snippet here compiles and runs with the current compiler. To try one, put it in
`src/main.nika` of a project (see [Getting started](getting-started.md)) and run
`nikaia run`.

The full rules are in the specification, [Part I](../docs/specification/10-nikaia-light.md).
Each section below ends with a **Specification** link to the chapter it comes from.

---

## Variables

```nika
let x = 10          // immutable: x cannot be assigned again
let mut count = 0   // `mut` says it may change
count += x
```

Values are **immutable unless declared `mut`**, as in Rust or Kotlin's `val`/`var`. Types
are inferred. You write one where you want to, `let big: i64 = 3000000000`.

There are no semicolons at the end of lines. A comment is `//` or `/* … */`.

📖 **Specification:** [Part I, §2.1 Variables and assignment](../docs/specification/10-nikaia-light.md#21-variables-and-assignment)

## Basic types

| Type | What it is |
| :--- | :--- |
| `i64`, `i32`, `u8`, `u64`, `u32` | integers. `i64` is the everyday one, and every length and index is one |
| `f64` | a floating-point number |
| `bool` | `true` or `false` |
| `char` | one Unicode character, `'a'` |
| `String` | text |

There is no `int`/`long`/`usize` zoo. An integer that overflows **stops the program** rather
than wrapping silently. Where you mean to wrap, you say so: `h.wrapping_mul(31)`.
Conversions are written with `as`: `(total as f64) / (count as f64)`.

📖 **Specification:** [Part I, §2.2 Primitive data types](../docs/specification/10-nikaia-light.md#22-primitive-data-types)

## Text and interpolation

```nika
let name = "Nika"
println(f"hello, {name}, your name has {name.len()} letters")
println("braces in a plain string are just braces: {name}")
```

`f"…"` interpolates, like Python's f-strings. A plain `"…"` never does, so JSON and CSS
can be written as they are.

You never choose between a string type and a string-slice type. There is one `String`,
and whether a value is text of its own or a view into text that already exists is the compiler's
decision.

📖 **Specification:** [Part I, §2.5 Strings, plain and interpolated](../docs/specification/10-nikaia-light.md#25-strings-plain-and-interpolated)

## Everything is an expression

```nika
let status = if count > 5 { "big" } else { "small" }
```

`if`, `match` and a `{ … }` block all have a value: the value of their last line. That is
why there is no ternary operator.

📖 **Specification:** [Part I, §3.1 Expressions and blocks](../docs/specification/10-nikaia-light.md#31-expressions-and-blocks) · [§3.2 if / else](../docs/specification/10-nikaia-light.md#32-conditional-logic-if--else)

## Loops

```nika
for i in 0..<3 {         // 0, 1, 2 — `..<` stops before the end
    println(f"step {i}")
}

let mut n = 0
while n < 5 {
    n += 1
}
```

`a..b` includes its end and `a..<b` excludes it. There is `for`, `while`, `break` and
`continue`. An endless loop is `while true { … }`.

📖 **Specification:** [Part I, §3.3 Loops](../docs/specification/10-nikaia-light.md#33-loops)

## Functions

```nika
fn greet(name: ref String; greeting: ref String = "Hello") -> String {
    return f"{greeting}, {name}!"
}

println(greet("Nika"))                       // Hello, Nika!
println(greet("Nika"; greeting: "Hallo"))    // Hallo, Nika!
```

The **semicolon in the parameter list** is the unusual part. Parameters before it are the
data the function works on and are passed by position. Parameters after it are options:
they always have a default, and they are always passed **by name**. It is the same idea
as keyword arguments in Python, made a rule.

`ref String` says the function only *reads* the text and does not keep it. Parameters
you do not mark are decided by the compiler.

📖 **Specification:** [Part I, §5.1 The subject ; config protocol](../docs/specification/10-nikaia-light.md#51-the-subject--config-protocol)

## Structs and methods

```nika
struct Point {
    x: i64,
    y: i64,
}

impl Point {
    fn length_squared(ref self) -> i64 {
        return self.x * self.x + self.y * self.y
    }
}

let p = Point { x: 3, y: 4 }
println(f"{p.length_squared()}")    // 25
let moved = p with { x: 5 }         // a copy with one field changed
```

There are **no classes and no inheritance**. A `struct` holds the data, and an `impl`
block adds behaviour to it, as in Go or Rust. Shared behaviour across types is a
`trait` (an interface).

Everything is private unless it says `pub`.

📖 **Specification:** [Part I, §4.1 Structs](../docs/specification/10-nikaia-light.md#41-structs-custom-data-types) · [§4.2 Constructors](../docs/specification/10-nikaia-light.md#42-constructors-and-instantiation) · [§4.3 Why no classes?](../docs/specification/10-nikaia-light.md#43-why-no-classes-data-vs-behavior) · [§4.7 Traits](../docs/specification/10-nikaia-light.md#47-traits-defining-behavior)

## Enums and `match`

```nika
enum Shape {
    Circle(f64),
    Rectangle { width: f64, height: f64 },
}

fn area(shape: Shape) -> f64 {
    match shape {
        Shape::Circle(r) => 3.14159 * r * r,
        Shape::Rectangle { width, height } => width * height,
    }
}
```

An `enum` is a value that is **one of a fixed set of shapes**, and each shape may carry
data. If you know TypeScript's discriminated unions, Swift's or Kotlin's sealed types,
this is that.

`match` must cover every case. Leave out `Shape::Rectangle` and the program does not
compile. When a match is not over an enum, `else =>` catches the rest.

📖 **Specification:** [Part I, §3.4 Pattern matching](../docs/specification/10-nikaia-light.md#34-pattern-matching-match) · [§4.4 Enums](../docs/specification/10-nikaia-light.md#44-enums-algebraic-data-types)

## No `null` surprises

```nika
fn find_nickname(name: ref String) -> String? {
    if name == "Nika" {
        return "Nik"
    }
    return null
}

let nick = find_nickname("Ada") ?? "no nickname"
```

A `String` is always a string. Only a `String?` may be `null`, and the compiler makes you
handle it: `??` supplies a fallback, and `?.` reaches into a value only where it exists.

📖 **Specification:** [Part I, §2.3 Nullable types](../docs/specification/10-nikaia-light.md#23-nullable-types-null-safety) · [§3.5 Null safety operators](../docs/specification/10-nikaia-light.md#35-null-safety-operators)

## Lists, maps and lambdas

```nika
use std::collections

let numbers = [1, 2, 3, 4, 5]
let doubled = numbers.map fn(n) { n * 2 }

let mut ages = collections::HashMap()
ages["Nika"] = 7
let age = ages["Nika"] ?? 0      // a key may be missing, so reading one gives a `T?`
```

A lambda is always written `fn(arguments) { body }`. When it is the last argument it
stands after the call, as with `map` above. There is no second short arrow form.

📖 **Specification:** [Part I, §4.5 Collections](../docs/specification/10-nikaia-light.md#45-collections) · [§5.2 One lambda form](../docs/specification/10-nikaia-light.md#52-there-is-one-lambda-form) · [§5.3 Lambdas](../docs/specification/10-nikaia-light.md#53-lambdas-fn---)

## Errors

```nika
enum AgeError {
    Empty,
    NotANumber(String),
}

impl Error for AgeError {
    fn message(ref self) -> String {
        match self {
            AgeError::Empty => "the input is empty",
            AgeError::NotANumber(text) => f"not a number: {text}",
        }
    }
}

fn parse_age(text: ref String) -> i64 throws {
    if text.len() == 0 {
        throw AgeError::Empty
    }
    // …
    return 42
}

fn main() {
    let age = parse_age("") catch {
        println(f"{error}")     // inside `catch`, the failure is called `error`
        0                       // …and the block supplies a replacement value
    }
}
```

A function that can fail says `throws`. What it can fail *with* is worked out by the
compiler. If a function calls something that can fail and neither declares `throws` nor
handles the failure with `catch`, it does not compile. So nothing is silently swallowed,
and there is no `try`, no `?` and no `if err != nil` on every line: an unhandled failure
simply travels up to the caller.

📖 **Specification:** [Part I, §7.1 Recoverable errors](../docs/specification/10-nikaia-light.md#71-recoverable-errors-throws)

## Waiting and doing things at the same time

```nika
use std::fs

fn size_of(path: ref String) -> i64 throws {
    let text = fs::read_to_string(path, fs::Root::Anywhere)
    return text.len()
}

fn main() throws {
    let (a, b) = overlap {        // both files are read at the same time
        size_of("a.txt")
        size_of("b.txt")
    }
    println(f"{a + b} characters in both files")

    let greeting = "hello from a task"
    let handle = spawn fn { println(greeting) }   // a background task
    handle.join()
}
```

This is where Nikaia differs most from JavaScript, Python or C#: **there is no `async` and
no `await`.** Reading a file pauses the function without blocking the program. You write
it as an ordinary call, and the compiler works out which functions can pause.

Lines run in the order they are written. Where you *want* things to happen together, you
say so: `overlap { … }` runs its lines at the same time and waits for all of them, and
`spawn` starts a task that runs on its own.

📖 **Specification:** [Part I, Chapter 8 Concurrency](../docs/specification/10-nikaia-light.md#chapter-8-concurrency-doing-things-at-the-same-time)

## Data that more than one task changes

```nika
let counter = SharedMut(0)
let first = spawn fn { counter.update fn(mut v) { v += 1 } }
let second = spawn fn { counter.update fn(mut v) { v += 2 } }
first.join()
second.join()
println(f"counter is {counter.get()}")    // counter is 3
```

A value that several tasks change is put into a `SharedMut`, and it is changed only
through its methods (`get`, `set`, `update`, `access`). The lock is part of the value, so
you cannot forget to take it. The compiler also refuses code that would wait for I/O while
holding it.

Whether this runs on one core or on all of them is not written here at all. It is one line
in `nikaia.toml` ([Getting started](getting-started.md#two-switches-one-language)), and
the same source is correct either way.

📖 **Specification:** [Part I, §6.2 Unified types](../docs/specification/10-nikaia-light.md#62-unified-types) · [§6.3 The four doors](../docs/specification/10-nikaia-light.md#63-changing-shared-data-the-four-doors) · [Part II, Chapter 12 Thread safety](../docs/specification/20-nikaia-advance.md#chapter-12-thread-safety-and-synchronization)

## Memory

There is nothing to write. There is no garbage collector and no `free`: a value is cleaned
up at the end of the block that owns it, and a file or a socket is closed there too. When
the compiler needs a copy that you did not ask for, it refuses and tells you to write
`.clone()`, so every copy is visible in the source.

📖 **Specification:** [Part I, Chapter 6 Memory and ownership](../docs/specification/10-nikaia-light.md#chapter-6-memory-and-ownership)

## Modules

```nika
// file: src/stock.nika
pub struct Item { pub name: String, pub count: i64 }
```

```nika
// file: src/main.nika
use std::fs            // a standard-library module: every name from it keeps its prefix

fn main() {
    let item = Item { name: "apples", count: 3 }   // same package: no `use` at all
    println(f"{item.count} {item.name}")
}
```

A package is a directory, and the `.nika` files in it simply see each other. Another
package is reached by its name, and what comes from it is always written with its prefix,
`fs::read_to_string(…)`. Nothing is imported unprefixed, so you always see where a name
comes from. `pub` makes something visible outside its package.

📖 **Specification:** [Part I, Chapter 9 Project organization](../docs/specification/10-nikaia-light.md#chapter-9-project-organization-and-visibility)

## Grammars

Nikaia can describe a text format as a **grammar** inside the program and get a parser
for it, instead of splitting strings by hand or reaching for regular expressions. It is
too large for this page. [`examples/calc/src/main.nika`](../examples/calc/src/main.nika) is a complete
calculator in about 100 lines.

📖 **Specification:** [Part II, Chapter 10 Metaprogramming](../docs/specification/20-nikaia-advance.md#chapter-10-metaprogramming-code-that-writes-code)

---

## Where next

* [Getting started](getting-started.md): install the compiler and run a project.
* [`examples/`](../examples/): complete programs that compile and run.
* [Specification, Part I](../docs/specification/10-nikaia-light.md): the rules, one
  chapter at a time.
