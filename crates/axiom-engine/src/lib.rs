//! Axiom engine — Phase 1 pipeline preserved, Phase 2 browsing context added.

mod browsing;
mod cookie_jar;
pub mod csp;
mod event_loop;
pub mod fetch;
pub mod forms;
mod hud;
pub mod import_map;
pub mod inline_svg;
pub mod keepalive;
mod lifecycle;
mod navigation;
mod page;
pub mod render;
pub mod sri;
mod web_storage;

pub use axiom_document::{
    BrowsingContextId, DocumentDiagnostics, DocumentEvent, DocumentEventKind, DocumentId,
    DocumentKind, NavigationId, ReadyState, ResourceDiagnostics, ResourceId,
};
pub use browsing::{BrowsingContext, RetiredDocument, ScriptError, SecurityState};
pub use cookie_jar::CookieJar;
pub use event_loop::{MicrotaskQueue, TaskQueue, TimerQueue};
pub use hud::{FrameClock, HudStats};
pub use keepalive::{DetachedKeepalive, KeepaliveLoads, KEEPALIVE_QUOTA};
pub use lifecycle::{DocumentReadyState, History, HistoryEntry};
pub use navigation::{NavigationCause, NavigationEvent, NavigationEventKind, NavigationState};
pub use page::{DecodedImage, DocumentJsHost, DocumentShared, DynamicInsertion, Page};
pub use web_storage::{StorageBinder, WebStorageHost};

use std::sync::Arc;
use std::time::{Duration, Instant};

use axiom_html::parse_html;
use axiom_layout::{layout_document, LayoutInputs};
use axiom_net::{NetworkService, NetworkServiceConfig, RequestScheduler, SchedulerConfig};
use axiom_paint::{build_display_list_with_images, rasterize, CssImages, DisplayList, Framebuffer};
use axiom_style::StyleEngine;
use axiom_trace::{TraceEvent, TraceKind};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Url(#[from] axiom_url::UrlError),
    #[error(transparent)]
    Loader(#[from] axiom_loader::LoaderError),
    #[error(transparent)]
    Html(#[from] axiom_html::ParseError),
    #[error("engine error: {0}")]
    Other(String),
}

#[derive(Debug, Clone, Default)]
pub struct StageTimings {
    pub html_parse_ms: f64,
    pub css_parse_ms: f64,
    pub style_ms: f64,
    pub layout_ms: f64,
    pub paint_ms: f64,
    pub raster_ms: f64,
    /// Main document request, from request start to the end of its body.
    pub net_ms: f64,
    pub script_ms: f64,
    pub total_ms: f64,
    /// `fetch()` calls, subresources and timers had all finished when the frame was taken
    /// (always true for [`Engine::render_html`], which runs no scripts).
    pub settled: bool,
}

impl StageTimings {
    pub fn report(&self) -> String {
        format!(
            "Network          {:>7.2} ms\n\
             HTML parse       {:>7.2} ms\n\
             Script           {:>7.2} ms\n\
             CSS parse        {:>7.2} ms\n\
             Style resolution {:>7.2} ms\n\
             Layout           {:>7.2} ms\n\
             Paint build      {:>7.2} ms\n\
             Raster           {:>7.2} ms\n\
             ────────────────────────\n\
             Frame            {:>7.2} ms{}",
            self.net_ms,
            self.html_parse_ms,
            self.script_ms,
            self.css_parse_ms,
            self.style_ms,
            self.layout_ms,
            self.paint_ms,
            self.raster_ms,
            self.total_ms,
            if self.settled {
                ""
            } else {
                "\n(settle timeout: pending work was still running)"
            }
        )
    }

    /// Stage totals from a browsing context's trace. Stages that ran several times while
    /// the page settled are summed; CSS parsing is reported apart from the style pass that
    /// contains it.
    fn from_trace(events: &[TraceEvent]) -> Self {
        let total = |kind: TraceKind| -> f64 {
            events
                .iter()
                .filter(|e| e.kind == kind)
                .map(|e| e.duration_ms)
                .sum()
        };
        let css = total(TraceKind::CssParse);
        Self {
            html_parse_ms: total(TraceKind::HtmlParse),
            css_parse_ms: css,
            style_ms: (total(TraceKind::Style) - css).max(0.0),
            layout_ms: total(TraceKind::Layout),
            paint_ms: total(TraceKind::Paint),
            raster_ms: total(TraceKind::Composite),
            net_ms: events
                .iter()
                .find(|e| e.kind == TraceKind::Network && e.name.starts_with("document "))
                .map_or(0.0, |e| e.duration_ms),
            script_ms: total(TraceKind::Javascript),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone)]
pub struct NavigateOptions {
    pub viewport_width: u32,
    pub viewport_height: u32,
    /// How long [`Engine::navigate`] lets scripts, `fetch()` and subresources settle
    /// before taking the frame.
    pub settle_timeout: Duration,
}

impl Default for NavigateOptions {
    fn default() -> Self {
        Self {
            viewport_width: 1024,
            viewport_height: 768,
            settle_timeout: Duration::from_secs(10),
        }
    }
}

pub struct RenderedPage {
    pub final_url: String,
    pub title: String,
    pub framebuffer: Framebuffer,
    pub display_list: DisplayList,
    pub timings: StageTimings,
}

/// One-shot rendering (CLI / test runner). [`Engine::navigate`] runs a real
/// [`BrowsingContext`] — scripts, `fetch()`, timers, subresources — on the engine's
/// scheduler and network service, the same path browser tabs use.
pub struct Engine {
    scheduler: Arc<RequestScheduler>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        let service = Arc::new(NetworkService::new(NetworkServiceConfig::default()));
        Self::with_scheduler(Arc::new(RequestScheduler::new(
            service,
            SchedulerConfig::default(),
        )))
    }

    pub fn with_scheduler(scheduler: Arc<RequestScheduler>) -> Self {
        Self { scheduler }
    }

    /// Load `url`, run the page's event loop until it is idle (or `settle_timeout`), and
    /// return the resulting frame.
    pub fn navigate(&self, url: &str, opts: NavigateOptions) -> Result<RenderedPage, EngineError> {
        let t0 = Instant::now();
        let mut ctx = BrowsingContext::headless(
            opts.viewport_width,
            opts.viewport_height,
            Arc::clone(&self.scheduler),
        );
        ctx.navigate(url);
        if let Some(e) = ctx.last_error.take() {
            return Err(EngineError::Other(e));
        }
        let settled = ctx.run_until_idle(opts.settle_timeout);
        ctx.page.update_rendering_if_needed();

        let mut timings = StageTimings::from_trace(&ctx.page.trace.events());
        timings.settled = settled;
        timings.total_ms = elapsed_ms(t0);
        let framebuffer = ctx
            .page
            .framebuffer
            .clone()
            .ok_or_else(|| EngineError::Other("page produced no frame".into()))?;
        Ok(RenderedPage {
            final_url: ctx.page.url.clone(),
            title: ctx.page.title.clone(),
            framebuffer,
            display_list: ctx.page.display_list.clone(),
            timings,
        })
    }

    pub fn render_html(
        &self,
        html: &str,
        opts: NavigateOptions,
    ) -> Result<RenderedPage, EngineError> {
        let t0 = Instant::now();
        let mut timings = StageTimings::default();

        let t_html = Instant::now();
        let document = parse_html(html)?;
        timings.html_parse_ms = elapsed_ms(t_html);

        let t_css = Instant::now();
        let author_css = document.collect_style_text();
        let mut style_engine =
            StyleEngine::with_viewport(opts.viewport_width as f32, opts.viewport_height as f32);
        style_engine.add_author_css(&author_css);
        for (_, root) in document.connected_shadow_roots() {
            let css: Vec<String> = document
                .collect_style_sources_in(root)
                .into_iter()
                .filter_map(|source| match source {
                    axiom_dom::StyleSource::Inline { text, .. } => Some(text),
                    axiom_dom::StyleSource::External { .. } => None,
                })
                .collect();
            style_engine.add_shadow_css(root, &css.join("\n"));
        }
        timings.css_parse_ms = elapsed_ms(t_css);

        let t_style = Instant::now();
        let styles = style_engine.compute_document(&document);
        let mut svg_cache = inline_svg::InlineSvgCache::default();
        let mut inline_svgs = svg_cache.rasterize(&document, &styles);
        timings.style_ms = elapsed_ms(t_style);

        let t_layout = Instant::now();
        let mut layout = layout_document(
            &document,
            &styles,
            &LayoutInputs {
                inline_svgs: &inline_svgs,
                ..LayoutInputs::empty()
            },
            opts.viewport_width as f32,
            opts.viewport_height as f32,
        );
        svg_cache.fit_to_layout(&document, &styles, &mut layout, &mut inline_svgs);
        timings.layout_ms = elapsed_ms(t_layout);

        let t_paint = Instant::now();
        let display_list = build_display_list_with_images(&layout, &data_url_images(&styles));
        timings.paint_ms = elapsed_ms(t_paint);

        let t_raster = Instant::now();
        let framebuffer = rasterize(&display_list, opts.viewport_width, opts.viewport_height);
        timings.raster_ms = elapsed_ms(t_raster);
        timings.total_ms = elapsed_ms(t0);
        timings.settled = true;

        let title = document
            .head()
            .and_then(|head| document.find_descendant(head, "title"))
            .map(|id| document.text_content(id).trim().to_string())
            .unwrap_or_else(|| "about:blank".into());

        Ok(RenderedPage {
            final_url: "about:blank".into(),
            title,
            framebuffer,
            display_list,
            timings,
        })
    }
}

/// `data:` background and mask images of a page rendered without a loader.
fn data_url_images(styles: &axiom_style::StyleMap) -> CssImages {
    render::css_image_urls(styles)
        .into_iter()
        .filter_map(|url| {
            let (_, bytes) = axiom_loader::parse_data_url(&url)?;
            let (width, height, rgba) = axiom_loader::decode_image(&bytes).ok()?;
            let image = axiom_layout::RasterImage {
                width,
                height,
                rgba,
            };
            Some((url, std::sync::Arc::new(image)))
        })
        .collect()
}

fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}
