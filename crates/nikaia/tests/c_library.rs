//! **A package C calls** ([ADR-284](../../../docs/specification/adr/adr-284.md)
//! D4, D5, D7, D8, D12): `artifact = "c-library"` makes a shared and a static
//! library of the package and its header, and a C program links it.
//!
//! Each C program below calls one part of D5's table: numbers, text, bytes,
//! enums, handles, `null`, callbacks and throws. Each part comes back with
//! its status and its out-parameter. A panic caught at the boundary poisons
//! the library until `shutdown` and `init` have run.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const LIBRARY: &str = "\
use std::time

pub extern fn napped(ms: i64) -> i64 {
    let other = spawn fn { 21 * 2 }
    time::sleep(ms.millis())
    let got = other.join() catch { 0 }
    return ms + got
}

pub extern fn add(a: i64, b: i64) -> i64 sync {
    return a + b
}

pub extern fn halve(x: f64) -> f64 sync {
    return x / 2.0
}

pub extern fn is_vowel(c: scalar) -> bool sync {
    return c == 'a' || c == 'e' || c == 'i' || c == 'o' || c == 'u'
}

pub extern fn pick(n: i64) -> i64 sync {
    let xs = [10, 20, 30]
    return xs[n]
}

pub extern fn greet(name: ref String) -> String sync {
    return f\"Hello, {name}\"
}

pub extern fn total(xs: ref Array[i64]) -> i64 sync {
    let mut sum = 0
    for x in xs {
        sum = sum + x
    }
    return sum
}

pub extern fn size(b: Bytes) -> i64 sync {
    return b.len()
}

pub enum Light {
    Red,
    Amber,
    Green,
}

pub extern fn after(light: Light) -> Light sync {
    match light {
        Light::Red => Light::Green
        Light::Amber => Light::Red
        Light::Green => Light::Amber
    }
}

pub struct Counter {
    pub count: i64,
    pub name: String,
    pub light: Light,
    steps: i64,
}

impl Counter {
    pub extern fn(name: ref String) -> Counter sync {
        return Counter { count: 0, name: name.clone(), light: Light::Red, steps: 0 }
    }

    pub extern fn bump(ref mut self, by: i64) -> i64 sync {
        self.count = self.count + by
        self.steps = self.steps + 1
        return self.count
    }

    pub extern fn steps(ref self) -> i64 sync {
        return self.steps
    }
}

pub extern fn sum_of(a: ref Counter, b: ref Counter) -> i64 sync {
    return a.count + b.count
}

pub extern fn count_up(to: i64, each: fn(i64) -> bool sync) -> i64 sync {
    let mut i = 0
    while i < to {
        if !each(i) {
            return i
        }
        i = i + 1
    }
    return i
}

pub extern fn words(text: ref String, each: fn(ref String, Light) sync) sync {
    for w in text.split(\" \") {
        each(w, Light::Amber)
    }
}

pub extern fn looking_at(c: ref Counter, each: fn(i64) sync) sync {
    each(c.count)
}

pub extern fn found(n: i64) -> Counter? sync {
    if n > 0 {
        let mut c = Counter(\"found\")
        c.bump(n)
        return c
    }
    return null
}

pub extern fn count_of(c: ref Counter?) -> i64 sync {
    return c?.count ?? -1
}

pub extern fn length_of(name: ref String?) -> i64 sync {
    return (name ?? \"none\").len()
}

pub extern struct Rect {
    corner: Point,
    width: f64,
    height: f64,
    light: Light,
}

pub extern struct Point {
    x: f64,
    y: f64,
}

impl Point {
    pub extern fn sum(ref self) -> f64 sync {
        return self.x + self.y
    }

    pub extern fn shift(ref mut self, by: f64) sync {
        self.x = self.x + by
        self.y = self.y + by
    }
}

pub extern struct Quad {
    sides: Array[f64, 4],
    lights: Array[Light, 2],
    corners: Array[Point, 2],
}

pub extern fn perimeter(q: Quad) -> f64 sync {
    let mut sum = 0.0
    for side in q.sides {
        sum = sum + side
    }
    for corner in q.corners {
        sum = sum + corner.x
    }
    return sum
}

pub extern fn flipped(q: Quad) -> Quad sync {
    return Quad {
        sides: q.sides,
        lights: [q.lights[1], q.lights[0]],
        corners: [q.corners[1], q.corners[0]],
    }
}

pub extern fn widest(points: ref Array[Point]) -> f64 sync {
    let mut most = 0.0
    for p in points {
        if p.x > most {
            most = p.x
        }
    }
    return most
}

pub extern fn diagonal(n: i64) -> Vec[Point] sync {
    let mut out: Vec[Point] = []
    let mut i = 0
    let mut at = 0.0
    while i < n {
        out.push(Point { x: at, y: 2.0 * at })
        i = i + 1
        at = at + 1.0
    }
    return out
}

pub extern fn grown(b: Rect, by: f64) -> Rect sync {
    return Rect {
        corner: Point { x: b.corner.x - by, y: b.corner.y - by },
        width: b.width + 2.0 * by,
        height: b.height + 2.0 * by,
        light: Light::Green,
    }
}

pub struct Hits {
    count: SharedMut[i64],
}

impl Hits {
    pub extern fn() -> Hits {
        return Hits { count: SharedMut(0) }
    }

    pub extern fn hit(ref self) -> i64 {
        self.count.update fn(mut v) { v += 1 }
        return self.count.get()
    }
}

pub enum Refusal {
    Negative,
    TooLarge(i64),
}

impl Error for Refusal {
    fn message(ref self) -> String {
        match self {
            Refusal::Negative => \"a negative count\".clone()
            Refusal::TooLarge(n) => f\"{n} is too large\"
        }
    }
}

pub extern fn checked(n: i64) -> i64 sync throws {
    if n < 0 {
        throw Refusal::Negative
    }
    if n > 100 {
        throw Refusal::TooLarge(n)
    }
    return n * 2
}
";

const CALLER: &str = r#"#include <stdio.h>
#include <stdlib.h>
#include "calc.h"

/* The caller's allocator (ADR-284 D6): malloc's, counting what it hands out. */
static void *counted(size_t size, size_t align, void *ctx) {
    __atomic_fetch_add((long *)ctx, 1, __ATOMIC_RELAXED);
    return aligned_alloc(align < sizeof(void *) ? sizeof(void *) : align,
                         (size + align - 1) / align * align);
}

static void given_back(void *at, size_t size, size_t align, void *ctx) {
    (void)size;
    (void)align;
    (void)ctx;
    free(at);
}

int main(void) {
    long blocks = 0;
    printf("allocator %d\n", calc_set_allocator(counted, given_back, &blocks));
    printf("no allocator %d\n", calc_set_allocator(NULL, given_back, &blocks));
    int64_t sum = 0;
    double half = 0;
    bool vowel = false;
    int64_t picked = 0;
    int status = calc_add(2, 40, &sum);
    printf("add %d %lld\n", status, (long long)sum);
    status = calc_halve(5.0, &half);
    printf("halve %d %.1f\n", status, half);
    status = calc_is_vowel('e', &vowel);
    printf("vowel %d %d\n", status, vowel);
    printf("not a scalar %d\n", calc_is_vowel(0xD800, &vowel));
    status = calc_pick(1, &picked);
    printf("pick %d %lld\n", status, (long long)picked);
    printf("past the end %d\n", calc_pick(7, &picked));
    printf("after the panic %d\n", calc_add(1, 1, &sum));
    status = calc_init();
    printf("init alone %d %d\n", status, calc_add(1, 1, &sum));
    status = calc_shutdown();
    printf("shutdown %d %d\n", status, calc_add(1, 1, &sum));
    status = calc_init();
    status = calc_add(1, 1, &sum);
    printf("started again %d %lld\n", status, (long long)sum);
    printf("the caller's blocks %d\n", __atomic_load_n(&blocks, __ATOMIC_RELAXED) > 0);
    printf("allocator after init %d\n", calc_set_allocator(counted, given_back, &blocks));
    return CALC_OK;
}
"#;

/// A function that throws (ADR-284 D7): each variant is its own positive
/// status, and `calc_last_error` renders the failure with its site.
const THROWING_CALLER: &str = r#"#include <stdio.h>
#include "calc.h"

int main(void) {
    int64_t n = 0;
    char said[128];
    size_t written = 0;
    int status = calc_checked(21, &n);
    printf("checked %d %lld\n", status, (long long)n);
    status = calc_checked(-1, &n);
    calc_last_error((uint8_t *)said, sizeof said, &written);
    printf("negative %d %.16s\n", status == CALC_E_NEGATIVE, said);
    status = calc_checked(500, &n);
    calc_last_error((uint8_t *)said, sizeof said, &written);
    printf("too large %d %.16s\n", status == CALC_E_TOOLARGE, said);
    return CALC_OK;
}
"#;

/// A `pub struct` is a handle (ADR-284 D5, D11): made by its constructor,
/// changed and read by its methods and getters, freed by `_free`.
const HANDLE_CALLER: &str = r#"#include <stdio.h>
#include "calc.h"

int main(void) {
    calc_Counter *counter = NULL;
    calc_Counter *other = NULL;
    int status = calc_Counter_new((const uint8_t *)"clicks", 6, &counter);
    printf("new %d %d\n", status, counter != NULL);
    calc_Counter_new((const uint8_t *)"taps", 4, &other);
    int64_t n = 0;
    calc_Counter_bump(counter, 5, &n);
    status = calc_Counter_bump(counter, 2, &n);
    printf("bump %d %lld\n", status, (long long)n);
    calc_Counter_bump(other, 10, &n);
    calc_Counter_steps(counter, &n);
    printf("steps %lld\n", (long long)n);
    calc_Counter_count(counter, &n);
    printf("count %lld\n", (long long)n);
    char name[16];
    size_t written = 0;
    status = calc_Counter_name(counter, (uint8_t *)name, sizeof name, &written);
    printf("name %d %.*s\n", status, (int)written, name);
    calc_Light light = CALC_LIGHT_GREEN;
    calc_Counter_light(counter, &light);
    printf("light %d\n", light == CALC_LIGHT_RED);
    calc_sum_of(counter, other, &n);
    printf("sum %lld\n", (long long)n);
    printf("same twice %d\n", calc_sum_of(counter, counter, &n));
    printf("no handle %d\n", calc_Counter_steps(NULL, &n));
    printf("free %d %d %d\n", calc_Counter_free(counter), calc_Counter_free(other), calc_Counter_free(NULL));
    calc_Counter *maybe = counter;
    status = calc_found(0, &maybe);
    printf("not found %d %d\n", status, maybe == NULL);
    status = calc_found(4, &maybe);
    printf("found %d %d\n", status, maybe != NULL);
    calc_count_of(maybe, &n);
    printf("count of %lld\n", (long long)n);
    calc_count_of(NULL, &n);
    printf("count of null %lld\n", (long long)n);
    calc_Counter_free(maybe);
    calc_length_of(NULL, 0, &n);
    printf("length of null %lld\n", (long long)n);
    calc_length_of((const uint8_t *)"", 0, &n);
    printf("length of empty %lld\n", (long long)n);
    return CALC_OK;
}
"#;

/// A callback is a function pointer and a `void *ctx` (ADR-284 D5), called
/// on the calling thread; a call back into a handle the library holds for
/// the call is `E_REENTRANT` rather than a deadlock (D11).
const CALLBACK_CALLER: &str = r#"#include <stdio.h>
#include "calc.h"

static bool below(int64_t i, void *ctx) {
    return i < *(int64_t *)ctx;
}

static void word(const uint8_t *w, size_t len, calc_Light light, void *ctx) {
    (void)ctx;
    printf("word %.*s %d\n", (int)len, (const char *)w, light == CALC_LIGHT_AMBER);
}

static void again(int64_t count, void *ctx) {
    int64_t n = 0;
    int status = calc_Counter_bump((calc_Counter *)ctx, 1, &n);
    printf("looking at %lld, bump %d\n", (long long)count, status);
}

int main(void) {
    int64_t limit = 3;
    int64_t n = 0;
    int status = calc_count_up(10, below, &limit, &n);
    printf("count up %d %lld\n", status, (long long)n);
    printf("no callback %d\n", calc_count_up(10, NULL, NULL, &n));
    calc_words((const uint8_t *)"one two", 7, word, NULL);
    calc_Counter *counter = NULL;
    calc_Counter_new((const uint8_t *)"c", 1, &counter);
    status = calc_looking_at(counter, again, counter);
    printf("looking %d\n", status);
    calc_Counter_free(counter);
    return CALC_OK;
}
"#;

/// A function that may pause is exported `_async` too (ADR-284 D9, D19):
/// `done` is called once, on a library thread, with the work's status or
/// `E_CANCELLED`; the ticket is freed after it and not before.
const ASYNC_CALLER: &str = r#"#include <stdio.h>
#include <unistd.h>
#include "calc.h"

static _Atomic int finished = 0;
static int reported = 99;

static void on_done(int status, void *ctx) {
    reported = status;
    *(int *)ctx += 1;
    finished = 1;
}

int main(void) {
    int64_t n = 0;
    int calls = 0;
    calc_op *op = NULL;
    int status = calc_napped_async(200, &n, on_done, &calls, &op);
    printf("started %d %d\n", status, op != NULL);
    printf("freed too early %d\n", calc_op_free(op));
    while (!finished) usleep(1000);
    printf("done %d %lld %d\n", reported, (long long)n, calls);
    status = calc_cancel(op);
    printf("cancel after done %d\n", status);
    printf("freed %d\n", calc_op_free(op));

    finished = 0;
    status = calc_napped_async(60000, &n, on_done, &calls, &op);
    calc_cancel(op);
    status = calc_cancel(op);
    printf("cancel twice %d\n", status);
    while (!finished) usleep(1000);
    printf("cancelled %d %d\n", reported == CALC_E_CANCELLED, calls);
    calc_op_free(op);
    printf("no done %d\n", calc_napped_async(1, &n, NULL, NULL, NULL));
    return CALC_OK;
}
"#;

/// **Two of the caller's threads at once** (ADR-284 D1, D2, #113): a handle's
/// `ref self` methods run together, and the `SharedMut` inside keeps the atomic
/// count and the crossing lock a library needs at `user_parallelism = no` too.
const THREADS_CALLER: &str = r#"#include <pthread.h>
#include <stdio.h>
#include "calc.h"

static void *hitting(void *hits) {
    int64_t n = 0;
    for (int i = 0; i < 1000; i++) {
        calc_Hits_hit((const calc_Hits *)hits, &n);
    }
    return NULL;
}

int main(void) {
    calc_Hits *hits = NULL;
    calc_Hits_new(&hits);
    pthread_t one, two;
    pthread_create(&one, NULL, hitting, hits);
    pthread_create(&two, NULL, hitting, hits);
    pthread_join(one, NULL);
    pthread_join(two, NULL);
    int64_t n = 0;
    calc_Hits_hit(hits, &n);
    printf("hits %lld\n", (long long)n);
    calc_Hits_free(hits);
    return CALC_OK;
}
"#;

/// Text, bytes and a run of numbers (ADR-284 D5, D6): in as an address and a
/// length, out into the caller's buffer.
const TEXT_CALLER: &str = r#"#include <stdio.h>
#include "calc.h"

int main(void) {
    char out[64];
    size_t written = 0;
    int status = calc_greet((const uint8_t *)"Ada", 3, NULL, 0, &written);
    printf("size query %d %zu\n", status, written);
    status = calc_greet((const uint8_t *)"Ada", 3, (uint8_t *)out, 4, &written);
    printf("too small %d %zu\n", status, written);
    status = calc_greet((const uint8_t *)"Ada", 3, (uint8_t *)out, sizeof out, &written);
    printf("greet %d %.*s\n", status, (int)written, out);
    printf("not utf-8 %d\n", calc_greet((const uint8_t *)"\xff", 1, (uint8_t *)out, sizeof out, &written));
    printf("no address %d\n", calc_greet(NULL, 3, (uint8_t *)out, sizeof out, &written));
    int64_t xs[] = {1, 2, 3, 4};
    int64_t sum = 0;
    status = calc_total(xs, 4, &sum);
    printf("total %d %lld\n", status, (long long)sum);
    int64_t n = 0;
    status = calc_size((const uint8_t *)"\x01\x02\x03", 3, &n);
    printf("size %d %lld\n", status, (long long)n);
    status = calc_napped(20, &n);
    printf("napped %d %lld\n", status, (long long)n);
    calc_Light light = CALC_LIGHT_RED;
    calc_Rect box = {{1.0, 2.0}, 3.0, 4.0, CALC_LIGHT_RED};
    calc_Rect bigger;
    status = calc_grown(box, 0.5, &bigger);
    printf("grown %d %.1f %.1f %.1f %.1f %d\n", status, bigger.corner.x, bigger.corner.y,
           bigger.width, bigger.height, bigger.light == CALC_LIGHT_GREEN);
    calc_Point p = {1.0, 2.0};
    double both = 0;
    calc_Point_shift(&p, 0.5);
    status = calc_Point_sum(&p, &both);
    printf("point %d %.1f %.1f %.1f\n", status, p.x, p.y, both);
    calc_Point many[3] = {{1.0, 0.0}, {7.0, 0.0}, {3.0, 0.0}};
    calc_Quad quad = {{1.0, 2.0, 3.0, 4.5}, {CALC_LIGHT_RED, CALC_LIGHT_GREEN}, {{10.0, 0.0}, {20.0, 0.0}}};
    double around = 0;
    status = calc_perimeter(quad, &around);
    printf("perimeter %d %.1f\n", status, around);
    calc_Quad turned;
    status = calc_flipped(quad, &turned);
    printf("flipped %d %d %.1f\n", status, turned.lights[0] == CALC_LIGHT_GREEN, turned.corners[0].x);
    quad.lights[1] = (calc_Light)9;
    printf("no such light in an array %d\n", calc_perimeter(quad, &around));
    double most = 0;
    status = calc_widest(many, 3, &most);
    printf("widest %d %.1f\n", status, most);
    size_t count = 0;
    status = calc_diagonal(3, NULL, 0, &count);
    printf("diagonal asked %d %zu\n", status, count);
    printf("diagonal too small %d\n", calc_diagonal(3, many, 2, &count));
    status = calc_diagonal(3, many, 3, &count);
    printf("diagonal %d %zu %.1f %.1f\n", status, count, many[2].x, many[2].y);
    box.light = (calc_Light)9;
    printf("no such light in a box %d\n", calc_grown(box, 0.5, &bigger));
    status = calc_after(CALC_LIGHT_GREEN, &light);
    printf("after %d %d\n", status, light == CALC_LIGHT_AMBER);
    printf("no such light %d\n", calc_after((calc_Light)7, &light));
    return CALC_OK;
}
"#;

fn package(purpose: &str, manifest_build: &str, source: &str) -> PathBuf {
    let root = common::scratch_dir(purpose);
    std::fs::create_dir_all(root.join("src")).expect("the package");
    std::fs::write(
        root.join("nikaia.toml"),
        format!("[package]\nname = \"calc\"\nversion = \"0.1.0\"\n\n[build]\n{manifest_build}"),
    )
    .expect("a manifest");
    std::fs::write(root.join("src/main.nika"), source).expect("a source");
    root
}

fn nikaia(root: &Path, subcommand: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .current_dir(root)
        .args([subcommand])
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("the nikaia binary runs")
}

fn said(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// **The library, its header, and a C program that calls both ways in**:
/// values come back through the out-parameter with `OK`, a number that is no
/// scalar is `E_ARGUMENT`, and a panic is `E_PANICKED` and poisons the library
/// for every call after it (D8).
#[test]
fn a_c_program_calls_the_library() {
    if Command::new("cc").arg("--version").output().is_err() {
        eprintln!("skipped: no C compiler");
        return;
    }
    let root = package("c-library", "artifact = \"c-library\"\n", LIBRARY);
    let built = nikaia(&root, "build");
    assert!(built.status.success(), "{}", said(&built));
    let made = root.join("target/nikaia/c-library");
    let header = std::fs::read_to_string(made.join("calc.h")).expect("the header");
    assert!(
        header.contains("int calc_add(int64_t a, int64_t b, int64_t *out);"),
        "{header}"
    );
    assert!(header.contains("#define CALC_E_PANICKED (-4)"), "{header}");
    assert!(header.contains("    CALC_LIGHT_AMBER = 1,"), "{header}");
    assert!(header.contains("ledger: "), "{header}");
    assert!(made.join("libcalc.a").is_file(), "the static library");

    std::fs::write(root.join("use.c"), CALLER).expect("the caller");
    let program = root.join("use");
    let compiled = Command::new("cc")
        .arg("-I")
        .arg(&made)
        .arg(root.join("use.c"))
        .arg("-L")
        .arg(&made)
        .args(["-lcalc", "-o"])
        .arg(&program)
        .output()
        .expect("cc runs");
    assert!(compiled.status.success(), "{}", said(&compiled));
    let ran = Command::new(&program)
        .env("LD_LIBRARY_PATH", &made)
        .output()
        .expect("the caller runs");
    assert!(ran.status.success(), "{}", said(&ran));
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout),
        "allocator 0\nno allocator -1\nadd 0 42\nhalve 0 2.5\nvowel 0 1\nnot a scalar -1\npick 0 20\npast the end -4\nafter the panic -4\ninit alone 0 -4\nshutdown 0 -3\nstarted again 0 2\nthe caller's blocks 1\nallocator after init -1\n"
    );

    // Text, bytes and a run of numbers, from a second program: the first one
    // poisoned its copy of the library.
    std::fs::write(root.join("text.c"), TEXT_CALLER).expect("the caller");
    let program = root.join("text");
    let compiled = Command::new("cc")
        .arg("-I")
        .arg(&made)
        .arg(root.join("text.c"))
        .arg("-L")
        .arg(&made)
        .args(["-lcalc", "-o"])
        .arg(&program)
        .output()
        .expect("cc runs");
    assert!(compiled.status.success(), "{}", said(&compiled));
    let ran = Command::new(&program)
        .env("LD_LIBRARY_PATH", &made)
        .output()
        .expect("the caller runs");
    assert!(ran.status.success(), "{}", said(&ran));
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout),
        "size query 0 10\ntoo small -2 10\ngreet 0 Hello, Ada\nnot utf-8 -1\nno address -1\ntotal 0 10\nsize 0 3\nnapped 0 62\ngrown 0 0.5 1.5 4.0 5.0 1\npoint 0 1.5 2.5 4.0\nperimeter 0 40.5\nflipped 0 1 20.0\nno such light in an array -1\nwidest 0 7.0\ndiagonal asked 0 3\ndiagonal too small -2\ndiagonal 0 3 2.0 4.0\nno such light in a box -1\nafter 0 1\nno such light -1\n"
    );

    std::fs::write(root.join("handle.c"), HANDLE_CALLER).expect("the caller");
    let program = root.join("handle");
    let compiled = Command::new("cc")
        .arg("-I")
        .arg(&made)
        .arg(root.join("handle.c"))
        .arg("-L")
        .arg(&made)
        .args(["-lcalc", "-o"])
        .arg(&program)
        .output()
        .expect("cc runs");
    assert!(compiled.status.success(), "{}", said(&compiled));
    let ran = Command::new(&program)
        .env("LD_LIBRARY_PATH", &made)
        .output()
        .expect("the caller runs");
    assert!(ran.status.success(), "{}", said(&ran));
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout),
        "new 0 1\nbump 0 7\nsteps 2\ncount 7\nname 0 clicks\nlight 1\nsum 17\nsame twice -5\nno handle -1\nfree 0 0 0\nnot found 0 1\nfound 0 1\ncount of 4\ncount of null -1\nlength of null 4\nlength of empty 0\n"
    );

    std::fs::write(root.join("callback.c"), CALLBACK_CALLER).expect("the caller");
    let program = root.join("callback");
    let compiled = Command::new("cc")
        .arg("-I")
        .arg(&made)
        .arg(root.join("callback.c"))
        .arg("-L")
        .arg(&made)
        .args(["-lcalc", "-o"])
        .arg(&program)
        .output()
        .expect("cc runs");
    assert!(compiled.status.success(), "{}", said(&compiled));
    let ran = Command::new(&program)
        .env("LD_LIBRARY_PATH", &made)
        .output()
        .expect("the caller runs");
    assert!(ran.status.success(), "{}", said(&ran));
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout),
        "count up 0 3\nno callback -1\nword one 1\nword two 1\nlooking at 0, bump -5\nlooking 0\n"
    );

    std::fs::write(root.join("async.c"), ASYNC_CALLER).expect("the caller");
    let program = root.join("async");
    let compiled = Command::new("cc")
        .arg("-I")
        .arg(&made)
        .arg(root.join("async.c"))
        .arg("-L")
        .arg(&made)
        .args(["-lcalc", "-o"])
        .arg(&program)
        .output()
        .expect("cc runs");
    assert!(compiled.status.success(), "{}", said(&compiled));
    let ran = Command::new(&program)
        .env("LD_LIBRARY_PATH", &made)
        .output()
        .expect("the caller runs");
    assert!(ran.status.success(), "{}", said(&ran));
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout),
        "started 0 1\nfreed too early -1\ndone 0 242 1\ncancel after done 0\nfreed 0\n\
         cancel twice 0\ncancelled 1 2\nno done -1\n"
    );

    std::fs::write(root.join("threads.c"), THREADS_CALLER).expect("the caller");
    let program = root.join("threads");
    let compiled = Command::new("cc")
        .arg("-I")
        .arg(&made)
        .arg(root.join("threads.c"))
        .arg("-L")
        .arg(&made)
        .args(["-lcalc", "-lpthread", "-o"])
        .arg(&program)
        .output()
        .expect("cc runs");
    assert!(compiled.status.success(), "{}", said(&compiled));
    let ran = Command::new(&program)
        .env("LD_LIBRARY_PATH", &made)
        .output()
        .expect("the caller runs");
    assert!(ran.status.success(), "{}", said(&ran));
    assert_eq!(String::from_utf8_lossy(&ran.stdout), "hits 2001\n");

    std::fs::write(root.join("throwing.c"), THROWING_CALLER).expect("the caller");
    let program = root.join("throwing");
    let compiled = Command::new("cc")
        .arg("-I")
        .arg(&made)
        .arg(root.join("throwing.c"))
        .arg("-L")
        .arg(&made)
        .args(["-lcalc", "-o"])
        .arg(&program)
        .output()
        .expect("cc runs");
    assert!(compiled.status.success(), "{}", said(&compiled));
    let ran = Command::new(&program)
        .env("LD_LIBRARY_PATH", &made)
        .output()
        .expect("the caller runs");
    assert!(ran.status.success(), "{}", said(&ran));
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout),
        "checked 0 42\nnegative 1 a negative count\ntoo large 1 500 is too large\n"
    );
}

/// A library has nothing to run, and a library that exports nothing is
/// refused rather than built empty.
#[test]
fn a_library_is_not_run_and_exports_something() {
    let root = package("c-library-run", "artifact = \"c-library\"\n", LIBRARY);
    let ran = nikaia(&root, "run");
    assert!(!ran.status.success());
    assert!(said(&ran).contains("nothing to run"), "{}", said(&ran));

    let root = package(
        "c-library-empty",
        "artifact = \"c-library\"\n",
        "pub fn add(a: i64, b: i64) -> i64 sync {\n    return a + b\n}\n",
    );
    let built = nikaia(&root, "build");
    assert!(!built.status.success());
    assert!(said(&built).contains("exports nothing"), "{}", said(&built));
}

/// `symbol-prefix` must be a C identifier, naming the character that is not.
#[test]
fn a_prefix_that_is_no_c_identifier_is_refused() {
    let root = package(
        "c-library-prefix",
        "artifact = \"c-library\"\nsymbol-prefix = \"my-lib\"\n",
        LIBRARY,
    );
    let built = nikaia(&root, "build");
    assert!(!built.status.success());
    assert!(
        said(&built).contains("`-` cannot stand there"),
        "{}",
        said(&built)
    );
}

/// **`examples/c-library` builds, in place and under `--locked`, and its C
/// program prints what its README says**: a handle fed text, a getter into
/// the caller's buffer, and a callback that stops the walk.
#[test]
fn the_example_builds_and_its_c_program_runs() {
    if Command::new("cc").arg("--version").output().is_err() {
        eprintln!("skipped: no C compiler");
        return;
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/c-library");
    let built = Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .current_dir(&root)
        .args(["build", "--locked"])
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("the nikaia binary runs");
    assert!(built.status.success(), "{}", said(&built));
    let made = root.join("target/nikaia/c-library");
    let program = root.join("target/use");
    let compiled = Command::new("cc")
        .arg("-I")
        .arg(&made)
        .arg(root.join("use.c"))
        .arg("-L")
        .arg(&made)
        .args(["-lwordtally", "-o"])
        .arg(&program)
        .output()
        .expect("cc runs");
    assert!(compiled.status.success(), "{}", said(&compiled));
    let ran = Command::new(&program)
        .env("LD_LIBRARY_PATH", &made)
        .output()
        .expect("the caller runs");
    assert!(ran.status.success(), "{}", said(&ran));
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout),
        "9 words, the longest \"quick\" (status 0)\n\
         the first two words of \"Hello NIKAIA from c\":\n  \
         Hello (mixed)\n  \
         NIKAIA (upper)\n\
         stopped after 2\n"
    );
}

/// **`nikaia bind python`** (ADR-284 D26, D27): a `ctypes` module over the
/// library, used as Python uses a module - every kind of entry point, a
/// failure as an exception of its variant, a handle as a class.
const PYTHON_CALLER: &str = r#"import calc

assert calc.add(2, 40) == 42
assert calc.halve(5.0) == 2.5
assert calc.is_vowel("e") is True
assert calc.greet("Ada") == "Hello, Ada"
assert calc.total([1, 2, 3, 4]) == 10
assert calc.size(b"\x01\x02\x03") == 3
assert calc.after(calc.Light.Green) == calc.Light.Amber
grown = calc.grown(calc.Rect(calc.Point(1.0, 2.0), 3.0, 4.0, calc.Light.Red), 0.5)
assert (grown.corner.x, grown.width, grown.light) == (0.5, 4.0, calc.Light.Green)
point = calc.Point(1.0, 2.0)
point.shift(0.5)
assert (point.x, point.sum()) == (1.5, 4.0)
quad = calc.Quad((1.0, 2.0, 3.0, 4.5), (calc.Light.Red, calc.Light.Green), (calc.Point(10.0, 0.0), calc.Point(20.0, 0.0)))
assert calc.perimeter(quad) == 40.5
assert calc.flipped(quad).lights[0] == calc.Light.Green
assert calc.widest([calc.Point(1.0, 0.0), calc.Point(7.0, 0.0)]) == 7.0
assert [(p.x, p.y) for p in calc.diagonal(3)] == [(0.0, 0.0), (1.0, 2.0), (2.0, 4.0)]
try:
    calc.checked(-1)
    raise AssertionError("no exception")
except calc.Negative as error:
    assert error.code == 1 and "a negative count" in str(error), str(error)
try:
    calc.checked(500)
    raise AssertionError("no exception")
except calc.Error as error:
    assert isinstance(error, calc.TooLarge) and "500 is too large" in str(error)
with calc.Counter("clicks") as counter:
    counter.bump(5)
    assert counter.bump(2) == 7
    assert (counter.count, counter.name, counter.light) == (7, "clicks", calc.Light.Red)
    assert counter.steps() == 2
    try:
        calc.looking_at(counter, lambda n: counter.bump(1))
        raise AssertionError("no exception")
    except calc.ReentrantError:
        pass
assert calc.found(0) is None
found = calc.found(4)
assert calc.count_of(found) == 4 and calc.count_of(None) == -1
assert calc.length_of(None) == 4 and calc.length_of("") == 0
assert calc.count_up(10, lambda i: i < 3) == 3
# A stream is a generator (ADR-284 D20, D21): read to its end, or stopped.
import itertools

assert list(calc.count_up_iter(4)) == [0, 1, 2, 3]
assert list(itertools.islice(calc.count_up_iter(1000000), 3)) == [0, 1, 2]
seen = []
calc.words("one two", lambda word, light: seen.append((word, light)))
assert seen == [("one", calc.Light.Amber), ("two", calc.Light.Amber)], seen
assert calc.napped(20) == 62

# The `_async` form (ADR-284 D19, D27): with `done`, or awaited.
import asyncio
import time

landed = []
ticket = calc.napped_async(10, done=landed.append)
while not landed:
    time.sleep(0.01)
assert landed == [52], landed


async def awaited():
    assert await calc.napped_async(10) == 52
    slow = calc.napped_async(60000)
    slow.cancel()
    try:
        await slow
        raise AssertionError("not cancelled")
    except asyncio.CancelledError:
        pass
    await asyncio.sleep(0.2)


asyncio.run(awaited())
print("ok")
"#;

#[test]
fn python_calls_the_library_through_its_binding() {
    let have = |tool: &str| Command::new(tool).arg("--version").output().is_ok();
    if !have("cc") || !have("python3") {
        eprintln!("skipped: no C compiler or no python3");
        return;
    }
    let root = package("c-library-python", "artifact = \"c-library\"\n", LIBRARY);
    let bound = nikaia_with(&root, &["bind", "python"]);
    assert!(bound.status.success(), "{}", said(&bound));
    let made = root.join("target/nikaia/c-library");
    assert!(made.join("calc/__init__.py").is_file(), "the binding");
    std::fs::write(root.join("use.py"), PYTHON_CALLER).expect("the caller");
    let ran = Command::new("python3")
        .arg(root.join("use.py"))
        .env("PYTHONPATH", &made)
        .output()
        .expect("python3 runs");
    assert!(ran.status.success(), "{}", said(&ran));
    assert_eq!(String::from_utf8_lossy(&ran.stdout), "ok\n");

    let refused = nikaia_with(&root, &["bind", "cobol"]);
    assert!(!refused.status.success());
    assert!(said(&refused).contains("`python` is"), "{}", said(&refused));
}

fn nikaia_with(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nikaia"))
        .current_dir(root)
        .args(args)
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("the nikaia binary runs")
}
