//! `axiom-compat <suite|all> [--update] [--filter S] [--jobs N] [--json PATH] [--failures N]`
//!
//! Runs conformance suites offline, prints per-group `passed / total`, and exits non-zero
//! when a run differs from `tests/expectations/<suite>.txt`. `--update` rewrites the
//! expectations file from the run instead (review the diff before committing).
//! `--failures N` lists the first N non-passing tests and up to N ratchet changes per
//! category (default 25).

use std::process::ExitCode;

use axiom_compat::{run_suite, update_expectations, workspace_root, RunOptions, Suite};

fn usage() -> ExitCode {
    let names: Vec<_> = Suite::ALL.iter().map(|s| s.name()).collect();
    eprintln!(
        "usage: axiom-compat <{}|all> [--update] [--filter S] [--jobs N] [--json PATH] [--failures N]",
        names.join("|")
    );
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(which) = args.first() else {
        return usage();
    };
    let suites: Vec<Suite> = if which == "all" {
        Suite::ALL.to_vec()
    } else if let Some(s) = Suite::from_name(which) {
        vec![s]
    } else {
        return usage();
    };
    let mut opts = RunOptions::default();
    let (mut update, mut json, mut failures) = (false, None, 0usize);
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--update" => update = true,
            "--filter" => opts.filter = it.next().cloned(),
            "--jobs" => opts.jobs = it.next().and_then(|n| n.parse().ok()).unwrap_or(opts.jobs),
            "--json" => json = it.next().cloned(),
            "--failures" => failures = it.next().and_then(|n| n.parse().ok()).unwrap_or(20),
            _ => return usage(),
        }
    }
    if update && opts.filter.is_some() {
        eprintln!("--update records a full run; it cannot be combined with --filter");
        return ExitCode::from(2);
    }

    let root = workspace_root();
    let mut clean = true;
    let mut reports = Vec::new();
    for suite in suites {
        let report = match run_suite(&root, suite, &opts) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("{}: {e}", suite.name());
                return ExitCode::FAILURE;
            }
        };
        print!("{}", report.table());
        if failures > 0 {
            for o in report
                .outcomes
                .iter()
                .filter(|o| o.status != axiom_compat::expectations::Status::Pass)
                .take(failures)
            {
                println!(
                    "  {} {} — {}",
                    o.status,
                    o.id,
                    axiom_compat::expectations::truncate(&o.message, 200)
                );
            }
        }
        if update {
            if let Err(e) = update_expectations(&root, suite, &report) {
                eprintln!("{}: {e}", suite.name());
                return ExitCode::FAILURE;
            }
            println!(
                "  expectations updated: {}",
                suite.expectations_path(&root).display()
            );
        } else if !report.ratchet.is_clean() {
            clean = false;
            print!("{}", report.ratchet.describe(failures.max(25)));
        } else {
            println!("  matches expectations");
        }
        reports.push(report);
    }
    if let Some(path) = json {
        match serde_json::to_string_pretty(&reports) {
            Ok(s) => {
                if let Err(e) = std::fs::write(&path, s) {
                    eprintln!("write {path}: {e}");
                    return ExitCode::FAILURE;
                }
            }
            Err(e) => {
                eprintln!("json: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    if clean {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
