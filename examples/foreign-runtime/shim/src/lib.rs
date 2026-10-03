//! The Rust half of the ADR-303 D7 experiment: a crate that brings its own
//! runtime and its own threads, wrapped in a surface a Nikaia program can call.
//!
//! Everything here is deliberately plain. What the experiment is after is what
//! the *Nikaia* side can and cannot say about a call into this, so this file
//! must not be clever - except in [`across_a_thread_unchecked`], which is
//! clever on purpose and says why.

use std::convert::Infallible;
use std::net::SocketAddr;

use http_body_util::Full;
use hyper::body::Bytes;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;

/// Start a `hyper` server on `port`, serve exactly one request, and return the
/// body the server sent back, with the name of the thread that produced it.
///
/// The whole runtime lives and dies inside this call: a multi-threaded `tokio`
/// runtime with two worker threads, a listener, one client task, one served
/// connection. A Nikaia program sees a function that takes a number and returns
/// text.
pub fn serve_once(port: i64) -> String {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("a tokio runtime");

    rt.block_on(async move {
        let addr = SocketAddr::from(([127, 0, 0, 1], port as u16));
        let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");

        let client = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
            // `Connection: close` is load-bearing, and finding that out cost
            // one hung build: HTTP/1.1 keeps the connection alive by default,
            // so `read_to_end` waits for a close the server has no reason to
            // perform and `serve_connection` waits for the client to go away.
            stream
                .write_all(
                    b"GET /hello HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
                )
                .await
                .expect("write");
            let mut buf = Vec::new();
            stream.read_to_end(&mut buf).await.ok();
            String::from_utf8_lossy(&buf).to_string()
        });

        let (socket, _) = listener.accept().await.expect("accept");
        // Spawned rather than awaited here, so that the request is parsed and
        // answered on a worker thread of the foreign runtime rather than on the
        // thread `block_on` is sitting on. The program says which.
        tokio::spawn(
            hyper::server::conn::http1::Builder::new().serve_connection(
                TokioIo::new(socket),
                service_fn(|req: Request<hyper::body::Incoming>| async move {
                    let line = format!("{} {} on {}", req.method(), req.uri().path(), thread());
                    Ok::<_, Infallible>(Response::new(Full::new(Bytes::from(line))))
                }),
            ),
        )
        .await
        .expect("the connection task")
        .ok();

        let answer = client.await.expect("the client task");
        answer
            .rsplit("\r\n\r\n")
            .next()
            .unwrap_or("")
            .trim()
            .to_string()
    })
}

/// Something a value can say about itself, so that a value crossing a thread
/// boundary has a reason to be there.
pub trait Describe {
    fn describe(&self) -> String;
}

impl Describe for String {
    fn describe(&self) -> String {
        self.clone()
    }
}

/// A handle that cannot leave the thread it was made on.
///
/// This used to stand in for `Shared` at `user_parallelism = no`, which
/// [ADR-037](../../../../docs/specification/adr/adr-037.md) D3 lowered to an
/// `Rc`, on the grounds that `Shared` was unbuilt. **Both halves are gone**:
/// D6 narrowed D3 so `Shared` is an atomic count at either setting, and
/// `Shared` is built.
///
/// It stands in for nothing now, and is the experiment's own value - which is
/// the sharper position. A Nikaia `Shared` cannot be written into this shape at
/// all: [ADR-312](../../../../docs/specification/adr/adr-312.md) D9 refuses one
/// handed to code nothing describes, because D7's per-value inference makes a
/// `Shared[Conn]` an `Rc` for one value and an `Arc` for another in the same
/// program and no foreign signature can name both. So a value that may not
/// cross a thread has to come from **this** side of the boundary, and the
/// question - what happens when one reaches a thread the foreign runtime owns -
/// is answered with a value that exists rather than a stand-in for one.
pub struct LocalHandle {
    name: std::rc::Rc<String>,
}

impl Describe for LocalHandle {
    fn describe(&self) -> String {
        format!("{} (refcount {})", self.name, std::rc::Rc::strong_count(&self.name))
    }
}

/// Make one. A Nikaia program can hold the result in a `let`.
pub fn local_handle(name: String) -> LocalHandle {
    LocalHandle {
        name: std::rc::Rc::new(name),
    }
}

/// Move a value onto a thread the foreign runtime owns, and bring back what it
/// said about itself there.
///
/// The `Send` bound is not decoration: it is the only thing standing between a
/// program and an `Rc` refcount incremented from two threads at once. Every
/// **safe** way of reaching another thread carries it - `std::thread::spawn`,
/// `tokio::spawn`, `rayon`'s scopes - so a foreign crate that takes a value
/// across a thread boundary in safe Rust demands it of its caller too.
pub fn across_a_thread<T>(value: T) -> String
where
    T: Describe + Send + 'static,
{
    on_one_worker(move || value.describe())
}

/// The same crossing, with the bound removed.
///
/// **This is the hole, opened on purpose.** `unsafe impl Send` is a promise the
/// crate makes about its own type, and a crate that makes it wrongly puts a
/// non-`Send` value on another thread with nothing anywhere complaining -
/// including in the Nikaia program that called it, which wrote one line and
/// cannot see any of this. It is here so the experiment can say what the
/// difference is between a rule the toolchain enforces and a rule it inherits.
///
/// No Nikaia program should call this. One that does is the measurement.
pub fn across_a_thread_unchecked<T>(value: T) -> String
where
    T: Describe + 'static,
{
    let smuggled = Smuggled(value);
    on_one_worker(move || smuggled.describe())
}

/// The lie. A wrapper that claims `Send` for whatever it holds.
struct Smuggled<T>(T);

// SAFETY: none. That is the point - see `across_a_thread_unchecked`.
unsafe impl<T> Send for Smuggled<T> {}

impl<T: Describe> Smuggled<T> {
    /// Taking `self` rather than reading `self.0` is deliberate: a closure in
    /// edition 2021 captures the *field* it uses, so `move || s.0.describe()`
    /// would capture the `T` and not the wrapper, and the lie would not be
    /// told. That cost one confused build to notice.
    fn describe(self) -> String {
        self.0.describe()
    }
}

/// Run `f` on a worker thread of a `tokio` runtime this crate owns.
fn on_one_worker<F>(f: F) -> String
where
    F: FnOnce() -> String + Send + 'static,
{
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("a tokio runtime");
    let here = thread();
    let there = rt.block_on(async move {
        tokio::spawn(async move { format!("{} on {}", f(), thread()) })
            .await
            .expect("the task")
    });
    format!("{there}, called from {here}")
}

/// The running thread, by name where it has one and by id where it does not.
fn thread() -> String {
    let current = std::thread::current();
    match current.name() {
        Some(name) => name.to_string(),
        None => format!("{:?}", current.id()),
    }
}
