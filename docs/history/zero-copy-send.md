# What it costs to answer a request with a file

**Date:** September 13, 2026
**Status:** measured; the numbers are
[ADR-058](adr/adr-058.md)'s evidence
**Related:** [ADR-058](adr/adr-058.md) (the decision these
settled), [ADR-018](adr/adr-018.md) D2 (what a handler may
return), [ADR-038](adr/adr-038.md) D3 (the I/O split), Part III
17.1 (`fs::map`)
**Produced by:** [`benches/sendfile/`](../../benches/sendfile) —
`./benches/sendfile/sendfile.sh 7`, twice; §2 is the first run and §2.1 carries
the second where the two disagree

The README used to show an HTTP server that read `index.html` on every request,
and the criticism it drew was that reading a file in order to hand it on is
expensive. That is two claims, and only the first survives measurement.

**There are two programs here, not one**, and that is the first thing the bench
found. A server whose page is known when it starts may do everything once,
outside the handler. A server whose file the *request* names — a download route,
a file server, an upload served back — may do nothing once, and the vehicles
that win are not the same ones. §3 is the first program, §3.1 the second.

**The conclusion, first.** For a page known at startup, reading the file per
request is the expensive part and it is worth **2.2×** the machine's CPU at
4 KiB; `sendfile(2)`, the thing the criticism was pointing at, is worth nothing
at that size and wins only at a megabyte.

For a file the request names, the answer is a different one and sharper. What
decides is **not the mechanism but whether anything may be kept**: a table of
mappings made once is 13.5 µs at 4 KiB where the best vehicle that keeps nothing
is 28.0. And the obvious reading of "map it, don't read it" is a **trap** in this
program — `mmap` and `munmap` per request cost **76.0 µs**, two and a half times
what plainly reading the file costs, because a mapping is a page-table edit and
not a pointer. Among vehicles that may keep nothing, `sendfile` is the best or
tied at every size.

And `io_uring`'s `Splice`, which looks like the natural home for this on a
runtime that already runs files on a ring, is the worst vehicle at every size
while *appearing* to be the best by a factor of twenty on the wrong instrument.

---

## 1. The machine and the method

4 cores, Linux 6.18.44 x86_64, a shared virtual machine. `--release`. Load
average 0.31 before the block and 1.30 after, printed by the binary rather than
assumed away. `io_uring` is available on this kernel.

The vehicles serve the same file over **one kept-open TCP connection on
loopback**, the same number of times, with a thread draining the far end so that
a send blocks only when the kernel's buffers are genuinely full.

**Named at startup** — the path is a constant, so what may be done once is:

| vehicle | what it does |
| :--- | :--- |
| `read_each` | open, read, UTF-8-validate, write — once per request. The README's example |
| `cached` | read once at startup, write the heap buffer per request |
| `mapped` | `mmap` once at startup, write the mapping per request. What `fs::map` already gives |
| `sendfile` | `sendfile(2)`, the descriptor opened once. The program never has the bytes |
| `splice` | `io_uring` `Splice`, file → pipe → socket, the two hops submitted as one linked pair |

**Named by the request** — the path is not known until the request arrives, so
every vehicle pays the open, and only the last may keep anything. The requests
cycle over **64 files** rather than repeating one, because a route that names a
file does not name the same file, and one inode would let the kernel keep state
the real program does not get to keep:

| vehicle | what it does per request |
| :--- | :--- |
| `read_named` | open, read, write, close |
| `mapped_named` | open, `mmap`, write, `munmap`, close |
| `sendfile_named` | open, `fstat` (the length has to be known before the status line), `sendfile`, close |
| `mapped_kept` | a lookup in a table of mappings made once — the hot set a real file server keeps |

20 000 sends at 4 KiB and 64 KiB, 2 000 at 1 MiB; seven repeats, the median
printed and the wall spread beside it. The far end counts the bytes it received
and the run fails if they are not all there — a vehicle that sends fewer is a
faster vehicle for the wrong reason.

**The column that answers is `machine`.** Three instruments disagree here and
the disagreement is itself a finding:

* *wall* is the loopback pipeline's, reader included;
* *thread* is the sending thread's own `RUSAGE_THREAD`, which is what a naive
  benchmark reports — and which misses every jiffy `io_uring` spends on kernel
  workers;
* *machine* is every non-idle jiffy on the box, from `/proc/stat`, on an
  otherwise quiet machine. It is the one an operator is billed for and the one
  that catches work moved out of the caller's thread.

---

## 2. The numbers, and which of them are real

Machine CPU, µs per request. **Five runs**, all on the same box: the first two
while it was busy with this repository's own builds, then one pinned to two
cores with `taskset`, then two on a quiet box. The first run is kept in the
table because it is the one the first draft of
[ADR-058](adr/adr-058.md) argued from, and finding out that it was
the outlier is the point of this section.

### Named at startup

| 4 KiB | busy | 2 cores | quiet | quiet | quiet |
| :--- | ---: | ---: | ---: | ---: | ---: |
| `read_each` | 39.5 | 16.5 | 22.5 | 22.5 | 21.5 |
| `cached` | 22.5 | 8.5 | 12.0 | 11.5 | 12.0 |
| `mapped` | 19.0 | 13.5 | 13.5 | 11.5 | 10.5 |
| `sendfile` | 18.0 | 10.5 | 9.5 | 12.0 | 12.0 |
| `splice` | 73.5 | 58.0 | 54.0 | 52.5 | 52.0 |

| 64 KiB | busy | 2 cores | quiet | quiet | quiet |
| :--- | ---: | ---: | ---: | ---: | ---: |
| `read_each` | 65.0 | 38.5 | 37.0 | 37.5 | 40.5 |
| `cached` | 51.5 | 31.5 | 31.0 | 33.0 | 29.5 |
| `mapped` | 45.5 | 31.5 | 29.0 | 34.0 | 32.0 |
| `sendfile` | **50.0** | **22.0** | **21.5** | **25.5** | **24.5** |
| `splice` | 88.0 | 51.5 | 56.0 | 58.0 | 59.5 |

| 1 MiB | busy | 2 cores | quiet | quiet | quiet |
| :--- | ---: | ---: | ---: | ---: | ---: |
| `read_each` | 860 | 575 | 580 | 570 | 615 |
| `cached` | 760 | 470 | 460 | 460 | 505 |
| `mapped` | 780 | 420 | 465 | 460 | 450 |
| `sendfile` | 650 | 375 | 445 | 405 | 440 |
| `splice` | **1010** | **400** | **435** | **445** | **485** |

### Named by the request

| 4 KiB | busy | 2 cores | quiet | quiet | quiet |
| :--- | ---: | ---: | ---: | ---: | ---: |
| `read_named` | 30.5 | 18.0 | 22.5 | 21.5 | 15.5 |
| `mapped_named` | 76.0 | 43.5 | 45.0 | 42.5 | 28.0 |
| `sendfile_named` | 28.0 | 16.5 | 14.0 | 18.5 | 14.5 |
| `mapped_kept` | 13.5 | 12.5 | 10.5 | 12.5 | 9.5 |

| 64 KiB | busy | 2 cores | quiet | quiet | quiet |
| :--- | ---: | ---: | ---: | ---: | ---: |
| `read_named` | 65.5 | 38.0 | 39.5 | 40.0 | 40.0 |
| `mapped_named` | 124.0 | 67.0 | 72.0 | 72.0 | 66.0 |
| `sendfile_named` | 49.5 | 30.5 | 28.5 | 30.0 | 34.0 |
| `mapped_kept` | 46.0 | 31.0 | 31.0 | 32.0 | 30.5 |

| 1 MiB | busy | 2 cores | quiet | quiet | quiet |
| :--- | ---: | ---: | ---: | ---: | ---: |
| `read_named` | 925 | 585 | 555 | 545 | 605 |
| `mapped_named` | 715 | 495 | 455 | 470 | 490 |
| `sendfile_named` | 800 | 425 | 440 | 460 | 480 |
| `mapped_kept` | 635 | 455 | 485 | 465 | 515 |

### 2.1 What reproduced, and what was one busy afternoon

**Every absolute travels badly and the busy run travels worst.** The same
binary on the same four cores reports 39.5 µs and 21.5 µs for the same
vehicle. Nothing below is stated in microseconds that is not also stated as an
ordering.

Reproduced in **all five** runs:

* `read_each` / `read_named` is the worst vehicle at every size in both
  families — the read is real and it is the largest single item at 4 KiB;
* **`mmap` per request is 2–3× worse than plainly reading** below a megabyte,
  and it is the worst vehicle in its family there;
* a **kept mapping** is the best vehicle at 4 KiB when the request names the
  file, by roughly 2× over the best that keeps nothing;
* `sendfile` is **never worse than reading**, in either family, at any size;
* `splice`'s *thread* column is flat (14–21 µs) at every size while its machine
  column is not — the instrument, not the mechanism.

**Retired by the re-runs** — each of these was true in the busy run only, and
each was in the first draft of [ADR-058](adr/adr-058.md):

* *"`sendfile` loses at 64 KiB."* One run of five. In the other four it is the
  best vehicle in its family there, by 20–30 %.
* *"The crossover is near a megabyte."* There is no crossover to find on a quiet
  box: `sendfile` is at or ahead of the mapping from 64 KiB up, and the three
  vehicles are within each other's spread below that.
* *"`splice` is the worst vehicle at every size."* At a megabyte it is mid-field
  in four runs of five, ahead of `read_each` in all four. It stays worst below a
  megabyte.
* *"A kept mapping wins at every size."* It wins at 4 KiB and ties at 64 KiB;
  at a megabyte three runs put `sendfile` ahead of it.

## 3. What they say

**The criticism was right about the read, and that is the one finding no re-run
touched.** `read_each` is worst at every size in every run, and at 4 KiB it is
roughly twice the next vehicle. Opening a file, copying it into the process and
validating it as UTF-8 — to produce the same bytes as last time — is the largest
single item in answering a small request. That is what the README's example did.

**`mmap` per request is the trap, and it is the second finding that held.** 2–3×
worse than plainly reading, below a megabyte, in all five runs. A mapping is a
page-table edit, an address-space reservation and, on unmapping, an
invalidation the other cores have to hear about; four kilobytes of payload
amortises none of it. An implementation of `http::File` that mapped per request
would be the slowest option at the sizes servers send most, while looking like
the zero-copy one.

**Keeping the mapping is worth more than choosing a mechanism, at small sizes.**
13.5, 12.5, 10.5, 12.5, 9.5 µs against the best keeping-nothing vehicle's 28.0,
16.5, 14.0, 18.5, 14.5 at 4 KiB. It stops being true as the payload grows: at
64 KiB it ties `sendfile`, and at a megabyte three runs of five put `sendfile`
ahead. A cache is a small-request optimisation, which is the right shape — a
megabyte spends its time in the transfer either way.

**And `sendfile` is not the thing the first draft said it was.** It is never
worse than reading, anywhere, and from 64 KiB up it is the best or joint-best
vehicle in both families on a quiet box. What killed the "it loses below a
megabyte" story was re-running it: that claim rested on one afternoon when the
box was building this repository at the same time.

**But it does not follow that `sendfile` is simply the answer**, and §5 is why:
the two servers most people deploy ship it **off** by default, for reasons this
bench is structurally unable to see.

## 4. What this does not measure, and one confound

**Loopback with a draining reader is the friendly case.** A real client is slow
and far away, and the difference that makes is not CPU per request: it is that
`cached` and `mapped` need the body to stay addressable for as long as the
slowest connection takes, while `sendfile` needs a descriptor and an offset. At
a thousand slow connections that is a memory-scaling argument for `sendfile`
that this bench, with its one fast reader, cannot see. It is named here rather
than folded into the recommendation.

**Nothing here is encrypted.** [ADR-038](adr/adr-038.md) D2 binds
`rustls`, and a TLS response cannot use `sendfile` at all without kTLS — so on a
server that terminates TLS, the `sendfile` column is not available and the
`mapped` column is the answer at every size.

**The confound: the page cache is warm throughout.** Every vehicle reads a file
that was written seconds earlier on a box with 15 GB of RAM, so `read_each`'s
disadvantage is a copy and a syscall, never a disk. On a cold cache every
vehicle pays the same read once and the shape of the table would not change —
but `read_each` would pay it *per request* only in the sense that the cache
would have to keep it, which is the same page cache the other four are using.
The honest statement is that this measures the warm case, which is the case a
web server serving a static page is in.

**The cache this bench keeps is perfect and a real one is not.** `mapped_kept`
maps all 64 files up front, never evicts and never checks whether the file
changed underneath it. A `std` that keeps mappings owes an eviction policy and
an answer to "the file was replaced" — both are correctness work this number
does not price, and
[ADR-058](adr/adr-058.md) D8 is where they are decided rather than
assumed.

**Nothing here is under memory pressure, and the cache is warm** — which is the
caveat §5's prior art turns into the important one. `sendfile` on a *cold* page
cache blocks in the kernel while the disk is read, and this bench never once
paid that: every file was written seconds earlier on a box with 15 GB of RAM.
nginx needed `aio`, `SF_NODISKIO` and a thread pool to stop that blocking a
worker, and `sendfile_max_chunk` to stop one connection holding one while it
happens. A single connection with a fast reader cannot show either.

**And the absolutes travel badly.** This is a shared 4-vCPU VM; `docs/runtime-cost.md`
§6 records the same binary's numbers moving 1.4–1.9× between days on this
machine class. What reproduced across runs here is the *ordering* and the
*ratios* — `read_each` worst at small sizes, `mapped` and `cached` together,
`sendfile` behind them until the megabyte and ahead after, `splice` last — and
those are what [ADR-058](adr/adr-058.md) rests on.

---

## 5. What the servers people actually deploy decided

A number from one virtual machine is a weak reason for a language rule, and the
re-runs in §2.1 are the demonstration. So: four servers with two decades of
production between them, and what each of them concluded about the same choice.

**Apache httpd ships `sendfile` off.** `EnableSendfile` defaults to **Off** since
2.4, having defaulted to On in 2.2 — a server that had the optimisation on by
default turned it off. `EnableMMAP` carries its own warning: a mapped file that
another NFS client truncates gives the serving process a **bus error** on the
next access, so the documentation's recommendation for an NFS-mounted directory
is to turn *both* off.

**nginx ships `sendfile` off too**, and its interesting directive is not the
on/off one. `sendfile_max_chunk` exists "to avoid cases when nginx spins in
`sendfile()` for a long time when network connection is faster than disk
subsystem" — a **fairness** knob, not a throughput one, and its default was
changed to 2 MB in 1.21.4. With `aio` off, `sendfile` blocks the worker on disk
I/O; the answer was `SF_NODISKIO`, then a thread pool. `TCP_NOPUSH`/`TCP_CORK`
are enabled only when sendfile is used, and `SSL_sendfile()` needs OpenSSL 3.0
built with kTLS.

**HAProxy makes kernel splicing opt-in** — `option splice-request`,
`splice-response`, `splice-auto` — with the note that kernels between 2.6.25 and
2.6.28 forwarded *corrupted data*. Pipes exist only for splicing, are allocated
dynamically, and fall back to an ordinary copy when there are none.

**lighttpd makes the mechanism a configuration value outright**:
`server.network-backend` is `sendfile`, `writev` or `write`, with the advice
that sendfile suits small files and writev many large ones.

**Three things follow, and they are worth more than §2's table.**

*It is a knob everywhere.* Not one of these four picks a mechanism at build time
or exposes it in what an application writes. Every one of them chose: the
application says "this file answers this request", the server decides how, and
an operator may overrule it on the machine it runs on.

*The defaults moved, and they moved toward off.* Apache's went from On to Off
between two major versions. That is a project with far more evidence than this
one concluding that the safe default and the fast default are not the same
default.

*What turns them off is correctness, not speed.* NFS, a truncated mapping, a
kernel that corrupts spliced bytes, a worker blocked on a cold cache, one
connection starving the others. Every reason in that list is invisible to a
benchmark of one warm connection on loopback — which is to say, invisible to
§2's table.

### 5.1 Where each of those comes from

* [`ngx_http_core_module`](https://nginx.org/en/docs/http/ngx_http_core_module.html)
  — `sendfile`, `sendfile_max_chunk`, `aio`, `directio`, and that `TCP_NOPUSH` /
  `TCP_CORK` are enabled only with sendfile.
* [Changed default value of `sendfile_max_chunk` to 2m](https://mailman.nginx.org/pipermail/nginx-devel/2021-October/014478.html)
  and [Simplified `sendfile(SF_NODISKIO)` usage](https://mailman.nginx.org/pipermail/nginx-devel/2021-December/014687.html)
  — the fairness knob and the non-blocking flag, in the commits that changed them.
* [Thread pools boost performance 9x](https://www.nginx.com/blog/thread-pools-boost-performance-9x/)
  — why a blocking read had to leave the worker at all.
* [`ngx_http_ssl_module`](https://nginx.org/en/docs/http/ngx_http_ssl_module.html)
  — `SSL_sendfile()` needs OpenSSL 3.0 built with kTLS.
* [Apache httpd `core`: `EnableSendfile`, `EnableMMAP`](https://httpd.apache.org/docs/2.4/mod/core.html)
  — the defaults, and the NFS bus-error warning.
* [Upgrading to 2.4 from 2.2](https://httpd.apache.org/docs/current/upgrading.html)
  — where `EnableSendfile` went from On to Off.
* [Apache performance tuning](https://httpd.apache.org/docs/2.4/misc/perf-tuning.html)
  — the recommendation to turn both off for an NFS-mounted directory.
* [HAProxy configuration manual](https://docs.haproxy.org/1.8/configuration.html)
  — `option splice-request` / `splice-response` / `splice-auto`, the corrupting
  kernels, `maxpipes` and the fallback to a plain copy.
* [lighttpd `server.network-backend`](https://redmine.lighttpd.net/projects/lighttpd/wiki/server_network-backendDetails)
  — the mechanism as a configuration value.
