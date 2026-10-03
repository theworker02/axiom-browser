//! Per-directory pass counts. There is deliberately no global percentage: every number
//! is `passed / total` for one bucket of one suite.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::expectations::{Outcome, Ratchet, Status};

#[derive(Debug, Clone, Default, Serialize)]
pub struct GroupCounts {
    pub passed: usize,
    pub total: usize,
    /// Non-pass statuses and how often they occurred.
    pub other: BTreeMap<Status, usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SuiteReport {
    pub suite: String,
    pub passed: usize,
    pub total: usize,
    pub groups: BTreeMap<String, GroupCounts>,
    pub elapsed_ms: f64,
    pub ratchet: Ratchet,
    pub outcomes: Vec<Outcome>,
}

impl SuiteReport {
    pub fn new(suite: &str, outcomes: Vec<Outcome>, ratchet: Ratchet, elapsed_ms: f64) -> Self {
        let mut groups: BTreeMap<String, GroupCounts> = BTreeMap::new();
        for o in &outcomes {
            let g = groups.entry(o.group.clone()).or_default();
            g.total += 1;
            if o.status == Status::Pass {
                g.passed += 1;
            } else {
                *g.other.entry(o.status).or_default() += 1;
            }
        }
        Self {
            suite: suite.to_string(),
            passed: outcomes.iter().filter(|o| o.status == Status::Pass).count(),
            total: outcomes.len(),
            groups,
            elapsed_ms,
            ratchet,
            outcomes,
        }
    }

    /// Plain-text table: one row per group, then the suite line.
    pub fn table(&self) -> String {
        let width = self
            .groups
            .keys()
            .map(String::len)
            .max()
            .unwrap_or(0)
            .max(5);
        let mut out = format!("== {} ==\n", self.suite);
        for (name, g) in &self.groups {
            let other: Vec<String> = g.other.iter().map(|(s, n)| format!("{s} {n}")).collect();
            out.push_str(&format!(
                "  {name:<width$}  {:>5} / {:<5}  {}\n",
                g.passed,
                g.total,
                other.join(", ")
            ));
        }
        out.push_str(&format!(
            "  {:<width$}  {:>5} / {:<5}  ({:.1} s)\n",
            "suite",
            self.passed,
            self.total,
            self.elapsed_ms / 1000.0
        ));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_carry_their_own_denominators() {
        let outcomes = vec![
            Outcome::new("a1", "a", Status::Pass),
            Outcome::new("a2", "a", Status::Fail),
            Outcome::new("b1", "b", Status::Timeout),
        ];
        let r = SuiteReport::new("s", outcomes, Ratchet::default(), 1.0);
        assert_eq!((r.passed, r.total), (1, 3));
        assert_eq!((r.groups["a"].passed, r.groups["a"].total), (1, 2));
        assert_eq!(r.groups["b"].other[&Status::Timeout], 1);
        let table = r.table();
        assert!(table.contains("1 / 2"), "{table}");
        assert!(!table.contains('%'), "no percentages: {table}");
    }
}
