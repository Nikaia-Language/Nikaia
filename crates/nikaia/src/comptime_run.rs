//! **A `comptime` that calls a function is compiled and run**
//! ([ADR-321](../../../docs/specification/adr/adr-321.md) D1).
//!
//! The initialiser and everything it calls are lowered as the program is,
//! compiled for the machine that builds and run there; what the program prints
//! is the value. One implementation of the language computes the build-time
//! and the run-time answer, so an overflow stops the build where it would stop
//! the program, and a function of `std`'s Rust half runs as it does when the
//! program runs.
//!
//! **What is built here** (#468's first stage): one small program per
//! `comptime`, against `std` alone. It is linked against one dynamic library
//! that names `std` (D3, the *bundle*), compiled by `rustc` directly and run
//! once; its answer is kept under a key of the code that produced it (D4), so
//! an unchanged `comptime` is neither compiled nor run again - which is also
//! what keeps a lowering inside Cargo's own build from starting another.
//!
//! The program is the file's items without its `fn main` and without its
//! `comptime`s, then the `comptime`s already worked out, each as the literal it
//! came to, and a function that hands back the initialiser. Its `main` writes
//! the value in the encoding a grammar run already uses
//! ([`crate::grammar_run::decode`]).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use crate::ast::{self, Block, Expr, Item, Span, Spanned, Stmt};
use crate::build_time::Value;
use crate::contracts::ty::Ty;
use crate::grammar_run::Wall;
use crate::parser::Parsed;

/// The name the sub-program hands the value back under.
pub(crate) const VALUE_FN: &str = "__nikaia_comptime_value";

/// **The sub-program for one initialiser**, or nothing where this stage does
/// not build one: a type with no written form, or an earlier `comptime` whose
/// value is not a literal.
///
/// `bound` is the `comptime` being worked out; `known` is what an earlier one
/// came to, by name, and nothing for one not worked out yet - which is left
/// out, as the one being worked out is.
pub(crate) fn sub_program(
    parsed: &Parsed,
    bound: &str,
    value: &Expr,
    ty: &Ty,
    known: &dyn Fn(&str) -> Option<Value>,
) -> Option<Parsed> {
    let mut kept = unit_without_its_constants(parsed, bound, known)?;
    let returns = written_type(ty, &kept)?;
    let main = kept.interner.intern_string("main");
    let value_fn = kept.interner.intern_string(VALUE_FN);
    kept.program.items.push(Spanned::new(
        a_function(main, None, Vec::new()),
        Span::nowhere(),
    ));
    kept.program.items.push(Spanned::new(
        a_function(
            value_fn,
            Some(returns),
            vec![Spanned::new(
                Stmt::Return(Some(value.clone())),
                Span::nowhere(),
            )],
        ),
        Span::nowhere(),
    ));
    Some(kept)
}

/// **One file of the program as the sub-program carries it**: without its
/// `fn main`, and with each `comptime` that is not `bound` as the literal it
/// came to - or left out, where it has not been worked out. Nothing where one
/// that was worked out has no literal here.
pub(crate) fn unit_without_its_constants(
    parsed: &Parsed,
    bound: &str,
    known: &dyn Fn(&str) -> Option<Value>,
) -> Option<Parsed> {
    let mut kept = parsed.keeping(|item| match item {
        // **The program's `main` is not this program's**, and its constants
        // are carried below as what they came to.
        Item::Fn {
            name: Some(name), ..
        } => parsed.text(*name) != "main",
        Item::Comptime { .. } => false,
        _ => true,
    });
    for item in &parsed.program.items {
        let Item::Comptime {
            name,
            ty: declared,
            public,
            ..
        } = &item.node
        else {
            continue;
        };
        let named = parsed.text(*name);
        if named == bound {
            continue;
        }
        let Some(held) = known(named) else {
            continue;
        };
        kept.program.items.push(Spanned::new(
            Item::Comptime {
                name: *name,
                ty: declared.clone(),
                value: literal_of(&held)?,
                public: *public,
            },
            item.span,
        ));
    }
    Some(kept)
}

/// **A program of several files, lowered as one** (Part I 9.1): each file
/// against all of them, in order, as `modules::Program` lowers a package -
/// `entry` is the file that holds the value function and so the `fn main`.
pub(crate) fn lowered_units(
    units: &[Parsed],
    entry: usize,
    build: crate::emit::Build,
    contracts: &crate::contracts::Ledger,
    library: &crate::contracts::Ledger,
) -> Option<crate::emit::Lowered> {
    use crate::emit::{Lowered, Needs, SourceMap};
    let beside: Vec<&Parsed> = units.iter().collect();
    let provenance = crate::contracts::trust::analyse(&units[entry], library).provenance;
    let needs = units.iter().fold(Needs::default(), |acc, unit| {
        acc.join(Needs::of(unit, build))
    });
    let mut rust = String::from("// Generated: a `comptime`'s program (ADR-321).\n\n");
    rust.push_str(&needs.preamble());
    rust.push('\n');
    let mut map = SourceMap::default();
    for (at, unit) in units.iter().enumerate() {
        let body = crate::emit::emit_module_body_at(
            unit,
            &beside,
            build,
            provenance,
            contracts,
            &crate::contracts::Ledger::blank(),
            at == entry,
            &crate::assets::Reads::none(),
        )
        .ok()?;
        map.extend(body.map.placed(rust.len(), at));
        rust.push_str(&body.rust);
        rust.push('\n');
    }
    rust.push_str("\nconst __NIKAIA_SITES: &[nikaia_std::abort::Site] = &[];\n");
    Some(Lowered {
        rust,
        map,
        published: Default::default(),
    })
}

fn a_function(
    name: winnow_grammar::Symbol,
    ret_type: Option<ast::Type>,
    stmts: Vec<Spanned<Stmt>>,
) -> Item {
    Item::Fn {
        name: Some(name),
        generics: Vec::new(),
        receiver: None,
        args: Vec::new(),
        config: Vec::new(),
        spread: None,
        ret_type,
        body: Block { stmts },
        is_sync: false,
        sync_by: Vec::new(),
        is_public: false,
        can_throw: false,
    }
}

/// An earlier `comptime`'s value as the literal that writes it: a number, a
/// truth value or text. Anything else has no literal here.
fn literal_of(value: &Value) -> Option<Expr> {
    match value {
        Value::Int(n) => Some(Expr::LitInt {
            value: n.magnitude,
            negative: n.negative,
        }),
        Value::Bool(b) => Some(Expr::LitBool(*b)),
        Value::Float(f) => Some(Expr::LitFloat(format!("{f:?}"))),
        Value::Text(text) => Some(Expr::LitStr {
            text: crate::build_time::written(text),
            at: 0,
        }),
        _ => None,
    }
}

/// `ty` as the sub-program writes it in the value function's signature.
fn written_type(ty: &Ty, parsed: &Parsed) -> Option<ast::Type> {
    let plain = |name: &str| ast::Type {
        name: parsed.interner.intern_string(name),
        generics: Vec::new(),
        is_view: false,
        is_tuple: false,
        is_nullable: false,
        code: Box::new(None),
        count: None,
        is_mut: false,
        is_slice: false,
        either: false,
    };
    match ty {
        Ty::Named { name, args, view } => {
            let mut out = plain(name);
            out.is_view = *view;
            out.generics = args
                .iter()
                .map(|arg| written_type(arg, parsed))
                .collect::<Option<_>>()?;
            Some(out)
        }
        Ty::Count(n) => {
            let mut out = plain(&n.to_string());
            out.count = Some(*n);
            Some(out)
        }
        Ty::Tuple(parts) => {
            let mut out = plain("");
            out.is_tuple = true;
            out.generics = parts
                .iter()
                .map(|part| written_type(part, parsed))
                .collect::<Option<_>>()?;
            Some(out)
        }
        Ty::Nullable(inner) => {
            let mut out = written_type(inner, parsed)?;
            out.is_nullable = true;
            Some(out)
        }
        _ => None,
    }
}

/// **The lowered sub-program with a `main` of its own**: the generated one
/// starts the runtime and runs the program's body, and this one reports a stop,
/// computes the value and writes it.
///
/// **A stop names the byte it came from**, where the program's own table names
/// a `.nika` line (ADR-300 D9): the sub-program has no file of its own, and the
/// checker turns the byte back into a place in the file it is checking. A row
/// is the generated line, [`STOPPED_AT`], and the byte the line's outermost
/// node starts at.
pub(crate) fn with_driver(
    lowered: &crate::emit::Lowered,
    dump: &str,
    bounds: Bounds,
) -> Option<String> {
    let rust = &lowered.rust;
    let start = rust.find("\nfn main() {\n")?;
    let end = start + 1 + rust[start + 1..].find("\n}\n")? + 3;
    // **The value is computed on a thread of its own**, with a stack that
    // holds `build_time::DEEPEST` calls, and against the budget (D7).
    let main = format!(
        "\nfn main() {{\n\
         \x20   nikaia_std::abort::report_in_nikaia_terms(__NIKAIA_SITES);\n\
         \x20   nikaia_bundle::LIVE.bound({memory});\n\
         \x20   let computed = std::thread::Builder::new()\n\
         \x20       .stack_size(nikaia_std::build_time::STACK)\n\
         \x20       .spawn(|| {{\n\
         \x20           nikaia_std::build_time::start({budget});\n\
         \x20           let value = {VALUE_FN}();\n\
         \x20           let mut out = String::new();\n\
         {dump}\
         \x20           out\n\
         \x20       }})\n\
         \x20       .expect(\"the build-time thread\")\n\
         \x20       .join();\n\
         \x20   match computed {{\n\
         \x20       Ok(out) => println!(\"{{out}}\"),\n\
         \x20       Err(_) => std::process::exit(101),\n\
         \x20   }}\n\
         }}\n",
        budget = bounds.steps,
        memory = bounds.bytes
    );
    let mut starts = vec![0usize];
    starts.extend(
        rust.char_indices()
            .filter(|(_, c)| *c == '\n')
            .map(|(i, _)| i + 1),
    );
    let mut rows: std::collections::BTreeMap<usize, (usize, usize)> =
        std::collections::BTreeMap::new();
    // A line below the `main` this replaces moves by what the new one adds.
    let lines = |text: &str| text.matches('\n').count() as isize;
    let moved = lines(&main) - lines(&rust[start..end]);
    for (generated, byte, unit) in lowered.map.rows() {
        if (start..end).contains(&generated) || byte == 0 {
            continue;
        }
        let line = match starts.binary_search(&generated) {
            Ok(i) => i + 1,
            Err(i) => i,
        };
        let line = match generated >= end {
            true => (line as isize + moved) as usize,
            false => line,
        };
        rows.entry(line).or_insert((unit, byte));
    }
    let mut table = String::from("const __NIKAIA_SITES: &[nikaia_std::abort::Site] = &[\n");
    for (line, (unit, byte)) in rows {
        table.push_str(&format!("    ({line}, \"{STOPPED_AT}{unit}\", {byte}),\n"));
    }
    table.push_str("];\n");
    let rest = &rust[end..];
    let empty = rest.find("const __NIKAIA_SITES")?;
    let after = empty + rest[empty..].find("];\n")? + 3;
    // **Nothing goes in front of the program**: the table maps its lines, and
    // one line more above them would name the wrong one.
    Some(format!(
        "{}{main}{}{table}{}{}\nextern crate nikaia_bundle;\n",
        &rust[..start],
        &rest[..empty],
        &rest[after..],
        crate::grammar_run::DUMP_HELPERS
    ))
}

/// **What a build-time run may spend** ([ADR-321](../../../docs/specification/adr/adr-321.md)
/// D7, D10): counted steps and bytes live at once. How a `comptime` raises
/// either is not decided (D11); a workshop carries them, so that this
/// compiler's own tests can ask for less.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    pub steps: u64,
    pub bytes: u64,
}

impl Default for Bounds {
    /// Ten billion steps (D7) and 4 GiB (D10).
    fn default() -> Bounds {
        Bounds {
            steps: 10_000_000_000,
            bytes: 4 << 30,
        }
    }
}

/// What a stop's row names instead of a file: the file's place among the
/// program's follows it, and then the byte.
pub(crate) const STOPPED_AT: &str = "@";

// --- compiling and running -----------------------------------------------------

/// **The dynamic library build-time code links against** (D3): one crate that
/// names `std`, built by Cargo from the same compiled dependencies as anything
/// else in the workshop.
#[derive(Debug)]
pub(crate) struct Bundle {
    /// The library itself.
    dylib: PathBuf,
    /// `std` as Cargo compiled it, which `rustc` is pointed at so that a path
    /// `nikaia_std::…` resolves in every module; the code is the library's.
    std_rlib: PathBuf,
    /// Where the libraries it was built from are.
    deps: PathBuf,
    /// What changes when the library is rebuilt: part of every key.
    stamp: String,
}

/// **The bundle's one file**: `std`, and the allocator every build-time run
/// uses ([ADR-321](../../../docs/specification/adr/adr-321.md) D10). Here and
/// not in each run's program, because a program linked against `std`
/// dynamically uses the allocator of the library it links; `unsafe` because an
/// allocator is, and generated because `std` has none (ADR-218).
const BUNDLE: &str = "// GENERATED (ADR-321 D3, D10).\n\
pub extern crate nikaia_std;\n\
\n\
/// What every run has live, against the bound its program sets.\n\
pub static LIVE: nikaia_std::build_time::Counted = nikaia_std::build_time::Counted::new();\n\
\n\
struct Heap;\n\
\n\
// SAFETY: every request is the system allocator's, unchanged; this only counts.\n\
unsafe impl std::alloc::GlobalAlloc for Heap {\n\
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {\n\
        LIVE.take(layout.size());\n\
        unsafe { std::alloc::GlobalAlloc::alloc(&std::alloc::System, layout) }\n\
    }\n\
    unsafe fn alloc_zeroed(&self, layout: std::alloc::Layout) -> *mut u8 {\n\
        LIVE.take(layout.size());\n\
        unsafe { std::alloc::GlobalAlloc::alloc_zeroed(&std::alloc::System, layout) }\n\
    }\n\
    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {\n\
        LIVE.give(layout.size());\n\
        unsafe { std::alloc::GlobalAlloc::dealloc(&std::alloc::System, ptr, layout) }\n\
    }\n\
    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, size: usize) -> *mut u8 {\n\
        match size >= layout.size() {\n\
            true => LIVE.take(size - layout.size()),\n\
            false => LIVE.give(layout.size() - size),\n\
        }\n\
        unsafe { std::alloc::GlobalAlloc::realloc(&std::alloc::System, ptr, layout, size) }\n\
    }\n\
}\n\
\n\
#[global_allocator]\n\
static HEAP: Heap = Heap;\n";

/// Build the bundle under `at`, or say why it could not be built.
pub(crate) fn bundle(at: &Path) -> Result<Bundle, String> {
    let dir = at.join("bundle");
    std::fs::create_dir_all(dir.join("src")).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut manifest = String::from(
        "# GENERATED. The library build-time code links against (ADR-321 D3).\n\n\
         [workspace]\n\n\
         [package]\nname = \"nikaia_bundle\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n\
         [lib]\npath = \"src/lib.rs\"\n\n[dependencies]\n",
    );
    for (name, value) in crate::project::runtime_dependencies_for("nikaia_std") {
        manifest.push_str(&format!("{name} = {value}\n"));
    }
    write_if_changed(&dir.join("Cargo.toml"), &manifest)?;
    write_if_changed(&dir.join("src").join("lib.rs"), BUNDLE)?;
    let built = Command::new(cargo())
        .args(["rustc", "--quiet", "--lib", "--crate-type", "dylib"])
        .arg("--message-format=json")
        .arg("--manifest-path")
        .arg(dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(at.join("target"))
        .args(["--", "-C", "prefer-dynamic"])
        .output()
        .map_err(|e| format!("running cargo: {e}"))?;
    if !built.status.success() {
        return Err(String::from_utf8_lossy(&built.stderr).to_string());
    }
    let mut dylib = None;
    let mut std_rlib = None;
    for line in String::from_utf8_lossy(&built.stdout).lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if message["reason"] != "compiler-artifact" {
            continue;
        }
        let files = message["filenames"].as_array().cloned().unwrap_or_default();
        let files = files.iter().filter_map(|f| f.as_str()).map(PathBuf::from);
        match message["target"]["name"].as_str() {
            Some("nikaia_std") => {
                std_rlib = files
                    .clone()
                    .find(|f| f.extension().is_some_and(|e| e == "rlib"))
            }
            Some("nikaia_bundle") => {
                dylib = files.clone().find(|f| {
                    f.extension()
                        .is_some_and(|e| e == std::env::consts::DLL_EXTENSION)
                })
            }
            _ => {}
        }
    }
    let (Some(dylib), Some(std_rlib)) = (dylib, std_rlib) else {
        return Err("cargo built the bundle and named no library".to_string());
    };
    let deps = std_rlib
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "no directory for std".to_string())?;
    let stamp = [&dylib, &std_rlib]
        .iter()
        .map(|file| {
            let meta = std::fs::metadata(file).map_err(|e| format!("{}: {e}", file.display()))?;
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or_default();
            Ok(format!("{}:{}:{modified}", file.display(), meta.len()))
        })
        .collect::<Result<Vec<String>, String>>()?
        .join("\n");
    Ok(Bundle {
        dylib,
        std_rlib,
        deps,
        stamp,
    })
}

/// **Compile and run one build-time program**, or hand back what it said when
/// it ran before: its standard output, or `Wall::Refused` with what it wrote
/// where it stopped. Keyed on the program, the toolchain and the bundle (D4).
pub(crate) fn run(at: &Path, bundle: &Bundle, program: &str, name: &str) -> Result<String, Wall> {
    let key =
        crate::assets::digest(format!("{program}\n{}\n{}", toolchain(), bundle.stamp).as_bytes());
    let dir = at.join("comptime").join(&key);
    let answered = dir.join("answer");
    let stopped = dir.join("stopped");
    if let Ok(answer) = std::fs::read_to_string(&answered) {
        return Ok(answer);
    }
    if let Ok(said) = std::fs::read_to_string(&stopped) {
        return Err(Wall::Refused { detail: said });
    }
    let did_not = |detail: String| Wall::DidNotBuild { detail };
    let source = dir.join("main.rs");
    write_if_changed(&source, program).map_err(did_not)?;
    let binary = dir.join("run");
    let compiled = Command::new(rustc())
        .args([
            "--edition=2024",
            "--crate-name",
            "comptime",
            "--crate-type",
            "bin",
        ])
        .args(["-C", "opt-level=0", "-C", "overflow-checks=on"])
        .args([
            "-C",
            "debug-assertions=off",
            "-C",
            "prefer-dynamic",
            "-A",
            "warnings",
        ])
        .arg("--extern")
        .arg(format!("nikaia_bundle={}", bundle.dylib.display()))
        .arg("--extern")
        .arg(format!("nikaia_std={}", bundle.std_rlib.display()))
        .arg("-L")
        .arg(format!("dependency={}", bundle.deps.display()))
        .arg("-o")
        .arg(&binary)
        .arg(&source)
        .output()
        .map_err(|e| did_not(format!("running rustc: {e}")))?;
    if !compiled.status.success() {
        return Err(did_not(
            String::from_utf8_lossy(&compiled.stderr).to_string(),
        ));
    }
    let mut libraries: Vec<PathBuf> = Vec::new();
    if let Some(dir) = bundle.dylib.parent() {
        libraries.push(dir.to_path_buf());
    }
    libraries.push(bundle.deps.clone());
    libraries.push(PathBuf::from(target_libdir()));
    if let Some(held) = std::env::var_os(LIBRARY_PATH) {
        libraries.extend(std::env::split_paths(&held));
    }
    let child = Command::new(&binary)
        .env("RUST_BACKTRACE", "0")
        .env(
            LIBRARY_PATH,
            std::env::join_paths(libraries).map_err(|e| did_not(e.to_string()))?,
        )
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| did_not(format!("running {}: {e}", binary.display())))?;
    let ran =
        waited(child, name).map_err(|e| did_not(format!("running {}: {e}", binary.display())))?;
    if ran.status.success() {
        let answer = String::from_utf8_lossy(&ran.stdout).trim().to_string();
        write_if_changed(&answered, &answer).map_err(did_not)?;
        return Ok(answer);
    }
    let mut said = String::from_utf8_lossy(&ran.stderr).trim().to_string();
    // **A run that ended without a word** - killed by the system, most often
    // for its memory - says how it ended instead.
    if said.is_empty() {
        said = format!("it ended without saying why ({})", ran.status);
    }
    // **A stop the program reported is an answer**, and kept as one: the same
    // code stops the same way. A run that died without a word is not, and is
    // tried again next time.
    if said.contains("the program stopped") || said.contains("nikaia-build-time:") {
        write_if_changed(&stopped, &said).map_err(did_not)?;
    }
    Err(Wall::Refused { detail: said })
}

/// **The run, waited for, and named while it takes long**
/// ([ADR-321](../../../docs/specification/adr/adr-321.md) D12): after a few
/// seconds the build says which `comptime` is still running and for how long.
/// A message, never a bound.
fn waited(mut child: std::process::Child, name: &str) -> std::io::Result<std::process::Output> {
    use std::io::Read;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out = std::thread::spawn(move || {
        let mut held = Vec::new();
        if let Some(pipe) = stdout.as_mut() {
            let _ = pipe.read_to_end(&mut held);
        }
        held
    });
    let err = std::thread::spawn(move || {
        let mut held = Vec::new();
        if let Some(pipe) = stderr.as_mut() {
            let _ = pipe.read_to_end(&mut held);
        }
        held
    });
    let began = std::time::Instant::now();
    let mut next = std::time::Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if began.elapsed() >= next {
            eprintln!(
                "note: `{name}` is still being computed while the program is built ({} s)",
                began.elapsed().as_secs()
            );
            next += std::time::Duration::from_secs(10);
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    Ok(std::process::Output {
        status,
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    })
}

/// The variable the loader reads for where libraries are.
#[cfg(target_os = "macos")]
const LIBRARY_PATH: &str = "DYLD_LIBRARY_PATH";
#[cfg(windows)]
const LIBRARY_PATH: &str = "PATH";
#[cfg(not(any(target_os = "macos", windows)))]
const LIBRARY_PATH: &str = "LD_LIBRARY_PATH";

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string())
}

fn rustc() -> String {
    std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string())
}

/// `rustc -vV`, once: the toolchain a key is about.
fn toolchain() -> &'static str {
    static HELD: OnceLock<String> = OnceLock::new();
    HELD.get_or_init(|| {
        Command::new(rustc())
            .arg("-vV")
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).to_string())
            .unwrap_or_default()
    })
}

/// Where the toolchain's own `std` is, which a program linked with
/// `prefer-dynamic` loads.
fn target_libdir() -> &'static str {
    static HELD: OnceLock<String> = OnceLock::new();
    HELD.get_or_init(|| {
        Command::new(rustc())
            .args(["--print", "target-libdir"])
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .unwrap_or_default()
    })
}

/// Written only when it changed, so that Cargo's freshness keeps working.
fn write_if_changed(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    if std::fs::read_to_string(path).is_ok_and(|held| held == text) {
        return Ok(());
    }
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}
