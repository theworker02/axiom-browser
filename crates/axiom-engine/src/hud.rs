//! Performance HUD measurements.

use std::time::Instant;

#[derive(Debug, Clone, Default)]
pub struct HudStats {
    pub fps: f32,
    pub frame_ms: f64,
    pub js_ms: f64,
    pub style_ms: f64,
    pub layout_ms: f64,
    pub paint_ms: f64,
    pub composite_ms: f64,
    pub gpu_ms: f64,
    pub network_ms: f64,
    pub dom_nodes: usize,
    pub layout_nodes: usize,
    pub paint_commands: usize,
    pub gpu_batches: usize,
    pub memory_mb: f64,
    pub network_requests: usize,
    pub network_bytes: u64,
    pub cache_hits: usize,
}

impl HudStats {
    pub fn report(&self) -> String {
        format!(
            "AXIOM ENGINE\n\n\
             FPS                {:>6.0}\n\
             Frame              {:>6.1} ms\n\n\
             JavaScript         {:>6.1} ms\n\
             Style              {:>6.1} ms\n\
             Layout             {:>6.1} ms\n\
             Paint              {:>6.1} ms\n\
             Composite          {:>6.1} ms\n\
             GPU/Raster         {:>6.1} ms\n\n\
             DOM nodes          {:>6}\n\
             Layout nodes       {:>6}\n\
             Paint commands     {:>6}\n\
             GPU batches        {:>6}\n\
             Memory (est.)      {:>6.1} MB\n\n\
             Network:\n\
             Requests           {:>6}\n\
             Transferred        {:>6} KB\n\
             Cache entries      {:>6}",
            self.fps,
            self.frame_ms,
            self.js_ms,
            self.style_ms,
            self.layout_ms,
            self.paint_ms,
            self.composite_ms,
            self.gpu_ms,
            self.dom_nodes,
            self.layout_nodes,
            self.paint_commands,
            self.gpu_batches,
            self.memory_mb,
            self.network_requests,
            self.network_bytes / 1024,
            self.cache_hits
        )
    }
}

#[derive(Debug)]
pub struct FrameClock {
    last: Instant,
    frames: u32,
    window_start: Instant,
    fps: f32,
}

impl Default for FrameClock {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            last: now,
            frames: 0,
            window_start: now,
            fps: 0.0,
        }
    }
}

impl FrameClock {
    pub fn tick(&mut self) -> f64 {
        let now = Instant::now();
        let frame_ms = now.duration_since(self.last).as_secs_f64() * 1000.0;
        self.last = now;
        self.frames += 1;
        if now.duration_since(self.window_start).as_secs_f64() >= 1.0 {
            self.fps = self.frames as f32 / now.duration_since(self.window_start).as_secs_f32();
            self.frames = 0;
            self.window_start = now;
        }
        frame_ms
    }

    pub fn fps(&self) -> f32 {
        self.fps
    }
}
