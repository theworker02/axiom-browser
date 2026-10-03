//! Expectations ratchet.
//!
//! An expectations file lists every test that is *not* expected to pass, one per line:
//!
//! ```text
//! # axiom-compat expectations: html5lib-tree
//! # total: 1791
//! FAIL tests1.dat:3
//! CRASH tests9.dat:12
//! ```
//!
//! Tests that are not listed are expected to PASS. A run is compared against the file and
//! *any* difference fails it: a regression (expected PASS, got something else), an
//! unexpected pass (listed, now passes), a status change between two failing statuses, a
//! stale entry (listed but no longer produced) or a changed total. Progress therefore
//! always shows up as a reviewed diff of this file (`--update`), and it can only move in
//! the direction someone looked at.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Status {
    Pass,
    Fail,
    /// The test page or harness failed before producing results (testharness ERROR).
    Error,
    Timeout,
    /// A panic in Axiom while running the test.
    Crash,
    /// A testharness subtest that never ran.
    NotRun,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pass => "PASS",
            Status::Fail => "FAIL",
            Status::Error => "ERROR",
            Status::Timeout => "TIMEOUT",
            Status::Crash => "CRASH",
            Status::NotRun => "NOTRUN",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "PASS" => Status::Pass,
            "FAIL" => Status::Fail,
            "ERROR" => Status::Error,
            "TIMEOUT" => Status::Timeout,
            "CRASH" => Status::Crash,
            "NOTRUN" => Status::NotRun,
            _ => return None,
        })
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Result of one test (or one testharness subtest).
#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    /// Stable identifier, unique within the suite (no newlines).
    pub id: String,
    /// Report bucket, e.g. the `.dat` file or the test's directory.
    pub group: String,
    pub status: Status,
    /// Short reason for a non-pass (first differing line, harness message, panic text).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub message: String,
}

impl Outcome {
    pub fn new(id: impl Into<String>, group: impl Into<String>, status: Status) -> Self {
        Self {
            id: sanitize_id(&id.into()),
            group: group.into(),
            status,
            message: String::new(),
        }
    }

    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }
}

/// Escapes `\` and control characters so every id is one line and distinct names stay
/// distinct (WPT subtest names differ by tabs, CRs and the like).
fn sanitize_id(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    for c in id.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[derive(Debug, Clone, Default)]
pub struct Expectations {
    /// Non-passing tests and their expected status.
    pub entries: BTreeMap<String, Status>,
    /// Number of outcomes the recorded run produced.
    pub total: Option<usize>,
}

impl Expectations {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(anyhow::anyhow!("read {}: {e}", path.display())),
        }
    }

    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let mut out = Self::default();
        for (n, line) in text.lines().enumerate() {
            if let Some(total) = line.strip_prefix("# total: ") {
                out.total = Some(total.trim().parse()?);
                continue;
            }
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (status, id) = line
                .split_once(' ')
                .ok_or_else(|| anyhow::anyhow!("line {}: expected `STATUS id`", n + 1))?;
            let status = Status::parse(status)
                .ok_or_else(|| anyhow::anyhow!("line {}: unknown status {status}", n + 1))?;
            if status == Status::Pass {
                anyhow::bail!("line {}: PASS is implied; remove the entry", n + 1);
            }
            if out.entries.insert(id.to_string(), status).is_some() {
                anyhow::bail!("line {}: duplicate id {id}", n + 1);
            }
        }
        Ok(out)
    }

    /// The file that records `outcomes` as the new baseline.
    pub fn render(suite: &str, outcomes: &[Outcome]) -> String {
        let mut failing: Vec<&Outcome> = outcomes
            .iter()
            .filter(|o| o.status != Status::Pass)
            .collect();
        failing.sort_by(|a, b| a.id.cmp(&b.id));
        let mut out = format!(
            "# axiom-compat expectations: {suite}\n\
             # Generated by `cargo run -p axiom-compat -- {suite} --update`; review the diff.\n\
             # Tests not listed are expected to PASS.\n\
             # total: {}\n",
            outcomes.len()
        );
        for o in failing {
            out.push_str(o.status.as_str());
            out.push(' ');
            out.push_str(&o.id);
            out.push('\n');
        }
        out
    }

    pub fn compare(&self, outcomes: &[Outcome]) -> Ratchet {
        let mut r = Ratchet::default();
        let mut seen = std::collections::HashSet::new();
        for o in outcomes {
            if !seen.insert(o.id.as_str()) {
                r.duplicate_ids.push(o.id.clone());
                continue;
            }
            let expected = self.entries.get(&o.id).copied().unwrap_or(Status::Pass);
            match (expected, o.status) {
                (e, g) if e == g => {}
                (Status::Pass, got) => r.regressions.push(Change::new(o, expected, got)),
                (_, Status::Pass) => r.unexpected_passes.push(Change::new(o, expected, o.status)),
                (_, got) => r.status_changes.push(Change::new(o, expected, got)),
            }
        }
        r.stale = self
            .entries
            .keys()
            .filter(|id| !seen.contains(id.as_str()))
            .cloned()
            .collect();
        if let Some(total) = self.total {
            if total != outcomes.len() {
                r.total_changed = Some((total, outcomes.len()));
            }
        }
        r
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Change {
    pub id: String,
    pub expected: Status,
    pub got: Status,
    pub message: String,
}

impl Change {
    fn new(o: &Outcome, expected: Status, got: Status) -> Self {
        Self {
            id: o.id.clone(),
            expected,
            got,
            message: o.message.clone(),
        }
    }
}

/// Differences between a run and its expectations. Any difference fails the run.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Ratchet {
    pub regressions: Vec<Change>,
    pub unexpected_passes: Vec<Change>,
    pub status_changes: Vec<Change>,
    pub stale: Vec<String>,
    pub duplicate_ids: Vec<String>,
    pub total_changed: Option<(usize, usize)>,
}

impl Ratchet {
    pub fn is_clean(&self) -> bool {
        self.regressions.is_empty()
            && self.unexpected_passes.is_empty()
            && self.status_changes.is_empty()
            && self.stale.is_empty()
            && self.duplicate_ids.is_empty()
            && self.total_changed.is_none()
    }

    /// Human-readable summary (at most `limit` lines per category).
    pub fn describe(&self, limit: usize) -> String {
        let mut out = String::new();
        let mut section = |title: &str, lines: Vec<String>| {
            if lines.is_empty() {
                return;
            }
            out.push_str(&format!("{title} ({}):\n", lines.len()));
            for l in lines.iter().take(limit) {
                out.push_str(&format!("  {l}\n"));
            }
            if lines.len() > limit {
                out.push_str(&format!("  ... {} more\n", lines.len() - limit));
            }
        };
        let fmt = |c: &Change| {
            let msg = if c.message.is_empty() {
                String::new()
            } else {
                format!(" — {}", truncate(&c.message, 160))
            };
            format!("{}: expected {}, got {}{msg}", c.id, c.expected, c.got)
        };
        section(
            "REGRESSIONS",
            self.regressions.iter().map(fmt).collect::<Vec<_>>(),
        );
        section(
            "UNEXPECTED PASSES (run with --update to lock them in)",
            self.unexpected_passes.iter().map(fmt).collect(),
        );
        section(
            "STATUS CHANGES",
            self.status_changes.iter().map(fmt).collect(),
        );
        section("STALE EXPECTATIONS", self.stale.clone());
        section("DUPLICATE IDS", self.duplicate_ids.clone());
        if let Some((was, now)) = self.total_changed {
            out.push_str(&format!(
                "TOTAL CHANGED: expectations recorded {was} outcomes, run produced {now}\n"
            ));
        }
        out
    }
}

pub fn truncate(s: &str, max: usize) -> String {
    let one_line = s.replace(['\n', '\r'], " ");
    if one_line.chars().count() <= max {
        return one_line;
    }
    let mut t: String = one_line.chars().take(max).collect();
    t.push('…');
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn o(id: &str, status: Status) -> Outcome {
        Outcome::new(id, "g", status)
    }

    #[test]
    fn render_then_parse_round_trips_and_is_clean() {
        let run = vec![
            o("a", Status::Pass),
            o("c", Status::Fail),
            o("b", Status::Timeout),
        ];
        let text = Expectations::render("s", &run);
        assert!(text.contains("FAIL c\n") && text.contains("TIMEOUT b\n"));
        assert!(!text.contains(" a\n"), "passes are implied");
        let exp = Expectations::parse(&text).unwrap();
        assert_eq!(exp.total, Some(3));
        assert!(exp.compare(&run).is_clean());
    }

    #[test]
    fn every_kind_of_difference_fails_the_ratchet() {
        let exp = Expectations::parse("# total: 4\nFAIL fixed\nFAIL crashes\nFAIL gone\n").unwrap();
        let run = vec![
            o("was-passing", Status::Fail),
            o("fixed", Status::Pass),
            o("crashes", Status::Crash),
        ];
        let r = exp.compare(&run);
        assert_eq!(r.regressions.len(), 1);
        assert_eq!(r.regressions[0].id, "was-passing");
        assert_eq!(r.unexpected_passes[0].id, "fixed");
        assert_eq!(r.status_changes[0].got, Status::Crash);
        assert_eq!(r.stale, ["gone"]);
        assert_eq!(r.total_changed, Some((4, 3)));
        assert!(!r.is_clean());
    }

    #[test]
    fn ids_with_spaces_and_newlines_survive() {
        let run = vec![o("file.html | a b\nc", Status::Fail)];
        let text = Expectations::render("s", &run);
        let exp = Expectations::parse(&text).unwrap();
        assert!(exp.compare(&run).is_clean());
        assert_eq!(exp.entries.keys().next().unwrap(), "file.html | a b\\nc");
    }

    #[test]
    fn malformed_files_are_rejected() {
        assert!(Expectations::parse("PASS x\n").is_err());
        assert!(Expectations::parse("BOGUS x\n").is_err());
        assert!(Expectations::parse("FAIL x\nFAIL x\n").is_err());
        assert!(Expectations::parse("FAIL\n").is_err());
    }

    #[test]
    fn duplicate_outcome_ids_fail_the_ratchet() {
        let r = Expectations::default().compare(&[o("x", Status::Pass), o("x", Status::Pass)]);
        assert_eq!(r.duplicate_ids, ["x"]);
    }
}
