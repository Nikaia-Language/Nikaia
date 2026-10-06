//! **The same tasks for every allocator**: many short tasks, each reading a
//! request and writing a response, as a server's are. Each binary includes
//! this file and differs from the others only in its `#[global_allocator]`,
//! so a count's difference is the allocator's.

/// What a binary's allocator is told around each task.
pub trait Tasks {
    fn begin(&self);
    fn end(&self);
}

/// One request: a request line, headers, a body.
fn request(n: u64) -> String {
    let mut text = format!("GET /items/{n}?page={} HTTP/1.1\r\n", n % 17);
    for (name, value) in [
        ("Host", "example.org"),
        ("User-Agent", "bench/1.0"),
        ("Accept", "application/json"),
        ("Accept-Language", "de, en;q=0.8"),
        ("Cookie", "session=4f2a9c; theme=dark"),
        ("X-Request-Id", "7d1e"),
    ] {
        text.push_str(name);
        text.push_str(": ");
        text.push_str(value);
        text.push_str("\r\n");
    }
    text.push_str("\r\n");
    text
}

/// Parse it into owned parts and answer it.
fn answer(text: &str) -> String {
    let mut lines = text.split("\r\n");
    let first = lines.next().unwrap_or("");
    let mut parts = first.split(' ');
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(": ") {
            headers.push((name.to_ascii_lowercase(), value.to_string()));
        }
    }
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let segments: Vec<String> = path
        .split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let mut body = String::from("{\"method\":\"");
    body.push_str(&method);
    body.push_str("\",\"segments\":[");
    for (at, segment) in segments.iter().enumerate() {
        if at > 0 {
            body.push(',');
        }
        body.push('"');
        body.push_str(segment);
        body.push('"');
    }
    body.push_str("],\"query\":\"");
    body.push_str(query);
    body.push_str("\",\"headers\":");
    body.push_str(&headers.len().to_string());
    body.push('}');
    let mut response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n", body.len());
    for (name, value) in &headers {
        if name == "x-request-id" {
            response.push_str("X-Request-Id: ");
            response.push_str(value);
            response.push_str("\r\n");
        }
    }
    response.push_str("\r\n");
    response.push_str(&body);
    response
}

/// Run `tasks` tasks and print what they came to, the same bytes whatever
/// the allocator.
pub fn run(tasks: u64, around: &dyn Tasks) {
    let mut sum: u64 = 0;
    let mut bytes: u64 = 0;
    for n in 0..tasks {
        around.begin();
        let response = answer(&request(n));
        bytes += response.len() as u64;
        sum = response
            .bytes()
            .fold(sum, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));
        drop(response);
        around.end();
    }
    println!("{tasks} tasks, {bytes} bytes, checksum {sum:016x}");
}

/// The task count from the first argument.
pub fn tasks() -> u64 {
    std::env::args()
        .nth(1)
        .and_then(|n| n.parse().ok())
        .unwrap_or(100_000)
}
