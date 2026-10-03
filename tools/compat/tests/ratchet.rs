//! The expectations ratchet, enforced by `cargo test`: every suite must reproduce its
//! checked-in expectations exactly (no regressions, no unexpected passes).

use axiom_compat::{run_suite, workspace_root, RunOptions, Suite};

fn check(suite: Suite) {
    let report = run_suite(&workspace_root(), suite, &RunOptions::default()).expect("suite runs");
    assert!(
        report.ratchet.is_clean(),
        "{}{}\nIf this change is intended, run `cargo run -p axiom-compat -- {} --update` and review the diff.",
        report.table(),
        report.ratchet.describe(25),
        suite.name()
    );
}

#[test]
fn html5lib_tree_construction() {
    check(Suite::Html5libTree);
}

#[test]
fn html5lib_tokenizer() {
    check(Suite::Html5libTokenizer);
}

#[test]
fn wpt_crashtests() {
    check(Suite::Crashtests);
}

#[test]
fn wpt_reftests() {
    check(Suite::Reftests);
}

#[test]
fn wpt_testharness() {
    check(Suite::Testharness);
}
