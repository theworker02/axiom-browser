//! WPT testharness.js tests. The page loads the vendored `testharness.js`; the server
//! swaps in Axiom's `testharnessreport.js`, whose completion callback stores the results
//! as JSON in `window.__axiom_wpt_result`, which the runner reads back through the
//! page's JS context.
//!
//! Each file yields one file-level outcome (harness OK → PASS, ERROR, TIMEOUT, CRASH)
//! and one outcome per subtest (`<file> | <subtest name>`).

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axiom_engine::BrowsingContext;
use serde::Deserialize;

use crate::expectations::{Outcome, Status};
use crate::runner::parallel_map;
use crate::wpt::{self, guarded, parent_dir, Harness, VIEWPORT};

pub const DIRS: &[&str] = &["dom/nodes", "dom/events", "dom/collections", "dom/lists"];

/// Budget for the harness to report (testharness.js's own timeout is 10 s).
const BUDGET: Duration = Duration::from_secs(15);
const HARD_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Deserialize)]
struct HarnessResult {
    status: u32,
    message: Option<String>,
    tests: Vec<SubtestResult>,
}

#[derive(Debug, Deserialize)]
struct SubtestResult {
    name: String,
    status: u32,
    message: Option<String>,
}

/// Test pages: files that load testharness.js, plus `.window.js` / `.any.js` tests
/// (as their generated `.window.html` / `.any.html` wrappers; worker variants are not run).
pub fn discover(root: &Path) -> Vec<String> {
    let wpt = wpt::wpt_root(root);
    wpt::walk(&wpt, DIRS)
        .into_iter()
        .filter(|p| {
            !p.contains("/support/") && !p.contains("/resources/") && !p.contains("/crashtests/")
        })
        .filter_map(|p| {
            if let Some(base) = p.strip_suffix(".window.js") {
                return Some(format!("{base}.window.html"));
            }
            if let Some(base) = p.strip_suffix(".any.js") {
                return Some(format!("{base}.any.html"));
            }
            let ext = p.rsplit('.').next().unwrap_or("");
            if !matches!(ext, "html" | "htm" | "xhtml" | "xht" | "svg") || p.contains("-ref.") {
                return None;
            }
            let src = std::fs::read_to_string(wpt.join(&p)).ok()?;
            src.contains("/resources/testharness.js").then_some(p)
        })
        .collect()
}

enum PageResult {
    Reported(String),
    /// The page went idle (or the budget ran out) without the harness reporting.
    Silent {
        idle: bool,
        detail: String,
    },
}

fn run_page(scheduler: Arc<axiom_net::RequestScheduler>, url: &str) -> PageResult {
    let mut ctx = BrowsingContext::headless(VIEWPORT.0, VIEWPORT.1, scheduler);
    ctx.navigate(url);
    if let Some(e) = ctx.last_error.take() {
        return PageResult::Silent {
            idle: true,
            detail: format!("navigation failed: {e}"),
        };
    }
    let deadline = Instant::now() + BUDGET;
    loop {
        let idle = ctx.run_until_idle(Duration::from_millis(25));
        let result = ctx
            .page
            .js
            .as_mut()
            .and_then(|js| {
                js.eval("typeof __axiom_wpt_result === 'string' ? __axiom_wpt_result : ''")
                    .ok()
            })
            .map(|v| v.display)
            .unwrap_or_default();
        if !result.is_empty() {
            return PageResult::Reported(result);
        }
        if idle || Instant::now() >= deadline {
            let errors: Vec<String> = ctx
                .script_errors()
                .iter()
                .map(|e| {
                    format!(
                        "{}: {}",
                        e.label.rsplit('/').next().unwrap_or(&e.label),
                        e.message
                    )
                })
                .collect();
            let detail = if errors.is_empty() {
                "no script errors recorded".to_string()
            } else {
                errors.join(" | ")
            };
            return PageResult::Silent { idle, detail };
        }
    }
}

fn subtest_status(code: u32) -> Status {
    match code {
        0 => Status::Pass,
        2 => Status::Timeout,
        3 => Status::NotRun,
        _ => Status::Fail,
    }
}

pub fn run(root: &Path, jobs: usize, filter: Option<&str>) -> anyhow::Result<Vec<Outcome>> {
    let tests: Vec<String> = discover(root)
        .into_iter()
        .filter(|t| filter.is_none_or(|f| t.contains(f)))
        .collect();
    let harness = Harness::new(root);
    let per_file = parallel_map(&tests, jobs, |rel| {
        let group = parent_dir(rel);
        let url = harness.server.url(rel);
        let scheduler = Arc::clone(&harness.scheduler);
        let result = guarded(rel, &group, HARD_TIMEOUT, move || run_page(scheduler, &url));
        let file = |status| Outcome::new(rel.as_str(), group.clone(), status);
        if harness.server.was_not_found(rel) {
            return vec![file(Status::Error).with_message("test file was not served (404)")];
        }
        match result {
            Err(o) => vec![o],
            Ok(PageResult::Silent { idle: true, detail }) => {
                vec![file(Status::Error).with_message(format!("harness never reported: {detail}"))]
            }
            Ok(PageResult::Silent {
                idle: false,
                detail,
            }) => {
                vec![file(Status::Timeout).with_message(format!(
                    "harness did not report within {} s: {detail}",
                    BUDGET.as_secs()
                ))]
            }
            Ok(PageResult::Reported(json)) => match serde_json::from_str::<HarnessResult>(&json) {
                Err(e) => {
                    vec![file(Status::Error).with_message(format!("unreadable harness result: {e}"))]
                }
                Ok(r) => {
                    let status = match r.status {
                        0 => Status::Pass,
                        2 => Status::Timeout,
                        3 => Status::Fail,
                        _ => Status::Error,
                    };
                    let mut out = vec![file(status).with_message(r.message.unwrap_or_default())];
                    let mut seen = std::collections::HashMap::<String, usize>::new();
                    for t in r.tests {
                        let n = seen.entry(t.name.clone()).or_default();
                        *n += 1;
                        let name = if *n > 1 {
                            format!("{} ({})", t.name, n)
                        } else {
                            t.name
                        };
                        out.push(
                            Outcome::new(
                                format!("{rel} | {}", cap_chars(&name, 300)),
                                format!("{group} [subtests]"),
                                subtest_status(t.status),
                            )
                            .with_message(t.message.unwrap_or_default()),
                        );
                    }
                    out
                }
            },
        }
    });
    Ok(per_file.into_iter().flatten().collect())
}

/// The first `max` characters of `s`, unchanged otherwise.
fn cap_chars(s: &str, max: usize) -> &str {
    s.char_indices().nth(max).map_or(s, |(i, _)| &s[..i])
}
