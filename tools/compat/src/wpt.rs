//! Shared plumbing for the engine-driven WPT suites: test discovery, the network stack,
//! and loading one page in a headless [`BrowsingContext`].

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axiom_engine::BrowsingContext;
use axiom_net::{NetworkService, NetworkServiceConfig, RequestScheduler, SchedulerConfig};

use crate::expectations::{Outcome, Status};
use crate::runner::{isolated, Abnormal};
use crate::server::WptServer;

/// WPT's reftest and testharness viewport.
pub const VIEWPORT: (u32, u32) = (800, 600);

pub fn wpt_root(root: &Path) -> PathBuf {
    root.join("tests").join("wpt")
}

/// The network stack shared by every page of one suite run (loopback only).
pub fn scheduler() -> Arc<RequestScheduler> {
    let service = Arc::new(NetworkService::new(NetworkServiceConfig::default()));
    Arc::new(RequestScheduler::new(service, SchedulerConfig::default()))
}

/// Everything the suite runs against: the server and the network stack.
pub struct Harness {
    pub server: WptServer,
    pub scheduler: Arc<RequestScheduler>,
}

impl Harness {
    pub fn new(root: &Path) -> Self {
        Self {
            server: WptServer::spawn(wpt_root(root)),
            scheduler: scheduler(),
        }
    }

    /// With `AXIOM_COMPAT_DEBUG` set, list every path the server answered 404.
    pub fn report_not_found(&self) {
        if std::env::var_os("AXIOM_COMPAT_DEBUG").is_some() {
            for p in self.server.not_found_paths() {
                eprintln!("404: {p}");
            }
        }
    }
}

/// Files under `dirs` (relative to the WPT root), recursively, as `/`-separated paths
/// relative to the WPT root, sorted.
pub fn walk(wpt: &Path, dirs: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = dirs.iter().map(|d| wpt.join(d)).collect();
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.filter_map(Result::ok) {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(rel) = p.strip_prefix(wpt) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out.sort();
    out
}

pub fn parent_dir(rel: &str) -> String {
    rel.rsplit_once('/')
        .map_or(String::new(), |(d, _)| d.to_string())
}

/// Result of loading a page to completion.
#[derive(Debug, Clone)]
pub struct Loaded {
    /// The event loop went idle within the budget.
    pub idle: bool,
    pub load_fired: bool,
    pub navigation_error: Option<String>,
    pub script_errors: Vec<String>,
}

/// Navigate a fresh headless context to `url` and run it until idle or `budget`.
pub fn load(ctx: &mut BrowsingContext, url: &str, budget: Duration) -> Loaded {
    ctx.navigate(url);
    let navigation_error = ctx.last_error.take();
    let idle = ctx.run_until_idle(budget);
    ctx.page.update_rendering_if_needed();
    Loaded {
        idle,
        load_fired: ctx.document_lifecycle().is_some_and(|l| l.load_fired),
        navigation_error,
        script_errors: ctx
            .script_errors()
            .iter()
            .map(|e| format!("{}: {}", e.label, e.message))
            .collect(),
    }
}

/// Run `f` isolated (panic → CRASH, over `hard_timeout` → TIMEOUT).
pub fn guarded<R, F>(id: &str, group: &str, hard_timeout: Duration, f: F) -> Result<R, Outcome>
where
    R: Send + 'static,
    F: FnOnce() -> R + Send + 'static,
{
    let short: String = id
        .rsplit('/')
        .next()
        .unwrap_or(id)
        .chars()
        .take(40)
        .collect();
    isolated(&short, hard_timeout, f).map_err(|a| match a {
        Abnormal::Crash(msg) => Outcome::new(id, group, Status::Crash).with_message(msg),
        Abnormal::Timeout => Outcome::new(id, group, Status::Timeout)
            .with_message(format!("no result within {} s", hard_timeout.as_secs())),
    })
}
