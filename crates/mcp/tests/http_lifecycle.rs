//! Loopback lifecycle over real sockets: admission without a queue, body limits, the body
//! deadline, a client that leaves mid-body, and clean shutdown.

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "common/mod.rs"]
mod common;
#[path = "http_support/mod.rs"]
mod support;

use std::io::Write;
use std::net::TcpStream;
use std::time::{Duration, Instant};

use jobniac_mcp::http::MAX_CONNECTIONS;

use support::{
    ACCEPT, Harness, TOKEN, exchange, hang_up, initialize_body, is_refusal, parse_reply,
    request_bytes,
};

/// Headers of a POST with the given declared body length, without the body.
fn post_head(port: u16, length: usize) -> Vec<u8> {
    let headers = vec![
        ("Accept".to_owned(), ACCEPT.to_owned()),
        ("Authorization".to_owned(), format!("Bearer {TOKEN}")),
        ("Content-Type".to_owned(), "application/json".to_owned()),
        ("Content-Length".to_owned(), length.to_string()),
    ];
    request_bytes(
        "POST",
        "/mcp",
        Some(&format!("127.0.0.1:{port}")),
        &headers,
        b"",
    )
}

/// A complete `{}` POST with a valid head: admitted it answers at once, refused it gets 503.
fn probe_bytes(port: u16) -> Vec<u8> {
    request_bytes(
        "POST",
        "/mcp",
        Some(&format!("127.0.0.1:{port}")),
        &[
            ("Accept".into(), ACCEPT.into()),
            ("Authorization".into(), format!("Bearer {TOKEN}")),
            ("Content-Type".into(), "application/json".into()),
            ("Content-Length".into(), "2".into()),
        ],
        b"{}",
    )
}

/// One connection whose request stalls in its body, holding one admission slot once its head is
/// read.
fn hold_slot(port: u16) -> TcpStream {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.write_all(&post_head(port, 100)).unwrap();
    stream.write_all(b"{\"jso").unwrap();
    stream
}

/// Whether the listener has answered or closed `stream`, without waiting. A held request that is
/// still waiting for its body has been neither.
fn is_answered_or_closed(stream: &TcpStream) -> bool {
    stream.set_nonblocking(true).unwrap();
    let mut buffer = [0_u8; 1];
    let outcome = std::io::Read::read(&mut &*stream, &mut buffer);
    stream.set_nonblocking(false).unwrap();
    !matches!(
        outcome,
        Err(ref error) if matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        )
    )
}

/// Send the probe until the listener refuses it with 503, which it does once every admission slot
/// is held. A probe sent before the listener has read all the held heads is admitted and ends at
/// once, so repeating it converges without a fixed wait.
///
/// An early probe can also take the last free slot while the listener reads the final held head,
/// so that the held request itself is refused (answered 503, connection closed) and its slot
/// never fills. Such a held connection is replaced before the next probe, so the loop ends with
/// every one of `held` waiting in its body. Returns the refused reply.
async fn probe_until_refused(port: u16, held: &mut [TcpStream]) -> (support::Reply, Duration) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let started = Instant::now();
        let reply = exchange(port, probe_bytes(port)).await;
        if reply.status == 503 {
            return (reply, started.elapsed());
        }
        assert!(
            Instant::now() < deadline,
            "the listener never refused the probe (last status {})",
            reply.status
        );
        for stream in held.iter_mut() {
            if is_answered_or_closed(stream) {
                *stream = hold_slot(port);
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Whether the listener has closed `stream` (end of input or a reset), without waiting.
fn is_closed_by_server(stream: &TcpStream) -> bool {
    stream.set_nonblocking(true).unwrap();
    let mut buffer = [0_u8; 1];
    let outcome = std::io::Read::read(&mut &*stream, &mut buffer);
    stream.set_nonblocking(false).unwrap();
    match outcome {
        Ok(0) => true,
        Ok(_) => false,
        Err(error) => !matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ),
    }
}

/// A complete, valid initialize request body padded with a string to `total` bytes.
fn padded_initialize(total: usize) -> Vec<u8> {
    let prefix = b"{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/list\",\"params\":{\"pad\":\"";
    let suffix = b"\"}}";
    let fill = total - prefix.len() - suffix.len();
    let mut body = prefix.to_vec();
    body.extend(std::iter::repeat_n(b'a', fill));
    body.extend_from_slice(suffix);
    body
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admission_is_immediate_and_capped_at_sixteen() {
    let harness = Harness::start().await;
    let port = harness.port;
    // Sixteen requests stall in their bodies, each holding one admission slot.
    let mut held: Vec<TcpStream> = (0..16).map(|_| hold_slot(port)).collect();
    // The probe is refused once the listener has read all the held heads; no fixed wait.
    let (probe, answered_in) = probe_until_refused(port, &mut held).await;
    assert_eq!(probe.status, 503, "the seventeenth request is refused");
    assert!(is_refusal(&probe));
    assert!(
        answered_in < Duration::from_secs(1),
        "refusal is immediate, not queued"
    );

    // Releasing the held bodies frees their slots; a later request is admitted again.
    for stream in held {
        hang_up(stream);
    }
    let mut admitted = false;
    for _ in 0..50 {
        let reply = exchange(
            port,
            request_bytes(
                "POST",
                "/mcp",
                Some(&format!("127.0.0.1:{port}")),
                &[
                    ("Accept".into(), ACCEPT.into()),
                    ("Authorization".into(), format!("Bearer {TOKEN}")),
                    ("Content-Type".into(), "application/json".into()),
                    ("Content-Length".into(), "2".into()),
                ],
                b"{}",
            ),
        )
        .await;
        if reply.status != 503 {
            admitted = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(admitted, "slots are released when the held bodies end");
    harness.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn four_mib_body_is_accepted_and_replayed_and_more_is_refused() {
    let harness = Harness::start().await;
    let port = harness.port;
    let four_mib = 4 * 1024 * 1024;
    let body = padded_initialize(four_mib);
    let mut bytes = post_head(port, body.len());
    bytes.extend_from_slice(&body);
    let reply = exchange(port, bytes).await;
    assert!(
        !is_refusal(&reply),
        "a complete 4 MiB body reaches the service (status {})",
        reply.status
    );

    // One byte more, sent chunked so that only the body read can refuse it: 413.
    let oversize = padded_initialize(four_mib + 1);
    let headers = vec![
        ("Accept".to_owned(), ACCEPT.to_owned()),
        ("Authorization".to_owned(), format!("Bearer {TOKEN}")),
        ("Content-Type".to_owned(), "application/json".to_owned()),
        ("Transfer-Encoding".to_owned(), "chunked".to_owned()),
    ];
    let mut chunked = request_bytes(
        "POST",
        "/mcp",
        Some(&format!("127.0.0.1:{port}")),
        &headers,
        b"",
    );
    chunked.extend_from_slice(format!("{:x}\r\n", oversize.len()).as_bytes());
    chunked.extend_from_slice(&oversize);
    chunked.extend_from_slice(b"\r\n0\r\n\r\n");
    let reply = exchange(port, chunked).await;
    assert_eq!(
        reply.status, 413,
        "more than 4 MiB is refused while reading"
    );
    assert!(is_refusal(&reply));
    harness.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_that_leaves_mid_body_gets_no_service_effect_and_the_listener_lives_on() {
    let harness = Harness::start().await;
    let port = harness.port;
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.write_all(&post_head(port, 500)).unwrap();
    stream.write_all(b"{\"id\"").unwrap();
    hang_up(stream);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let body = initialize_body(9);
    let bytes = request_bytes(
        "POST",
        "/mcp",
        Some(&format!("127.0.0.1:{port}")),
        &[
            ("Accept".into(), ACCEPT.into()),
            ("Authorization".into(), format!("Bearer {TOKEN}")),
            ("Content-Type".into(), "application/json".into()),
            ("Content-Length".into(), body.len().to_string()),
        ],
        &body,
    );
    let reply = exchange(port, bytes).await;
    assert_eq!(reply.status, 200, "a later client is served");
    assert!(reply.text().contains("protocolVersion"));
    harness.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slow_body_is_cut_off_at_the_total_deadline_with_408() {
    let harness = Harness::start().await;
    let port = harness.port;
    let reply = tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(60)))
            .unwrap();
        stream.write_all(&post_head(port, 50)).unwrap();
        stream.write_all(b"{").unwrap();
        // Trickle one byte at a time: the deadline is for the whole body, not each chunk.
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(21) {
            if stream.write_all(b" ").is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1500));
        }
        let mut raw = Vec::new();
        let _ = std::io::Read::read_to_end(&mut stream, &mut raw);
        parse_reply(&raw)
    })
    .await
    .unwrap();
    assert_eq!(
        reply.status, 408,
        "the body deadline is 20 seconds in total"
    );
    assert!(is_refusal(&reply));
    harness.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_is_clean_and_the_port_stops_accepting() {
    let harness = Harness::start().await;
    let port = harness.port;
    let body = initialize_body(1);
    let reply = exchange(
        port,
        request_bytes(
            "POST",
            "/mcp",
            Some(&format!("127.0.0.1:{port}")),
            &[
                ("Accept".into(), ACCEPT.into()),
                ("Authorization".into(), format!("Bearer {TOKEN}")),
                ("Content-Type".into(), "application/json".into()),
                ("Content-Length".into(), body.len().to_string()),
            ],
            &body,
        ),
    )
    .await;
    assert_eq!(reply.status, 200);
    let started = Instant::now();
    harness.stop().await.expect("shutdown returns Ok");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "shutdown is bounded"
    );
    assert!(
        TcpStream::connect(("127.0.0.1", port)).is_err(),
        "the port is closed after shutdown"
    );
}

/// A client that connects and sends nothing is closed within the head timeout (5 seconds).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_silent_connection_is_closed_within_the_head_timeout() {
    let harness = Harness::start().await;
    let port = harness.port;
    let outcome = tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(15)))
            .unwrap();
        let started = Instant::now();
        let mut raw = Vec::new();
        let read = std::io::Read::read_to_end(&mut stream, &mut raw);
        (read.is_ok(), started.elapsed(), raw)
    })
    .await
    .unwrap();
    let (closed, elapsed, raw) = outcome;
    assert!(
        closed,
        "the server closed the silent connection (no read timeout)"
    );
    assert!(raw.is_empty(), "a silent connection gets no answer");
    assert!(
        elapsed < Duration::from_secs(10),
        "closed after {elapsed:?}"
    );
    harness.stop().await.unwrap();
}

/// A head trickled one byte at a time is cut off at the head timeout, not kept open while the
/// client keeps writing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_trickled_request_head_is_cut_off_at_the_head_timeout() {
    let harness = Harness::start().await;
    let port = harness.port;
    let outcome = tokio::task::spawn_blocking(move || {
        let mut writer = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut reader = writer.try_clone().unwrap();
        reader
            .set_read_timeout(Some(Duration::from_secs(15)))
            .unwrap();
        let trickle = std::thread::spawn(move || {
            let started = Instant::now();
            let _ = writer.write_all(b"POST /mcp HTTP/1.1\r\nHost: 127.0.0.1");
            while started.elapsed() < Duration::from_secs(12) {
                std::thread::sleep(Duration::from_secs(1));
                if writer.write_all(b"x").is_err() {
                    break;
                }
            }
        });
        let started = Instant::now();
        let mut raw = Vec::new();
        let read = std::io::Read::read_to_end(&mut reader, &mut raw);
        let elapsed = started.elapsed();
        let _ = trickle.join();
        // The server closes while the client is still writing, so on Linux the close can reach
        // the reader as a reset instead of an end of stream; both are the server hanging up. A
        // read that ran into its own timeout is not.
        let hung_up = match read {
            Ok(_) => true,
            Err(error) => matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::BrokenPipe
            ),
        };
        (hung_up, elapsed)
    })
    .await
    .unwrap();
    assert!(outcome.0, "the server closed the trickled head");
    assert!(
        outcome.1 < Duration::from_secs(10),
        "cut off after {:?}",
        outcome.1
    );
    harness.stop().await.unwrap();
}

/// An idle keep-alive connection is closed after the same five seconds, once its answer is sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idle_keep_alive_connection_is_closed_after_five_seconds() {
    let harness = Harness::start().await;
    let port = harness.port;
    let body = initialize_body(1);
    let head = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: {ACCEPT}\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let outcome = tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(15)))
            .unwrap();
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(&body).unwrap();
        // The answer is complete once the first status line and the JSON body have arrived.
        let mut raw = Vec::new();
        let mut buffer = [0_u8; 4096];
        while !String::from_utf8_lossy(&raw).contains("protocolVersion") {
            let read = std::io::Read::read(&mut stream, &mut buffer).unwrap();
            assert!(read > 0, "the answer arrives before the connection ends");
            raw.extend_from_slice(&buffer[..read]);
        }
        let started = Instant::now();
        let closed = std::io::Read::read_to_end(&mut stream, &mut raw).is_ok();
        (
            String::from_utf8_lossy(&raw).into_owned(),
            closed,
            started.elapsed(),
        )
    })
    .await
    .unwrap();
    let (raw, closed, idle) = outcome;
    assert!(raw.starts_with("HTTP/1.1 200"), "the request is answered");
    assert!(closed, "the idle connection was closed by the server");
    assert!(
        idle < Duration::from_secs(10),
        "idle for {idle:?} before close"
    );
    harness.stop().await.unwrap();
}

/// At the cap of sixty-four connections a silent flood cannot lock a real client out: the new
/// connection takes the place of the oldest connection that has no request in progress.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_silent_flood_at_the_connection_cap_does_not_lock_a_client_out() {
    let harness = Harness::start().await;
    let port = harness.port;
    let held = tokio::task::spawn_blocking(move || {
        (0..MAX_CONNECTIONS)
            .map(|_| TcpStream::connect(("127.0.0.1", port)).unwrap())
            .collect::<Vec<_>>()
    })
    .await
    .unwrap();
    let body = initialize_body(2);
    let reply = exchange(
        port,
        request_bytes(
            "POST",
            "/mcp",
            Some(&format!("127.0.0.1:{port}")),
            &[
                ("Accept".into(), ACCEPT.into()),
                ("Authorization".into(), format!("Bearer {TOKEN}")),
                ("Content-Type".into(), "application/json".into()),
                ("Content-Length".into(), body.len().to_string()),
            ],
            &body,
        ),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "a real client is served although sixty-four silent connections are open"
    );
    // The oldest silent connection made room; the newest still stands.
    let (oldest_closed, newest_closed) = tokio::task::spawn_blocking(move || {
        std::thread::sleep(Duration::from_millis(200));
        let outcome = (
            is_closed_by_server(&held[0]),
            is_closed_by_server(&held[MAX_CONNECTIONS - 1]),
        );
        drop(held);
        outcome
    })
    .await
    .unwrap();
    assert!(oldest_closed, "the oldest silent connection was evicted");
    assert!(!newest_closed, "the newest silent connection was kept");
    harness.stop().await.unwrap();
}

/// A connection with a request in progress is never evicted, and the cap holds: with sixteen
/// stalled requests and silent connections on top, more connections than the cap push out only
/// silent ones, never more than sixty-four stay open, and the stalled requests are still answered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn busy_connections_are_not_evicted_and_the_cap_holds() {
    let harness = Harness::start().await;
    let port = harness.port;
    let mut busy: Vec<TcpStream> = (0..16).map(|_| hold_slot(port)).collect();
    // Every stalled head has been read once the listener refuses a seventeenth request.
    probe_until_refused(port, &mut busy).await;
    let outcome = tokio::task::spawn_blocking(move || {
        let silent: Vec<TcpStream> = (0..(MAX_CONNECTIONS + 40 - busy.len()))
            .map(|_| TcpStream::connect(("127.0.0.1", port)).unwrap())
            .collect();
        std::thread::sleep(Duration::from_millis(500));
        let open = busy
            .iter()
            .chain(silent.iter())
            .filter(|stream| !is_closed_by_server(stream))
            .count();
        let busy_closed = busy
            .iter()
            .filter(|stream| is_closed_by_server(stream))
            .count();
        let newest_closed = silent
            .iter()
            .rev()
            .take(40)
            .filter(|stream| is_closed_by_server(stream))
            .count();
        // The stalled requests complete and are answered.
        let mut answered = 0;
        for mut stream in busy {
            stream.write_all(&[b' '; 95]).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut raw = Vec::new();
            let mut buffer = [0_u8; 1024];
            while !raw.windows(4).any(|window| window == b"\r\n\r\n") {
                match std::io::Read::read(&mut stream, &mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => raw.extend_from_slice(&buffer[..read]),
                }
            }
            if raw.starts_with(b"HTTP/1.1 ") {
                answered += 1;
            }
        }
        drop(silent);
        (open, busy_closed, newest_closed, answered)
    })
    .await
    .unwrap();
    let (open, busy_closed, newest_closed, answered) = outcome;
    assert!(
        open <= MAX_CONNECTIONS,
        "{open} connections open, the cap is {MAX_CONNECTIONS}"
    );
    assert_eq!(busy_closed, 0, "no connection with a request in progress");
    assert_eq!(newest_closed, 0, "the newest connections are kept");
    assert_eq!(answered, 16, "the stalled requests are still answered");
    harness.stop().await.unwrap();
}
