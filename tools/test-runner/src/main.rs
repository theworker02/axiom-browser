//! Minimal fixture runner for local HTML files under tests/.

use std::env;
use std::fs;
use std::process::ExitCode;

use axiom_engine::{Engine, NavigateOptions};

fn main() -> ExitCode {
    let path = match env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: axiom-test-runner <file.html>");
            return ExitCode::FAILURE;
        }
    };
    let html = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let engine = Engine::new();
    match engine.render_html(
        &html,
        NavigateOptions {
            viewport_width: 800,
            viewport_height: 600,
            ..NavigateOptions::default()
        },
    ) {
        Ok(page) => {
            println!("{}", page.timings.report());
            let out = format!("{path}.ppm");
            if let Err(e) = fs::write(&out, page.framebuffer.to_ppm()) {
                eprintln!("write {out}: {e}");
                return ExitCode::FAILURE;
            }
            println!("Wrote {out}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
