//! Compositor architecture foundations.
//!
//! Phase 2 keeps compositing simple (one primary content layer + scroll
//! offset) while leaving room for independent scrolling, opacity, and GPU
//! layers in later phases.

use axiom_layout::Rect;
use axiom_paint::{DisplayList, Framebuffer};

#[derive(Debug, Clone, Copy, Default)]
pub struct Transform2D {
    pub tx: f32,
    pub ty: f32,
    pub opacity: f32,
}

impl Transform2D {
    pub fn identity() -> Self {
        Self {
            tx: 0.0,
            ty: 0.0,
            opacity: 1.0,
        }
    }

    pub fn translate(tx: f32, ty: f32) -> Self {
        Self {
            tx,
            ty,
            opacity: 1.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompositorLayer {
    pub id: u64,
    pub bounds: Rect,
    pub transform: Transform2D,
    pub scroll_offset: (f32, f32),
    pub display_list: DisplayList,
    pub clips_content: bool,
}

#[derive(Debug, Default)]
pub struct Compositor {
    pub layers: Vec<CompositorLayer>,
    next_id: u64,
}

impl Compositor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.layers.clear();
    }

    pub fn add_layer(&mut self, bounds: Rect, display_list: DisplayList) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.layers.push(CompositorLayer {
            id,
            bounds,
            transform: Transform2D::identity(),
            scroll_offset: (0.0, 0.0),
            display_list,
            clips_content: true,
        });
        id
    }

    pub fn set_scroll(&mut self, layer_id: u64, x: f32, y: f32) {
        if let Some(layer) = self.layers.iter_mut().find(|l| l.id == layer_id) {
            layer.scroll_offset = (x, y);
            layer.transform.tx = -x;
            layer.transform.ty = -y;
        }
    }

    /// Composite into a framebuffer by re-rasterizing with scroll offset applied
    /// at paint time by the caller. For Phase 2 we expose scroll offsets and let
    /// the engine shift hit-testing / paint origin.
    pub fn primary_scroll(&self) -> (f32, f32) {
        self.layers
            .first()
            .map(|l| l.scroll_offset)
            .unwrap_or((0.0, 0.0))
    }

    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }
}

/// Placeholder for future GPU surfaces.
#[derive(Debug, Clone)]
pub struct Surface {
    pub width: u32,
    pub height: u32,
    pub framebuffer: Framebuffer,
}
