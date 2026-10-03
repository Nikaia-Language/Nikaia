//! `std::net` — the socket `std` lends, so a Nikaia package can speak a
//! protocol over it
//! ([ADR-289](../../../docs/specification/adr/adr-289.md) D6).
//!
//! [ADR-289](../../../docs/specification/adr/adr-289.md) D2's subtraction,
//! carried one item further: what was pushed out of `std` is the *protocol and
//! the framework*, and a socket is neither. It is an operating-system resource
//! of exactly the kind `fs` and `io` already own, and the `http` package is
//! Nikaia and cannot make a syscall.
//!
//! ## What pauses, and on what
//!
//! Everything that waits. `accept`, `read` and `write` give the thread up
//! rather than holding it: the socket is non-blocking, and where the kernel
//! says *not yet* this asks
//! [`crate::rt::io::waiting`]([ADR-121](../../../docs/specification/adr/adr-121.md)
//! D4) for readiness and awaits it. That is the same park `fs` uses and the
//! same one a task uses, so one thread serves many connections at
//! `user_parallelism = no` exactly as it does at `yes`.
//!
//! `bind` and `connect` are the two that do **not**. Binding is a syscall that
//! answers immediately; connecting is not, and this one blocks on it — stated
//! rather than hidden, because what the MVP connects to is a listener on the
//! same machine ([ADR-289](../../../docs/specification/adr/adr-289.md) D13) and a
//! non-blocking connect is a second readiness shape for a case nothing here
//! has yet.
//!
//! ## What comes out of it is **untrusted**
//!
//! [ADR-010](../../../docs/specification/adr/adr-010.md) D2, and this is the
//! first source in `std` that is: every other one is the operator's own — a
//! file they named, a pipe they connected, the arguments they typed. `std`'s
//! own ledger predicted this one by name.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

use crate::bytes::Bytes;
use crate::io::IoError;
use crate::rt::{Interest, io as readiness};

/// How much one `read` asks the kernel for.
///
/// **One page-sized buffer per read and not a reusable one**, which is the
/// allocation `Bytes` costs here: what comes back is a shared buffer a program
/// may keep, so it cannot be a window into something the next read overwrites.
/// A reusable buffer is a second shape with a lifetime story of its own, and
/// what it would buy is not this step's.
const CHUNK: usize = 64 * 1024;

/// A bound socket, waiting for connections.
pub struct Listener {
    inner: TcpListener,
}

/// One connection, open until it is dropped.
///
/// **Dropping it closes it**, which is Rust's own answer and not a decision
/// this makes. A plain TCP close hands the socket to the kernel and waits for
/// nothing, so this needs no pausable `Cleanup`
/// ([ADR-297](../../../docs/specification/adr/adr-297.md)); a program that
/// wants the close at a named moment writes `close`.
pub struct Connection {
    inner: TcpStream,
}

/// Bind `address` and start listening.
///
/// `"127.0.0.1:8080"` — the address is the caller's and this makes no choice
/// about it. Which address a *server* binds when nobody says is
/// [ADR-289](../../../docs/specification/adr/adr-289.md) D13's, one layer up.
pub async fn listen(address: &str) -> Result<Listener, IoError> {
    let inner = TcpListener::bind(address).map_err(|e| IoError::of(e, address))?;
    inner
        .set_nonblocking(true)
        .map_err(|e| IoError::of(e, address))?;
    Ok(Listener { inner })
}

/// Connect to `address`.
///
/// Blocking, which the module header says is the one thing here that is.
pub async fn connect(address: &str) -> Result<Connection, IoError> {
    let inner = TcpStream::connect(address).map_err(|e| IoError::of(e, address))?;
    inner
        .set_nonblocking(true)
        .map_err(|e| IoError::of(e, address))?;
    Ok(Connection { inner })
}

impl Listener {
    /// The next connection, awaiting one where there is none yet.
    pub async fn accept(&self) -> Result<Connection, IoError> {
        loop {
            match self.inner.accept() {
                Ok((stream, _)) => {
                    stream
                        .set_nonblocking(true)
                        .map_err(|e| IoError::of(e, "a connection"))?;
                    return Ok(Connection { inner: stream });
                }
                Err(error) if would_block(&error) => ready(&self.inner, Interest::Readable).await?,
                Err(error) => return Err(IoError::of(error, "accept")),
            }
        }
    }

    /// The address this is bound to, which is what a caller that asked for port
    /// `0` needs to know.
    pub fn address(&self) -> String {
        match self.inner.local_addr() {
            Ok(at) => at.to_string(),
            Err(_) => String::new(),
        }
    }
}

impl Connection {
    /// The next bytes the peer sent, awaiting them where there are none yet.
    ///
    /// **Empty means the peer closed**, which is what a read of zero bytes means
    /// on every socket there has ever been and is the one thing a caller has to
    /// know to write a loop.
    pub async fn read(&mut self) -> Result<Bytes, IoError> {
        let mut buffer = vec![0_u8; CHUNK];
        loop {
            match self.inner.read(&mut buffer) {
                Ok(read) => {
                    buffer.truncate(read);
                    return Ok(Bytes::from(buffer));
                }
                Err(error) if would_block(&error) => ready(&self.inner, Interest::Readable).await?,
                Err(error) => return Err(IoError::of(error, "a connection")),
            }
        }
    }

    /// All of `bytes`, awaiting room where there is none yet.
    ///
    /// **All of them**, because a partial write is not something a caller can
    /// do anything useful with: the loop that would follow one is this loop.
    pub async fn write(&mut self, bytes: impl AsRef<[u8]>) -> Result<(), IoError> {
        // `impl AsRef<[u8]>` for `fs::write`'s reason, and the ledger writes
        // `?` beside it for the same one: a caller hands this a `Bytes`, a
        // `ref String` or a buffer, and naming one of them would refuse a
        // correct program.
        let bytes = bytes.as_ref();
        let mut at = 0;
        while at < bytes.len() {
            match self.inner.write(&bytes[at..]) {
                Ok(0) => return Err(IoError::Other("a connection closed while writing".into())),
                Ok(written) => at += written,
                Err(error) if would_block(&error) => ready(&self.inner, Interest::Writable).await?,
                Err(error) => return Err(IoError::of(error, "a connection")),
            }
        }
        Ok(())
    }

    /// Who is on the other end.
    pub fn peer(&self) -> String {
        match self.inner.peer_addr() {
            Ok(at) => at.to_string(),
            Err(_) => String::new(),
        }
    }

    /// Close it now rather than when it is dropped.
    pub fn close(self) {}
}

/// Whether the kernel said *not yet*.
///
/// `Interrupted` is on this list because a signal is not a failure of the
/// socket: the call is retried, which is what every other socket library does
/// and what a caller cannot do anything else about.
fn would_block(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
    )
}

/// Give the thread up until the socket is ready.
async fn ready(socket: &impl std::os::fd::AsFd, interest: Interest) -> Result<(), IoError> {
    readiness::waiting(socket, interest, None)
        .await
        .map(|_| ())
        .map_err(|error| IoError::of(error, "waiting for a socket"))
}
