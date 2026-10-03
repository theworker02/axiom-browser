//! Axiom desktop host — Phase 3 multi-tab browser with omnibox chrome.
//!
//! ```text
//! cargo run -p axiom-desktop -- demos/click-works.html
//! cargo run -p axiom-desktop -- --headless https://example.com
//!
//! Shortcuts: Ctrl+L omnibox, Ctrl+T/W/Shift+T, Ctrl+Tab, Ctrl+R, Alt+Left/Right, F1 HUD
//! ```

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use axiom_browser::{Browser, BrowserDataStore};
use axiom_engine::{Engine, NavigateOptions};
use axiom_gfx::run_browser;

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut args: Vec<String> = env::args().skip(1).collect();
    let headless = args.iter().any(|a| a == "--headless");
    let private = args.iter().any(|a| a == "--private");
    args.retain(|a| a != "--headless" && a != "--private");

    let url = args
        .first()
        .cloned()
        .unwrap_or_else(|| "axiom://newtab".into());

    if headless {
        return run_headless(&url);
    }

    let nav = resolve_nav_url(&url);

    // Installed applications cannot assume that their working directory is writable
    // (on Windows it is commonly Program Files). Keep persistent browser data in the
    // user's application-data location instead. The development fallback remains
    // deterministic for environments without an OS application-data directory.
    let data = default_profile_dir();
    let browser = if private {
        // Preferences (search engine, homepage) carry over; history, cookies and cache
        // of the normal profile do not.
        match BrowserDataStore::read_settings(data.clone()) {
            Ok(Some(settings)) => Browser::new_private_inheriting(&settings, 1024, 768),
            Ok(None) => Browser::new_private(1024, 768),
            Err(e) => {
                log::warn!("normal profile settings unreadable, private window uses defaults: {e}");
                Browser::new_private(1024, 768)
            }
        }
    } else {
        if let Err(e) = fs::create_dir_all(&data) {
            eprintln!(
                "axiom: cannot create profile directory {}: {e}",
                data.display()
            );
            return ExitCode::FAILURE;
        }
        Browser::new_normal(data, 1024, 768)
    };
    let mut browser = match browser {
        Ok(b) => b,
        Err(e) => {
            eprintln!("axiom: profile error: {e}");
            return ExitCode::FAILURE;
        }
    };

    browser.set_viewport(1024, 768);

    match run_browser(browser, &nav) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("axiom: {e}");
            ExitCode::FAILURE
        }
    }
}

fn default_profile_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Some(root) = env::var_os("LOCALAPPDATA") {
            return PathBuf::from(root).join("Axiom").join("Profile");
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        if let Some(root) = env::var_os("XDG_DATA_HOME") {
            return PathBuf::from(root).join("axiom").join("profile");
        }
        if let Some(home) = env::var_os("HOME") {
            return PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("axiom")
                .join("profile");
        }
    }

    env::temp_dir().join("axiom-profile")
}

fn resolve_nav_url(url: &str) -> String {
    if url.starts_with("http://")
        || url.starts_with("https://")
        || url.starts_with("file:")
        || url.starts_with("axiom://")
        || url.starts_with("about:")
    {
        url.to_string()
    } else {
        PathBuf::from(url)
            .canonicalize()
            .ok()
            .and_then(|path| url::Url::from_file_path(path).ok())
            .map(|url| url.into())
            .unwrap_or_else(|| url.to_string())
    }
}

fn run_headless(url: &str) -> ExitCode {
    if url.starts_with("http://") || url.starts_with("https://") {
        let engine = Engine::new();
        let page = match engine.navigate(
            url,
            NavigateOptions {
                viewport_width: 1024,
                viewport_height: 768,
                ..NavigateOptions::default()
            },
        ) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("axiom: {e}");
                return ExitCode::FAILURE;
            }
        };
        println!("Axiom — {}", page.final_url);
        println!("Title: {}", page.title);
        println!();
        println!("{}", page.timings.report());
        let path = PathBuf::from("axiom-frame.ppm");
        if let Err(e) = fs::write(&path, page.framebuffer.to_ppm()) {
            eprintln!("write {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
        println!("\nWrote {}", path.display());
        return ExitCode::SUCCESS;
    }

    let mut browser = match Browser::new_private(1024, 768) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("axiom: {e}");
            return ExitCode::FAILURE;
        }
    };
    let path = resolve_nav_url(url);
    browser.navigate(&path);
    let settled = browser
        .window
        .tabs
        .active_tab_mut()
        .context
        .run_until_idle(NavigateOptions::default().settle_timeout);
    if !settled {
        eprintln!("axiom: settle timeout: pending work was still running");
    }
    if let Some(fb) = browser.window.tabs.active_tab().framebuffer() {
        let out = PathBuf::from("axiom-frame.ppm");
        if let Err(e) = fs::write(&out, fb.to_ppm()) {
            eprintln!("write {}: {e}", out.display());
            return ExitCode::FAILURE;
        }
        println!("Axiom — {}", browser.window.tabs.active_tab().url());
        println!("Title: {}", browser.window.tabs.active_tab().title());
        println!("Tabs: {}", browser.window.tabs.len());
        println!(
            "{}",
            browser.window.tabs.active_tab().context.page.hud.report()
        );
        println!("Wrote {}", out.display());
        ExitCode::SUCCESS
    } else {
        eprintln!("axiom: no framebuffer");
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::default_profile_dir;

    #[test]
    fn default_profile_directory_is_app_scoped() {
        let path = default_profile_dir();
        assert!(path.ends_with("Profile") || path.ends_with("profile"));
        assert!(path
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains("axiom"));
    }
}
