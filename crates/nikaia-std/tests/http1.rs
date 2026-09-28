//! **HTTP/1.1's text half**, asked every question a server asks it
//! ([ADR-194](../../../docs/specification/adr/adr-194.md) D5).
//!
//! Nothing here reaches a socket, which is the module's own claim: a program
//! reads bytes with `net` and hands them over, so every function below is `sync`
//! and a test of them is arithmetic over bytes.

use nikaia_std::http1;

/// The bytes of one request, taken in the pieces a socket would have handed over.
fn buffer(pieces: &[&[u8]]) -> http1::Buffer {
    let mut buffer = http1::Buffer::new();
    for piece in pieces {
        buffer.take(piece);
    }
    buffer
}

/// **A head that has arrived, read whole.**
#[test]
fn a_head_says_its_method_its_path_and_its_length() {
    let buffer = buffer(&[b"POST /a/b?c=1 HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\n\r\nabcd"]);
    let end = buffer.head_end(16_384).expect("a head within the cap");
    let head = buffer.head().expect("a head");
    assert_eq!(head.method(), "POST");
    // **Not decoded**: what a `%20` means is the program's question. The query
    // string is its own accessor since
    // [ADR-018](../../../docs/specification/adr/adr-018.md) D4, and `target()`
    // is what the client wrote.
    assert_eq!(head.path(), "/a/b");
    assert_eq!(head.target(), "/a/b?c=1");
    assert_eq!(head.query("c"), Some("1"));
    assert_eq!(head.length(), 4);
    assert_eq!(
        head.size(),
        end,
        "`size` is where `head_end` said the head ends"
    );
    assert_eq!(buffer.text_from(head.size()).expect("a body"), "abcd");
}

/// **`0` is *not yet***, and the shape is the reason: a read loop over
/// `while ended == 0` is one condition where a loop over an absence is a
/// condition plus the unwrapping after it.
#[test]
fn a_head_that_has_not_ended_is_zero_and_not_a_failure() {
    let mut buffer = http1::Buffer::new();
    buffer.take(b"GET / HTTP/1.1\r\nHost: ");
    assert_eq!(buffer.head_end(16_384).expect("no answer yet"), 0);
    buffer.take(b"x\r\n\r\n");
    assert_eq!(
        buffer.head_end(16_384).expect("and now there is one"),
        b"GET / HTTP/1.1\r\nHost: x\r\n\r\n".len() as i64
    );
}

/// **The cap answers before the head ends**, because a head that is already over
/// it never will end within it — which is what stops a client from making a
/// server hold its bytes.
#[test]
fn a_head_over_the_cap_is_refused_while_it_is_still_arriving() {
    let arriving = buffer(&[&vec![b'a'; 4_000]]);
    let refused = arriving.head_end(1_024).expect_err("over the cap");
    assert!(
        format!("{refused}").starts_with("431 "),
        "the message is what a server sends back: {refused}"
    );

    // And once it *has* ended past the cap, the same answer: a head is measured
    // whole or not at all.
    let long = [
        b"GET /".to_vec(),
        vec![b'a'; 4_000],
        b" HTTP/1.1\r\n\r\n".to_vec(),
    ]
    .concat();
    let ended = buffer(&[&long]);
    assert!(format!("{}", ended.head_end(1_024).expect_err("over the cap")).starts_with("431 "));
}

/// **Every refusal by name**, and each of them a message a server sends back
/// rather than a guess ([ADR-010](../../../docs/specification/adr/adr-010.md)
/// D2: these bytes are somebody else's).
#[test]
fn what_it_refuses_it_refuses_by_name() {
    for (asked, expected) in [
        (
            b"PUT / HTTP/1.1\r\n\r\n".to_vec(),
            "400 a method this does not answer",
        ),
        (
            b"GET / HTTP/2.0\r\n\r\n".to_vec(),
            "400 a version this does not speak",
        ),
        (
            b"GET /\r\n\r\n".to_vec(),
            "400 a request line that is not three words",
        ),
        (
            b"GET / HTTP/1.1\r\nContent-Length: none\r\n\r\n".to_vec(),
            "400 a content-length that is not a count",
        ),
        (
            b"GET / HTTP/1.1\r\nContent-Length: -1\r\n\r\n".to_vec(),
            "400 a content-length that is not a count",
        ),
        (
            // **Refused and not ignored** (D5: no chunked). A server that
            // ignored this would read a chunk header as a body.
            b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec(),
            "400 a transfer-encoding this does not speak",
        ),
        (
            [b"GET /\xff\xfe HTTP/1.1\r\n\r\n".to_vec()].concat(),
            "400 a request head that is not text",
        ),
    ] {
        let buffer = buffer(&[&asked]);
        let refused = buffer.head().expect_err("refused");
        assert_eq!(
            format!("{refused}"),
            expected,
            "{:?}",
            String::from_utf8_lossy(&asked)
        );
    }
}

/// A header's name does not care about case and its value's blanks are not part
/// of it, which is what HTTP/1.1 says and not a convenience.
#[test]
fn a_header_is_read_whatever_case_it_was_written_in() {
    let buffer =
        buffer(&[b"GET / HTTP/1.0\r\ncOnTeNt-LeNgTh:   7  \r\nCONNECTION: Keep-Alive\r\n\r\n"]);
    let head = buffer.head().expect("a head");
    assert_eq!(head.length(), 7);
    // **Read and not honoured**: `Connection: close` is the MVP's scope, and a
    // server that read the header and ignored it silently would be lying to a
    // client that asked.
    assert!(head.keep_alive());
}

/// A head with no `Content-Length` promised no body, which is `0` and not an
/// absence: a `GET` is the ordinary case and a caller should not have to ask
/// twice about it.
#[test]
fn a_head_with_no_length_promised_no_body() {
    let buffer = buffer(&[b"GET / HTTP/1.1\r\nHost: x\r\n\r\n"]);
    let head = buffer.head().expect("a head");
    assert_eq!(head.length(), 0);
    assert!(!head.keep_alive());
    assert_eq!(buffer.text_from(head.size()).expect("no body"), "");
}

/// **What `head` is asked before `head_end` said yes**, which a server never
/// does and a program might: a named refusal rather than the first line of
/// whatever arrived read as a request.
#[test]
fn a_head_asked_for_before_it_ended_says_so() {
    let buffer = buffer(&[b"GET / HTTP/1.1\r\nHost: x\r\n"]);
    assert_eq!(
        format!("{}", buffer.head().expect_err("not yet")),
        "400 a request head that has not ended"
    );
}

/// The body is text or it is refused, and an offset past the bytes is refused
/// too — neither of them a panic in a `std` a server calls on somebody else's
/// bytes.
#[test]
fn a_body_that_is_not_text_is_refused_and_so_is_one_that_starts_past_its_bytes() {
    let buffer = buffer(&[b"POST / HTTP/1.1\r\nContent-Length: 2\r\n\r\n\xff\xfe"]);
    let head = buffer.head().expect("a head");
    assert_eq!(
        format!("{}", buffer.text_from(head.size()).expect_err("not text")),
        "400 a request body that is not text"
    );
    assert_eq!(
        format!("{}", buffer.text_from(9_000).expect_err("past the end")),
        "400 a body that starts past its bytes"
    );
}

/// A buffer fills from whatever pieces the socket handed over, and the answer is
/// the same as if one piece had carried all of it — which is the whole point of
/// the type.
#[test]
fn how_the_bytes_arrived_changes_nothing() {
    let whole = b"POST /x HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello";
    let at_once = buffer(&[whole]);
    let one_at_a_time = buffer(&whole.chunks(1).collect::<Vec<_>>());
    assert_eq!(at_once.len(), one_at_a_time.len());
    assert_eq!(
        at_once.head_end(16_384).expect("a head"),
        one_at_a_time.head_end(16_384).expect("a head")
    );
    let head = one_at_a_time.head().expect("a head");
    assert_eq!(head.method(), "POST");
    assert_eq!(
        one_at_a_time.text_from(head.size()).expect("a body"),
        "hello"
    );
}

// --- What a handler asks a head (ADR-018 D4) ---

/// **A header is found whatever case it was written in**, which is the protocol's
/// own rule and not a convenience
/// ([ADR-018](../../../docs/specification/adr/adr-018.md) D4).
#[test]
fn a_header_is_found_by_name_whatever_case_either_side_wrote() {
    let buffer = buffer(&[b"GET / HTTP/1.1\r\nHost: example.org\r\nX-Trace: 7\r\n\r\n"]);
    let head = buffer.head().expect("a head");
    assert_eq!(head.header("host"), Some("example.org"));
    assert_eq!(head.header("HOST"), Some("example.org"));
    assert_eq!(head.header("x-trace"), Some("7"));
    // **Nothing, and not an empty string**, for a name the client did not send.
    assert_eq!(head.header("authorization"), None);
    assert_eq!(head.headers(), 2);
}

/// **The first, where a client sent the same name twice.** Joining them with a
/// comma is what the protocol says a *list-valued* header means, and which
/// headers those are is not something this module knows.
#[test]
fn a_repeated_header_answers_with_the_first() {
    let buffer = buffer(&[b"GET / HTTP/1.1\r\nAccept: a\r\nAccept: b\r\n\r\n"]);
    assert_eq!(buffer.head().expect("a head").header("accept"), Some("a"));
}

/// **`path()` and `query()` are two things**
/// ([ADR-018](../../../docs/specification/adr/adr-018.md) D4), and `target()` is
/// the whole of what the client wrote, for a log.
#[test]
fn the_path_and_the_query_are_two_things() {
    let buffer = buffer(&[b"GET /a/b?name=ada&debug HTTP/1.1\r\n\r\n"]);
    let head = buffer.head().expect("a head");
    assert_eq!(head.path(), "/a/b");
    assert_eq!(head.target(), "/a/b?name=ada&debug");
    assert_eq!(head.query("name"), Some("ada"));
    // **A name with no `=` has an empty value**, which is not being absent.
    assert_eq!(head.query("debug"), Some(""));
    assert_eq!(head.query("missing"), None);
}

/// **Nothing is decoded**, which is `path`'s own sentence read one field over: a
/// `%20` stays `%20` and a `+` stays a `+`, because which of the two a `+` means
/// depends on who wrote the form.
#[test]
fn a_query_hands_back_what_arrived() {
    let buffer = buffer(&[b"GET /?q=a%20b+c HTTP/1.1\r\n\r\n"]);
    assert_eq!(buffer.head().expect("a head").query("q"), Some("a%20b+c"));
}

/// And a target with no query at all answers nothing rather than reaching past
/// the end of it.
#[test]
fn a_target_with_no_query_answers_nothing() {
    let buffer = buffer(&[b"GET /plain HTTP/1.1\r\n\r\n"]);
    let head = buffer.head().expect("a head");
    assert_eq!(head.path(), "/plain");
    assert_eq!(head.target(), "/plain");
    assert_eq!(head.query("q"), None);
}

/// **The head is read in Nikaia now** (0.0.247, `src/tools/http1.nika`), and
/// two readings of the Rust it replaced are held here, because they are the
/// ones a grammar written from scratch gets differently: a line without a
/// colon says nothing and is skipped, and two spaces in a row are an empty
/// word between them - so the version is the fourth word, and refused.
#[test]
fn what_the_rust_parser_read_the_grammar_reads() {
    let read = buffer(&[b"GET /a?b=1 HTTP/1.1\r\nno colon here\r\nHost:  x \r\n\r\n"]);
    let head = read.head().expect("a head");
    assert_eq!(head.target(), "/a?b=1");
    assert_eq!(head.path(), "/a");
    assert_eq!(head.header("HOST"), Some("x"));
    assert_eq!(
        head.headers(),
        1,
        "the line without a colon is not a header"
    );

    let doubled = buffer(&[b"GET  / HTTP/1.1\r\n\r\n"]);
    assert_eq!(
        format!("{}", doubled.head().expect_err("refused")),
        "400 a version this does not speak"
    );
}
