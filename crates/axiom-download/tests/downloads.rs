//! Download manager against the local test server: streaming to disk, safe names,
//! pause/resume with ranges, cancel, failures, private cleanup and the concurrency cap.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axiom_download::{DownloadConfig, DownloadId, DownloadManager, DownloadRecord, DownloadState};
use axiom_net::test_server::{download_body, TestServer, DOWNLOAD_LEN};
use axiom_net::{NetworkService, NetworkServiceConfig, RequestScheduler, SchedulerConfig};
use axiom_url::Url;

const WAIT: Duration = Duration::from_secs(20);

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "axiom-dl-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        Self(p)
    }

    fn entries(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(&self.0)
            .map(|d| {
                d.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scheduler() -> Arc<RequestScheduler> {
    Arc::new(RequestScheduler::new(
        Arc::new(NetworkService::new(NetworkServiceConfig::default())),
        SchedulerConfig::default(),
    ))
}

fn manager(dir: &Path, private: bool, max_concurrent: usize) -> DownloadManager {
    let mut config = DownloadConfig::new(dir);
    config.private = private;
    config.max_concurrent = max_concurrent;
    DownloadManager::new(scheduler(), config)
}

fn url(srv: &TestServer, path: &str) -> Url {
    Url::parse(&srv.url(path)).unwrap()
}

fn wait_for_progress(m: &DownloadManager, id: DownloadId) -> DownloadRecord {
    let deadline = Instant::now() + WAIT;
    loop {
        let r = m.get(id).unwrap();
        if r.state == DownloadState::Downloading && r.downloaded > 0 {
            return r;
        }
        assert!(Instant::now() < deadline, "no progress: {r:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn download_streams_to_disk_with_a_sanitized_name() {
    let srv = TestServer::spawn_standard();
    let dir = TempDir::new("basic");
    let m = manager(&dir.0, false, 3);
    let id = m.start(url(&srv, "/download"), None);
    let r = m.wait_until_settled(id, WAIT).unwrap();
    assert_eq!(r.state, DownloadState::Completed, "{r:?}");
    // Content-Disposition said "../../evil name.bin": only the last component survives.
    assert_eq!(r.filename.as_deref(), Some("evil name.bin"));
    let dest = r.destination.clone().unwrap();
    assert_eq!(dest.parent(), Some(dir.0.as_path()));
    assert_eq!(std::fs::read(&dest).unwrap(), download_body());
    assert_eq!(r.downloaded, DOWNLOAD_LEN as u64);
    assert_eq!(r.expected_size, Some(DOWNLOAD_LEN as u64));
    assert_eq!(r.mime.as_deref(), Some("application/octet-stream"));
    assert!(r.resumable);
    assert!(r.request_id.is_some());
    assert!(r.completed_at_unix_ms.is_some());
    assert_eq!(dir.entries(), vec!["evil name.bin".to_string()]);
}

#[test]
fn duplicate_names_are_made_unique() {
    let srv = TestServer::spawn_standard();
    let dir = TempDir::new("dupe");
    let m = manager(&dir.0, false, 1);
    let a = m.start(url(&srv, "/download"), None);
    let b = m.start(url(&srv, "/download"), None);
    assert_eq!(
        m.wait_until_settled(a, WAIT).unwrap().state,
        DownloadState::Completed
    );
    assert_eq!(
        m.wait_until_settled(b, WAIT).unwrap().state,
        DownloadState::Completed
    );
    assert_eq!(
        dir.entries(),
        vec!["evil name (1).bin".to_string(), "evil name.bin".to_string()]
    );
}

#[test]
fn pause_and_resume_continue_with_a_range_request() {
    let srv = TestServer::spawn_standard();
    let dir = TempDir::new("pause");
    let m = manager(&dir.0, false, 3);
    let id = m.start(url(&srv, "/download/slow"), None);
    let r = wait_for_progress(&m, id);
    assert!(r.resumable);
    assert!(m.pause(id));
    let paused = m.wait_for_state(id, DownloadState::Paused, WAIT).unwrap();
    assert_eq!(paused.state, DownloadState::Paused);
    assert!(paused.downloaded > 0 && paused.downloaded < DOWNLOAD_LEN as u64);
    assert!(dir.entries().iter().any(|e| e.ends_with(".axiomdownload")));

    assert!(m.resume(id));
    let done = m
        .wait_for_state(id, DownloadState::Completed, WAIT)
        .unwrap();
    assert_eq!(done.state, DownloadState::Completed, "{done:?}");
    assert_eq!(
        std::fs::read(done.destination.unwrap()).unwrap(),
        download_body()
    );
    let ranged = srv
        .requests_for("/download/slow")
        .into_iter()
        .find(|r| r.header("range").is_some())
        .expect("resume sent a Range request");
    assert_eq!(
        ranged.header("range"),
        Some(format!("bytes={}-", paused.downloaded).as_str())
    );
    assert_eq!(ranged.header("if-range"), Some("\"dl-1\""));
    assert_eq!(dir.entries(), vec!["evil name.bin".to_string()]);
}

#[test]
fn non_resumable_downloads_cannot_pause() {
    let srv = TestServer::spawn(|_| axiom_net::test_server::TestResponse::Chunked {
        status: 200,
        headers: vec![(
            "Content-Disposition".into(),
            "attachment; filename=plain.txt".into(),
        )],
        chunks: vec![vec![1u8; 1024]; 50],
        delay_ms: 20,
    });
    let dir = TempDir::new("nopause");
    let m = manager(&dir.0, false, 3);
    let id = m.start(url(&srv, "/file"), None);
    let r = wait_for_progress(&m, id);
    assert!(!r.resumable);
    assert!(!m.pause(id));
    assert_eq!(
        m.wait_until_settled(id, WAIT).unwrap().state,
        DownloadState::Completed
    );
    assert_eq!(dir.entries(), vec!["plain.txt".to_string()]);
}

#[test]
fn cancel_removes_the_partial_file() {
    let srv = TestServer::spawn_standard();
    let dir = TempDir::new("cancel");
    let m = manager(&dir.0, false, 3);
    let id = m.start(url(&srv, "/download/slow"), None);
    wait_for_progress(&m, id);
    m.cancel(id);
    let r = m.wait_until_settled(id, WAIT).unwrap();
    assert_eq!(r.state, DownloadState::Canceled);
    assert!(dir.entries().is_empty(), "{:?}", dir.entries());
}

#[test]
fn http_errors_fail_without_leaving_files() {
    let srv = TestServer::spawn_standard();
    let dir = TempDir::new("fail");
    let m = manager(&dir.0, false, 3);
    let id = m.start(url(&srv, "/status/404"), None);
    let r = m.wait_until_settled(id, WAIT).unwrap();
    assert_eq!(r.state, DownloadState::Failed);
    assert!(r.error.as_deref().unwrap().contains("404"), "{r:?}");
    assert!(dir.entries().is_empty());
}

#[test]
fn concurrency_is_capped() {
    let srv = TestServer::spawn_standard();
    let dir = TempDir::new("cap");
    let m = manager(&dir.0, false, 1);
    let a = m.start(url(&srv, "/download/slow"), None);
    let b = m.start(url(&srv, "/download/slow"), None);
    wait_for_progress(&m, a);
    assert_eq!(m.get(b).unwrap().state, DownloadState::Queued);
    m.cancel(a);
    wait_for_progress(&m, b);
    m.cancel(b);
    assert_eq!(
        m.wait_until_settled(b, WAIT).unwrap().state,
        DownloadState::Canceled
    );
}

#[test]
fn private_shutdown_forgets_records_and_deletes_partials() {
    let srv = TestServer::spawn_standard();
    let dir = TempDir::new("private");
    let mut m = manager(&dir.0, true, 3);
    let id = m.start(url(&srv, "/download/slow"), None);
    wait_for_progress(&m, id);
    m.shutdown();
    assert!(m.list().is_empty());
    assert!(dir.entries().is_empty(), "{:?}", dir.entries());
}

#[test]
fn normal_shutdown_keeps_completed_files_and_records() {
    let srv = TestServer::spawn_standard();
    let dir = TempDir::new("normal");
    let mut m = manager(&dir.0, false, 3);
    let done = m.start(url(&srv, "/download"), None);
    m.wait_until_settled(done, WAIT).unwrap();
    let running = m.start(url(&srv, "/download/slow"), None);
    wait_for_progress(&m, running);
    m.shutdown();
    let list = m.list();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].state, DownloadState::Completed);
    assert_eq!(list[1].state, DownloadState::Canceled);
    assert_eq!(dir.entries(), vec!["evil name.bin".to_string()]);
}
