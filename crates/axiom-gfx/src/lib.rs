//! Windowing and framebuffer present for Axiom.

use std::num::NonZeroU32;
use std::rc::Rc;

use softbuffer::{Context, Surface};
use thiserror::Error;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Icon, Window, WindowId};

use axiom_browser::{
    Browser, ChromeControl, FocusOwner, ReloadStopMode, SecurityDisplay, CHROME_H, STATUS_H,
    TAB_MAX_W, TAB_MIN_W, TAB_STRIP_H, TOOLBAR_H, TOP_CHROME_H,
};
use axiom_engine::BrowsingContext;
use axiom_paint::Framebuffer;

#[derive(Debug, Error)]
pub enum GfxError {
    #[error("window error: {0}")]
    Window(String),
}

fn axiom_window_icon() -> Icon {
    const SIDE: usize = 32;
    let mut pixels = vec![0_u8; SIDE * SIDE * 4];
    for y in 0..SIDE {
        for x in 0..SIDE {
            let dx = x as i32 - 15;
            let dy = y as i32 - 15;
            let i = (y * SIDE + x) * 4;
            if (72..=185).contains(&(dx * dx + dy * dy))
                || ((x as i32 - (15 - dy / 2)).abs() <= 2 && (7..=25).contains(&y))
                || ((14..=18).contains(&y) && (10..=21).contains(&x))
            {
                pixels[i..i + 4].copy_from_slice(&[68, 122, 255, 255]);
            }
        }
    }
    Icon::from_rgba(pixels, SIDE as u32, SIDE as u32).expect("valid Axiom icon")
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

/// Present a static frame (Phase 1 compatibility).
pub fn present_window(title: &str, frame: Frame) -> Result<(), GfxError> {
    let event_loop = EventLoop::new().map_err(|e| GfxError::Window(e.to_string()))?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = StaticApp {
        title: title.to_string(),
        frame,
        window: None,
        context: None,
        surface: None,
        surface_width: 0,
        surface_height: 0,
    };
    event_loop
        .run_app(&mut app)
        .map_err(|e| GfxError::Window(e.to_string()))
}

struct StaticApp {
    title: String,
    frame: Frame,
    window: Option<Rc<Window>>,
    context: Option<Context<Rc<Window>>>,
    surface: Option<Surface<Rc<Window>, Rc<Window>>>,
    surface_width: u32,
    surface_height: u32,
}

impl ApplicationHandler for StaticApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let size = LogicalSize::new(self.frame.width.max(1), self.frame.height.max(1));
        let window = Rc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title(self.title.clone())
                        .with_window_icon(Some(axiom_window_icon()))
                        .with_inner_size(size),
                )
                .expect("create window"),
        );
        let context = Context::new(window.clone()).expect("softbuffer context");
        let mut surface = Surface::new(&context, window.clone()).expect("softbuffer surface");
        surface
            .resize(
                NonZeroU32::new(self.frame.width.max(1)).unwrap(),
                NonZeroU32::new(self.frame.height.max(1)).unwrap(),
            )
            .expect("resize");
        self.surface_width = self.frame.width.max(1);
        self.surface_height = self.frame.height.max(1);
        self.context = Some(context);
        self.surface = Some(surface);
        window.request_redraw();
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::Resized(size) => {
                if let Some(surface) = self.surface.as_mut() {
                    if let (Some(w), Some(h)) =
                        (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
                    {
                        let _ = surface.resize(w, h);
                        self.surface_width = size.width;
                        self.surface_height = size.height;
                    }
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            _ => {}
        }
    }
}

impl StaticApp {
    fn redraw(&mut self) {
        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        let Ok(mut buffer) = surface.buffer_mut() else {
            return;
        };
        let bw = self.surface_width.max(1) as usize;
        let bh = self.surface_height.max(1) as usize;
        let fw = self.frame.width as usize;
        let fh = self.frame.height as usize;
        for y in 0..bh {
            for x in 0..bw {
                let idx = y * bw + x;
                if idx >= buffer.len() {
                    break;
                }
                buffer[idx] = if x < fw && y < fh {
                    self.frame.pixels[y * fw + x]
                } else {
                    0xff_ff_ff_ff
                };
            }
        }
        let _ = buffer.present();
    }
}

/// Legacy single-context entry (wraps into a Browser).
pub fn run_browser_context(ctx: BrowsingContext, initial_url: &str) -> Result<(), GfxError> {
    let browser = Browser::new_private(1024, 768).map_err(|e| GfxError::Window(e.to_string()))?;
    let _ = ctx;
    run_browser(browser, initial_url)
}

/// Run the interactive multi-tab browser UI.
pub fn run_browser(mut browser: Browser, initial_url: &str) -> Result<(), GfxError> {
    browser.set_viewport(1024, 768);
    // Pages load while the event loop keeps painting; `Browser::tick` commits them.
    browser.set_background_navigation(true);
    if !initial_url.is_empty() && initial_url != "axiom://newtab" {
        browser.navigate(initial_url);
    }
    let event_loop = EventLoop::new().map_err(|e| GfxError::Window(e.to_string()))?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = BrowserApp {
        browser,
        window: None,
        context: None,
        surface: None,
        surface_width: 1024,
        surface_height: 768,
        cursor: (0.0, 0.0),
        composed: Framebuffer::new(1024, 768),
        modifiers: ModifiersState::default(),
    };

    event_loop
        .run_app(&mut app)
        .map_err(|e| GfxError::Window(e.to_string()))
}

struct BrowserApp {
    browser: Browser,
    window: Option<Rc<Window>>,
    context: Option<Context<Rc<Window>>>,
    surface: Option<Surface<Rc<Window>, Rc<Window>>>,
    surface_width: u32,
    surface_height: u32,
    cursor: (f32, f32),
    composed: Framebuffer,
    modifiers: ModifiersState,
}

impl ApplicationHandler for BrowserApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let window = Rc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title(self.browser.chrome_state().window_title)
                        .with_window_icon(Some(axiom_window_icon()))
                        .with_inner_size(LogicalSize::new(1024u32, 768u32)),
                )
                .expect("window"),
        );
        let context = Context::new(window.clone()).expect("context");
        let mut surface = Surface::new(&context, window.clone()).expect("surface");
        surface
            .resize(
                NonZeroU32::new(1024).unwrap(),
                NonZeroU32::new(768).unwrap(),
            )
            .ok();
        self.context = Some(context);
        self.surface = Some(surface);
        window.request_redraw();
        self.window = Some(window);
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        let _ = self.browser.tick();
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::Resized(size) => {
                if let Some(surface) = self.surface.as_mut() {
                    if let (Some(w), Some(h)) =
                        (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
                    {
                        let _ = surface.resize(w, h);
                        self.surface_width = size.width;
                        self.surface_height = size.height;
                        self.browser.set_viewport(size.width, size.height);
                    }
                }
            }
            WindowEvent::ModifiersChanged(m) => {
                self.modifiers = m.state();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as f32, position.y as f32);
                let top = TOP_CHROME_H as f32;
                let bottom = self.surface_height.saturating_sub(STATUS_H) as f32;
                if self.cursor.1 >= top && self.cursor.1 < bottom {
                    self.browser
                        .set_hover_status(self.cursor.0, self.cursor.1 - top);
                } else {
                    self.browser.chrome.status_text.clear();
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } => {
                let (x, y) = self.cursor;
                let top = TOP_CHROME_H as f32;
                let bottom = self.surface_height.saturating_sub(STATUS_H) as f32;
                match button {
                    MouseButton::Left => {
                        if y < top {
                            self.handle_chrome_click(x, y, false);
                        } else if y < bottom {
                            self.browser.chrome.focus_content();
                            self.browser.handle_content_click(x, y);
                        }
                    }
                    MouseButton::Middle if y < TAB_STRIP_H as f32 => {
                        self.handle_chrome_click(x, y, true);
                    }
                    _ => {}
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -y * 40.0,
                    MouseScrollDelta::PixelDelta(p) => -p.y as f32,
                };
                self.browser
                    .window
                    .tabs
                    .active_tab_mut()
                    .context
                    .handle_scroll(dy);
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let ctrl = self.modifiers.control_key();
                let shift = self.modifiers.shift_key();
                let alt = self.modifiers.alt_key();
                let key_str = match &event.logical_key {
                    Key::Character(c) => c.to_string(),
                    Key::Named(NamedKey::Space) => " ".into(),
                    Key::Named(NamedKey::Tab) => "Tab".into(),
                    Key::Named(NamedKey::Enter) => "Enter".into(),
                    Key::Named(NamedKey::Escape) => "Escape".into(),
                    Key::Named(NamedKey::Backspace) => "Backspace".into(),
                    Key::Named(NamedKey::Delete) => "Delete".into(),
                    Key::Named(NamedKey::Home) => "Home".into(),
                    Key::Named(NamedKey::End) => "End".into(),
                    Key::Named(NamedKey::ArrowLeft) => "ArrowLeft".into(),
                    Key::Named(NamedKey::ArrowRight) => "ArrowRight".into(),
                    Key::Named(NamedKey::ArrowUp) => "ArrowUp".into(),
                    Key::Named(NamedKey::ArrowDown) => "ArrowDown".into(),
                    Key::Named(NamedKey::F1) => "F1".into(),
                    Key::Named(NamedKey::F5) => "F5".into(),
                    _ => String::new(),
                };
                if key_str == "F1" {
                    self.browser.show_hud = !self.browser.show_hud;
                    return;
                }
                if key_str == "F5" {
                    self.browser.reload_or_stop();
                    return;
                }
                if key_str.is_empty() {
                    return;
                }
                // Trusted browser shortcuts + chrome focus routing.
                if self.browser.handle_chrome_key(&key_str, ctrl, shift, alt) {
                    return;
                }
                // Web content only when focus is content and not a browser shortcut.
                if self.browser.chrome.focus == FocusOwner::WebContent {
                    self.browser
                        .window
                        .tabs
                        .active_tab_mut()
                        .context
                        .handle_key(&key_str);
                }
            }
            _ => {}
        }
    }
}

impl BrowserApp {
    fn tab_metrics(&self) -> (u32, usize, usize) {
        let n = self.browser.window.tabs.len();
        let avail = self.surface_width.saturating_sub(40);
        let max_fit = (avail / TAB_MIN_W).max(1) as usize;
        let visible = n.min(max_fit);
        let tab_w = if visible == 0 {
            TAB_MAX_W
        } else {
            (avail / visible as u32).clamp(TAB_MIN_W, TAB_MAX_W)
        };
        let offset = self
            .browser
            .chrome
            .tab_strip_offset
            .min(n.saturating_sub(visible));
        (tab_w, visible, offset)
    }

    fn handle_chrome_click(&mut self, x: f32, y: f32, middle: bool) {
        if y < TAB_STRIP_H as f32 {
            let (tab_w, visible, offset) = self.tab_metrics();
            let n = self.browser.window.tabs.len();
            let idx_vis = (x / tab_w as f32) as usize;
            if idx_vis < visible {
                let idx = offset + idx_vis;
                let close_x = (idx_vis as u32 + 1) * tab_w - 18;
                if middle || ((x as u32) >= close_x && (x as u32) < close_x + 14) {
                    self.browser.window.tabs.select(idx);
                    self.browser.close_tab();
                } else {
                    self.browser.select_tab(idx);
                }
                return;
            }
            let plus_x = visible as u32 * tab_w + 4;
            if x as u32 >= plus_x && (x as u32) < plus_x + 28 {
                self.browser.new_tab();
            }
            // Overflow arrows
            if n > visible && x < 16.0 && offset > 0 {
                self.browser.chrome.tab_strip_offset = offset - 1;
            }
            return;
        }

        let ty = y - TAB_STRIP_H as f32;
        if ty < 0.0 || ty > TOOLBAR_H as f32 {
            return;
        }
        let state = self.browser.chrome_state();
        // Back 8-44, Forward 52-88, Reload 96-132, Security 140-168, Omnibox 176..
        if (8.0..44.0).contains(&x) {
            if state.can_go_back {
                self.browser.back();
            }
        } else if (52.0..88.0).contains(&x) {
            if state.can_go_forward {
                self.browser.forward();
            }
        } else if (96.0..132.0).contains(&x) {
            self.browser.reload_or_stop();
        } else if (140.0..168.0).contains(&x) {
            self.browser.toggle_site_info();
        } else if (168.0..196.0).contains(&x) {
            self.browser.toggle_bookmark();
        } else if x >= 204.0 {
            self.browser.focus_omnibox();
        }
    }

    fn redraw(&mut self) {
        let hud = self.browser.tick();
        // Omnibox typing must not dirty the page — tick_active may paint; that's OK.
        let w = self.surface_width.max(1);
        let h = self.surface_height.max(1);
        let state = self.browser.chrome_state();
        let mut fb = Framebuffer::new(w, h);

        // --- Tab strip (trusted) ---
        // Axiom's "orbit rail": a high-contrast control surface kept outside untrusted content.
        fill(&mut fb, 0, 0, w, TAB_STRIP_H, 0xff_111827);
        let (tab_w, visible, offset) = self.tab_metrics();
        let active = self.browser.window.tabs.active_index();
        let tabs = self.browser.window.tabs.tabs();
        for vi in 0..visible {
            let i = offset + vi;
            let Some(tab) = tabs.get(i) else { break };
            let x = vi as u32 * tab_w;
            let color = if i == active {
                0xff_253455
            } else {
                0xff_18233b
            };
            fill(
                &mut fb,
                x + 1,
                3,
                tab_w.saturating_sub(2),
                TAB_STRIP_H - 5,
                color,
            );
            // Favicon placeholder
            fill(&mut fb, x + 8, 10, 12, 12, 0xff_447aff);
            let mut label = tab.title();
            if label.len() > 12 {
                label.truncate(12);
            }
            if tab.pinned {
                label = format!("*{label}");
            }
            blit_text_approx(&mut fb, x + 24, 10, &label, 0xff_f3f7ff);
            // Close control
            blit_text_approx(&mut fb, x + tab_w - 16, 10, "x", 0xff_9eb6e8);
            if tab.is_loading() {
                fill(
                    &mut fb,
                    x + 2,
                    TAB_STRIP_H - 3,
                    tab_w.saturating_sub(4),
                    2,
                    0xff_72a7ff,
                );
            }
        }
        let plus_x = visible as u32 * tab_w + 6;
        fill(&mut fb, plus_x, 6, 22, 20, 0xff_36528a);
        blit_text_approx(&mut fb, plus_x + 7, 10, "+", 0xff_f3f7ff);
        if tabs.len() > visible {
            blit_text_approx(
                &mut fb,
                w.saturating_sub(48),
                10,
                &format!("+{}", tabs.len() - visible),
                0xff_b7c6e5,
            );
        }

        // --- Toolbar ---
        fill(&mut fb, 0, TAB_STRIP_H, w, TOOLBAR_H, 0xff_19243b);
        draw_nav_button(&mut fb, 8, TAB_STRIP_H + 8, 36, 28, "<", state.can_go_back);
        draw_nav_button(
            &mut fb,
            52,
            TAB_STRIP_H + 8,
            36,
            28,
            ">",
            state.can_go_forward,
        );
        let reload_label = match state.reload_stop {
            ReloadStopMode::Reload => "R",
            ReloadStopMode::Stop => "S",
        };
        draw_nav_button(&mut fb, 96, TAB_STRIP_H + 8, 36, 28, reload_label, true);

        // Security indicator
        let sec_color = match state.security {
            SecurityDisplay::Https => 0xff_1a7f37,
            SecurityDisplay::Mixed => 0xff_9a6b00,
            SecurityDisplay::Http => 0xff_9a6b00,
            SecurityDisplay::Internal => 0xff_2a6aff,
            SecurityDisplay::Local => 0xff_5a6270,
            SecurityDisplay::CertificateError => 0xff_c62828,
            SecurityDisplay::Failed => 0xff_c62828,
            SecurityDisplay::Unknown => 0xff_5a6270,
        };
        fill(&mut fb, 140, TAB_STRIP_H + 8, 28, 28, 0xff_263653);
        let sec_label = match state.security {
            SecurityDisplay::Https => "A",
            SecurityDisplay::Mixed => "m",
            SecurityDisplay::Http => "h",
            SecurityDisplay::Internal => "A",
            SecurityDisplay::Local => "F",
            SecurityDisplay::CertificateError => "X",
            SecurityDisplay::Failed => "!",
            SecurityDisplay::Unknown => "?",
        };
        blit_text_approx(&mut fb, 148, TAB_STRIP_H + 14, sec_label, sec_color);

        // Bookmark star
        fill(&mut fb, 172, TAB_STRIP_H + 8, 28, 28, 0xff_263653);
        let star = if state.bookmarked { "*" } else { "o" };
        let star_color = if state.bookmarked {
            0xff_c9a227
        } else {
            0xff_5a6270
        };
        blit_text_approx(&mut fb, 180, TAB_STRIP_H + 14, star, star_color);

        // Omnibox
        let omni_x = 204u32;
        let omni_w = w.saturating_sub(omni_x + 12);
        let omni_focused = state.focused_control == ChromeControl::Omnibox;
        fill(
            &mut fb,
            omni_x,
            TAB_STRIP_H + 8,
            omni_w,
            28,
            if omni_focused {
                0xff_ffffff
            } else {
                0xff_243554
            },
        );
        // Focus ring
        if omni_focused {
            fill(&mut fb, omni_x, TAB_STRIP_H + 8, omni_w, 1, 0xff_2a6aff);
            fill(&mut fb, omni_x, TAB_STRIP_H + 35, omni_w, 1, 0xff_2a6aff);
        }
        let omni = &self.browser.chrome.omnibox;
        let text = if omni.editing {
            omni.display_text()
        } else {
            state.tab_url.as_str()
        };
        // Selection highlight (approx by darker background span)
        if omni.editing && omni.has_selection() {
            let (a, b) = omni.selection_range();
            let sx = omni_x + 8 + (a as u32 * 8);
            let sw = ((b - a) as u32 * 8).min(omni_w.saturating_sub(16));
            fill(&mut fb, sx, TAB_STRIP_H + 12, sw, 20, 0xff_c5d8ff);
        }
        blit_text_approx(&mut fb, omni_x + 8, TAB_STRIP_H + 14, text, 0xff_f3f7ff);
        if omni.editing {
            let cx = omni_x + 8 + (omni.caret as u32 * 8);
            fill(&mut fb, cx, TAB_STRIP_H + 12, 2, 20, 0xff_1c1f26);
        }

        if state.private {
            blit_text_approx(
                &mut fb,
                w.saturating_sub(72),
                TAB_STRIP_H + 14,
                "PRIVATE",
                0xff_5b2d8e,
            );
        }

        // Suggestions dropdown
        if omni.editing && !self.browser.chrome.suggestions.is_empty() {
            let mut sy = TOP_CHROME_H;
            for (i, sug) in self.browser.chrome.suggestions.items.iter().enumerate() {
                let bg = if Some(i) == self.browser.chrome.suggestions.selected {
                    0xff_dce6ff
                } else {
                    0xff_ffffff
                };
                fill(&mut fb, omni_x, sy, omni_w, 22, bg);
                blit_text_approx(&mut fb, omni_x + 8, sy + 6, &sug.title, 0xff_1c1f26);
                sy += 22;
            }
        }

        // Site info panel
        if state.site_info_open {
            fill(&mut fb, 140, TOP_CHROME_H, 260, 90, 0xff_ffffff);
            let origin = state
                .origin
                .as_ref()
                .map(|o| o.serialize())
                .unwrap_or_else(|| "opaque".into());
            blit_text_approx(&mut fb, 148, TOP_CHROME_H + 10, "Connection", 0xff_5a6270);
            blit_text_approx(&mut fb, 148, TOP_CHROME_H + 28, &origin, 0xff_1c1f26);
            blit_text_approx(
                &mut fb,
                148,
                TOP_CHROME_H + 48,
                &format!("{:?}", state.security),
                0xff_5a6270,
            );
            blit_text_approx(
                &mut fb,
                148,
                TOP_CHROME_H + 66,
                &state.connection,
                0xff_8a93a3,
            );
        }

        // --- Page content (untrusted) — never drawn into chrome bands ---
        let content_top = TOP_CHROME_H;
        let content_bottom = h.saturating_sub(STATUS_H);
        if let Some(page_fb) = self.browser.window.tabs.active_tab().framebuffer() {
            let max_h = content_bottom.saturating_sub(content_top);
            for y in 0..page_fb.height.min(max_h) {
                for x in 0..page_fb.width.min(w) {
                    fb.pixels[((y + content_top) * w + x) as usize] =
                        page_fb.pixels[(y * page_fb.width + x) as usize];
                }
            }
        }

        // --- Status bar (trusted) ---
        fill(&mut fb, 0, content_bottom, w, STATUS_H, 0xff_e8ebf0);
        if !state.status_text.is_empty() {
            blit_text_approx(
                &mut fb,
                8,
                content_bottom + 6,
                &state.status_text,
                0xff_5a6270,
            );
        } else if state.loading {
            blit_text_approx(&mut fb, 8, content_bottom + 6, "Loading…", 0xff_2a6aff);
        }

        if let Some(window) = &self.window {
            window.set_title(&state.window_title);
        }

        if self.browser.show_hud {
            let report = hud.report();
            let mut yy = content_top + 8;
            for line in report.lines() {
                blit_text_approx(&mut fb, w.saturating_sub(280), yy, line, 0xff_101010);
                yy += 14;
            }
        }

        if let Some(err) = &self.browser.window.tabs.active_tab().context.last_error {
            blit_text_approx(
                &mut fb,
                8,
                content_top + 8,
                &format!("Error: {err}"),
                0xff_c62828,
            );
        }

        let _ = CHROME_H; // documented total chrome including status

        self.composed = fb;
        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        let Ok(mut buffer) = surface.buffer_mut() else {
            return;
        };
        let copy = (w * h) as usize;
        let n = copy.min(buffer.len()).min(self.composed.pixels.len());
        buffer[..n].copy_from_slice(&self.composed.pixels[..n]);
        let _ = buffer.present();
    }
}

fn fill(fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32, color: u32) {
    for yy in y..y.saturating_add(h).min(fb.height) {
        for xx in x..x.saturating_add(w).min(fb.width) {
            fb.pixels[(yy * fb.width + xx) as usize] = color;
        }
    }
}

fn draw_nav_button(
    fb: &mut Framebuffer,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    label: &str,
    enabled: bool,
) {
    let bg = if enabled { 0xff_d8dde6 } else { 0xff_eceff3 };
    let fg = if enabled { 0xff_1c1f26 } else { 0xff_a0a7b4 };
    fill(fb, x, y, w, h, bg);
    blit_text_approx(fb, x + 12, y + 8, label, fg);
}

fn blit_text_approx(fb: &mut Framebuffer, x: u32, y: u32, text: &str, color: u32) {
    let mut cx = x;
    for ch in text.chars().take(72) {
        let glyph = glyph5(ch);
        for row in 0..5u32 {
            for col in 0..3u32 {
                if glyph[row as usize] & (1 << (2 - col)) != 0 {
                    let px = cx + col * 2;
                    let py = y + row * 2;
                    if px < fb.width && py < fb.height {
                        fb.pixels[(py * fb.width + px) as usize] = color;
                    }
                }
            }
        }
        cx += 8;
        if cx >= fb.width {
            break;
        }
    }
}

fn glyph5(c: char) -> [u8; 5] {
    match c.to_ascii_uppercase() {
        'A' => [0b010, 0b101, 0b111, 0b101, 0b101],
        'B' => [0b110, 0b101, 0b110, 0b101, 0b110],
        'C' => [0b011, 0b100, 0b100, 0b100, 0b011],
        'D' => [0b110, 0b101, 0b101, 0b101, 0b110],
        'E' => [0b111, 0b100, 0b110, 0b100, 0b111],
        'F' => [0b111, 0b100, 0b110, 0b100, 0b100],
        'G' => [0b011, 0b100, 0b101, 0b101, 0b011],
        'H' => [0b101, 0b101, 0b111, 0b101, 0b101],
        'I' => [0b111, 0b010, 0b010, 0b010, 0b111],
        'J' => [0b001, 0b001, 0b001, 0b101, 0b010],
        'K' => [0b101, 0b101, 0b110, 0b101, 0b101],
        'L' => [0b100, 0b100, 0b100, 0b100, 0b111],
        'M' => [0b101, 0b111, 0b111, 0b101, 0b101],
        'N' => [0b101, 0b111, 0b111, 0b111, 0b101],
        'O' => [0b010, 0b101, 0b101, 0b101, 0b010],
        'P' => [0b110, 0b101, 0b110, 0b100, 0b100],
        'Q' => [0b010, 0b101, 0b101, 0b111, 0b011],
        'R' => [0b110, 0b101, 0b110, 0b101, 0b101],
        'S' => [0b011, 0b100, 0b010, 0b001, 0b110],
        'T' => [0b111, 0b010, 0b010, 0b010, 0b010],
        'U' => [0b101, 0b101, 0b101, 0b101, 0b111],
        'V' => [0b101, 0b101, 0b101, 0b101, 0b010],
        'W' => [0b101, 0b101, 0b111, 0b111, 0b101],
        'X' => [0b101, 0b101, 0b010, 0b101, 0b101],
        'Y' => [0b101, 0b101, 0b010, 0b010, 0b010],
        'Z' => [0b111, 0b001, 0b010, 0b100, 0b111],
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b010, 0b010, 0b010],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        '.' => [0b000, 0b000, 0b000, 0b000, 0b010],
        ':' => [0b000, 0b010, 0b000, 0b010, 0b000],
        '/' => [0b001, 0b001, 0b010, 0b100, 0b100],
        '-' => [0b000, 0b000, 0b111, 0b000, 0b000],
        '_' => [0b000, 0b000, 0b000, 0b000, 0b111],
        ' ' => [0b000, 0b000, 0b000, 0b000, 0b000],
        '|' => [0b010, 0b010, 0b010, 0b010, 0b010],
        '+' => [0b000, 0b010, 0b111, 0b010, 0b000],
        '*' => [0b101, 0b010, 0b111, 0b010, 0b101],
        '<' => [0b001, 0b010, 0b100, 0b010, 0b001],
        '>' => [0b100, 0b010, 0b001, 0b010, 0b100],
        '!' => [0b010, 0b010, 0b010, 0b000, 0b010],
        '?' => [0b111, 0b001, 0b010, 0b000, 0b010],
        '"' => [0b101, 0b101, 0b000, 0b000, 0b000],
        '\'' => [0b010, 0b010, 0b000, 0b000, 0b000],
        '(' => [0b001, 0b010, 0b010, 0b010, 0b001],
        ')' => [0b100, 0b010, 0b010, 0b010, 0b100],
        ',' => [0b000, 0b000, 0b000, 0b010, 0b100],
        '=' => [0b000, 0b111, 0b000, 0b111, 0b000],
        '%' => [0b101, 0b001, 0b010, 0b100, 0b101],
        '&' => [0b010, 0b101, 0b010, 0b101, 0b011],
        '#' => [0b101, 0b111, 0b101, 0b111, 0b101],
        '@' => [0b010, 0b101, 0b111, 0b100, 0b011],
        _ => [0b111, 0b101, 0b101, 0b101, 0b111],
    }
}
