//! What it costs to answer a request with a file, five ways.
//!
//! The question is [ADR-289](../../../../docs/specification/adr/adr-289.md)'s:
//! can a file reach a socket without the program reading it, and is that worth
//! a decision. There are **two families of answer**, because there are two
//! programs: one whose page is known when the server starts, and one whose file
//! the *request* names - a download route, a file server, an upload served
//! back. What may be done once outside the handler is the whole difference, and
//! the vehicles that win are not the same.
//!
//! **Named at startup.** Five vehicles serve the same file over one kept-open
//! connection the same number of times, with a thread draining the far end:
//!
//! | vehicle | what it does |
//! | :--- | :--- |
//! | `read_each` | open, read, validate, write - once per request |
//! | `cached` | read once at startup, write the buffer per request |
//! | `mapped` | `mmap` once at startup, write the mapping per request |
//! | `sendfile` | `sendfile(2)`: the program never has the bytes |
//! | `splice` | `io_uring` `Splice`, file to pipe to socket, the two linked |
//!
//! **Named by the request.** The same transfer where the path is not known
//! until the request arrives, so every vehicle pays the open. The files are
//! cycled rather than repeated, because a server whose route names a file does
//! not serve one file:
//!
//! | vehicle | what it does per request |
//! | :--- | :--- |
//! | `read_named` | open, read, write, close |
//! | `mapped_named` | open, `mmap`, write, `munmap`, close |
//! | `sendfile_named` | open, `fstat`, `sendfile`, close |
//! | `mapped_kept` | a lookup in a table of mappings made once - the hot set a real file server keeps |
//!
//! **The column that answers is `machine`, not `thread`.** The sending thread
//! is not the whole cost: the reader drains the far end, and `io_uring`
//! performs a splice on kernel workers no per-thread accounting sees - which
//! is why `splice`'s thread column is flat at ~20 µs while the machine pays
//! three times what `sendfile` costs. Machine-wide busy time from `/proc/stat`
//! is what a server operator is billed for, and the box is otherwise quiet.

use std::fs::File;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::time::Instant;

#[cfg(target_os = "linux")]
use io_uring::{opcode, types, IoUring};

/// Named at startup: what may be done once, is.
const AT_STARTUP: &[&str] = &["read_each", "cached", "mapped", "sendfile", "splice"];

/// Named by the request: only the vehicle in the last row may keep anything.
const BY_REQUEST: &[&str] = &[
    "read_named",
    "mapped_named",
    "sendfile_named",
    "mapped_kept",
];

/// How many files a request may name. One would let the kernel keep everything
/// hot for a single inode and measure the wrong program.
const NAMED_FILES: usize = 64;

/// Every jiffy the machine spent not idle, from `/proc/stat`.
fn machine_busy_seconds() -> f64 {
    let stat = std::fs::read_to_string("/proc/stat").expect("/proc/stat");
    let line = stat.lines().next().expect("the cpu line");
    let ticks: Vec<f64> = line
        .split_whitespace()
        .skip(1)
        .map(|field| field.parse().expect("a number"))
        .collect();
    // user nice system idle iowait irq softirq steal …
    let idle = ticks[3] + ticks[4];
    (ticks.iter().sum::<f64>() - idle) / 100.0
}

/// The sending thread's own CPU. Kept beside the machine column because the
/// gap between the two is the finding for `splice`.
fn thread_seconds() -> f64 {
    unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_THREAD, &mut usage);
        let seconds = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
        seconds(usage.ru_utime) + seconds(usage.ru_stime)
    }
}

/// A connection whose far end is drained, so a send blocks only when the
/// kernel's buffers are genuinely full rather than because nobody is reading.
fn connected(expect: u64) -> (TcpStream, std::thread::JoinHandle<u64>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("the bound address");
    let reader = std::thread::spawn(move || {
        let (mut far, _) = listener.accept().expect("accept");
        let mut sink = vec![0u8; 256 * 1024];
        let mut seen = 0u64;
        while seen < expect {
            match far.read(&mut sink) {
                Ok(0) => break,
                Ok(n) => seen += n as u64,
                Err(e) => panic!("the far end: {e}"),
            }
        }
        seen
    });
    let near = TcpStream::connect(address).expect("connect");
    near.set_nodelay(true).expect("nodelay");
    (near, reader)
}

/// What the README's example did until this bench was written.
fn read_each(path: &Path, socket: &mut TcpStream, times: usize) {
    for _ in 0..times {
        let page = std::fs::read_to_string(path).expect("read");
        socket.write_all(page.as_bytes()).expect("write");
    }
}

fn cached(path: &Path, socket: &mut TcpStream, times: usize) {
    let page = std::fs::read(path).expect("read");
    for _ in 0..times {
        socket.write_all(&page).expect("write");
    }
}

/// What the language can already express: `fs::map` once, write per request.
/// Still one copy into the socket - but no heap page, no read syscall, and no
/// second residency of a file the page cache already holds.
fn mapped(path: &Path, socket: &mut TcpStream, times: usize, size: usize) {
    let file = File::open(path).expect("open");
    let pages = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            size,
            libc::PROT_READ,
            libc::MAP_PRIVATE,
            file.as_raw_fd(),
            0,
        )
    };
    assert!(pages != libc::MAP_FAILED, "mmap");
    let page = unsafe { std::slice::from_raw_parts(pages as *const u8, size) };
    for _ in 0..times {
        socket.write_all(page).expect("write");
    }
    unsafe { libc::munmap(pages, size) };
}

/// The question itself: the descriptor is opened once and the bytes never
/// enter the process.
fn sendfile(path: &Path, socket: &mut TcpStream, times: usize, size: usize) {
    let file = File::open(path).expect("open");
    for _ in 0..times {
        let mut offset: libc::off_t = 0;
        while (offset as usize) < size {
            let sent = unsafe {
                libc::sendfile(
                    socket.as_raw_fd(),
                    file.as_raw_fd(),
                    &mut offset,
                    size - offset as usize,
                )
            };
            assert!(sent >= 0, "sendfile: {}", std::io::Error::last_os_error());
        }
    }
}

/// The same transfer on the ring ADR-038 D3 already runs files on.
#[cfg(target_os = "linux")]
fn splice(path: &Path, socket: &mut TcpStream, times: usize, size: usize) {
    let file = File::open(path).expect("open");
    let mut ring = IoUring::new(64).expect("a ring");
    let mut pipe = [0i32; 2];
    assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0, "pipe");
    let (out_of_file, into_socket) = (pipe[1], pipe[0]);

    // A pipe holds 64 KiB by default, which would chop a megabyte into sixteen
    // round trips and measure the pipe rather than the mechanism.
    let want = size.clamp(64 * 1024, 1024 * 1024) as i32;
    unsafe { libc::fcntl(out_of_file, libc::F_SETPIPE_SZ, want) };

    for _ in 0..times {
        let mut offset: i64 = 0;
        while (offset as usize) < size {
            let left = (size - offset as usize) as u32;

            // The two hops are submitted as one linked pair, so a chunk costs
            // one `io_uring_enter` rather than two. Even so it loses: a pipe
            // between the file and the socket is a second transfer, and no
            // batching removes it.
            let to_pipe = opcode::Splice::new(
                types::Fd(file.as_raw_fd()),
                offset,
                types::Fd(out_of_file),
                -1,
                left,
            )
            .build()
            .flags(io_uring::squeue::Flags::IO_LINK)
            .user_data(1);
            let to_socket = opcode::Splice::new(
                types::Fd(into_socket),
                -1,
                types::Fd(socket.as_raw_fd()),
                -1,
                left,
            )
            .build()
            .user_data(2);
            unsafe {
                ring.submission().push(&to_pipe).expect("push");
                ring.submission().push(&to_socket).expect("push");
            }
            ring.submit_and_wait(2).expect("submit");

            let mut sent = 0i32;
            for _ in 0..2 {
                let completion = ring.completion().next().expect("a completion");
                let result = completion.result();
                assert!(result > 0, "splice {}: {result}", completion.user_data());
                if completion.user_data() == 2 {
                    sent = result;
                }
            }
            offset += sent as i64;
        }
    }
    unsafe {
        libc::close(pipe[0]);
        libc::close(pipe[1]);
    }
}

#[cfg(not(target_os = "linux"))]
fn splice(_: &Path, _: &mut TcpStream, _: usize, _: usize) {
    eprintln!("splice: `io_uring` is Linux's, and this is not Linux");
}

/// Named by the request: open, read, write, close - every request.
fn read_named(paths: &[std::path::PathBuf], socket: &mut TcpStream, times: usize) {
    for turn in 0..times {
        let page = std::fs::read(&paths[turn % paths.len()]).expect("read");
        socket.write_all(&page).expect("write");
    }
}

/// Named by the request: the mapping cannot be made once, so it is made and
/// unmade per request - and that is a page-table edit, not a pointer.
fn mapped_named(paths: &[std::path::PathBuf], socket: &mut TcpStream, times: usize, size: usize) {
    for turn in 0..times {
        let file = File::open(&paths[turn % paths.len()]).expect("open");
        let pages = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size,
                libc::PROT_READ,
                libc::MAP_PRIVATE,
                file.as_raw_fd(),
                0,
            )
        };
        assert!(pages != libc::MAP_FAILED, "mmap");
        let page = unsafe { std::slice::from_raw_parts(pages as *const u8, size) };
        socket.write_all(page).expect("write");
        unsafe { libc::munmap(pages, size) };
    }
}

/// Named by the request, and the bytes still never enter the process. The
/// `fstat` is not decoration: `Content-Length` has to be known before the
/// status line ([ADR-289](../../../../docs/specification/adr/adr-289.md) D27).
fn sendfile_named(paths: &[std::path::PathBuf], socket: &mut TcpStream, times: usize) {
    for turn in 0..times {
        let file = File::open(&paths[turn % paths.len()]).expect("open");
        let length = file.metadata().expect("fstat").len() as usize;
        let mut offset: libc::off_t = 0;
        while (offset as usize) < length {
            let sent = unsafe {
                libc::sendfile(
                    socket.as_raw_fd(),
                    file.as_raw_fd(),
                    &mut offset,
                    length - offset as usize,
                )
            };
            assert!(sent >= 0, "sendfile: {}", std::io::Error::last_os_error());
        }
    }
}

/// The hot set: a file server that keeps what it has already mapped. The
/// request still names the file, but the mapping is only made once.
fn mapped_kept(paths: &[std::path::PathBuf], socket: &mut TcpStream, times: usize, size: usize) {
    let kept: Vec<*const u8> = paths
        .iter()
        .map(|path| {
            let file = File::open(path).expect("open");
            let pages = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    size,
                    libc::PROT_READ,
                    libc::MAP_PRIVATE,
                    file.as_raw_fd(),
                    0,
                )
            };
            assert!(pages != libc::MAP_FAILED, "mmap");
            pages as *const u8
        })
        .collect();

    for turn in 0..times {
        let page = unsafe { std::slice::from_raw_parts(kept[turn % kept.len()], size) };
        socket.write_all(page).expect("write");
    }

    for pages in kept {
        unsafe { libc::munmap(pages as *mut libc::c_void, size) };
    }
}

fn once(
    name: &str,
    path: &Path,
    named: &[std::path::PathBuf],
    size: usize,
    times: usize,
) -> (f64, f64, f64) {
    let total = (size * times) as u64;
    let (mut socket, reader) = connected(total);

    let thread_before = thread_seconds();
    let busy_before = machine_busy_seconds();
    let clock = Instant::now();
    match name {
        "read_each" => read_each(path, &mut socket, times),
        "cached" => cached(path, &mut socket, times),
        "mapped" => mapped(path, &mut socket, times, size),
        "sendfile" => sendfile(path, &mut socket, times, size),
        "splice" => splice(path, &mut socket, times, size),
        "read_named" => read_named(named, &mut socket, times),
        "mapped_named" => mapped_named(named, &mut socket, times, size),
        "sendfile_named" => sendfile_named(named, &mut socket, times),
        "mapped_kept" => mapped_kept(named, &mut socket, times, size),
        other => panic!("no vehicle named {other}"),
    }
    let wall = clock.elapsed();
    let thread = thread_seconds() - thread_before;
    let busy = machine_busy_seconds() - busy_before;

    drop(socket);
    // The bytes are checked rather than assumed: a vehicle that sends fewer of
    // them is a faster vehicle for the wrong reason.
    let seen = reader.join().expect("the reader");
    assert_eq!(seen, total, "{name}: the far end saw {seen} of {total}");

    let per = |seconds: f64| seconds * 1e6 / times as f64;
    (per(wall.as_secs_f64()), per(busy), per(thread))
}

#[allow(clippy::too_many_arguments)]
fn run(
    name: &str,
    path: &Path,
    named: &[std::path::PathBuf],
    size: usize,
    times: usize,
    repeats: usize,
) {
    let (mut walls, mut machine, mut thread) = (Vec::new(), Vec::new(), Vec::new());
    for _ in 0..repeats {
        let (wall, busy, own) = once(name, path, named, size, times);
        walls.push(wall);
        machine.push(busy);
        thread.push(own);
    }
    for column in [&mut walls, &mut machine, &mut thread] {
        column.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    }
    let middle = repeats / 2;
    println!(
        "{name:>15} {:>9.1} {:>11.1} {:>10.1}      {:.1}-{:.1}",
        walls[middle],
        machine[middle],
        thread[middle],
        walls[0],
        walls[repeats - 1],
    );
}

fn main() {
    let repeats: usize = std::env::args()
        .nth(1)
        .map(|a| a.parse().expect("a repeat count"))
        .unwrap_or(5);

    let directory = std::env::temp_dir().join("sendfile-bench");
    std::fs::create_dir_all(&directory).expect("a directory to work in");

    println!("machine: {} cores, {}", num_cores(), uname());
    println!("loadavg before: {}", loadavg());
    println!("repeats: {repeats} per size per vehicle; the median is printed\n");

    for &size in &[4 * 1024usize, 64 * 1024, 1024 * 1024] {
        let path = directory.join(format!("page-{size}.html"));
        let body: Vec<u8> = (0..size).map(|i| b'a' + (i % 26) as u8).collect();
        std::fs::write(&path, &body).expect("the page");

        // A request that names a file does not name the same one every time,
        // and one inode would let the kernel keep state the real program does
        // not get to keep.
        let named: Vec<std::path::PathBuf> = (0..NAMED_FILES)
            .map(|n| {
                let one = directory.join(format!("named-{size}-{n}.html"));
                std::fs::write(&one, &body).expect("a named page");
                one
            })
            .collect();

        let times = if size >= 1024 * 1024 { 2_000 } else { 20_000 };

        for (family, vehicles) in [
            ("named at startup", AT_STARTUP),
            ("named by the request", BY_REQUEST),
        ] {
            println!(
                "{} KiB, {times} sends over one connection - {family}",
                size / 1024
            );
            println!(
                "{:>15} {:>9} {:>11} {:>10}      wall spread",
                "vehicle", "µs/op", "µs machine", "µs thread"
            );
            for vehicle in vehicles {
                run(vehicle, &path, &named, size, times, repeats);
            }
            println!();
        }
    }

    println!("loadavg after: {}", loadavg());
}

fn loadavg() -> String {
    std::fs::read_to_string("/proc/loadavg")
        .map(|l| l.split_whitespace().take(3).collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|_| "unknown".into())
}

fn num_cores() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(0)
}

fn uname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|r| format!("Linux {}", r.trim()))
        .unwrap_or_else(|_| "unknown".into())
}
