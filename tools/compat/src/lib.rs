//! Conformance harness for Axiom.
//!
//! Runs vendored, pinned conformance suites fully offline and compares every run with a
//! checked-in expectations file (see [`expectations`]):
//!
//! | Suite | Source | What passes |
//! |-------|--------|-------------|
//! | `html5lib-tree` | WPT `html/syntax/parsing/resources/*.dat` | parsed tree equals `#document` |
//! | `html5lib-tokenizer` | html5lib-tests `tokenizer/*.test` | token stream equals `output` |
//! | `crashtests` | WPT `*/crashtests/` | page reaches `load` without a panic |
//! | `reftests` | WPT CSS reftests | frame equals (or differs from) the reference |
//! | `testharness` | WPT `dom/{nodes,events,collections,lists}` testharness.js tests | harness OK / subtest PASS |
//!
//! Results are reported per group (file or directory) as `passed / total`; there is no
//! global percentage. Engine-driven suites load pages from a loopback server over the
//! vendored tree ([`server`]) through the same `BrowsingContext` path browser tabs use.

pub mod crashtest;
pub mod expectations;
pub mod html5lib_tokenizer;
pub mod html5lib_tree;
pub mod reftest;
pub mod report;
pub mod runner;
pub mod server;
pub mod testharness;
pub mod wpt;

use std::path::{Path, PathBuf};
use std::time::Instant;

use expectations::{Expectations, Outcome};
use report::SuiteReport;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Suite {
    Html5libTree,
    Html5libTokenizer,
    Crashtests,
    Reftests,
    Testharness,
}

impl Suite {
    pub const ALL: &'static [Suite] = &[
        Suite::Html5libTree,
        Suite::Html5libTokenizer,
        Suite::Crashtests,
        Suite::Reftests,
        Suite::Testharness,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Suite::Html5libTree => "html5lib-tree",
            Suite::Html5libTokenizer => "html5lib-tokenizer",
            Suite::Crashtests => "crashtests",
            Suite::Reftests => "reftests",
            Suite::Testharness => "testharness",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|s| s.name() == name)
    }

    pub fn expectations_path(self, root: &Path) -> PathBuf {
        root.join("tests")
            .join("expectations")
            .join(format!("{}.txt", self.name()))
    }

    fn run(self, root: &Path, opts: &RunOptions) -> anyhow::Result<Vec<Outcome>> {
        let filter = opts.filter.as_deref();
        match self {
            Suite::Html5libTree => html5lib_tree::run(root, opts.jobs, filter),
            Suite::Html5libTokenizer => html5lib_tokenizer::run(root, opts.jobs, filter),
            Suite::Crashtests => crashtest::run(root, opts.jobs, filter),
            Suite::Reftests => reftest::run(root, opts.jobs, filter),
            Suite::Testharness => testharness::run(root, opts.jobs, filter),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub jobs: usize,
    /// Only run tests whose id contains this substring (the ratchet then only checks
    /// those ids, and the total is not compared).
    pub filter: Option<String>,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            jobs: runner::default_jobs(),
            filter: None,
        }
    }
}

/// Repository root (where `tests/wpt` and `tests/expectations` live).
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("tools/compat sits two levels below the workspace root")
        .to_path_buf()
}

/// Run `suite` and compare it with its expectations file.
pub fn run_suite(root: &Path, suite: Suite, opts: &RunOptions) -> anyhow::Result<SuiteReport> {
    let started = Instant::now();
    let outcomes = suite.run(root, opts)?;
    let mut expected = Expectations::load(&suite.expectations_path(root))?;
    if let Some(f) = &opts.filter {
        expected.entries.retain(|id, _| id.contains(f.as_str()));
        expected.total = None;
    }
    let ratchet = expected.compare(&outcomes);
    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
    Ok(SuiteReport::new(suite.name(), outcomes, ratchet, elapsed))
}

/// Write `report`'s outcomes as the new expectations baseline for `suite`.
pub fn update_expectations(root: &Path, suite: Suite, report: &SuiteReport) -> anyhow::Result<()> {
    let path = suite.expectations_path(root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, Expectations::render(suite.name(), &report.outcomes))?;
    Ok(())
}
