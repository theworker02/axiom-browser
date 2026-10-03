//! Local networking micro-benchmarks against the in-process test server.
//!
//! ```text
//! cargo run -p axiom-net --example netbench --release
//! ```
//!
//! Prints one JSON object on stdout. Loopback numbers measure Axiom's own overhead
//! (decoding, cache, scheduling, cancellation), not real-world network performance.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axiom_net::test_server::{TestResponse, TestServer};
use axiom_net::{
    CacheState, CancellationToken, NetworkError, NetworkEvent, NetworkRequest, NetworkService,
    NetworkServiceConfig, RequestScheduler, ResourceType, SchedulerConfig,
};
use axiom_url::Url;
use crossbeam_channel::bounded;

#[path = "../tests/support/tls_server.rs"]
mod tls_server;

fn get(u: &str) -> NetworkRequest {
    NetworkRequest::get(Url::parse(u).expect("url"), ResourceType::Other)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1_000.0
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx]
}

fn throughput() -> String {
    const SIZE: usize = 16 * 1024 * 1024;
    let srv = TestServer::spawn(|_| {
        TestResponse::ok(vec![0x5au8; SIZE], "application/octet-stream")
            .with_header("Cache-Control", "no-store")
    });
    let svc = NetworkService::new(NetworkServiceConfig::default());
    let t = Instant::now();
    let resp = svc
        .execute_blocking(get(&srv.url("/big")))
        .expect("request");
    let body = resp.body.read_all(u64::MAX).expect("body");
    let elapsed = t.elapsed();
    assert_eq!(body.len(), SIZE);
    format!(
        r#"{{"bytes":{SIZE},"ms":{:.2},"mib_per_s":{:.1}}}"#,
        ms(elapsed),
        SIZE as f64 / (1024.0 * 1024.0) / elapsed.as_secs_f64()
    )
}

fn concurrency() -> String {
    const REQUESTS: usize = 36;
    const DELAY_MS: u64 = 100;
    let srv = TestServer::spawn(|_| {
        TestResponse::ok("x", "text/plain")
            .with_header("Cache-Control", "no-store")
            .delayed(DELAY_MS)
    });
    let svc = Arc::new(NetworkService::new(NetworkServiceConfig::default()));
    let sched = RequestScheduler::new(Arc::clone(&svc), SchedulerConfig::default());
    let limit = sched.config().max_concurrent;
    let (tx, rx) = bounded(256);
    let t = Instant::now();
    for i in 0..REQUESTS {
        sched
            .enqueue(get(&srv.url(&format!("/r{i}"))), tx.clone())
            .expect("enqueue");
    }
    let mut done = 0;
    while done < REQUESTS {
        match rx.recv_timeout(Duration::from_secs(30)).expect("event") {
            NetworkEvent::Complete { .. } => done += 1,
            NetworkEvent::Failed { error, .. } => panic!("request failed: {error}"),
            _ => {}
        }
    }
    let elapsed = t.elapsed();
    let serial_ms = (REQUESTS as u64 * DELAY_MS) as f64;
    let out = format!(
        r#"{{"requests":{REQUESTS},"server_delay_ms":{DELAY_MS},"limit":{limit},"wall_ms":{:.1},"serial_ms":{serial_ms:.0},"server_peak_concurrency":{},"scheduler_peak_in_flight":{}}}"#,
        ms(elapsed),
        srv.peak_concurrency(),
        sched.stats().peak_in_flight
    );
    sched.shutdown();
    out
}

fn cache_hit_latency() -> String {
    const ITERATIONS: usize = 500;
    let srv = TestServer::spawn(|_| {
        TestResponse::ok(vec![b'c'; 32 * 1024], "text/css")
            .with_header("Cache-Control", "max-age=3600")
    });
    let svc = NetworkService::new(NetworkServiceConfig::default());
    let prime = svc
        .execute_blocking(get(&srv.url("/style.css")))
        .expect("prime");
    prime.body.read_all(u64::MAX).expect("prime body");

    let mut samples = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let t = Instant::now();
        let resp = svc
            .execute_blocking(get(&srv.url("/style.css")))
            .expect("hit");
        let body = resp.body.read_all(u64::MAX).expect("hit body");
        samples.push(t.elapsed().as_secs_f64() * 1_000_000.0);
        assert_eq!(resp.meta.cache_state, CacheState::Hit);
        assert_eq!(body.len(), 32 * 1024);
    }
    assert_eq!(srv.request_count(), 1, "cache hits reached the server");
    samples.sort_by(|a, b| a.total_cmp(b));
    format!(
        r#"{{"iterations":{ITERATIONS},"body_bytes":32768,"p50_us":{:.1},"p95_us":{:.1},"max_us":{:.1},"server_requests":{}}}"#,
        percentile(&samples, 0.5),
        percentile(&samples, 0.95),
        samples[samples.len() - 1],
        srv.request_count()
    )
}

fn streaming() -> String {
    const CHUNKS: usize = 20;
    const CHUNK_DELAY_MS: u64 = 25;
    let srv = TestServer::spawn(|_| TestResponse::Chunked {
        status: 200,
        headers: vec![("Content-Type".into(), "application/octet-stream".into())],
        chunks: (0..CHUNKS).map(|_| vec![1u8; 8 * 1024]).collect(),
        delay_ms: CHUNK_DELAY_MS,
    });
    let svc = Arc::new(NetworkService::new(NetworkServiceConfig::default()));
    let sched = RequestScheduler::new(Arc::clone(&svc), SchedulerConfig::default());
    let (tx, rx) = bounded(4);
    let t = Instant::now();
    sched
        .enqueue(get(&srv.url("/stream")), tx)
        .expect("enqueue");
    let mut first_data = None;
    let mut bytes = 0usize;
    loop {
        match rx.recv_timeout(Duration::from_secs(30)).expect("event") {
            NetworkEvent::Data { bytes: b, .. } => {
                first_data.get_or_insert_with(|| t.elapsed());
                bytes += b.len();
            }
            NetworkEvent::Complete { .. } => break,
            NetworkEvent::Failed { error, .. } => panic!("stream failed: {error}"),
            _ => {}
        }
    }
    let total = t.elapsed();
    sched.shutdown();
    format!(
        r#"{{"chunks":{CHUNKS},"chunk_delay_ms":{CHUNK_DELAY_MS},"bytes":{bytes},"first_data_ms":{:.1},"complete_ms":{:.1}}}"#,
        ms(first_data.expect("no data events")),
        ms(total)
    )
}

fn cancellation_latency() -> String {
    const RUNS: usize = 10;
    let srv = TestServer::spawn(|_| TestResponse::Chunked {
        status: 200,
        headers: vec![("Content-Type".into(), "application/octet-stream".into())],
        chunks: (0..200).map(|_| vec![7u8; 16 * 1024]).collect(),
        delay_ms: 20,
    });
    let svc = NetworkService::new(NetworkServiceConfig::default());
    let mut samples = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let cancel = CancellationToken::new();
        let resp = svc
            .execute_with_cancel(get(&srv.url("/stream")), &cancel)
            .expect("request");
        resp.body.read_chunk(16 * 1024).expect("first chunk");
        let t = Instant::now();
        cancel.cancel();
        let err = loop {
            match resp.body.read_chunk(16 * 1024) {
                Ok(c) if c.is_empty() => panic!("stream finished despite cancel"),
                Ok(_) => continue,
                Err(e) => break e,
            }
        };
        samples.push(t.elapsed().as_secs_f64() * 1_000_000.0);
        assert_eq!(err, NetworkError::Cancelled);
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while (srv.streams_aborted() as usize) < RUNS && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    samples.sort_by(|a, b| a.total_cmp(b));
    format!(
        r#"{{"runs":{RUNS},"p50_us":{:.1},"max_us":{:.1},"server_streams_aborted":{},"server_streams_completed":{}}}"#,
        percentile(&samples, 0.5),
        samples[samples.len() - 1],
        srv.streams_aborted(),
        srv.streams_completed()
    )
}

/// Cold: a fresh service per sample (TCP + TLS handshake + ALPN). Warm: further requests
/// on the same service reuse the HTTP/2 connection.
fn https_cold_warm() -> String {
    const COLD: usize = 10;
    const WARM: usize = 50;
    let server = tls_server::spawn_tls_server();
    let mut cold = Vec::with_capacity(COLD);
    let mut warm = Vec::with_capacity(WARM);
    for i in 0..COLD {
        let svc = tls_server::client(&server, true, false);
        let t = Instant::now();
        let resp = svc.execute_blocking(get(&server.url("/"))).expect("https");
        resp.body.read_all(1024).expect("body");
        cold.push(ms(t.elapsed()));
        if i == 0 {
            for _ in 0..WARM {
                let t = Instant::now();
                let resp = svc.execute_blocking(get(&server.url("/"))).expect("https");
                resp.body.read_all(1024).expect("body");
                warm.push(ms(t.elapsed()));
            }
        }
    }
    cold.sort_by(|a, b| a.total_cmp(b));
    warm.sort_by(|a, b| a.total_cmp(b));
    format!(
        r#"{{"cold_samples":{COLD},"cold_p50_ms":{:.2},"cold_max_ms":{:.2},"warm_samples":{WARM},"warm_p50_ms":{:.3},"warm_p95_ms":{:.3},"server_connections":{}}}"#,
        percentile(&cold, 0.5),
        cold[cold.len() - 1],
        percentile(&warm, 0.5),
        percentile(&warm, 0.95),
        server.connections.load(std::sync::atomic::Ordering::SeqCst)
    )
}

fn redirect_chain() -> String {
    const RUNS: usize = 50;
    let srv = TestServer::spawn_standard();
    let svc = NetworkService::new(NetworkServiceConfig::default());
    let mut samples = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let t = Instant::now();
        let resp = svc
            .execute_blocking(get(&srv.url("/redirect-chain")))
            .expect("chain");
        resp.body.read_all(1024).expect("body");
        samples.push(ms(t.elapsed()));
    }
    samples.sort_by(|a, b| a.total_cmp(b));
    format!(
        r#"{{"runs":{RUNS},"hops":6,"p50_ms":{:.3},"p95_ms":{:.3},"server_requests":{}}}"#,
        percentile(&samples, 0.5),
        percentile(&samples, 0.95),
        srv.request_count()
    )
}

/// Hit latency from a disk-backed cache after a restart (body read from `objects/`).
fn disk_cache_after_restart() -> String {
    const ITERATIONS: usize = 200;
    const BODY: usize = 256 * 1024;
    let srv = TestServer::spawn(|_| {
        TestResponse::ok(vec![b'd'; BODY], "application/octet-stream")
            .with_header("Cache-Control", "max-age=3600")
    });
    let dir = std::env::temp_dir().join(format!("axiom-netbench-cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let config = || NetworkServiceConfig {
        cache_dir: Some(dir.clone()),
        ..NetworkServiceConfig::default()
    };
    {
        let svc = NetworkService::new(config());
        let resp = svc.execute_blocking(get(&srv.url("/blob"))).expect("prime");
        resp.body.read_all(u64::MAX).expect("prime body");
        svc.shutdown();
    }
    let t = Instant::now();
    let svc = NetworkService::new(config());
    let open_ms = ms(t.elapsed());
    let mut samples = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let t = Instant::now();
        let resp = svc.execute_blocking(get(&srv.url("/blob"))).expect("hit");
        let body = resp.body.read_all(u64::MAX).expect("hit body");
        samples.push(t.elapsed().as_secs_f64() * 1_000_000.0);
        assert_eq!(resp.meta.cache_state, CacheState::Hit);
        assert_eq!(body.len(), BODY);
    }
    assert_eq!(srv.request_count(), 1, "disk cache hits reached the server");
    drop(svc);
    let _ = std::fs::remove_dir_all(&dir);
    samples.sort_by(|a, b| a.total_cmp(b));
    format!(
        r#"{{"iterations":{ITERATIONS},"body_bytes":{BODY},"open_ms":{open_ms:.2},"p50_us":{:.1},"p95_us":{:.1},"server_requests":{}}}"#,
        percentile(&samples, 0.5),
        percentile(&samples, 0.95),
        srv.request_count()
    )
}

fn main() {
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let results = [
        ("throughput", throughput()),
        ("concurrency", concurrency()),
        ("cache_hit_latency", cache_hit_latency()),
        ("streaming", streaming()),
        ("cancellation_latency", cancellation_latency()),
        ("https_cold_warm", https_cold_warm()),
        ("redirect_chain", redirect_chain()),
        ("disk_cache_after_restart", disk_cache_after_restart()),
    ];
    let body = results
        .iter()
        .map(|(k, v)| format!(r#"  "{k}": {v}"#))
        .collect::<Vec<_>>()
        .join(",\n");
    println!(
        "{{\n  \"profile\": \"{profile}\",\n  \"transport\": \"loopback http/1.1 + local TLS (h2)\",\n{body}\n}}"
    );
}
