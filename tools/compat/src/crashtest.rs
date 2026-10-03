//! WPT crashtests: a test passes when Axiom loads it to the `load` event without
//! panicking. Not reaching `load` within the budget is a TIMEOUT.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axiom_engine::BrowsingContext;

use crate::expectations::{Outcome, Status};
use crate::runner::parallel_map;
use crate::wpt::{self, guarded, parent_dir, Harness, VIEWPORT};

pub const DIRS: &[&str] = &[
    "html/syntax/parsing/crashtests",
    "dom/nodes/crashtests",
    "css/css-backgrounds/crashtests",
    "css/css-color/crashtests",
    "css/css-flexbox/crashtests",
];

const BUDGET: Duration = Duration::from_secs(10);
const HARD_TIMEOUT: Duration = Duration::from_secs(45);

pub fn discover(root: &Path) -> Vec<String> {
    wpt::walk(&wpt::wpt_root(root), DIRS)
        .into_iter()
        .filter(|p| {
            let ext = p.rsplit('.').next().unwrap_or("");
            matches!(ext, "html" | "htm" | "xhtml" | "xht" | "svg")
                && !p.contains("/support/")
                && !p.contains("/resources/")
                && !p.contains("-ref.")
        })
        .collect()
}

pub fn run(root: &Path, jobs: usize, filter: Option<&str>) -> anyhow::Result<Vec<Outcome>> {
    let tests: Vec<String> = discover(root)
        .into_iter()
        .filter(|t| filter.is_none_or(|f| t.contains(f)))
        .collect();
    let harness = Harness::new(root);
    Ok(parallel_map(&tests, jobs, |rel| {
        let group = parent_dir(rel);
        let url = harness.server.url(rel);
        let scheduler = Arc::clone(&harness.scheduler);
        let result = guarded(rel, &group, HARD_TIMEOUT, move || {
            let mut ctx = BrowsingContext::headless(VIEWPORT.0, VIEWPORT.1, scheduler);
            wpt::load(&mut ctx, &url, BUDGET)
        });
        if harness.server.was_not_found(rel) {
            return Outcome::new(rel.as_str(), group, Status::Error)
                .with_message("test file was not served (404)");
        }
        match result {
            Err(o) => o,
            Ok(l) if l.load_fired => Outcome::new(rel.as_str(), group, Status::Pass),
            Ok(l) => Outcome::new(rel.as_str(), group, Status::Timeout).with_message(
                l.navigation_error
                    .unwrap_or_else(|| "load event did not fire".to_string()),
            ),
        }
    }))
}
