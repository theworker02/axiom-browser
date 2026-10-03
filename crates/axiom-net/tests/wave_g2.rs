//! Wave G.2 — channel-fed request bodies and per-request response flow control,
//! verified against the local recording server.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axiom_net::test_server::{TestResponse, TestServer};
use axiom_net::{
    FlowControl, HttpMethod, NetworkError, NetworkEvent, NetworkRequest, NetworkService,
    NetworkServiceConfig, RequestBody, RequestScheduler, ResourceType, SchedulerConfig,
    StreamingBody,
};
use axiom_url::Url;
use crossbeam_channel::bounded;

fn service() -> NetworkService {
    NetworkService::new(NetworkServiceConfig::default())
}

fn post(srv: &TestServer, path: &str) -> NetworkRequest {
    NetworkRequest::new(
        HttpMethod::Post,
        Url::parse(&srv.url(path)).unwrap(),
        ResourceType::Fetch,
    )
}

#[test]
fn channel_body_is_sent_chunked_as_it_is_written() {
    let srv =
        TestServer::spawn(|req| TestResponse::ok(format!("got {}", req.body.len()), "text/plain"));
    let (sender, body) = StreamingBody::channel();
    let mut req = post(&srv, "/upload");
    req.body = RequestBody::Stream(body);
    let producer = std::thread::spawn(move || {
        for part in [&b"alpha-"[..], b"beta-", b"gamma"] {
            sender.write(part.to_vec()).unwrap();
            std::thread::sleep(Duration::from_millis(40));
        }
        sender.close();
    });
    let resp = service().execute_blocking(req).expect("request");
    let text = resp.body.read_all(u64::MAX).unwrap();
    producer.join().unwrap();
    assert_eq!(text, b"got 16");
    let seen = &srv.requests_for("/upload")[0];
    assert_eq!(seen.body, b"alpha-beta-gamma");
    assert_eq!(seen.header("transfer-encoding"), Some("chunked"));
    assert!(seen.body_chunks > 1, "chunks {}", seen.body_chunks);
}

#[test]
fn channel_body_error_or_drop_fails_the_request() {
    let srv = TestServer::spawn(|_| TestResponse::ok("ok", "text/plain"));
    let svc = service();

    let (sender, body) = StreamingBody::channel();
    let mut req = post(&srv, "/errored");
    req.body = RequestBody::Stream(body);
    sender.write(b"partial".to_vec()).unwrap();
    sender.error("producer failed");
    assert!(svc.execute_blocking(req).is_err());

    let (sender, body) = StreamingBody::channel();
    let mut req = post(&srv, "/dropped");
    req.body = RequestBody::Stream(body);
    drop(sender);
    assert!(svc.execute_blocking(req).is_err());
}

#[test]
fn writes_after_the_body_is_gone_are_rejected() {
    let (sender, body) = StreamingBody::channel();
    drop(body);
    assert_eq!(sender.write(b"late".to_vec()), Err(NetworkError::Cancelled));
    assert!(!sender.is_open());
}

#[test]
fn flow_window_pauses_the_worker_until_the_consumer_releases() {
    const CHUNK: usize = 64 * 1024;
    const CHUNKS: usize = 256;
    const WINDOW: u64 = 256 * 1024;
    let total = (CHUNK * CHUNKS) as u64;
    let srv = TestServer::spawn(|_| TestResponse::Chunked {
        status: 200,
        headers: vec![
            ("Content-Type".into(), "application/octet-stream".into()),
            ("Cache-Control".into(), "no-store".into()),
        ],
        chunks: (0..CHUNKS).map(|_| vec![3u8; CHUNK]).collect(),
        delay_ms: 0,
    });
    let svc = Arc::new(service());
    let config = SchedulerConfig::default();
    let chunk_size = config.chunk_size as u64;
    let sched = RequestScheduler::new(Arc::clone(&svc), config);
    let flow = FlowControl::new(WINDOW);
    let mut req = NetworkRequest::get(Url::parse(&srv.url("/big")).unwrap(), ResourceType::Fetch);
    req.flow = Some(flow.clone());
    let (tx, rx) = bounded(1024);
    sched.enqueue(req, tx).unwrap();

    // Consume nothing for a while: delivery must stop at the window.
    let mut received = 0u64;
    let stall = Instant::now() + Duration::from_millis(600);
    while Instant::now() < stall {
        if let Ok(NetworkEvent::Data { bytes, .. }) = rx.recv_timeout(Duration::from_millis(20)) {
            received += bytes.len() as u64;
        }
    }
    assert!(
        received <= WINDOW + chunk_size,
        "delivered {received} past a {WINDOW} window"
    );
    assert!(flow.outstanding() <= WINDOW + chunk_size);
    let written = srv.stream_bytes_written();
    assert!(
        written < total / 2,
        "server wrote {written} of {total} while the consumer was stalled"
    );

    // Release as we go: the whole body arrives.
    flow.release(received);
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut complete = false;
    while !complete && Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(NetworkEvent::Data { bytes, .. }) => {
                received += bytes.len() as u64;
                flow.release(bytes.len() as u64);
            }
            Ok(NetworkEvent::Complete { .. }) => complete = true,
            Ok(NetworkEvent::Failed { error, .. }) => panic!("failed: {error:?}"),
            _ => {}
        }
    }
    assert!(complete, "body did not finish after release");
    assert_eq!(received, total);
    assert_eq!(flow.outstanding(), 0);
}

#[test]
fn cancelling_a_paused_request_frees_its_worker() {
    let srv = TestServer::spawn(|_| TestResponse::Chunked {
        status: 200,
        headers: vec![("Cache-Control".into(), "no-store".into())],
        chunks: (0..128).map(|_| vec![1u8; 64 * 1024]).collect(),
        delay_ms: 0,
    });
    let svc = Arc::new(service());
    let sched = RequestScheduler::new(
        Arc::clone(&svc),
        SchedulerConfig {
            max_concurrent: 1,
            ..SchedulerConfig::default()
        },
    );
    let flow = FlowControl::new(64 * 1024);
    let mut paused = NetworkRequest::get(
        Url::parse(&srv.url("/paused")).unwrap(),
        ResourceType::Fetch,
    );
    paused.flow = Some(flow);
    paused.context_id = Some(7);
    let (tx, rx) = bounded(1024);
    sched.enqueue(paused, tx.clone()).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    sched.cancel_context(7);

    let next = NetworkRequest::get(Url::parse(&srv.url("/next")).unwrap(), ResourceType::Fetch);
    let next_id = next.id;
    sched.enqueue(next, tx).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut next_done = false;
    while !next_done && Instant::now() < deadline {
        if let Ok(NetworkEvent::Complete { id } | NetworkEvent::Failed { id, .. }) =
            rx.recv_timeout(Duration::from_millis(50))
        {
            next_done = id == next_id;
        }
    }
    assert!(
        next_done,
        "the single worker stayed parked on a cancelled request"
    );
}
