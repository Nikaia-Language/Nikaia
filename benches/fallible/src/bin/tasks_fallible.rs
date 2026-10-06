//! Many short tasks on fallible collections: the same steps as
//! `tasks_std.rs`, with every list and text taken from a budget that may
//! refuse (ADR-327 D5).

#[path = "../budget.rs"]
mod budget;

use budget::{extend, push, text, Budget, List, Text};
use std::fmt::Write;

/// One request: a request line, headers, a body.
fn request(n: u64) -> Text {
    let mut out = text();
    let _ = write!(
        budget::Into(&mut out),
        "GET /items/{n}?page={} HTTP/1.1\r\n",
        n % 17
    );
    for (name, value) in [
        ("Host", "example.org"),
        ("User-Agent", "bench/1.0"),
        ("Accept", "application/json"),
        ("Accept-Language", "de, en;q=0.8"),
        ("Cookie", "session=4f2a9c; theme=dark"),
        ("X-Request-Id", "7d1e"),
    ] {
        extend(&mut out, name.as_bytes());
        extend(&mut out, b": ");
        extend(&mut out, value.as_bytes());
        extend(&mut out, b"\r\n");
    }
    extend(&mut out, b"\r\n");
    out
}

/// An owned copy of `s` from the budget.
fn owned(s: &str) -> Text {
    let mut out = text();
    extend(&mut out, s.as_bytes());
    out
}

/// The text as a `str`, as `String` hands one out: every byte in it came from
/// a `str`, so it is not checked again - a text type built on this list would
/// know that, as `String` does.
fn as_str(t: &Text) -> &str {
    // SAFETY: only whole `str`s and ASCII bytes are ever appended.
    unsafe { std::str::from_utf8_unchecked(t) }
}

/// Parse it into owned parts and answer it.
fn answer(text_in: &Text) -> Text {
    let text_in = as_str(text_in);
    let mut lines = text_in.split("\r\n");
    let first = lines.next().unwrap_or("");
    let mut parts = first.split(' ');
    let method = owned(parts.next().unwrap_or(""));
    let target = owned(parts.next().unwrap_or(""));
    let mut headers: List<(Text, Text)> = List::new_in(Budget);
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(": ") {
            let mut lower = owned(name);
            lower.make_ascii_lowercase();
            push(&mut headers, (lower, owned(value)));
        }
    }
    let target = as_str(&target);
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut segments: List<Text> = List::new_in(Budget);
    for segment in path.split('/').filter(|s| !s.is_empty()) {
        push(&mut segments, owned(segment));
    }
    let mut body = owned("{\"method\":\"");
    extend(&mut body, &method);
    extend(&mut body, b"\",\"segments\":[");
    for (at, segment) in segments.iter().enumerate() {
        if at > 0 {
            push(&mut body, b',');
        }
        push(&mut body, b'"');
        extend(&mut body, segment);
        push(&mut body, b'"');
    }
    extend(&mut body, b"],\"query\":\"");
    extend(&mut body, query.as_bytes());
    extend(&mut body, b"\",\"headers\":");
    let _ = write!(budget::Into(&mut body), "{}", headers.len());
    push(&mut body, b'}');
    let mut response = text();
    let _ = write!(
        budget::Into(&mut response),
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (name, value) in &headers {
        if &name[..] == b"x-request-id" {
            extend(&mut response, b"X-Request-Id: ");
            extend(&mut response, value);
            extend(&mut response, b"\r\n");
        }
    }
    extend(&mut response, b"\r\n");
    extend(&mut response, &body);
    response
}

fn main() {
    let tasks: u64 = std::env::args()
        .nth(1)
        .and_then(|n| n.parse().ok())
        .unwrap_or(100_000);
    let mut sum: u64 = 0;
    let mut bytes: u64 = 0;
    for n in 0..tasks {
        let response = answer(&request(n));
        bytes += response.len() as u64;
        sum = response
            .iter()
            .fold(sum, |acc, &b| acc.wrapping_mul(31).wrapping_add(b as u64));
    }
    println!("{tasks} tasks, {bytes} bytes, checksum {sum:016x}");
}
