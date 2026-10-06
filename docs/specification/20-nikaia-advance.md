# Nikaia Language Specification
**Part II: Advanced Features & Metaprogramming**
**Version:** 0.0.568 (Draft)
**Date:** 2026-10-06

---

## Chapter 10: Metaprogramming (Code that writes Code)

Metaprogramming extends the language from user code. Nikaia does not do it with text replacement, and it does not do it by writing code as data. Two mechanisms exist, and they are kept apart. A **`grammar`** reads a language the compiler does not know (10.1, 10.5). **`comptime`** reads what the compiler already knows: a type's shape (10.3). Nothing is generated behind user code.

> A piece of code can be understood by reading it, because nothing was generated behind it.

### 10.1. Parsing with `grammar` (Scannerless)
A `grammar` is a parser written declaratively. It reads a data format or a language the compiler does not know.

Nikaia grammars are **scannerless**: there is no separate tokenizer stage. A grammar consumes the raw character stream directly and tokenizes only its own body. A foreign language can therefore be embedded in a Nikaia file (10.5).

**Key features:**
*   **Commit points (`=>`)** control backtracking. Once the parser passes a commit point, it stays in that branch. A later failure is an error, not a reason to try the next alternative.
*   **Lexical and syntactic rules.** A rule whose name starts with an uppercase letter is **lexical**: no whitespace is skipped between its parts. A rule whose name starts with a lowercase letter is **syntactic**: whitespace is skipped.
*   **Typed actions (`{ … }` after the pattern).** Each rule builds the program's own types directly.

```nika
grammar Json {
    entry rule value -> Value =
        o:object { Value::Object(o) }
      | a:array  { Value::Array(a) }
      | s:string { Value::String(s) }

    // Commit point: once '{' matched, members and '}' MUST follow, or we error.
    rule object -> Object
        = "{" => members:list(pair, ",") "}"
        { Object { members } }

    rule pair -> Pair = key:string ":" => val:value { Pair { key, val } }

    // Lexical rule (uppercase): no whitespace inside a hex byte. Two hex digits
    // always fit in a `u8`, so this action cannot fail; where one can, what its
    // failure means is decided by the commit point above it.
    rule HEX -> u8 = d:hex_digit{2} { hex_byte(d) }
}
```

### 10.2. Dual-Mode Parsing (Static vs. Dynamic)
A grammar defined once runs at compile time and at runtime, **with the same
syntax and the same meaning**. A grammar is entered by an **ordinary call**.
Every `entry rule` in it is an entry named after the rule: `Json::value(x)`
runs the rule `value` of the grammar `Json`. A rule without `entry` is the
grammar's own, and a call to it is refused. Without `pub`, a grammar is
entered only from the file that declares it; `pub grammar` offers all of its
entries to other packages at once - a rule is never `pub` on its own (ADR-296
D25). The word in front of the binding decides
*when* the call runs.

**The separator is `::`, as it is for every other qualified name.** The dot is
for a **value's** members. `Json.value(x)` is refused with `NK1147`.
`T::fields` in 10.3 follows the same rule.

**A. Static Embedding (Compile-Time)**
In a `comptime` binding the parser runs *during the build*. Invalid input fails
the build, in the parser's own words. The result is embedded in the binary at no
runtime cost.

Three words each decide one thing. `comptime` says **when**. `asset("…")` says
**where the bytes come from**. The call says **what is done with them**.

```nika
// The compiler runs the Json grammar at build time.
// If "config.json" is malformed, the build stops.
comptime CONFIG: Json::Value = Json::value(asset("config.json"))
```

**B. Dynamic Parsing (Runtime)**
The same grammar processes user input or network data while the program runs.

```nika
fn parse_input(input: String) throws {
    let data = Json::value(input)
    println(f"Parsed: {data}")
}
```

**The `comptime` declaration.** `comptime` declares a binding whose initialiser
is evaluated while the program is built. It stands where an item stands and
inside a function body. Its type may be written or omitted. What reaches the
emitted program is the value, not the expression: `comptime LIMIT = 4 * 1024`
arrives as `const LIMIT: i32 = 4096;`. A `let` may be folded while the program
is built; a `comptime` must be. A `comptime` binding the compiler cannot
evaluate is refused with `NK1127`. It is never evaluated at run time instead.

**What an initialiser may hold:** an integer, a float, a `bool`, **text**, a **list**, an **`enum` variant**, a
**pair** or a **struct**: literals, `f"… {n} …"`, arithmetic and comparisons
over them and over other constants, `+`, `==` and `.len()` over text, `xs[i]`,
`xs[i] = …`, `xs.push(…)` and `xs.len()` over a list, a struct literal and a
field of one, an `if`, and a **call** to a function or a **method** that this
program, `std` or a dependency declares. A called body must be `sync` and must
touch nothing but the build's own parameters, as the ledger records it; a
function the ledger does not describe touches everything. For a Nikaia
dependency the build reads the entries it derives from the dependency's
sources, and refuses where they disagree with the ledger the dependency ships;
a dependency without sources runs nothing at build time. No Rust code runs at
build time but `std`'s, and no C: a function of a Rust crate or an
`extern` block is refused, and so is a function whose calls reach one. A
grammar's actions call under the same rule, its own package's functions and its
Nikaia dependencies among them. Nothing lets foreign code run while the program
is built. A callee that fails either condition is refused with
`NK1152`. `NK1127` says *not yet*; `NK1152` says *not allowed*.

**`comptime` marks the name.** It is not an expression: `let x = comptime f()`
does not parse, and a function is never marked.

**An integer `comptime` without a written type is an open number** (Part I
2.4): each use takes it in the type the use asks for, and the value is checked
against that type (`NK1116`). Two uses may ask for two types. Where no use asks,
it takes the type of its expression: a call's result type, or, for an
expression of literals alone, the first type that holds the value. A written
type, `comptime A: i64 = big(1)`, holds at every use. The body that computes the
value keeps its own types: an overflow there stops the build as it would stop
the program.

A `comptime` at item level may read one declared below it; a ring of them is
refused with `NK1168`. A `comptime` inside a function body is read in written
order, as a `let` is: one declared further down is `NK1117`. It may read an
item-level one.

**A `comptime` whose initialiser calls a function is compiled and run** on the
machine that builds, against the package's dependencies, and behaves exactly as
the same code does when the program runs: an overflow stops the build where it
would stop the program. An initialiser of literals alone is folded by the
checker. A package's build-time code is compiled once per change and its
results are kept; an unchanged `comptime` is not run again.

**A build-time run has a step budget and a memory bound.** Each turn of a loop
and each call counts as a step, and a call into `std` counts as one; the
default budget is 10 000 000 000 steps. The bytes the run holds are counted at
each allocation, `std`'s included; the default bound is 4 GiB. Past either, the
build is refused with `NK1152`, naming the `comptime` and the call path. Both
counts are the same on every machine of one architecture. A `comptime` that
needs more raises the bound itself, at its declaration:
`comptime(steps: 50G, ram: 16Gi) TABLE = …`. Either may be left out; a name
other than `steps` or `ram` is refused (`NK1109`). The call depth is
bounded as well. A run that takes more than a few seconds is reported by name
while it runs.

**What crosses from build time to run time.** A result arrives in its **view**
form: `Vec[T]` as a `ref Array[T]`, or as an `Array[T, N]` where the program
writes the length into the type, and `String` as a `ref String`. A run of
`struct`s arrives as a `ref Array[T]`. A value built with `push` is fixed once
it has crossed. A value that owns memory is refused by what it *is* and never by
its parse: by its **type** for a `struct`, and for an `enum` by the **variant
the value is**. `Shape::Empty` is a `const` and `Shape::Many([1, 2])` is not,
though both are a `Shape`. A value of a `std` type crosses where its ledger
entry names a `constant` (Part III 13.5): it is written as that constructor over
its parts, so `comptime T: time::Duration = 30.seconds()` is a `const`. A `std`
type without one is refused with `NK1167`. A grammar call's result crosses when it is a whole
number, a float, a `bool`, text, a list, a `struct` or an `enum` variant. A rule
that hands back a value it built into a position that views a run is refused
with `NK1179` on the action's line. A map that crosses is a **fixed** map with a
closed key set. How it is looked up is the compiler's decision.

**A map the build can see is written as a list of pairs**, and the declared type
says it is a map. There is no map literal:

```nika
comptime ROUTES: Fixed[ref String, i64] = [("get", 1), ("post", 2)]
```

`Fixed[ref String, V]` is the fixed map, a type in `std`. `ROUTES.get(k)` is a `V?`.
Keys are text; a key of any other type is refused with `NK1170`, and a key
written twice with `NK1169`. **A value may be a `struct` or an `enum` this
program declares**, and what the program reads is a **view** of the row, which
lives in the binary. `TABLE.get(k)?.field ?? …` reaches a field of one (2.3). A
list of text crosses as an array of views, `Array[ref String, N]`.

**Reading a file while the program is built.** A build given no allowlist reads
nothing. A file the build reads is named three times: in the source, as the
`asset("…")` literal; in an allowlist file, one path per line; and in the
invocation that puts the list in effect, `--allow-read-from-list=…`. A read
missing one of the three is refused with `NK1175`, which names the missing one.
The path may not be computed; a computed path is refused with `NK1176`. There
are no patterns. `asset("…")` yields the file's **text**, which crosses to the
program as the `ref String` a `const` holds; bytes that are not UTF-8 are
refused. An `asset` written outside a `comptime` is refused with `NK1177`,
naming the run-time read. The list binds the whole build, a dependency's read
included. A path stays inside the project: one with a `..` component, or an
absolute one, is refused. A list entry nothing reads is a warning at the end
of the build.

### 10.3. Generating Code from a Type's Shape
Where 10.1 reads data, this section reads **types**. A function such as
`describe` is written once and works for every struct.

**There is no macro system**: no `macro`, no code written as data, nothing
attached to a declaration. A type's shape is **ordinary data**, and a loop over
it runs while the program is built.

```nika
fn describe[T: Struct](value: T) {
    for field in T::fields {
        println(f"{field.name} = {field.of(value)}")
    }
}
```

**The bound makes `T::fields` exist.** `T: Struct` is an ordinary bound (4.7),
answered from the **declaration** rather than an `impl`. It says that a shape
may be asked for. A `T: Enum` has `T::variants`, the variants in declared
order. There is no builtin and no
special syntax: **reflection is reached as a member**, and `field.of(value)` is
a method on the reflected field. The separator is `::` (10.2).

A caller that passes something that is not a struct is refused with `NK1164`
**once, at the call**. What remains inside the body is per-field. A
`[T: Struct]` function whose body never writes `T::fields` is an ordinary
generic function.

**The loop needs no second keyword.** `T::fields` is a list of reflected fields
known while the program is built, so a loop over it is unrolled, once per type a
call gave the function. The loop means what a loop always means: *walk these
elements*. Only the **stage** is earlier.

**The body is checked once per unrolled turn.** `field.of(value)` is a `String`
for one field and an `i32` for the next. What is emitted is one copy per type,
with no generic original: the `println` calls a program would have written by
hand, with no loop and no dispatch at run time. At build time it costs fields ×
the types actually used. An ordinary `for x in xs` over a run-time list is
checked once.

A body that is wrong for one field is wrong at one unrolled copy. The diagnostic
names the field:

```text
error: `println` cannot format a `Vec[u8]`
  --> describe.nika:3:9
     = unrolling `T::fields` for `User`, at field `avatar`
```

A reflected field answers `.name`, its name as text, and `.of(value)`, what it
holds on that value. A reflected variant answers `.name` and `.is(value)`,
whether the value is that variant; what a variant carries is read with a
`match`. Any other member is refused with `NK1180`. `T::fields` without a
`Struct` bound, or `T::variants` without an `Enum` bound, is refused with
`NK1171`.

**What cannot be read is printed.** `--comptime` (on `nikaia build` and `nikaia lower`) prints what was
unrolled, once for the program, for the types actually used, as `--overlaps`,
`--sharing`, `--tethers` and `--trust` print their own analyses. There is no
syntax for it. A function that walks a shape and is **never called** has a line
of its own.

`with` is a reserved word (Part I 2.1); `macro` and `quote` are ordinary names,
and a program may use them as names.

Capturing an *expression* as a tree is not part of the language. A query is
written in the database's own SQL and checked by the driver while the program is
built (10.5). Reflection describes types and does not capture expressions.

### 10.4. Hygiene, and Why the Question Dissolved
Nikaia has no hygiene rule. Nothing is injected from elsewhere. The loop body of
10.3 is written by the program, in place. A name bound inside that loop is bound
where it is written, as a name bound in any other block is.

### 10.5. Using DSLs (The `dsl` Keyword)
The `dsl` keyword embeds **foreign syntax** in a Nikaia file: SQL, HTML, regex, assembly. It hands a byte slice to a grammar that knows a language the compiler does not.

**The protocol:**

1.  **Explicit termination.** A DSL block ends with `} eod` ("end of DSL"). The core parser does not count braces in the body. The compiler scans ahead for the marker, skips the body, and **parses the rest of the file in parallel** while the DSL parser works.
2.  **Scannerless delegation.** The core hands the isolated byte slice to the grammar. There is no global lexer.
3.  **Hybrid binding.** A DSL author chooses per hole between a compile-time capture and a runtime parameter (below).
4.  **Subject `;` config.** Runtime parameters are *configuration*. They are passed as named arguments after the `;` (Part I 5.1).

**Hybrid Binding: immediate vs. deferred**

| Intrinsic | When it resolves | Use for |
| :--- | :--- | :--- |
| `meta::capture(id)` | compile time, from the surrounding scope | assembly operands, table names — anything meaning *this variable, here* |
| `meta::parameter(name, type)` | runtime, as a named argument | SQL placeholders — anything the statement should be *reusable* over |
| `meta::column(name, type)` | build time, declared by the grammar | the statement's **result**: one field per column, so a row is a type |

`meta::column(name, type)` gives a grammar control over the statement's result type. The compiler builds the row type from the declared columns, one typed field per column, as it builds the parameter type from the holes. The `type` of `meta::column` and of `meta::parameter` is a `std::meta::Type`, a type described as data: `I64`, `I32`, `U8`, `U64`, `U32`, `F64`, `Bool`, `Text`, `Bytes`, `Maybe(t)` for `t?`, `List(t)` for `Vec[t]`, `Record(fields)` for a struct the compiler writes, and `Named("Mode")` for a type the program declares, resolved where the block stands (`NK1135` if nothing declares it) ([ADR-323](adr/adr-323.md)). A message about a field of a type the compiler wrote names the block and the grammar that declared it, and `--comptime` prints the type of each block.

**A grammar from a package is named with its package** ([ADR-299](adr/adr-299.md) D20): `dsl sqlite::Sql { … } eod`, as every name from a package is (Part I 9.2), with no short form. A grammar the program declares is named bare: `dsl Json { … } eod`. A block that names a package where a grammar stands is `NK1228`.

**How a row's fields are named** ([ADR-299](adr/adr-299.md) D12-D14). The row has one field per table of the query, named by its alias or its table, holding that table's columns, and one flat field per column whose name is unique in the row: `u.a.id`, and `u.name` where only one column is called `name`. Where two columns share a name, or a column is named as a table or an alias is, neither is a flat field, and reading it is `NK1107`, whose help names the qualified paths and `AS`. A column named with `AS` is a flat field. A column whose name cannot be a field is refused at the query, asking for `AS`. A database driver's grammar reads the statement and the schema and declares one column per result column. The schema is a build-time argument of the block, `dsl sqlite::Sql(schema: APP) { … } eod`, resolved from a `comptime` value. A column the schema lacks is refused by the grammar at the query, while the program is built. The compiler does not understand SQL; the driver owns the grammar, and a vendor's database is a package.

**A block may name the struct its rows are** ([ADR-299](adr/adr-299.md) D15-D19): `dsl sqlite::Sql(schema: APP) -> Contact { … } eod`, and each row is a `Contact`. Columns match fields by name, never by position: a field of a scalar type matches the flat column of its name, a field of a struct type named like an alias or a table matches that table's columns, by the same rules. Every column needs a field (`NK1219`) and every field a column (`NK1220`); `extra_columns: true` leaves a column without a field unread, and `missing_fields: true` makes a `T?` field without a column `null`. Both are the compiler's, taken from any block with `-> T` and never handed to the grammar. A flat field whose name two tables' columns share is `NK1222`. A column that may be `NULL` goes only into a `T?` field (`NK1221`); one that cannot may go into either. A `NULL` that reaches a field that is not a `T?` while the program runs makes `execute` fail with an error naming the field, the column and its table. A field is a scalar, its `T?`, or a struct for a group; anything else is `NK1218`.

**Example: Embedding SQL**
SQL written in a `dsl` block is verified while the program is built, against the
schema the connection is typed by ([ADR-299](adr/adr-299.md)).

```nika
use sqlite                                // a driver package, not `std`

comptime APP = asset("schema.sql")        // the schema, read while building

fn query_users(min_age: i32) {
    let db = sqlite::open("app.db"; schema: APP)   // typed by its schema

    // 1. The driver's grammar parses this SQL while the program is built,
    //    checks it against the schema, sees ':target_age' (a parameter hole)
    //    and declares the result columns; the compiler derives the row type.
    let query = dsl sqlite::Sql(schema: APP) {
        SELECT name, email FROM users WHERE age >= :target_age
    } eod

    // 2. Subject: the connection; after the `;`, the statement's parameters
    //    (Part I 5.1). Omitting 'target_age' is a compile error.
    let users = query.execute(db; target_age: min_age)
    for u in users { println(u.email ?? "-") }
}
```

`execute` hands back a `Seq` of the derived row, so `u.email` is checked; a
statement handed a connection opened for another schema is refused at the call
([ADR-299](adr/adr-299.md) D7-D11). A database driver writes no `execute` of its
own: it implements `std::db`'s `Connection` trait, and the compiler turns each
row into the row type it derived (D8).

Every other DSL a library drives accepts its parameters with the **typed
spread**:

```nika
impl Template {
    // Subject: self (the parsed block) ; Config: the DSL's parameters
    pub fn prepare(ref self; ...args: Self::dsl) -> Self::dsl {
        return args
    }
}
```

`Self::dsl` is the constraint that ties these named arguments to this grammar. The compiler monomorphizes `args` per DSL string and allocates it on the stack. There is no heap traffic per call.

**What the call site is checked against.** The statement's parameters are read
out of the body as written. A call that omits one is refused with `NK1112`. A
call that passes a name the statement has no hole for is refused with `NK1113`:

```text
error[NK1112]: `query` needs `:target_age`, but this call doesn't pass it.
   --> users.nika:17:5
    |
 17 |     let users = query.execute(db; targt_age: min_age)
    |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    |
    = note: The statement's parameters are `:target_age`.
    = help: Pass it after the `;`: `target_age: …`.
error[NK1113]: `query` has no parameter called `:targt_age`.
   --> users.nika:17:5
    |
 17 |     let users = query.execute(db; targt_age: min_age)
    |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    |
    = note: The statement's parameters are `:target_age`.
    = help: Did you mean `target_age`?
```

A `:name` is a hole where the body wrote `:` and a name. A path (`a::b`) and a
time (`12:30`) are not holes. The same `:name` twice is one parameter. Which
holes a body has is the **grammar's** answer.

### 10.6. Advanced Parser Features
Nikaia grammars serve tooling that reads large inputs.

**Zero-Copy Parsing**
A Nikaia parser never copies the source text. A grammar picks per rule between two mechanisms:

*Identifiers and keywords are interned symbols.* The parser **interns** an identifier: each distinct spelling is stored once in a shared table, and the parser yields a small `Symbol` handle. Comparing two identifiers is an integer comparison.

*Bulk text is a tethered slice.* String literals, comments, doc text and DSL bodies are not interned. The parser yields **slices** into the original buffer. Whether a slice stays a plain reference or becomes a **tether**, the source buffer moved into a keep and the slice a plain reference, follows Part I 6.6: transient is a borrow, escaping is a tether. A token that lives and dies inside the scope holding the source text costs nothing. A token that outlives that scope is tethered automatically. An AST handed back to a caller keeps its source alive; the caller's frame keeps the buffer, and no token carries a handle.

**The source text cannot be freed while any token still points into it.** User code does not manage the relationship and meets no error about it. The cost is nothing where a frame outlives the tokens, one count where a task keeps them, and nothing for a token that never escapes.

**What a rule is called, when it fails where it began**

A grammar rule with several alternatives fails by reporting what each of them
could have started with. A rule may name itself, between its return type and
its `=`:

```nika
rule expr -> Expr # "expression" =
      c:closure_expr { c }
    | e:catch_expr   { e }
```

The name replaces the list **only where the rule failed at its own starting
position**: where none of its alternatives consumed anything. A rule that got
further reports what it was in the middle of: `(1` reports the missing `)`, not
`expected expression`.

The same syntax after a single alternative names that alternative.

**Fault Tolerance (Recovery)**
A grammar declares **synchronization points**: where a rule fails, the parser discards input until it reaches the sync token, then resumes and continues parsing the rest of the file.

```nika
// If parsing fails inside the block, skip ahead to the closing brace
// and carry on - the errors after this point are still worth reporting.
rule block -> Vec[Stmt] =
    "{" => statements:recover(stmt, "}")* "}" { statements }
```

The commit point (`=>`) and the recovery work together. Once the opening brace matched, a failure inside the block is reported rather than abandoning the alternative. The commit point decides where an error is reported; the recovery decides where parsing resumes.

### 10.7. Parallel Parsing

Nikaia parallelises the *parse itself* when the grammar declares where the input may be cut and how two pieces combine.

**Where the file may be cut.** A rule marked `@frame` declares that it can be found from an arbitrary offset by scanning to the next **boundary**, with no knowledge of what came before:

```nika
// Up to ";" - or to the end of the frame, whichever comes first. `frame_end`
// is the boundary of the frame this rule is reached from: written once, in
// the attribute below, and referenced here.
rule NAME -> ref String = s:until(";" | frame_end) { s }

@frame(boundary: "\n")
rule MEASUREMENT -> Reading =
    name:NAME ";" => temp:TENTHS frame_end { Reading { name, temp } }
```

A bare `@frame` takes the boundary from the rule's trailing literal. A frame that ends in `frame_end` names the boundary in the attribute. A frame must end in its boundary. A frame is usually a **lexical** (uppercase) rule. A syntactic one, and a syntactic rule a frame reaches, is accepted where the whitespace it skips cannot consume the boundary: a grammar's own `WS` is walked like any other rule, and the default `WS` is refused for a boundary it can match, such as `"\n"`.

**The declaration is checked, and a pattern is never rewritten.** The boundary may not appear *inside* a frame. The compiler walks everything the rule can reach, and each element that consumes input is one of two things:

* **Safe:** a literal without the boundary in it; a built-in that cannot produce it (`digit`, `ident`); lookahead; an `until(…)` whose terminator *covers* the boundary, as `NAME` above does with `frame_end`.
* **Rejected**, with the rule and the pattern named: a literal that contains the boundary; a built-in that can consume it (`any`, `multispace0`); the whitespace a syntactic rule skips, where it can consume the boundary; an `until(…)` that does not cover the boundary, where the message says what to add; and `recover(…)`. A grammar recovers per frame instead: `(item | until(frame_end)) frame_end`.

Nothing here changes what a pattern means. `until(";")` consumes up to the next `;` wherever it is written; inside a frame the compiler refuses it and names what to write. The parser generated for a rule is the same whether or not a frame reaches it.

**When the check cannot see through the format.** `@frame(boundary: "\n", unchecked)` skips the check. The program asserts the invariant, as with `unsafe`.

**How two pieces combine.** The entry rule folds with a **merge**:

```nika
entry rule file -> Summary =
    par_fold(MEASUREMENT, Summary, fn(acc, m) { acc.record(m) }, Summary::merge)
```

`par_fold` is `fold` plus that merge, and it must be the whole body of its rule. The compiler cuts the file into one piece per core. Each piece repairs its own start to the next boundary, so every frame belongs to exactly one worker. Each worker folds into its own accumulator with nothing shared. The accumulators are merged at the end. A failing piece reports its error at its position in the whole file. Nothing about chunks appears in user code:

```nika
let data = fs::map(path, fs::Root::Anywhere)
let totals = Measurements::file(data)
```

**Pieces and the whole agree.** A `par_fold` rule's parser skips no whitespace at its entry, unlike every other rule. A frame that begins with a space keeps it. Whitespace-only text between two frames is an error, in pieces and in one go alike. The number of cores does not change the answer, on inputs the grammar accepts and on inputs it rejects.

**`par_fold` is written, never inferred.** The compiler does not turn a `fold` into a `par_fold`, even where the step looks associative. A written `par_fold` states that a different chunk count gives the same result.

At `user_parallelism = no`, `par_fold` runs as an ordinary sequential `fold`: same accumulator, same merge, same result, no threads. A grammar written this way compiles unchanged for `wasm32`.

**What the declared format buys.** The generated parser may scan for a separator or a frame boundary a machine word at a time, on every target and with no `unsafe`. A terminator with up to three alternatives, such as `until(";" | frame_end)`, is one scan.

### 10.8. The Grammar's Vocabulary

Everything a grammar may write, one line each. An element not on this page is
not in the language. Where a line says *text*, the value is a view of the input
and nothing is copied (10.6).

**A rule.** `rule name -> Type = pattern { action }`. The `->` names the
result type; the block after the pattern is the action, one per alternative,
and it may read every binding of that alternative. `entry` before `rule` makes
the rule an entry a call can reach (`Json::value(input)`).
A rule may take arguments and be used as `list(pair, ",")` is.

**An action may not pause.** A call that can pause is refused where it stands
with `NK2209`. A fold's `init`, `step` and `merge` are action code. An action
may **fail**, which is what an entry's `throws` is about: an entry throws
`ParseError`, which carries the parser's rendered message and nothing else. A grammar may be
entered from a body that pauses; what is refused is a pause *inside* the parse.

**An entry is therefore `sync`**, and `nikaia.contracts` says so. A function
whose body is one `Json::value(text)` is `sync`.

**Lexical and syntactic.** A rule whose name starts with an **uppercase**
letter is lexical: nothing is skipped between its elements. A lowercase name
is syntactic: the `WS` rule is matched between elements. `WS` defaults to any
run of whitespace. A grammar that declares `rule WS = …` says what is skipped,
and `rule WS = "" { }` skips nothing.

| element | matches | yields |
| :--- | :--- | :--- |
| `"text"` | that text, exactly | nothing |
| `x:p` | `p`, and binds what it yields to `x` for the action | — |
| `p q` | `p` then `q` (whitespace between them in a syntactic rule) | each binding |
| `p \| q` | `p`, or where `p` fails without a cut, `q` | the alternative's action |
| `p => q` | the **cut**: after `p`, `q` must follow; a failure in `q` is an error and no enclosing alternative is retried | as `p q` |
| `p?` | `p` or nothing | a `T?` |
| `p*`, `p+` | zero or more, one or more `p` | a list of what `p` yields, or the text matched where `p` is a character class |
| `p{n}`, `p{n,}`, `p{n,m}` | exactly, at least, between `n` and `m` times; greedy and never giving one back — written with the brace against the digit, since `p { 1 }` is `p` with an action | as `p*` |
| `( p )` | grouping | as `p` |
| `[ p ]`, `{ p }`, `paren( p )` | the delimiter around `p` in the **input**: `[`, `{`, `(` | as `p` |
| `not(p)` | succeeds where `p` does not match, consuming nothing | nothing |
| `peek(p)` | `p` without consuming it | as `p` |
| `until(p)` | everything up to `p`, not consuming `p`; an error where `p` never comes | text |
| `text(p)` | `p`, handing back the text it covered instead of its value | text |
| `dec[T](p)` | `p`, its text read as the number type `T`; an error where it does not fit | `T` |
| `intern(p)`, `ident` | `p`'s text interned (10.6); `ident` is an identifier, interned | a symbol |
| `raw_ident` | an identifier | text |
| `string` | a quoted string, quotes excluded | text |
| `char` | a character literal such as `'\n'` | a `scalar` |
| `any` | one scalar | a `scalar` |
| `digit` | one decimal digit; `digit+` is a run of them | a `scalar`; text for the run |
| `alpha1` | a run of letters | text |
| `hex_digit` | one hexadecimal digit | a `scalar` |
| `multispace0`, `multispace1` | zero or more, one or more whitespace characters, newlines included | text |
| `line_ending` | `\n` or `\r\n` | nothing |
| `empty` | nothing, always succeeding | nothing |
| `eof` | the end of the input | nothing |
| `frame_end` | the boundary of the enclosing `@frame` (10.7) | nothing |
| `fail("message")` | never; fails with the message | — |
| `recover(p, sync)` | `p`, and where `p` fails, skips to `sync` and reports the failure without stopping the parse | `p`'s value or the report |
| `list(item, sep)` | `item`, separated by `sep`, zero or more | a list |
| `fold(p, init, step)` | `p` repeated, each value folded into an accumulator started from `init`; nothing is collected | the accumulator |
| `par_fold(F, init, step, merge)` | `fold` over the `@frame` rule `F`, cut into pieces and joined with `merge`; the whole body of its rule (10.7) | the accumulator |

**Names for failures.** `rule expr -> Expr # "expression" = …` names the rule
for the message where it fails at its own start, in place of the list of what
its alternatives could have begun with (10.6). `alt # "label"` after one
alternative names that alternative alone.

**Frames.** `@frame(boundary: "\n")` before a rule says a record can be
found from any offset by scanning to the boundary, which lets the input be cut
and parsed in parallel; the compiler checks that no rule reachable from it
consumes the boundary in the middle (10.7).

**What is not written.** `tag("x")` is `"x"`, and `digit1` is `digit+`. A
grammar that writes either is refused with `NK1187`, which carries the spelling
to use.

## Chapter 11: Running Your Code at Once

The build option `user_parallelism = yes` (1.2) turns the same source into a program that uses more than one core. Nothing in the source changes.

### 11.1. Implicit Async & The Scheduler
The syntax is identical at both values of the option. A function definition carries no `async` keyword.
* **At `no`:** functions yield on I/O events, cooperatively, on one thread.
* **At `yes`:** the runtime runs tasks on a pool of worker threads; `user-pool` in the runtime configuration sets their number.

User code is written in direct style, as if it were synchronous. The compiler places the suspension points. `async` and `await` are not words of the language.

### 11.2. Parallelism via spawn
Calling a function runs it now. A task that runs concurrently or in parallel is started with `spawn`.

#### The @detached Contract
`spawn` is declared with the `@detached` attribute, which gives it **implicit move semantics**: a captured value moves into the task. The parent scope cannot use the captured data while the detached task owns it. A use after the move is refused with `NK2101` (Part I 8.3).
* **Ordinary data is cloned.** A program that keeps **data** in the parent, a string, a number, a struct or a collection of those, calls `.clone()` before spawning, and the copy is what the task takes (Part I 8.3).
* **A handle is duplicated.** A handle on a `Shared[T]` is **duplicated** where it is handed to the task, so the name in the parent keeps working and there is nothing to call. Copying a handle copies none of the data (Part I 6.2). `--sharing` names the duplication site.

A move is refused only where the type is **known** and a move takes the value away. A number, a `bool`, a `scalar` and a view are *copied*, so the parent keeps them: `let message = "Hello"` is not this case, and `let message: String = "Hello"` is. A value whose type nothing describes is not refused (Part III C.4). An assignment between the spawn and the later use clears the refusal.

#### What a task may take with it
At `yes` a task runs on a thread of its own, so **everything it uses must be able to cross a thread**. The compiler checks that structurally, with no syntax to write. A type built out of plain data may cross, and so may a struct or collection of those. A value that counts its owners may cross when **what it holds** may cross; which count it gets follows that answer. **A lock may cross** into a task of the program's own. A `SharedMut[T]` may therefore be used by a task, and a struct holding one is no worse than the field. A lock may **not** go to code nothing written down describes (Part III 15.2): there the caller opens the lock and hands over the value inside it, so the called code sees an ordinary value and no lock.

The answer is **the same at both values of the option**. The question is asked of a value **and a destination**, and each destination gets one answer that does not depend on the option. The option changes only whether the build performs the crossing: at `no` nothing in user code runs concurrently, and the refusal is a lint rather than an error. The diagnostic is `NK2501` (Part III C.5). At the foreign destination a lock is refused with `NK2503` (Part III C.6) and a `Shared[T]` with `NK2502`.

**The nesting rule does not reach into a spawned task.** A `spawn`'s body runs later and elsewhere, not during the call. The rule that refuses a lock taken while a lock is held (12.3) does not reach into it. A scope is the other case, because it waits for its tasks (12.7).

**A lock may be handed to a spawned task.** A `SharedMut[T]` may be captured by a `spawn` at both values of the option, and the counter of 12.2 is a program user code can write. Two handles are two places at one lock. A task may also build a lock of its own, which shares nothing.

#### Return Values & Handles
`spawn` returns a `TaskHandle`. At `yes` it represents a running thread; at `no` a scheduled event. `.join()` on it behaves identically at both, which is what the *uniform API* below means.

**`.join()` is the one spelling, and there is no `.await`.** A Nikaia program contains the word `await` nowhere (Part I 8.1). `.join()` is where two **tasks** meet again, and it is a suspension point. The compiler writes the `.await` in the emitted Rust.

```nika
fn process_image(path: String) -> Image { ... }

fn main() {
    let img_path = "a.jpg"

    // 'spawn' is a @detached context.
    // 'img_path' is implicitly moved into the task.
    let handle = spawn fn { process_image(img_path) }

    // Compiler Error: img_path is gone.
    // println("Processing: " + img_path) 

    // Uniform API: the same at either `user_parallelism`
    let result = handle.join()
}
```

`join` does not fail. It hands back the body's last value, and a body that can fail hands back a value that says so, as any other function does.

---

## Chapter 12: Thread Safety and Synchronization

At `user_parallelism = yes` user code runs on several CPU cores at once. The rules of this chapter keep shared data intact.

### 12.1. The `sync` Keyword (CPU Constraints)
Every function in Nikaia may pause unless it says otherwise. The `sync` keyword marks a function that **never pauses** and is never moved between threads mid-execution.

* **Constraint:** a `sync` function calls **only** `sync` functions.
* **What it is not:** `sync` is not a promise that the function has no *effect*.
  `println` is `sync`: it writes and returns, and it never suspends. A panic
  hook is `sync` and may block. What a function **reaches** is a second column,
  answered by `touches` (13.5).

Two worlds follow: the pausable world, which is the default, and the **`sync` world** of computation.

```nika
use std::fs

// 'sync' guarantees one thing: I will never pause.
fn calculate_physics(mut obj: Object) sync {   // `mut`: it changes the caller's value (6.5)
    obj.x += obj.velocity
    // fs::read("log.txt", fs::Root::Anywhere) // Compiler Error: that call can pause, and this cannot
}

// Usage in Parallel Iterator
// par_iter requires a 'sync' closure because it runs purely on CPU cores.
// A trailing lambda goes outside the parentheses.
particles.par_iter().for_each fn (p) { calculate_physics(p) }
```

**The word is not required for the rule to be satisfied.** A function that
calls nothing which can pause cannot pause, and the compiler infers that from
the body. The helper below needs no annotation to be callable from a `sync`
function, from `access` (12.2), or from a `par_iter` body:

```nika
fn bonus(score: i32) -> i32 { score * 2 + 1 }   // no `sync`, and cannot pause

fn total(p: ref Player) -> i32 sync {
    return p.score + bonus(p.score)             // fine: `bonus` provably cannot pause
}
```

Every construct in this chapter that makes concurrency safe demands a `sync`
lambda: `access`, `update`, `access_all`, `par_iter`, a scope's parallel tasks,
the panic hook. Because `sync` is inferred, those lambdas may call every
function that cannot pause.

**A library function that runs a lambda does what the lambda does.** The
contracts of `xs.map`, `xs.filter`, `xs.sort_by_key` and the rest say that *the
lambda decides*: the same `map` cannot pause over `fn(n) { n + 1 }` and can over
a lambda that reads a file. This is accepted:

```nika
counter.access fn(n) { n + xs.sort_by_key fn(x) { x } }  // pure lambda, pure call
```

and this is refused:

```nika
counter.access fn(n) { xs.map fn(x) { fs::read("log", fs::Root::Anywhere) } } // the lambda does I/O
```

User code writes nothing for this.

**A function the program writes says it with `sync(f)`**
([ADR-288](adr/adr-288.md) D31). The word stands where `sync` stands, after the
result, and names the parameters whose lambdas decide; several are written
`sync(f, g)`:

```nika
fn apply(f: fn(i64) -> i64, x: i64) -> i64 sync(f) {
    return f(x)
}

fn twice(x: i64) -> i64 sync {
    return apply(fn(n) { n * 2 }, x)   // the lambda cannot pause, so neither can the call
}
```

The body may call `f`, and nothing else in it may pause: a call that can is
refused with `NK2202`, *`apply` is `sync(f)`, and `fs::write` can pause*. A
caller is `sync` where the lambda it hands over is, and may pause where the
lambda may; the lambda's body is the caller's, so a lambda that pauses inside a
`sync` caller is refused there. `sync(f)` names a parameter that takes a
function and **runs it during the call**: a name that is no parameter, one that
is not a function, and one the function keeps or hands to a task are refused
with `NK2210`. The ledger records it as `sync = "sync(f)"` (Part III 13.5).

**What the keyword is for.** A written `sync` states a property so that losing
it is an error. A body written `sync` is held to it, and a call inside it that
can pause is refused with `NK2202`. A body without the word that qualifies is
`sync` all the same.

Where the compiler **cannot tell**, it does not guess in the program's favour. A
call it cannot resolve costs the function its inferred promise. A written `sync`
overrules the inference: it is an assertion, checked as far as the compiler can
see.

`sync` decides what a function is compiled to. A function that cannot pause is a
plain `fn`; every other function is an `async fn`. An unresolved call therefore
makes a function `async`, and an `async fn` that never awaits finishes on its
first poll. A `sync(f)` function is an `async fn`, because its
lambda may pause; a caller that cannot pause drives the call once, which the
checker has proved is enough, and a caller that can awaits it.

### 12.2. The Dual Nature of the Lock
To share data that changes, a program uses **`SharedMut[T]`**: several owners, one value, and the lock inside the type (Part I 6.2). A program makes one by calling it: `let counter = SharedMut(0)`. `Shared[Locked[T]]` is refused, and the message names `SharedMut[T]`. `Locked[T]` is the same lock on its own, for individually locked fields inside a shared structure. Everything in this section is about the lock and holds for both.

**The name says what the program gets, not what is underneath.** `SharedMut[T]` means *several own it and any of them may change it*. Which shapes carry that is the compiler's decision, made per value as the owner count is. Below, it is a count around a lock. At `user_parallelism = no` the pair is always the cheap one; at `yes` it is decided per value.

**The counter below may go into a task of the program's own, and not into foreign code.** The answer is about the **lock**, not the owner count (Part I 6.2), and it depends on where the value goes. Into a task of the program's own: yes, at both values of the option. Into code nothing written down describes: no, at both values of the option (Part III C.5).

**At `user_parallelism = no`:**
* **Implementation:** a cell like a `RefCell`, with a re-entrancy check. No OS primitive is used.
* **Cost:** an integer increment.
* **Purpose:** the check catches a **logical deadlock**: task A locks data and waits for the network, task B tries to lock the same data. 12.3 refuses that nesting when the program is compiled, so the check is self-control of the refusal rather than error handling (below).

**At `user_parallelism = yes`:**
* **Implementation:** decided per value: an OS-level **mutex** where something may cross a thread with the value, an atomic word for a number, `bool` or `scalar` that crosses and no door over several locks names, and the cell above where nothing crosses.
* **Cost:** atomic operations.
* **Purpose:** it protects against **memory corruption**. Two threads cannot write to the same memory address at the same time.

**Four Doors, Not One**
A change to locked data has four shapes, and each has its own door. Only two of them run user code while the lock is open, and only those two carry the rules further down this section:

| form | for | the block may wait |
| :--- | :--- | :--- |
| `kasse.get()` | taking a copy out | — |
| `kasse.set(value)` | replacing; the value is computed outside | yes, outside |
| `kasse.set(neu; after: stand)` | the same, **if nothing moved** since `stand` was seen | yes, outside |
| `kasse.update fn(mut v) { v += 100 }` | changing it, under the lock | no |
| `kasse.access fn(state) { … }` | reading in place, large values | no |

* **`set` needs no block.** Arguments are evaluated before the call, so producing the new value, including waiting for I/O, happens outside, and the lock is open for one store. `get` is the same in the other direction: one load.
* **`update` is handed the value as `mut v`, changes it, and returns nothing.** It is **where locked data changes**, small or large. What `v` is, is the compiler's decision, by the value's type and by what it meets. It may be a **copy** where the value fits a machine word: the block runs on the copy, the result is swapped in, and the block runs again if another task got there first — which is what a value that crosses a thread gets, unless an `access_all` or `update_all` holds it, since a retry cannot be held. It is the **address** in the lock otherwise, and the block runs once. A program cannot tell the two apart, and `--sharing` says which a value got. Nothing is moved out of the lock in either case. A block that hands a value back is refused with `NK1141`.
* **`set` takes a witness where the new value came out of the lock.** `after:` makes the store compare, and store only if the lock still holds what was seen. `kasse.set(neu; after: stand)` **is** `kasse.update fn(mut v) { if v == stand { v = neu } else { throw Overtaken } }`, and like every `update` it takes the lock once. The comparison is of the whole value.
* **`access` is for reading in place.** It hands the block the value where it lies, and the block **may not change it**.

**Exactly one lambda in this language is handed something it may change**, and it carries `mut`, the word every changed parameter carries. A change to locked data is written in `update`.

Two mistakes are refused at the doors. **Assigning to a `SharedMut` directly** is refused, and the message names `set`. **A `set` given a value that was seen in a lock, or standing under a condition that was,** is refused, and the message names `update` and `after:`. The check reads the **stamp**: what a lock hands out is a `Seen[T]`. The stamp travels with the value through `let`, arithmetic, calls to lock-free functions, declared fields and time, and it never comes off, so the pair spread over two lines, two functions or two requests is refused like the one written inline. An `update` block that assigns to `v` without reading it is refused with `NK2207`. `set_after` is refused with `NK2208`; `after:` is the form to write.

**`after:` is the way through for a stamped value**, and `Overtaken` is an error like any other:

```nika
fn charge(kasse: SharedMut[i64], wieviel: i64) throws {
    let stand = kasse.get()
    // Computed outside the lock, which is what `set` is for - and stored only
    // if nothing moved while it was being computed.
    kasse.set(stand + wieviel; after: stand)
}

fn main() {
    let kasse = SharedMut(100)
    charge(kasse, 23) catch { println("somebody got there first") }
    println(f"{kasse.get()}")
}
```

A retry is a loop and a `catch`. `Overtaken` carries nothing. A caller that wants the value in the lock takes the door again and gets a fresh stamp.

**The `sync` Rule (No Pausing While Holding a Lock)**
A program may not pause while it holds a lock. Nikaia rules it out **at compile time**:

> **`update`, `access`, `access_all` and `update_all` require a lambda that is `sync` and touches no lock, at both values of `user_parallelism`.**

A `sync` lambda (12.1) never pauses. A lambda that touches no lock cannot take a second one (12.3). **These are two conditions and not one**: `sync` answers the first, and `touches`, the fourth derived column (13.5), answers the second. A `println` inside a door fails the second and not the first: it takes standard output's own lock while the door's lock is open. **`get` and `set` need no condition: while the lock is open in either of them, no user code runs.** Inside a door, a call that pauses is refused with `NK2202`, a call that takes a lock, directly or through a chain of calls, with `NK2203`, and I/O that does neither with `NK2201`:

```nika
let counter = SharedMut(0)

// OK: pure computation
counter.update fn(mut n) { n += 1 }

// Compiler Error (NK2203): a lock inside a lock. `println` takes standard
// output's own, and `fs::write` can pause besides (NK2202).
// counter.access fn(n) { println(f"{n}") }
```

**`NK2201` is the third case.** `fs::write` inside a door is `NK2202`, because
it pauses; a `println` is `NK2203`, because it takes a lock. `NK2201` is I/O
that does **neither**: reading a `fs::Mapped`, the one such thing in `std`, at
both shapes of read. A mapping is a file held as memory, so touching a page that
is not there yet is a disk read with no call in the source, and inside a door it
is a disk read with the lock held.

```text
error[NK2201]: `len` reads a file, but you're holding a lock here.
  --> main.nika:7:9
   |
 7 |         n = n + page.len()
   |         ^^^^^^^^^^^^^^^^^^
   |
   = note: `Mapped` is a file mapped into memory, so reading it can wait on the disk, and a block that holds a lock must never wait.
   = note: This wait doesn't show up as a pause or a second lock, which is why it gets its own message.
   = help: Read what you need before taking the lock, and pass the value in.
```

The claim is the **type's own**: `fs::Mapped` records `touches = ["file read"]`
in the ledger (13.5). A type that records nothing is claimed nothing about; here
the compiler does not read silence fail-closed
([Part III C.4](30-nikaia-tooling.md)).

Re-entering the *same* lock through a chain of calls is refused by 12.3 when the program is compiled, at both values of the option. The runtime check described above is **self-control of that refusal rather than error handling**. No input can make it fire. It is a build option a program may decline (Part I 1.2), and poisoning on several threads is left as it is.

**A lock is a resource, so two doors onto the same lock keep their order where both of them write.** Two operations whose touch sets are disjoint have no order between them *inside an `overlap { … }`* (Part I 8.1.2), and a touch set can name a lock. **`get` and `access` are reads; `set` and `update` are writes.** Two reads of one resource are unordered, so two `get`s, or two `access`es, on one lock may run in either order. Any two of the writing forms are ordered by the rule that orders two `println`s. Two doors onto *different* locks need not wait for each other.

Where two doors would meet on something the touch sets cannot see, a program writes them as ordinary statements rather than as branches of an `overlap`: statement order is the written order (Part I 8.1.1).

### 12.3. Deadlock Prevention: Atomic Composition
Taking a lock while a lock is held is **always** a compile-time error. What is refused is the nesting of lock *acquisitions*, one door (12.2) opened while another is still open. One door on its own is one acquisition, whatever it opens.

**The Solution: `access_all`**
Instead of nesting `access` calls, a program requests several locks at once with `access_all`.

* **Mechanism:** the runtime sorts the locks by memory address (or internal ID) before locking. The locking order is therefore the same in every thread.
* **Safety:** a deadlock cycle between A and B cannot form when every acquisition goes through `access_all(A, B)`.

```nika
let account_a = SharedMut(Account(…))
let account_b = SharedMut(Account(…))

// ERROR: Manual Nesting is forbidden to prevent Deadlocks.
// Taking one lock inside another is what creates the inconsistent order
// this section is about — a single door (12.2) is of course fine.
// account_a.access fn(from) {
//     account_b.update fn(mut to) { … }
// }

// Atomic Locking (Deadlock Proof)
// The runtime sorts A and B by address and takes them in that order, so this
// and `access_all(account_b, account_a)` in another task take them alike.
access_all(account_a, account_b) fn(a, b) {
    // Both are read in place, and neither may be changed here.
    a.balance + b.balance
}

// And the transfer, which is a write on both: one `mut` per lock, and the
// block changes them.
update_all(account_a, account_b) fn(mut von, mut nach) {
    von.balance -= 30
    nach.balance += 30
}
```

**`update_all` is `update`'s rule widened, not a second rule.** One `mut` per
lock, nothing returned, nothing sees either lock between the two writes.
Whether the block runs on copies and is retried, or on the addresses under the
locks, is the compiler's decision by the pair of types, as it is for one lock.
`access_all` reads several locks at once; `update` writes one; `update_all`
writes several, one `mut` each. A door takes two locks at a time: a door handed
one lock, or a block that does not name one value per lock, is refused with
`NK1124`. Neither name is a keyword.

**How the compiler sees a chain.** A block calls a function, which calls another, and the third opens a lock. Every function therefore carries a second derived property beside `sync` (12.1): **whether it touches a lock.** The property is inferred over the same call graph and never written by hand. Inside a blocking door (`update`, `access`, `access_all`, `update_all`), a call to anything that carries it is refused. That catches chains and self-calls.

**The property is coarse.** It says *a lock*, never *which* lock.

The property is not 12.2's ordering rule. That rule needs to know *which* lock, and takes it from what the operation touches.

**A function handed outward is judged by its body, never by its type.** Where a rule asks whether a lambda touches a lock and the lambda is not run on the spot, because it is handed to foreign code or kept by the callee, the answer is computed from the body. A lambda is its captures, and no type writes those down.

### 12.4. Racing Tasks (`select`)
`select` runs several tasks and keeps the result of the one that finishes *first*.

```nika
use std::time

select {
    // Case 1: Computation finishes first
    result = heavy_math() => { return result }

    // Case 2: Timeout happens first
    _ = time::sleep(5.seconds()) => { throw Timeout::TooSlow }
}
```
`select` is a block whose arms bind. The first branch to finish wins. The losers are cancelled by being dropped. The handle `spawn` returns has `cancel()`, which is the same mechanism, and cancelling takes the handle. `5.seconds()` is a `std::time::Duration`, made by a `std` extension on the integers; there is no suffix literal. `sleep` lives in `std::time` and is written with it; nothing in the prelude pauses (Part I 1.3). An error type is an `enum`, so the error a branch throws is an **enum variant**.

`select` and `overlap` are a **pair**: both start every branch at once; `overlap` keeps every result, `select` keeps the first. A block needs at least two arms and at most eight.

**What "cleaned up" means.** The losing task stops at its current pause point and its values are torn down. A resource with a pausable `cleanup` (Part I 6.4) is not awaited by the *winner*. The runtime **adopts** such `cleanup` runs and finishes them in the background ("parked cleanup"). The program does not exit before parked cleanups are done, bounded by the runtime configuration `cleanup-deadline` (Part III 13.3b). An error from a parked cleanup has no caller and is said on standard error.

### 12.5. Channels (Message Passing)
Nikaia offers **message passing** beside shared memory. A channel is `std`'s and not the language's. A channel is **bounded**; there is no `unbounded()`, and a capacity below one is refused. `send` pauses when the channel is full, so a `sync` body cannot send on one, and the ledger's column says so. `recv` hands back a `T?`; `null` means every sender is gone. The value type must `crosses`, checked where `tx` moves into the `spawn` as any move is.

```nika
use std::channel

// Subject: 100 (capacity)
let (tx, rx) = channel::bounded(100)

// Implicit Move transfers 'tx' into the background task
spawn fn {
    tx.send("Calculation complete")
}

let msg = rx.recv() // Waits (non-blocking yield) for the message
```

### 12.6. Data Parallelism (`par_iter`)
`par_iter` (parallel iterator) processes a large list on all CPU cores.

```nika
let pixels = [/* 1 million pixels */]

// The compiler splits the array into chunks and distributes them
// across all cores. The closure may not pause (NK2209) or change a name outside it (NK2107).
// Methods chained with trailing lambdas. A block ends at its `}`, so the chain
// continues after it and means what it reads as.
let bright_pixels = pixels.par_iter()
    .map fn(pixel) { pixel.brightness * 1.5 }
    .filter fn(value) { value > 0.5 }
    .collect()
```

### 12.7. Scoped Tasks (@immediate Parallelism)

A `spawn` task takes its captured variables *with* it (Part I 8.3). A **scoped** task *borrows* them, because the scope waits until every task inside it has finished.

`task::scope` is an **immediate context** (Part I 5.4): it does not return until every task spawned inside it has completed. A task inside the scope therefore reads the function's variables without moving or cloning them.

```nika
let data = [1, 2, 3]

// 'task::scope' waits for all inner tasks before it returns.
task::scope fn(s) {
    // Note: s.spawn is tied to the scope, unlike global spawn.
    s.spawn fn { println(f"Reading: {data}") } // Safe Borrow
    s.spawn fn { println(f"Reading: {data}") } // Safe Borrow
}
// 'data' is still valid here
```

**A scope may not be opened while a lock is open.** Because the scope waits for its tasks, their bodies belong to the function that writes the scope. Whatever a task touches, the surrounding function touches, including a lock (12.3). Opening a scope inside an open lock is therefore refused. A task started with `spawn` is the other case (11.2).

**One rule differs with `user_parallelism`.** The scope's promise holds only where the runtime can wait the tasks out:

* **At `no`:** everything runs on one thread, and the runtime owns every task. When a scope ends, including when an error tears it down early, the runtime collects its tasks *before* the function's variables disappear. **A scoped task may do anything, including I/O.**
* **At `yes`:** tasks run on other CPU cores *in parallel*, and the scope keeps its promise only for tasks that finish on their own. **At `yes`, a scoped task must be `sync`**: it never pauses (12.1), the same rule as `par_iter` (12.6).

A task that needs I/O is a background task, not a scoped one. A program starts it with `spawn` (the task takes ownership, Part I 8.3) and collects the result through its handle. A scoped task that can pause is refused with `NK2102` where tasks run in parallel:

```text
error[NK2102]: A task in `task::scope` has to be `sync` here, but `fetch_url` does network I/O.
   --> worker.nika:12:18
    |
 12 |     s.spawn fn { fetch_url(url) }
    |                  ^^^^^^^^^^^^^^ network I/O here
    |
    = note: A scope promises to wait for its tasks. With tasks running in parallel, that promise only holds for work that finishes on its own (`sync` functions), and a task waiting on the network might not.
    = help: Do the I/O before the scope and keep only the computation in it, or run it as a background task: `let target = url.clone()`, then `let handle = spawn fn { fetch_url(target) }` and `handle.join()`.
```

At `no` the runtime owns all task state and tears a scope down synchronously. The language exposes no way to leak a live scope.

### 12.8. Supervision Trees
A task may panic. A **supervisor** monitors tasks. When a child task panics, the supervisor does one of three things, according to its policy:
* **Restart** the task.
* **Crash** the parent (escalate).
* **Ignore** the error.

```nika
// Subject: The Task (Lambda)
// Config: restart_policy (Protocol: separated by ;)
// Note: 'spawn' syntax (fn {}) is the subject.
supervisor::start_link(fn {
    server.run()
}; restart_policy: RestartPolicy::Always)
```

The policy is an **enum**. A misspelled policy is refused at the call, and the
policies are listed where they are declared (Part I 4.4).
