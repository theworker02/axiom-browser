//! WPT reftests: render the test and each `<link rel=match|mismatch>` reference at
//! 800×600 through the real navigation pipeline and compare the frames.
//!
//! As in wptrunner, the test passes if any one reference relation holds; a `match` may
//! differ within `<meta name=fuzzy>` tolerances. Reference chains (references that have
//! references of their own) are not followed. `reftest-wait` is approximated by running
//! the page until its event loop is idle. With `AXIOM_COMPAT_DUMP=<dir>` set, failing
//! test and reference frames are written there as PNG images.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axiom_engine::BrowsingContext;
use axiom_html::tokenizer::{tokenize, Token};

use crate::expectations::{Outcome, Status};
use crate::runner::parallel_map;
use crate::wpt::{self, guarded, parent_dir, Harness, VIEWPORT};

pub const DIRS: &[&str] = &[
    "css/CSS2/colors",
    "css/CSS2/box-display",
    "css/CSS2/margin-padding-clear",
    "css/css-display",
];

const BUDGET: Duration = Duration::from_secs(5);
const HARD_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Relation {
    Match,
    Mismatch,
}

/// Allowed difference: per-channel maximum and number of differing pixels (inclusive
/// ranges).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fuzzy {
    pub max_difference: (u32, u32),
    pub total_pixels: (u32, u32),
}

impl Fuzzy {
    pub const EXACT: Fuzzy = Fuzzy {
        max_difference: (0, 0),
        total_pixels: (0, 0),
    };

    /// `maxDifference=0-2;totalPixels=0-300`, `0-2;0-300`, optionally prefixed `ref.html:`.
    pub fn parse(content: &str) -> Option<(Option<String>, Fuzzy)> {
        let (url, spec) = match content.split_once(':') {
            Some((u, s)) if !u.contains('=') && !u.contains(';') => (Some(u.trim().to_string()), s),
            _ => (None, content),
        };
        let range = |s: &str| -> Option<(u32, u32)> {
            let s = s.trim();
            match s.split_once('-') {
                Some((a, b)) => Some((a.trim().parse().ok()?, b.trim().parse().ok()?)),
                None => {
                    let n = s.parse().ok()?;
                    Some((0, n))
                }
            }
        };
        let mut parts = spec.split(';');
        let (a, b) = (parts.next()?, parts.next()?);
        let value = |p: &str, key: &str| p.trim().strip_prefix(key).map(str::to_string);
        let md = value(a, "maxDifference=").unwrap_or_else(|| a.to_string());
        let tp = value(b, "totalPixels=").unwrap_or_else(|| b.to_string());
        Some((
            url,
            Fuzzy {
                max_difference: range(&md)?,
                total_pixels: range(&tp)?,
            },
        ))
    }
}

#[derive(Debug, Clone)]
pub struct Reftest {
    pub path: String,
    pub references: Vec<(String, Relation)>,
    pub fuzzy: Vec<(Option<String>, Fuzzy)>,
}

/// Parse a test file's references and fuzzy annotations (with Axiom's own tokenizer).
pub fn parse_reftest(path: &str, source: &str) -> Option<Reftest> {
    let dir = parent_dir(path);
    let mut references = Vec::new();
    let mut fuzzy = Vec::new();
    for tok in tokenize(source) {
        let Token::StartTag(tag) = tok else { continue };
        match tag.name.as_str() {
            "link" => {
                let rel = tag.attr("rel").unwrap_or("").to_ascii_lowercase();
                let relation = match rel.trim() {
                    "match" => Relation::Match,
                    "mismatch" => Relation::Mismatch,
                    _ => continue,
                };
                if let Some(href) = tag.attr("href") {
                    references.push((resolve(&dir, href.trim()), relation));
                }
            }
            "meta" if tag.attr("name") == Some("fuzzy") => {
                if let Some(f) = tag.attr("content").and_then(Fuzzy::parse) {
                    fuzzy.push(f);
                }
            }
            _ => {}
        }
    }
    (!references.is_empty()).then(|| Reftest {
        path: path.to_string(),
        references,
        fuzzy,
    })
}

/// Resolve `href` against the test's directory into a WPT-root-relative path.
fn resolve(dir: &str, href: &str) -> String {
    let href = href.split(['?', '#']).next().unwrap_or(href);
    let mut parts: Vec<&str> = if href.starts_with('/') {
        Vec::new()
    } else {
        dir.split('/').filter(|s| !s.is_empty()).collect()
    };
    for seg in href.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

pub fn discover(root: &Path) -> Vec<Reftest> {
    let wpt = wpt::wpt_root(root);
    wpt::walk(&wpt, DIRS)
        .into_iter()
        .filter(|p| {
            let ext = p.rsplit('.').next().unwrap_or("");
            matches!(ext, "html" | "htm" | "xhtml" | "xht" | "svg")
                && !p.contains("/support/")
                && !p.contains("/reference/")
                && !p.contains("/crashtests/")
        })
        .filter_map(|p| {
            let src = std::fs::read_to_string(wpt.join(&p)).ok()?;
            parse_reftest(&p, &src)
        })
        .collect()
}

/// Per-channel maximum difference and number of differing pixels.
pub fn difference(a: &[u32], b: &[u32]) -> (u32, u32) {
    let mut max = 0u32;
    let mut count = 0u32;
    for (&x, &y) in a.iter().zip(b) {
        if x != y {
            count += 1;
            for shift in [0, 8, 16, 24] {
                let d = ((x >> shift) & 0xff).abs_diff((y >> shift) & 0xff);
                max = max.max(d);
            }
        }
    }
    (max, count)
}

/// Save a `0xAABBGGRR` frame as PNG, for `AXIOM_COMPAT_DUMP` debugging.
fn save_png(pixels: &[u32], path: &Path) {
    let rgb: Vec<u8> = pixels
        .iter()
        .flat_map(|p| [*p as u8, (p >> 8) as u8, (p >> 16) as u8])
        .collect();
    if let Some(img) = image::RgbImage::from_raw(VIEWPORT.0, VIEWPORT.1, rgb) {
        if let Err(e) = img.save(path) {
            eprintln!("could not write {}: {e}", path.display());
        }
    }
}

fn render(scheduler: Arc<axiom_net::RequestScheduler>, url: &str) -> Result<Vec<u32>, String> {
    let mut ctx = BrowsingContext::headless(VIEWPORT.0, VIEWPORT.1, scheduler);
    let loaded = wpt::load(&mut ctx, url, BUDGET);
    if std::env::var_os("AXIOM_COMPAT_DEBUG").is_some() {
        eprintln!(
            "{url}: idle={} load={} errors={:?}",
            loaded.idle, loaded.load_fired, loaded.script_errors
        );
    }
    if let Some(e) = loaded.navigation_error {
        return Err(e);
    }
    let fb = ctx.framebuffer().ok_or("no frame")?;
    if (fb.width, fb.height) != VIEWPORT {
        return Err(format!("frame is {}x{}", fb.width, fb.height));
    }
    Ok(fb.pixels.clone())
}

pub fn run(root: &Path, jobs: usize, filter: Option<&str>) -> anyhow::Result<Vec<Outcome>> {
    let tests: Vec<Reftest> = discover(root)
        .into_iter()
        .filter(|t| filter.is_none_or(|f| t.path.contains(f)))
        .collect();
    let harness = Harness::new(root);
    let outcomes = parallel_map(&tests, jobs, |t| {
        let group = parent_dir(&t.path);
        let test_url = harness.server.url(&t.path);
        let refs: Vec<(String, String, Relation)> = t
            .references
            .iter()
            .map(|(p, r)| (p.clone(), harness.server.url(p), r.clone()))
            .collect();
        let fuzzy = t.fuzzy.clone();
        let t_path = t.path.clone();
        let scheduler = Arc::clone(&harness.scheduler);
        let result = guarded(
            &t.path,
            &group,
            HARD_TIMEOUT,
            move || -> Result<Option<String>, String> {
                let test =
                    render(Arc::clone(&scheduler), &test_url).map_err(|e| format!("test: {e}"))?;
                let mut failures = Vec::new();
                for (ref_path, ref_url, relation) in refs {
                    let reference = render(Arc::clone(&scheduler), &ref_url)
                        .map_err(|e| format!("reference {ref_path}: {e}"))?;
                    let (max, count) = difference(&test, &reference);
                    let name = ref_path.rsplit('/').next().unwrap_or(&ref_path).to_string();
                    let tolerance = fuzzy
                        .iter()
                        .find(|(u, _)| {
                            u.as_deref()
                                .is_none_or(|u| u == name || ref_path.ends_with(u))
                        })
                        .map_or(Fuzzy::EXACT, |(_, f)| *f);
                    let within = count == 0
                        || (tolerance.max_difference.0 <= max
                            && max <= tolerance.max_difference.1
                            && tolerance.total_pixels.0 <= count
                            && count <= tolerance.total_pixels.1);
                    let ok = match relation {
                        Relation::Match => within,
                        Relation::Mismatch => count != 0,
                    };
                    if ok {
                        if std::env::var_os("AXIOM_COMPAT_DEBUG").is_some()
                            && test.iter().all(|&p| p == test[0])
                        {
                            eprintln!("uniform-frame pass: {test_url}");
                        }
                        return Ok(None);
                    }
                    if let Some(dir) = std::env::var_os("AXIOM_COMPAT_DUMP") {
                        let stem = t_path.replace('/', "_");
                        let dir = Path::new(&dir);
                        save_png(&test, &dir.join(format!("{stem}.test.png")));
                        save_png(&reference, &dir.join(format!("{stem}.ref.png")));
                    }
                    failures.push(match relation {
                        Relation::Match => {
                            format!("{name}: {count} pixels differ (max channel diff {max})")
                        }
                        Relation::Mismatch => {
                            format!("{name}: rendering is identical to the mismatch reference")
                        }
                    });
                }
                Ok(Some(failures.join("; ")))
            },
        );
        if let Some(missing) = std::iter::once(&t.path)
            .chain(t.references.iter().map(|(p, _)| p))
            .find(|p| harness.server.was_not_found(p))
        {
            return Outcome::new(t.path.as_str(), group, Status::Error)
                .with_message(format!("{missing} was not served (404)"));
        }
        match result {
            Err(o) => o,
            Ok(Ok(None)) => Outcome::new(t.path.as_str(), group, Status::Pass),
            Ok(Ok(Some(msg))) => {
                Outcome::new(t.path.as_str(), group, Status::Fail).with_message(msg)
            }
            Ok(Err(msg)) => Outcome::new(t.path.as_str(), group, Status::Error).with_message(msg),
        }
    });
    harness.report_not_found();
    Ok(outcomes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_and_fuzzy_are_read_from_markup() {
        let src = r#"<!DOCTYPE html><link rel="match" href="reference/a-ref.html"><link rel=mismatch href=/css/b.html><link rel=help href=x><meta name="fuzzy" content="maxDifference=0-2;totalPixels=0-300"><meta name=fuzzy content="a-ref.html:5;10-20">"#;
        let t = parse_reftest("css/CSS2/colors/t.html", src).unwrap();
        assert_eq!(
            t.references,
            vec![
                (
                    "css/CSS2/colors/reference/a-ref.html".to_string(),
                    Relation::Match
                ),
                ("css/b.html".to_string(), Relation::Mismatch),
            ]
        );
        assert_eq!(t.fuzzy[0].1.max_difference, (0, 2));
        assert_eq!(t.fuzzy[0].1.total_pixels, (0, 300));
        assert_eq!(t.fuzzy[1].0.as_deref(), Some("a-ref.html"));
        assert_eq!(t.fuzzy[1].1.max_difference, (0, 5));
        assert_eq!(t.fuzzy[1].1.total_pixels, (10, 20));
        assert!(parse_reftest("x.html", "<p>no refs</p>").is_none());
    }

    #[test]
    fn relative_references_resolve_like_urls() {
        assert_eq!(
            resolve("css/CSS2/colors", "../reference/r.xht"),
            "css/CSS2/reference/r.xht"
        );
        assert_eq!(resolve("css/a", "./r.html?x#y"), "css/a/r.html");
    }

    #[test]
    fn pixel_difference_counts_channels() {
        assert_eq!(
            difference(&[0xff000000, 0xff102030], &[0xff000000, 0xff102033]),
            (3, 1)
        );
    }
}
