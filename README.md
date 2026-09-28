<div align="center">
  <img src="nikaia-logo.jpg" alt="Nikaia Logo" width="300" height="164" />
  <h1>N I K A I A</h1>
  <p><strong>Good wins.</strong></p>

  <p>
    <a href="#the-idea">The idea</a> •
    <a href="#hello-world">Hello, world</a> •
    <a href="guide/getting-started.md">Getting started</a> •
    <a href="guide/syntax.md">Syntax tour</a> •
    <a href="manifesto.md">Manifesto</a> •
    <a href="docs/project_status_and_roadmap.md">Status &amp; roadmap</a> •
    <a href="docs/specification">Specification</a> •
    <a href="https://nikaia-lang.org/">Documentation site</a> •
    <a href="https://gemini.google.com/gem/1T8viw7ZHA0TwDZDhr6h1mgRBVnw3aTNP?usp=sharing">Gemini explains Nikaia</a>
  </p>

  <img src="assets/badges/version.svg" alt="Version" />
  <img src="assets/badges/status.svg" alt="Status" />
  <img src="assets/badges/self-hosted.svg" alt="Self-hosted share of the compiler" />
  <img src="assets/badges/license.svg" alt="License" />
  <a href="https://nikaia-lang.org/"><img src="assets/badges/docs.svg" alt="Documentation site" /></a>
  <a href="https://github.com/Nikaia-Language/Nikaia"><img src="assets/badges/github.svg" alt="GitHub repository" /></a>
</div>

---

## The idea

Most programming languages ask you to pick a side.

**Easy languages** like Python or JavaScript let you write what you mean, quickly. You pay
for that later: the program is slower than it could be, it needs more memory and more
machines, and some mistakes only show up in production, under load, at night.

**Fast languages** like C++ or Rust give you the full power of the machine. You pay for that
up front: a large part of every program is not about your problem at all, but about
explaining to the compiler how memory, waiting and sharing should work.

Nikaia starts from a simple observation: **most of that explaining is work a compiler can do
by itself.** When a compiler can see the whole program, it can work out on its own which data
needs protecting, where a function waits for the network, when memory can be freed, and how
to spread work across the cores of a machine. So in Nikaia you write down *what* should
happen, in straight, readable code, and the compiler decides *how*.

That shows up in a few concrete ways:

* **Waiting is not your problem.** A program that reads files or talks to the network is
  written like any other program. There are no special keywords for "this might wait", and
  the program still never sits idle while it waits.
* **Mistakes are caught before the program runs.** Two parts of a program changing the same
  data at the same time, a value that might be missing, an error nobody handles: the compiler
  refuses these, and says in plain words what to write instead.
* **No garbage collector, and no manual memory management.** Memory and files are released at
  a point you can see in the code, without pauses and without writing `free`.
* **One program, small or large machine.** Whether your code uses one core or all of them is
  one line in a configuration file, not a rewrite.
* **Formats are part of the language.** Parsing logs, protocols, or your own little language
  is done by describing the format as a grammar, not by splitting strings by hand.

Nothing here is magic. Nikaia translates your program into Rust, a language whose compiler
already guarantees memory safety, and then lets that compiler do the rest. Nikaia's job is to
make those guarantees available without making you operate them by hand.

The name comes from my daughter, **Nika**. In Persian, *nik* means *good*, and the motto is
the name with a verb: **good wins**. Not over anyone: it means that being kind to the person
writing the code and producing fast programs are not opposites. The whole story, and the
technical case behind every claim above, is in the [**Manifesto**](manifesto.md).

---

## Hello, world

```nika
// src/main.nika
fn main() {
    println("Hello, Nikaia!")
}
```

```sh
nikaia run
```

A slightly bigger taste: read two files **at the same time** and add up their sizes. There is
no `async`, no `await`, no callback and no thread in sight.

```nika
use std::fs

fn size_of(path: ref String) -> i64 throws {
    let text = fs::read_to_string(path, fs::Root::Anywhere)
    return text.len()
}

fn main() throws {
    let (a, b) = overlap {
        size_of("a.txt")
        size_of("b.txt")
    }
    println(f"{a + b} characters in both files")
}
```

**Next:** [Getting started](guide/getting-started.md) installs the compiler and runs your
first project. [A tour of the syntax](guide/syntax.md) shows the whole language on one page,
for readers who come from Python, JavaScript, Go, Java or similar.

---

## Good at, and not good at

**Nikaia is aimed at:**

* **Services that mostly wait**: web APIs, proxies, command-line tools that talk to the
  network. Easy to write like Go or Node.js, without garbage-collection pauses.
* **Data in odd formats**: logs, telemetry, protocol frames, anything you would otherwise
  parse with regular expressions and hope.
* **Heavy computation**: simulations, image and signal processing, aggregating huge inputs,
  with the parallel parts checked by the compiler instead of hoped for.
* **Code written by AI**: when a model writes most of the code, a compiler that refuses races,
  deadlocks and unhandled errors is the reviewer you want
  ([why](manifesto.md), §8).

**Look elsewhere if you need:**

* **An interactive REPL.** This is a design choice, not a gap: Nikaia works out its guarantees
  by looking at the whole program at once, and a line typed into a prompt has no whole program
  around it.

For now, also look elsewhere if you need the things only time brings:

* **Something to ship next quarter.** Nikaia is pre-alpha (see below).
* **A large ecosystem** of libraries.
* **Years of answered questions online.**

---

## Where the project stands

**Pre-alpha.** The compiler builds real programs: the [`examples/`](examples/) directory holds
a calculator, a JSON parser, a log analyser, a small HTTP server, the One Billion Row
Challenge and more, and the test suite compiles and runs all of them except the one that needs
a database. You will also hit
missing pieces quickly: there is no package registry, no test runner, no formatter or editor
integration yet, and no database access.

The [**status & roadmap**](docs/project_status_and_roadmap.md#where-the-project-actually-stands)
lists what works today, the walls you will hit, and which parts of the design matter most to
test. Bug reports and "this cannot work because…" arguments are very welcome, especially the
latter: an unfinished language is the cheapest place to be told you are wrong.

---

## Where to read next

| If you want to… | Read |
| :--- | :--- |
| install the compiler and run a program | [Getting started](guide/getting-started.md) |
| see what the language looks like | [A tour of the syntax](guide/syntax.md) |
| know why the project exists, and how it compares | [Manifesto](manifesto.md) |
| know what works today and what comes next | [Status & roadmap](docs/project_status_and_roadmap.md) |
| see real programs | [examples/](examples/) |
| learn the full rules | [Specification, Part I](docs/specification/10-nikaia-light.md) |
| see the concurrency and parallelism model | [Specification, Part II](docs/specification/20-nikaia-advance.md) |
| understand a design decision | [the ADR index](docs/specification/adr/README.md) |
| let a model write Nikaia for you | [the spec](docs/specification) + [examples/](examples/) in one context |
| work on the compiler | [toolchain architecture](docs/toolchain_architecture.md) |

---

## License

Nikaia is licensed under the Apache License, Version 2.0. See the [LICENSE](LICENSE) file.
On prior art and governance, see the [Manifesto](manifesto.md), §10.

<div align="center">
  <sub>For Nika. For everyone downstream.</sub>
</div>
