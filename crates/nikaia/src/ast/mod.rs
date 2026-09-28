// crates/nikaia/src/ast/mod.rs
// Nikaia AST definition.
// Based on ADR-001 and Part I/II/III documents.

// Identifiers are interned: the AST stores a `Symbol` handle and the text is
// recovered through the `InternerContext` that produced it.
//
// This is Spec Part II, 10.6 in practice - identifiers are short, repeat
// constantly, and are compared far more often than they are read, so interning
// stores each distinct spelling once and makes equality an integer comparison.
use winnow_grammar::Symbol as Ident;

/// A byte range in the `.nika` source a node was parsed from.
///
/// Every diagnostic a user ever sees has to end up pointing at one of these:
/// the compiler emits Rust, and an error reported against the emitted file
/// names a line nobody wrote.
pub type Span = std::ops::Range<usize>;

/// A node and where it came from.
#[derive(Debug, Clone)]
pub struct Spanned<T> {
    pub node: T,
    pub span: Span,
    /// The run of `///` lines standing immediately before it
    /// ([ADR-139](../../../docs/specification/adr/adr-139.md) D1), with the
    /// slashes and one leading space taken off and the line breaks kept.
    ///
    /// **Here rather than on each item**, because it is the same fact about
    /// every one of them and this is already the wrapper that says *where this
    /// came from*: prose in front of an item is a second answer to that
    /// question. Only an **item** carries one today; a field's and a variant's
    /// wait on `nikaia doc`, which is what would read them
    /// ([ADR-139](../../../docs/specification/adr/adr-139.md) §4).
    ///
    /// The compiler does not read it (D3). What it does is travel.
    pub doc: Option<String>,
}

impl<T> Spanned<T> {
    pub fn new(node: T, span: Span) -> Self {
        Self {
            node,
            span,
            doc: None,
        }
    }

    /// The same, with the documentation standing in front of it.
    pub fn documented(node: T, span: Span, doc: Option<String>) -> Self {
        Self { node, span, doc }
    }
}

/// Lets a rule written `-> Spanned<T> @=` wrap its value without an action.
impl<T> winnow_grammar::WithSpan<T> for Spanned<T> {
    fn with_span(node: T, span: Span) -> Self {
        Self {
            node,
            span,
            doc: None,
        }
    }
}

/// Ein Nikaia-Programm ist eine Liste von Top-Level Items.
#[derive(Debug, Clone)]
pub struct Program {
    pub items: Vec<Spanned<Item>>,
}

/// Top-Level Konstrukte (außerhalb von Funktionen)
#[derive(Debug, Clone)]
pub enum Item {
    // Kap 5.1: fn add(a: i32) -> i32 { ... }
    Fn {
        /// `None` for the anonymous constructor of Kap 4.2 - a `pub fn` with no
        /// name inside an `impl`, called as `Type(…)`.
        name: Option<Ident>,
        generics: Vec<GenericParam>, // Kap 4.5: [T]
        /// Kap 4.2/5.1: `&self`, `&mut self` or `self`, when this is a method.
        receiver: Option<Receiver>,
        args: Vec<FnArg>,
        /// Kap 5.1: what stands after the `;` - options, named at the call and
        /// never positional. Empty for a function that has no `;`.
        config: Vec<ConfigParam>,
        /// ADR-007 D5: `...args: Self::dsl` - the typed spread a driver accepts
        /// a DSL's deferred parameters with. It stands where the options stand,
        /// because a DSL parameter *is* configuration, and it is exclusive with
        /// them: a function takes options or a spread, never both.
        spread: Option<Ident>,
        ret_type: Option<Type>,
        body: Block,
        is_sync: bool,   // Kap 12.1: sync keyword
        is_public: bool, // Kap 9.2
        throws: bool,    // Kap 7.1
    },

    // Kap 4.4: enum Message { Quit, Move { x: i32 }, Write(String) }
    Enum {
        name: Ident,
        variants: Vec<EnumVariant>,
        is_public: bool,
    },

    // Kap 4.1: struct User { ... }
    Struct {
        name: Ident,
        generics: Vec<GenericParam>,
        fields: Vec<FieldDef>,
        is_public: bool,
    },

    // Kap 4.2: impl User { ... }
    //
    // Kap 4.7 and 7.1: `impl Summarize for User` names a trait, and `trait` is
    // `None` for the inherent form. `Error` is the one the compiler reads
    // rather than relays - a type that implements it is a type that may be
    // thrown ([ADR-023](../../../docs/specification/adr/adr-023.md) D3).
    Impl {
        /// The trait being implemented, or `None` for an inherent `impl`.
        trait_name: Option<Ident>,
        target: Type,
        methods: Vec<Spanned<Item>>, // Enthält Item::Fn
    },

    // Kap 4.7: trait Summarize { fn summary(&self) -> String }
    //
    // **Signatures and nothing else.** A method here has no body, which is what
    // makes it a declaration rather than an `impl` - and it is why this cannot
    // reuse `Item::Fn`, whose `body` is not an `Option`. A default body is a
    // separate decision and not this one
    // ([ADR-078](../../../docs/specification/adr/adr-078.md) §4).
    Trait {
        name: Ident,
        methods: Vec<Spanned<TraitMethod>>,
        is_public: bool,
    },

    /// Part III 15.1: `extern "C" { fn getpid() -> i32 }`
    /// ([ADR-124](../../../docs/specification/adr/adr-124.md) D1).
    ///
    /// **Signatures and nothing else**, which is `Trait`'s reason one item
    /// over: a declaration has no body, so `Item::Fn` cannot serve. The
    /// declarations are `TraitMethod`s because a signature without a body is
    /// the same shape wherever it stands; what differs is how it **reads**,
    /// and that is D2's, not the AST's — an `extern` declaration is `sync` and
    /// carries no `throws`, where a trait method without `sync` may pause.
    ///
    /// `abi` is the string the source wrote. Only `"C"` is accepted today and
    /// the field carries what was written anyway, so a second one is a check
    /// rather than a shape.
    Extern {
        abi: String,
        declarations: Vec<Spanned<TraitMethod>>,
        /// **The handles the block declares**
        /// ([ADR-147](../../../docs/specification/adr/adr-147.md) D3):
        /// `opaque type sqlite3 released by sqlite3_close`.
        ///
        /// A list beside the declarations rather than a kind of declaration,
        /// because the two are different shapes: one is a signature and the
        /// other is a type and the function that ends its life. The block's
        /// source may interleave them freely; what reads them never has to.
        opaque: Vec<Spanned<OpaqueType>>,
    },

    /// `test "a blank line is counted" { … }`
    /// ([ADR-245](../../../docs/specification/adr/adr-245.md) D1, Part III
    /// 14.1): a test of the package it stands in. The name is kept as written,
    /// escapes and all, as a string literal's text is. `modules` removes it
    /// from every build but `nikaia test`'s, which turns it into a function.
    Test {
        name: String,
        body: Block,
    },

    // Part III, Kap 13.4: bench "Name" { ... }
    Bench {
        name: String,
        body: Block,
    },

    // Part II, Kap 10.1: grammar ColorParser { ... }
    //
    // The rules are parsed, not kept as text: the whole point of this item is
    // that the compiler lowers it onto the parser backend (`grammar!`), and it
    // cannot check a frame or generate a fold driver from a string.
    Grammar(GrammarDef),

    // Kap 9.2: use std::fs
    /// Kap 9.2 / Part II 10.2: `comptime MAX = 1000`, at the top of a file.
    ///
    /// **The keyword is a demand rather than an ability**
    /// ([ADR-073](../../../docs/specification/adr/adr-073.md) D3). The compiler
    /// folds constants already, so what this adds is that the fold *has* to
    /// succeed: a `let` may fold, a `comptime` must, and says so where it
    /// cannot. What may stand in the initialiser is staged (D5), and the stage
    /// that is built is the shared fold plus a literal of another kind.
    ///
    /// **The word says *when*, not *whether it changes***
    /// ([ADR-078](../../../docs/specification/adr/adr-078.md)): everything in
    /// this language is immutable unless it says `mut`, so `const` would have
    /// named a property every other binding already has.
    Comptime {
        name: Ident,
        /// Written or inferred, the way a `let`'s is (D4).
        ty: Option<Type>,
        value: Expr,
        /// `pub comptime`, which is Part I 9.2's existing rule for Constants
        /// rather than a new one.
        public: bool,
    },

    Import {
        path: Vec<Ident>,
        /// `use http as h` - what this file calls the package
        /// ([ADR-046](../../../docs/specification/adr/adr-046.md) D3). `None`
        /// where the package's own name is used.
        alias: Option<Ident>,
    },
}

/// Ein Block von Statements { ... }
#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Spanned<Stmt>>,
}

/// Anweisungen innerhalb eines Blocks
#[derive(Debug, Clone)]
pub enum Stmt {
    // Kap 2.1: let mut x = 10
    Let {
        /// **One name, or a flat tuple of them**
        /// ([ADR-098](../../../docs/specification/adr/adr-098.md)):
        /// `let x = 1` and `let (tx, rx) = channel::bounded(100)`.
        ///
        /// A `Vec` rather than a second variant, because the two are one
        /// statement and a walk that has to remember a second one is a walk
        /// that will forget it. Never empty - the grammar has no way to write
        /// `let () = …`.
        names: Vec<Ident>,
        mutable: bool,
        ty: Option<Type>, // Type Inference macht dies optional
        value: Expr,
    },

    /// `comptime LIMIT = 4 * 1024`, inside a body
    /// ([ADR-073](../../../docs/specification/adr/adr-073.md) D2,
    /// [ADR-078](../../../docs/specification/adr/adr-078.md) for the word).
    /// Scoped like a `let` and evaluated like the item form: the difference
    /// between the two is where the name is visible, never what may stand to the
    /// right of the `=`.
    Comptime {
        name: Ident,
        ty: Option<Type>,
        value: Expr,
    },

    // Kap 2.1: x = 20, und x += 1
    Assign {
        target: Expr,
        /// `Some(Add)` for `+=`; `None` for a plain assignment.
        op: Option<BinaryOp>,
        value: Expr,
    },

    // Kap 3.3: for x in xs { ... }, for (k, v) in map { ... }
    For {
        bindings: Vec<Ident>,
        iter: Expr,
        body: Block,
    },

    /// Kap 3.3: `while count < 5 { … }`.
    ///
    /// A statement rather than an expression, like `for` and unlike `if`: it
    /// repeats until a condition stops holding, and what that is worth as a
    /// value is nothing. Part I 3.3 writes both loops as statements and neither
    /// as something a `let` takes.
    While {
        cond: Expr,
        body: Block,
    },

    // Kap 7.1: return, return value
    Return(Option<Expr>),

    /// Part I 3.3: `break`, and it leaves the **innermost** loop around it
    /// ([ADR-084](../../../docs/specification/adr/adr-084.md) D1).
    ///
    /// A statement and not an expression, which is the whole of what makes it
    /// cheap: Rust's `break` carries a value out of a `loop`, and that is what
    /// makes `loop { … }` an expression there and forces a keyword of its own
    /// ([ADR-070](../../../docs/specification/adr/adr-070.md) D2). Nikaia's
    /// loops are statements and hand back nothing, so there is nothing for a
    /// `break` to carry and no type for anyone to infer.
    ///
    /// **No label.** A label is a second thing - a name that is not a value,
    /// scoped to a construct rather than to a block - and the unlabelled form
    /// answers the cases the tree actually writes.
    Break,

    /// Part I 3.3: `continue`, which starts the innermost loop's next turn.
    ///
    /// One variant per word rather than one with a flag, for the reason
    /// `LitInterpolated` is a variant: every analysis that walks a statement has
    /// to say what it does with each, and the two are not the same statement -
    /// one ends a loop and one does not.
    Continue,

    // Ein "nackter" Ausdruck (z.B. Funktionsaufruf oder Return-Value)
    Expr(Expr),
}

/// Ausdrücke (Alles, was einen Wert zurückgibt)
#[derive(Debug, Clone)]
pub enum Expr {
    // Primitive
    LitInt(i64),
    /// Kap 3.4: `match value { 1 => …, _ => … }`. An expression, like `if`.
    Match {
        value: Box<Expr>,
        arms: Vec<MatchArm>,
    },

    /// Kap 3.3: `0..n` and `0..=n`. What a `for` counts over, and the reason
    /// the loop needs no index arithmetic of its own.
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
        /// `..=`, which includes its end.
        inclusive: bool,
    },

    /// `(a, b)` - Kap 4.5. Two or more values of different types, with no
    /// name for the pair and none for its parts.
    Tuple(Vec<Expr>),

    /// `[1, 2, 3]` - a list, and `[]` an empty one
    /// ([ADR-135](../../../docs/specification/adr/adr-135.md) D1).
    ///
    /// **Its type is `Vec[T]`** and there is no second container behind the
    /// brackets: Part I 2.2 offers one, and this writes that one down. What `T`
    /// is, is what the elements agree on; an **empty** literal says nothing and
    /// takes its element type from the first use that needs one (D2), which is
    /// [ADR-060](../../../docs/specification/adr/adr-060.md)'s rule for a
    /// number literal one level up.
    /// `at` is the byte the `[` stands at, and it is the one expression here
    /// that carries a position. **`Array[T, N]` is what needs it**
    /// ([ADR-152](../../../docs/specification/adr/adr-152.md) D4): whether a
    /// literal is laid out inline or allocated is decided by its *use*, the
    /// use is known to the checker and the lowering is the emitter's, so the
    /// answer has to be handed over - and a statement's byte is not enough,
    /// because one statement may hold a list and an array both.
    ListLit {
        items: Vec<Expr>,
        at: usize,
    },
    /// `"…"` - Kap 2.5. **Inert text.** A `{` is a brace and nothing else, so
    /// a program that writes JSON, CSS or a regular expression says what it
    /// means. The body is kept as written, escapes and all, for the same
    /// reason `LitChar` is.
    ///
    /// **`at` is the byte the opening quote stands at**, for `ListLit`'s
    /// reason ([ADR-207](../../../docs/specification/adr/adr-207.md) D2):
    /// whether the literal stays a view of static text or becomes text of its
    /// own is decided by its *use*, the use is the checker's and the lowering
    /// is the emitter's, and one statement may hold both kinds.
    LitStr {
        text: String,
        at: usize,
    },
    /// `f"… {expr} …"` - Kap 2.5. **Text with code in it**, and the `f` is what
    /// says so ([ADR-035](../../../docs/specification/adr/adr-035.md)).
    ///
    /// This is a separate variant rather than a flag for a reason the compiler
    /// has already paid for once: before ADR-032 every analysis walked past a
    /// string as if it held no code, because *whether* it did was a property of
    /// its text and nothing made anyone look. A variant makes the question
    /// unavoidable - every `match` on an expression has to say what it does
    /// with this one, and the compiler names the sites that forgot.
    LitInterpolated(String),
    /// Kap 2.2: `'a'`, `'\n'`. The body is kept **as written**, escape and
    /// all: the language below spells a character literal the same way, so the
    /// lowering is a transcription and nothing has to decode it twice.
    LitChar(String),
    LitBool(bool),
    /// Part I 2.3: `null`, the absence of a value. It lowers to `None`.
    ///
    /// **It has no type of its own**, exactly as an integer literal does not:
    /// what it stands for is whatever the type beside it says, so `let mut m:
    /// &str? = null` is an `Option<&str>` and `let m = null` on its own is a
    /// program `rustc` will ask for an annotation about - which is right,
    /// because nothing here can say what it is the absence of.
    LitNull,
    Variable(Ident),

    // Kap 3.1: Blöcke sind Expressions
    Block(Block),

    /// Part I 8.1.2: `overlap { … }` - **each statement in the block is a
    /// branch**, the block starts every branch and waits for all of them, and
    /// its value is the tuple of their results in written order
    /// ([ADR-050](../../../docs/specification/adr/adr-050.md) D2).
    ///
    /// A `Block` and not a `Vec<Expr>`, because a branch is a *statement* in the
    /// source and the one thing that makes it a branch is standing here. What a
    /// branch may be is narrower than what a statement may be, and that is the
    /// checker's to say rather than the grammar's: `let` inside an `overlap`
    /// would bind a name the block's own value already carries.
    Overlap(Block),

    /// Part II 12.4: `select { … }` — **every arm is started and the first one
    /// to finish wins** ([ADR-148](../../../docs/specification/adr/adr-148.md)
    /// D1).
    ///
    /// [ADR-050](../../../docs/specification/adr/adr-050.md)'s `overlap` is the
    /// other half of the pair: it runs its branches at once and keeps **every**
    /// result; this one keeps the **first** and cancels the rest (D4).
    ///
    /// A `Vec<SelectArm>` and not a `Block`, because an arm is not a statement:
    /// it binds a name and then runs a block, which is the whole of why this
    /// costs a keyword rather than being a function (D1).
    Select(Vec<SelectArm>),

    // Kap 3.2: if cond { ... } else { ... }
    If {
        cond: Box<Expr>,
        then_branch: Block,
        else_branch: Option<Block>,
    },

    // Kap 5.1: Funktionsaufruf add(1, 2)
    Call {
        func: Box<Expr>,
        args: Vec<Expr>,
        /// Kap 5.1: what stands after the `;` at the call - `request(url;
        /// timeout: 60)`. Named, in whatever order the caller wrote them;
        /// putting them in the callee's order is the lowering's business,
        /// because only the declaration knows what that order is.
        config: Vec<ConfigArg>,
    },

    // Kap 8.2: spawn({ ... }) oder spawn(move { ... })
    // Auch Kap 5.2: Block Lambdas
    Spawn {
        body: Box<Expr>, // Meistens ein Expr::Block
        is_move: bool,   // Kap 8.3
    },

    // Part II, Kap 10.5: dsl sql db { ... }
    Dsl {
        target: Ident,          // z.B. sql
        context: Option<Ident>, // z.B. db (optional)
        content: String,        // Simplified from TokenStream
    },

    // Ein qualifizierter Pfad: Summary::new, u8::from_str_radix
    Path(Vec<Ident>),

    // Methodenaufruf: acc.record(m)
    MethodCall {
        receiver: Box<Expr>,
        method: Ident,
        args: Vec<Expr>,
        /// Kap 5.1 at a method call: what stands after the `;`. A DSL's
        /// deferred parameters arrive here (ADR-007 D5), which is why a method
        /// call carries them at all - before that they were parsed and dropped.
        config: Vec<ConfigArg>,
    },

    // Feldzugriff: self.min
    Field {
        base: Box<Expr>,
        name: Ident,
    },

    // Struct-Literal: Reading { name, temp }
    StructLit {
        name: Ident,
        fields: Vec<FieldInit>,
    },

    /// `p with { x: p.x + 1 }` — a **copy of a value with named fields
    /// changed** ([ADR-118](../../../docs/specification/adr/adr-118.md) D1).
    ///
    /// The braces are the literal's, with its field list and its shorthand, so
    /// `fields` is the same `Vec<FieldInit>` a `StructLit` carries and every
    /// rule about naming a field is the one a literal already has.
    ///
    /// **`at` is here because the language below needs a name this node does
    /// not carry.** Rust's functional update is `Point { x: 1, ..p }` — the
    /// type is written — and the operand's type is the checker's answer rather
    /// than the parser's. So the checker records it under this byte and the
    /// emitter reads it back, which is the handover a `comptime`'s value
    /// already makes ([ADR-011](../../../docs/specification/adr/adr-011.md) D2:
    /// the emitter is told, it does not work it out).
    With {
        base: Box<Expr>,
        fields: Vec<FieldInit>,
        at: usize,
    },

    // Kap 5.2: `fn(acc, m) { … }`. One form, and its arguments are the ones it
    // names - the automatic `a`, `b`, `c` are withdrawn (ADR-049), so `fn { … }`
    // is a lambda of no arguments and nothing is read off the body.
    Closure {
        params: Vec<Ident>,
        /// The parameters written **`mut`**, in declaration order
        /// ([ADR-110](../../../docs/specification/adr/adr-110.md) D1).
        ///
        /// `kasse.update fn(mut v) { v += 100 }`: the block changes `v` in
        /// place and returns nothing, and the caller whose value changes is the
        /// **lock**. It is [ADR-094](../../../docs/specification/adr/adr-094.md)
        /// D3's word with its meaning unchanged, one position over.
        ///
        /// A list beside `params` rather than a field inside it, which is the
        /// arrangement `contracts::Signature` uses for the same question:
        /// almost no parameter is one, and a bare `Ident` is what every reader
        /// of `params` already has in hand.
        mutable: Vec<Ident>,
        body: Block,
    },

    // -value, !flag
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },

    // a + b, a < b
    Binary {
        op: BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        /// Where the operator and its right-hand side stand.
        ///
        /// **The first expression-level span in this AST**
        /// ([ADR-081](../../../docs/specification/adr/adr-081.md) D1), and it
        /// exists because an answer had to be keyed by *which* `+` rather than
        /// by the statement around it: `a + b + c` is two of them, and a
        /// statement-keyed channel — the shape every other answer the checker
        /// hands the emitter uses — cannot tell them apart.
        ///
        /// The **tail's** span rather than the whole expression's, because that
        /// is what the grammar has in hand where the node is built, and
        /// uniqueness is the only property a key needs.
        span: Span,
    },

    /// Part I 3.5: `x?.field`, safe navigation.
    ///
    /// The receiver is a `T?`; the whole expression is a `U?`, where `U` is what
    /// the field holds. A separate variant rather than a flag on
    /// [`Expr::Field`], for the reason [`Expr::LitInterpolated`] gives: every
    /// analysis has to say what it does with a reach that may not happen.
    SafeField {
        base: Box<Expr>,
        name: Ident,
    },

    /// Kap 3.5: `x?.m(args)` - safe navigation onto a **method**.
    ///
    /// Part I 3.5 says `?.` accesses a *member*, and a method is one
    /// ([ADR-066](../../../docs/specification/adr/adr-066.md)): the receiver is
    /// a `T?`, the call happens only where there is something to call it on,
    /// and the whole expression is a `U?` where `U` is what the method hands
    /// back.
    ///
    /// A separate variant and not a flag on [`Expr::MethodCall`], for
    /// [`Expr::SafeField`]'s reason: every analysis has to say what it does
    /// with a call that may not happen, and a flag is the shape that lets one
    /// forget.
    SafeMethod {
        receiver: Box<Expr>,
        method: Ident,
        args: Vec<Expr>,
        /// Kap 5.1's deferred parameters, exactly as [`Expr::MethodCall`]
        /// carries them: a `?.` changes whether the call happens, never what a
        /// call is.
        config: Vec<ConfigArg>,
    },

    // Kap 7.1: expr?
    Try(Box<Expr>),

    /// Kap 7.1: `throw ConfigError::NotFound(path)` - raise an error. The value
    /// must implement `Error`; there is no other way to originate one, which is
    /// why [ADR-023](../../../docs/specification/adr/adr-023.md) D2 calls it
    /// load-bearing rather than convenient.
    Throw(Box<Expr>),

    /// `return`, `break` and `continue` **as expressions**
    /// ([ADR-138](../../../docs/specification/adr/adr-138.md) D1).
    ///
    /// Their type is **never**, so they fit every expected type without
    /// widening anything: an arm that returns sits beside an arm that hands
    /// back a `&str` and the `match` is a `&str`, because the arm that returns
    /// hands back nothing at all.
    ///
    /// **Beside the statement forms and not instead of them** (D2). A statement
    /// whose expression is one of these is exactly what the statement was, so
    /// nothing about what they *do* changes — and `NK1133` still refuses a
    /// statement after a `break` in the same block, which is what keeps
    /// `break x` from becoming a quietly dropped value one position over (D3).
    Return(Option<Box<Expr>>),
    Break,
    Continue,

    // Kap 2.2: 10.0
    LitFloat(String),

    // Kap 4.5: map[key]
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
    },

    // `self.sum as f64`
    Cast {
        expr: Box<Expr>,
        ty: Type,
    },

    // Kap 3.5: value ?? fallback
    Coalesce {
        value: Box<Expr>,
        fallback: Box<Expr>,
    },

    // Part III, Kap 16: unsafe asm { Bindings } { Body }
    Asm {
        bindings: Vec<AsmBinding>, // Block 1
        code: String,              // Block 2 (Roher Text)
    },

    /// Part III 15.1: `unsafe { … }`
    /// ([ADR-124](../../../docs/specification/adr/adr-124.md) D3).
    ///
    /// **A block with a value, and no other rule.** What is inside is checked
    /// exactly as anything else is; what the word buys is that the boundary is
    /// visible *at the call*, in the body somebody reads, which is why it is a
    /// word rather than an attribute on the declaration.
    Unsafe(Block),

    // Kap 7.1: Error Handling ?{ ... }
    TryCatch {
        expr: Box<Expr>,
        handler: Block, // Der Block mit 'error' Variable
    },
}

// --- Helper Strukturen ---

#[derive(Debug, Clone)]
pub struct Type {
    pub name: Ident,
    pub generics: Vec<Type>, // Recursive: Shared[Locked[T]]
    // Part II, 10.6 / ADR-008: `&str` is a view marker, not a lifetime. The
    // flag records that the source said `&`; what it lowers to is the emitter's
    // business.
    pub is_view: bool,
    /// `(A, B)`: the parts are in `generics` and `name` says nothing. A tuple
    /// is a type with no name and a fixed number of parts, and putting the
    /// parts where the arguments go is what lets the view analysis
    /// (`holds_view`, `names_borrowing`) reach them without knowing about
    /// tuples at all.
    pub is_tuple: bool,
    /// Part I 2.3: a trailing `?`. A type is non-nullable unless it says
    /// otherwise, and `&str?` is a nullable view - so this is a flag beside
    /// `is_view` rather than a wrapper, because the two are independent.
    pub is_nullable: bool,
    /// `fn(A, B) -> R sync throws`
    /// ([ADR-102](../../../docs/specification/adr/adr-102.md) D1): a parameter
    /// may be **code**, and the type says what the code may do.
    ///
    /// The parameters go in `generics`, where a tuple's parts go and for the
    /// same reason: everything that walks a type's arguments walks them
    /// without knowing what this is. What is here is the part a tuple has no
    /// room for - the result and the two promises.
    pub code: Option<Box<Code>>,
    /// **An integer where a type argument stands**
    /// ([ADR-152](../../../docs/specification/adr/adr-152.md) D1): the `3` of
    /// `Array[f64, 3]`. Nothing else in the type grammar is a number, so this
    /// is set on exactly the position the record opened and is `None`
    /// everywhere else - which is what keeps every reader of a type that does
    /// not care about arrays unchanged.
    ///
    /// `name` holds the digits as they were written, so a message that prints
    /// a type prints the number the source wrote.
    pub count: Option<i64>,
    /// **`&mut T`** ([ADR-147](../../../docs/specification/adr/adr-147.md) D1):
    /// a view the callee may write through.
    ///
    /// Beside `is_view` rather than inside it, because the two are independent
    /// in exactly the way `is_view` and `is_nullable` are: what the `&` says is
    /// *a view*, and what the `mut` adds is *and it may be written*. Set only
    /// where the source wrote the word, which today is an `extern "C"`
    /// declaration and nowhere else - the C boundary is the one place this
    /// language has a use for the distinction, because a parameter's
    /// mutability is otherwise written `mut name: T`
    /// ([ADR-094](../../../docs/specification/adr/adr-094.md) D3).
    pub is_mut: bool,
    /// **`[T]`** ([ADR-147](../../../docs/specification/adr/adr-147.md) D1): a
    /// run of `T` whose length the caller knows and the type does not.
    ///
    /// The element goes in `generics`, where a tuple's parts and a function
    /// type's parameters go, and for the same reason: everything that walks a
    /// type's arguments walks this without knowing what it is.
    ///
    /// It is always behind a `&` - `&[u8]` and `&mut [u8]` are the two forms
    /// D1 writes, and a bare `[T]` is a value of no size, which this language
    /// has nowhere to put.
    pub is_slice: bool,
    /// **Text that is a view or text of its own, decided per value**
    /// ([ADR-222](../../../docs/specification/adr/adr-222.md) D3). Never
    /// written: set by [`crate::text_tiers`] on a `String` field or result
    /// that both kinds of text flow into, which is then also `is_view`. Below
    /// it is `nikaia_std::either_text::EitherText`, which borrows a view and
    /// owns what it was handed, so neither kind pays for the other.
    pub either: bool,
}

/// One line of an `extern "C"` block, before the two shapes are taken apart
/// ([ADR-147](../../../docs/specification/adr/adr-147.md) D3).
///
/// The grammar's own type and nothing else's: a block may interleave the two
/// freely and `Item::Extern` holds them in two lists, so this exists for the
/// length of one rule's action.
#[derive(Debug, Clone)]
pub enum ExternMember {
    Declared(Spanned<TraitMethod>),
    Opaque(Spanned<OpaqueType>),
}

/// **An address this language never dereferences**
/// ([ADR-147](../../../docs/specification/adr/adr-147.md) D3).
///
/// `opaque type sqlite3 released by sqlite3_close`. It is moved and stored like
/// any value, it has no fields and no indexing, and its release is a `cleanup`
/// the compiler runs at the end of its scope (Part I 6.4) — so a handle cannot
/// outlive what it names unless a C function makes it, which is the one thing
/// this language cannot check.
#[derive(Debug, Clone)]
pub struct OpaqueType {
    pub name: Ident,
    /// The function that ends its life, declared in the same block.
    pub released_by: Ident,
}

/// What a function type says besides its parameters
/// ([ADR-102](../../../docs/specification/adr/adr-102.md) D1, D2).
///
/// **The defaults are the language's** (D2): without `sync` the code may pause,
/// without `throws` it cannot fail. That is the reading a *declaration* already
/// has, applied to a type - which is the whole of why it needs no words of its
/// own.
#[derive(Debug, Clone)]
pub struct Code {
    /// `-> R`, absent where the code hands nothing back.
    pub result: Option<Type>,
    pub is_sync: bool,
    pub throws: bool,
}

/// One variant of an enum (Kap 4.4).
#[derive(Debug, Clone)]
pub struct EnumVariant {
    pub name: Ident,
    pub fields: VariantFields,
}

/// What a variant carries. The three shapes Kap 4.4 shows, and no others.
#[derive(Debug, Clone)]
pub enum VariantFields {
    /// `Quit`
    Unit,
    /// `Write(String)` - positional, read by position.
    Tuple(Vec<Type>),
    /// `Move { x: i32, y: i32 }` - named, read by name.
    Named(Vec<FieldDef>),
}

/// One arm of a `select { … }`: `pattern = expr => { … }`
/// ([ADR-148](../../../docs/specification/adr/adr-148.md) D1).
///
/// The expression is started with every other arm's; if it is the first to
/// finish, its value is bound to the pattern and the body runs.
#[derive(Debug, Clone)]
pub struct SelectArm {
    /// The name the winner's value is bound to, or `None` for `_`.
    ///
    /// **`_` is the ignore pattern and not a catch-all arm**
    /// ([ADR-126](../../../docs/specification/adr/adr-126.md) D1): a value
    /// *arrives* here and is ignored, which is that record's own position and
    /// not what [ADR-145](../../../docs/specification/adr/adr-145.md) took away
    /// from it one construct over.
    pub binding: Option<Ident>,
    /// What is started. It is an expression and not a statement, because an arm
    /// races a value rather than an effect.
    pub value: Expr,
    /// What runs if this arm wins.
    pub body: Block,
    /// Where the arm starts, which is the key the checker's answers are filed
    /// under ([ADR-028](../../../docs/specification/adr/adr-028.md)).
    pub at: usize,
}

/// One arm of a `match` (Kap 3.4).
#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: MatchPattern,
    /// `if x == y` after the pattern
    /// ([ADR-137](../../../docs/specification/adr/adr-137.md) D2).
    ///
    /// **The word is `if`**, which is the one every neighbouring language uses
    /// for this and the one this language already uses for a condition; a
    /// second word for the same idea would be a keyword spent on nothing.
    ///
    /// On the *arm* and not on the pattern, because that is what it is: the
    /// pattern says which values reach the arm and the guard says which of
    /// those the arm takes, and an or-pattern has one guard rather than one per
    /// alternative.
    pub guard: Option<Expr>,
    pub body: Expr,
}

/// What an arm matches. Deliberately small: every shape here is one the
/// language it lowers to spells the same way, so the lowering stays name for
/// name (ADR-011 D2) and no pattern means something different in the two.
#[derive(Debug, Clone)]
pub enum MatchPattern {
    /// `else`: the arm taken when none of the others matched
    /// ([ADR-145](../../../docs/specification/adr/adr-145.md) D1).
    ///
    /// **Not `Wildcard`**, which the node was called while the arm was written
    /// `_`: [ADR-126](../../../docs/specification/adr/adr-126.md) argues
    /// against that word for `_` itself, and it is wrong here twice over —
    /// nothing is being matched loosely, and nothing arrived to be ignored.
    Otherwise,
    /// `1`, `"text"`, `true`
    Literal(Expr),
    /// `Op::Times`, and a bare name - which *binds*, as it does in the
    /// language below. One rule, drawn in one place.
    Path(Vec<Ident>),
    /// `Message::Write(text)`, `(0, y)`, `Event::Click(Point { x, .. })`
    /// ([ADR-137](../../../docs/specification/adr/adr-137.md) D1).
    ///
    /// **The parts are patterns**, which is what makes a pattern nest: a
    /// binding is a one-segment `Path`, so `Message::Write(text)` is what it
    /// always was and `Event::Click(Point { x, .. })` is the same shape one
    /// level deeper.
    ///
    /// An empty `path` is the **bare** tuple `(0, 0)`, which names no type.
    Tuple {
        path: Vec<Ident>,
        parts: Vec<MatchPattern>,
    },
    /// `Message::Move { x, y }` - shorthand only, because a rename is a `let`
    /// in the arm and needs no syntax of its own.
    Named {
        path: Vec<Ident>,
        bindings: Vec<Ident>,
        /// `..` for the fields this pattern does not name
        /// ([ADR-137](../../../docs/specification/adr/adr-137.md) D1).
        rest: bool,
    },
    /// `(0, y) | (y, 0)` - one arm, several shapes
    /// ([ADR-137](../../../docs/specification/adr/adr-137.md) D1).
    ///
    /// **Every alternative binds the same set of names**, which is the rule
    /// that keeps the arm's body answerable: a name the body reads has to be
    /// bound whichever alternative matched. `NK1155` is the refusal.
    Or(Vec<MatchPattern>),
    /// `200..299` - a range, **inclusive at both ends**
    /// ([ADR-137](../../../docs/specification/adr/adr-137.md) D3).
    ///
    /// A pattern is a set of values and a reader reads it as one; there is no
    /// counting-to-`n` in it, which is what makes the exclusive reading natural
    /// in a `for` and unnatural here. `..<` is never written in a pattern (D4):
    /// an exclusive range is written by moving the end.
    Range { start: Expr, end: Expr },
}

#[derive(Debug, Clone)]
pub struct GenericParam {
    pub name: Ident,
    /// Kap 4.7: `[T: Summarize]` - the traits a caller's type has to implement.
    ///
    /// Several are written `[T: A + B]`, which is why this is a list rather than
    /// an `Option`. Empty for a parameter with no bound, which says that a body
    /// may move and pass its value and nothing else
    /// ([ADR-074](../../../docs/specification/adr/adr-074.md) D5's `NK1126`).
    pub bounds: Vec<Ident>,
}

/// One method of a `trait` declaration: a signature, with no body (Kap 4.7).
///
/// A shape of its own rather than an `Item::Fn` with an empty block, because the
/// difference between *"declares this method"* and *"this method does nothing"*
/// is the whole of what a trait is - and a body that is absent cannot be
/// accidentally emitted.
#[derive(Debug, Clone)]
pub struct TraitMethod {
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub receiver: Option<Receiver>,
    pub args: Vec<FnArg>,
    pub config: Vec<ConfigParam>,
    pub ret_type: Option<Type>,
    pub is_sync: bool,
    pub throws: bool,
}

#[derive(Debug, Clone)]
pub struct FnArg {
    pub name: Ident,
    pub ty: Type,
    /// `mut out: Vec[i64]`: the callee changes this parameter **in place**, and
    /// the caller's value is what changes
    /// ([ADR-094](../../../docs/specification/adr/adr-094.md) D3). It lowers to
    /// `&mut T`.
    ///
    /// `&mut self`'s rule, held for every parameter: the call shows nothing,
    /// exactly as `xs.push(1)` shows nothing. A language that hides mutation
    /// through a receiver and shows it through an argument has two rules for
    /// one thing.
    ///
    /// A callee that wants a mutable *copy* of an owned parameter writes
    /// `let mut v = x` inside, which is what it writes for any other value.
    pub mutable: bool,
    /// Where the parameter is written.
    ///
    /// **A declaration needs one of its own**, which a statement's span cannot
    /// stand in for: a diagnostic about a parameter has to put its caret on the
    /// parameter, and the nearest span a body's walk has is its first statement,
    /// which is a different line
    /// ([ADR-051](../../../docs/specification/adr/adr-051.md) D4).
    pub span: Span,
}

/// `timeout: 60` at a call site.
#[derive(Debug, Clone)]
pub struct ConfigArg {
    pub name: Ident,
    pub value: Expr,
}

/// Kap 5.1: one option, after the `;`.
///
/// It always has a default, which is what makes it an *option*: a caller may
/// name it or leave it out, and leaving it out is never a question about what
/// the value is. A parameter that must be passed belongs before the `;`.
#[derive(Debug, Clone)]
pub struct ConfigParam {
    pub name: Ident,
    pub ty: Type,
    /// A **literal**, and only a literal.
    ///
    /// An option's default is a constant in every program anyone writes, and an
    /// arbitrary expression would raise a question Stage 0 has no answer for -
    /// whether it is evaluated where the function is declared or where it is
    /// called. Deciding that is worth doing when something needs it.
    pub default: Expr,
}

#[derive(Debug, Clone)]
pub struct FieldDef {
    pub name: Ident,
    pub ty: Type,
    /// Kap 9.2: a field is visible outside the file that declares its struct
    /// only where it says `pub`.
    pub is_public: bool,
    /// Where the field is written, for the reason [`FnArg::span`] gives.
    pub span: Span,
}

// Part III, Kap 16.1: $dst = out(reg) result
#[derive(Debug, Clone)]
pub struct AsmBinding {
    pub alias: Ident,      // $dst
    pub direction: String, // "out", "in", "inout"
    pub location: String,  // "reg", "mem"
    pub variable: Ident,   // result
}

/// `Reading { name, temp }` - a field with no value is shorthand for `name: name`.
#[derive(Debug, Clone)]
pub struct FieldInit {
    pub name: Ident,
    pub value: Option<Expr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
    Ref,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

// --- Part II, Kapitel 10: Grammatiken ---

/// `grammar Measurements { ... }`
#[derive(Debug, Clone)]
pub struct GrammarDef {
    pub name: Ident,
    pub rules: Vec<GrammarRule>,
}

/// `@frame(boundary: "\n") pub rule MEASUREMENT -> Reading = ... -> { ... }`
#[derive(Debug, Clone)]
pub struct GrammarRule {
    pub name: Ident,
    pub is_public: bool,
    /// Part II, 10.7: the rule is a resynchronization unit.
    pub frame: Option<FrameAttr>,
    pub ret_type: Option<Type>,
    /// `# "expression"`: what this rule is called in a message that fails at
    /// its own start, instead of everything its alternatives could begin with.
    pub label: Option<String>,
    pub alts: Vec<GrammarAlt>,
    pub span: Span,
}

/// The keyed attribute of ADR-009 D1: `@frame`, `@frame(boundary: "\n")`,
/// `@frame(boundary: "\n", unchecked)`. A bare `@frame` leaves `boundary`
/// empty and the boundary is inferred downstream from the trailing literal.
#[derive(Debug, Clone, Default)]
pub struct FrameAttr {
    pub boundary: Option<String>,
    pub unchecked: bool,
}

/// One alternative of a rule: a pattern and the action that builds its value.
#[derive(Debug, Clone)]
pub struct GrammarAlt {
    pub pattern: Spanned<Pattern>,
    pub action: Option<Block>,
}

#[derive(Debug, Clone)]
pub enum Pattern {
    /// `a b c` - matched in order.
    Seq(Vec<Spanned<Pattern>>),
    /// `a | b` - first match wins.
    Choice(Vec<Spanned<Pattern>>),
    /// `name:pattern` - binds the result for the action block.
    Bind {
        name: Ident,
        pat: Box<Spanned<Pattern>>,
    },
    /// `";"`
    Literal(String),
    /// A rule reference (`NAME`), a built-in (`digit`, `frame_end`), or a call
    /// to either (`until(";" | frame_end)`, `list(pair, ",")`). One node,
    /// because the grammar cannot tell them apart and does not need to.
    ///
    /// `generics` is what `dec[i32](digit{1,2})` writes between the name and
    /// the arguments. Nikaia spells a type argument with brackets everywhere
    /// (Kap 4.3), and the backend spells it with angles; the emitter is where
    /// the two meet.
    Ref {
        name: Ident,
        generics: Vec<Type>,
        args: Vec<Spanned<Pattern>>,
    },
    /// `p*`, `p+`, `p?`, `p{1,2}`
    Repeat {
        pat: Box<Spanned<Pattern>>,
        rep: Repeat,
    },
    /// `( ... )` - grouping only, never a delimiter.
    Group(Box<Spanned<Pattern>>),
    /// `=>` - the commit point (Part II, 10.1).
    Cut,
    /// `fold(...)` / `par_fold(...)`
    Fold(Box<FoldSpec>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repeat {
    Star,
    Plus,
    Optional,
    /// `p{n}`
    Exactly(u32),
    /// `p{n,}`
    AtLeast(u32),
    /// `p{n,m}`
    Between(u32, u32),
}

/// `fold(rule, init, step)` and `par_fold(rule, init, step, merge)`.
///
/// The merge is what separates them: ADR-009 D2 - parallel parsing is a frame
/// plus a monoid, and the monoid is exactly this merge.
#[derive(Debug, Clone)]
pub struct FoldSpec {
    pub parallel: bool,
    pub rule: Ident,
    pub init: Expr,
    pub step: Expr,
    pub merge: Option<Expr>,
}

/// Kap 4.2: what a method takes as its subject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Receiver {
    pub is_ref: bool,
    pub is_mut: bool,
}

/// What a function declaration takes: an optional subject and the rest.
#[derive(Debug, Clone, Default)]
pub struct FnParams {
    pub receiver: Option<Receiver>,
    pub args: Vec<FnArg>,
    pub config: Vec<ConfigParam>,
    /// ADR-007 D5: `...args: Self::dsl`, where a config zone holds one instead
    /// of options.
    pub spread: Option<Ident>,
}

/// What stands after the `;` in a declaration: options, or the typed spread of
/// [ADR-007](../../../docs/specification/adr/adr-007.md) D5.
///
/// Exclusive on purpose. An option has a default and a DSL parameter does not -
/// it is required, and what makes it configuration is that it is named rather
/// than positional. A declaration that mixed them would have to say which rule
/// applies to which name.
#[derive(Debug, Clone)]
pub enum ConfigZone {
    Options(Vec<ConfigParam>),
    /// The name the parameter struct arrives under.
    Spread(Ident),
}
