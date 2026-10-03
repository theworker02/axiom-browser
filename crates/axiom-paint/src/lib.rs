//! Display list construction and CPU rasterization for Axiom.
//!
//! The display list is built from the layout fragment tree in CSS painting order
//! (CSS 2 Appendix E, simplified): each positioned box paints as its own layer,
//! ordered by `z-index` within its parent layer, after the normal flow. Glyphs are
//! drawn exactly where layout shaped them.

mod layers;

use std::sync::{Arc, OnceLock};

use axiom_layout::axiom_text::{self, ShapedRun};
use axiom_layout::{BoxEdges, Fragment, FragmentKind, LayoutTree, RasterImage, Rect, Shape};
use axiom_style::{
    BorderStyle, BoxArea, Color, ComputedStyle, Float, Position, LINE_THROUGH, OVERLINE, UNDERLINE,
};

pub use layers::{CssImages, GradientShape, ImageLayer, LayerSource, ResolvedGradient};

#[derive(Debug, Clone)]
pub enum DisplayCommand {
    FillRect {
        rect: Rect,
        color: Color,
    },
    FillRoundedRect {
        rect: Rect,
        radius: f32,
        color: Color,
    },
    /// Border of `rect` (the border box); sides are top, right, bottom, left.
    Border {
        rect: Rect,
        widths: [f32; 4],
        colors: [Color; 4],
        styles: [BorderStyle; 4],
        radius: f32,
    },
    /// A shaped run; glyph positions are relative to (`x`, `baseline`).
    Text {
        text: String,
        x: f32,
        baseline: f32,
        font_size: f32,
        color: Color,
        run: Arc<ShapedRun>,
    },
    Image {
        rect: Rect,
        image: Arc<RasterImage>,
    },
    Shape {
        rect: Rect,
        shape: Shape,
        color: Color,
    },
    /// A background image layer, tiled across its painting area.
    ImageLayer(Box<ImageLayer>),
    /// Starts a group over `bounds`: what follows composites as one unit at `PopGroup`.
    PushGroup(Rect),
    /// Ends the innermost group, keeping its paint only where the mask layers cover it.
    PopGroup {
        mask: Vec<ImageLayer>,
    },
    /// Replaces the clip rectangle (page coordinates) for the following commands.
    SetClip(Option<Rect>),
    /// Replaces the opacity multiplier for the following commands.
    SetOpacity(f32),
}

impl DisplayCommand {
    /// Page-space bounds, or `None` for state commands.
    fn bounds(&self) -> Option<Rect> {
        match self {
            DisplayCommand::FillRect { rect, .. }
            | DisplayCommand::FillRoundedRect { rect, .. }
            | DisplayCommand::Border { rect, .. }
            | DisplayCommand::Image { rect, .. }
            | DisplayCommand::Shape { rect, .. } => Some(*rect),
            DisplayCommand::ImageLayer(layer) => Some(layer.clip),
            DisplayCommand::Text {
                x,
                baseline,
                font_size,
                run,
                ..
            } => Some(Rect::new(
                x - font_size,
                baseline - font_size * 1.5,
                run.width + font_size * 2.0,
                font_size * 2.5,
            )),
            DisplayCommand::SetClip(_)
            | DisplayCommand::SetOpacity(_)
            | DisplayCommand::PushGroup(_)
            | DisplayCommand::PopGroup { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct DisplayList {
    pub commands: Vec<DisplayCommand>,
}

pub fn build_display_list(tree: &LayoutTree) -> DisplayList {
    static NO_IMAGES: OnceLock<CssImages> = OnceLock::new();
    build_display_list_with_images(tree, NO_IMAGES.get_or_init(CssImages::new))
}

/// Like [`build_display_list`], painting `url()` background and mask images from `images`.
pub fn build_display_list_with_images(tree: &LayoutTree, images: &CssImages) -> DisplayList {
    let mut b = Builder {
        list: DisplayList::default(),
        clip: None,
        opacity: 1.0,
        images,
    };
    let canvas = Rect::new(
        0.0,
        0.0,
        tree.viewport_width.max(tree.root.rect.right()),
        tree.content_height.max(tree.viewport_height),
    );
    b.list.commands.push(DisplayCommand::FillRect {
        rect: canvas,
        color: tree.canvas_background,
    });
    b.layer(&tree.root, None, 1.0);
    b.list
}

struct Deferred<'f> {
    frag: &'f Fragment,
    z: i32,
    clip: Option<Rect>,
    opacity: f32,
    /// A float: painted after the layer's normal flow, before positioned layers.
    float: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Gather positioned layers and (with `floats`) floats.
    Collect { floats: bool },
    /// Backgrounds and borders of block-level boxes.
    Blocks,
    /// Text, inline boxes, atomic inlines and replaced content.
    Inlines,
}

struct Builder<'i> {
    list: DisplayList,
    clip: Option<Rect>,
    opacity: f32,
    images: &'i CssImages,
}

fn clips(f: &Fragment) -> bool {
    matches!(f.kind, FragmentKind::Box(_))
        && (f.style.overflow_x.clips() || f.style.overflow_y.clips())
}

fn intersect(clip: Option<Rect>, r: Rect) -> Option<Rect> {
    Some(clip.map_or(r, |c| c.intersect(&r)))
}

/// Whether `child` starts its own paint layer instead of painting in `parent`'s flow:
/// positioned and masked boxes do. Text and replaced content share their box's style,
/// so style identity tells them apart.
fn starts_layer(child: &Fragment, parent: &Fragment, layer_style: &Arc<ComputedStyle>) -> bool {
    (child.style.is_positioned() || child.style.mask.has_images())
        && !Arc::ptr_eq(&child.style, &parent.style)
        && !Arc::ptr_eq(&child.style, layer_style)
}

/// Union of the rects of `f` and its descendants.
fn subtree_bounds(f: &Fragment) -> Rect {
    f.children
        .iter()
        .fold(f.rect, |acc, c| acc.union(&subtree_bounds(c)))
}

fn border_radius(f: &Fragment, edges: BoxEdges) -> f32 {
    let r = f.rect;
    if edges.left && edges.right {
        f.style
            .border_radius
            .min(r.width / 2.0)
            .min(r.height / 2.0)
            .max(0.0)
    } else {
        0.0
    }
}

impl Builder<'_> {
    fn state(&mut self, clip: Option<Rect>, opacity: f32) {
        if clip != self.clip {
            self.list.commands.push(DisplayCommand::SetClip(clip));
            self.clip = clip;
        }
        if opacity != self.opacity {
            self.list.commands.push(DisplayCommand::SetOpacity(opacity));
            self.opacity = opacity;
        }
    }

    /// Paints `root` and its subtree as one layer.
    fn layer(&mut self, root: &Fragment, clip: Option<Rect>, opacity: f32) {
        let opacity = opacity * root.style.opacity;
        if opacity <= 0.0 {
            return;
        }
        let masked = root.style.mask.has_images() && !root.style.visibility_hidden;
        if masked {
            self.push(DisplayCommand::PushGroup(subtree_bounds(root)));
        }
        self.layer_contents(root, clip, opacity);
        if masked {
            let edges = match root.kind {
                FragmentKind::Box(edges) => edges,
                _ => BoxEdges::BOTH,
            };
            let radius = border_radius(root, edges);
            let mask = layers::resolve(root, &root.style.mask, radius, self.images);
            self.push(DisplayCommand::PopGroup { mask });
        }
    }

    fn layer_contents(&mut self, root: &Fragment, clip: Option<Rect>, opacity: f32) {
        self.paint_self(root, clip, opacity);
        let inner = if clips(root) {
            intersect(clip, root.padding_rect())
        } else {
            clip
        };
        // CSS 2 Appendix E: negative z-index layers, block backgrounds, floats, inline
        // content, then positioned layers with z-index >= 0.
        let mut deferred = Vec::new();
        self.flow(
            root,
            inner,
            inner,
            opacity,
            &root.style,
            Phase::Collect { floats: true },
            Some(&mut deferred),
        );
        deferred.sort_by_key(|d| d.z);
        for d in deferred.iter().filter(|d| d.z < 0 && !d.float) {
            self.layer(d.frag, d.clip, d.opacity);
        }
        self.flow(
            root,
            inner,
            inner,
            opacity,
            &root.style,
            Phase::Blocks,
            None,
        );
        for d in deferred.iter().filter(|d| d.float) {
            self.layer(d.frag, d.clip, d.opacity);
        }
        self.flow(
            root,
            inner,
            inner,
            opacity,
            &root.style,
            Phase::Inlines,
            None,
        );
        for d in deferred.iter().filter(|d| d.z >= 0 && !d.float) {
            self.layer(d.frag, d.clip, d.opacity);
        }
    }

    /// Paints an inline-level box and its content in one go (its own block
    /// backgrounds, floats and inline content, but not its positioned descendants).
    fn atomic(
        &mut self,
        f: &Fragment,
        clip: Option<Rect>,
        cb_clip: Option<Rect>,
        opacity: f32,
        layer_style: &Arc<ComputedStyle>,
    ) {
        self.paint_self(f, clip, opacity);
        if f.children.is_empty() {
            return;
        }
        let inner = if clips(f) {
            intersect(clip, f.padding_rect())
        } else {
            clip
        };
        let inner_cb = if f.style.is_positioned() {
            inner
        } else {
            cb_clip
        };
        let mut floats = Vec::new();
        self.flow(
            f,
            inner,
            inner_cb,
            opacity,
            layer_style,
            Phase::Collect { floats: true },
            Some(&mut floats),
        );
        self.flow(
            f,
            inner,
            inner_cb,
            opacity,
            layer_style,
            Phase::Blocks,
            None,
        );
        for d in floats.iter().filter(|d| d.float) {
            self.layer(d.frag, d.clip, d.opacity);
        }
        self.flow(
            f,
            inner,
            inner_cb,
            opacity,
            layer_style,
            Phase::Inlines,
            None,
        );
    }

    /// Walks `f`'s normal-flow descendants: collects the layers (and floats) to paint
    /// later, or paints block backgrounds or inline content.
    #[allow(clippy::too_many_arguments)]
    fn flow<'f>(
        &mut self,
        f: &'f Fragment,
        clip: Option<Rect>,
        cb_clip: Option<Rect>,
        opacity: f32,
        layer_style: &Arc<ComputedStyle>,
        phase: Phase,
        mut collect: Option<&mut Vec<Deferred<'f>>>,
    ) {
        for c in &f.children {
            if starts_layer(c, f, layer_style) {
                if let Some(out) = collect.as_deref_mut() {
                    out.push(Deferred {
                        frag: c,
                        z: c.style.z_index.unwrap_or(0),
                        clip: match c.style.position {
                            Position::Fixed => None,
                            Position::Absolute => cb_clip,
                            _ => clip,
                        },
                        opacity,
                        float: false,
                    });
                }
                continue;
            }
            let is_float = matches!(c.kind, FragmentKind::Box(_))
                && c.style.float != Float::None
                && !Arc::ptr_eq(&c.style, &f.style);
            if is_float {
                if let (Some(out), Phase::Collect { floats: true }) =
                    (collect.as_deref_mut(), phase)
                {
                    out.push(Deferred {
                        frag: c,
                        z: 0,
                        clip,
                        opacity,
                        float: true,
                    });
                }
                continue;
            }
            let op = if Arc::ptr_eq(&c.style, &f.style) {
                opacity
            } else {
                opacity * c.style.opacity
            };
            if op <= 0.0 {
                continue;
            }
            let inline_level = match &c.kind {
                FragmentKind::Box(edges) => edges.inline,
                FragmentKind::AbsPlaceholder(_) => false,
                _ => true,
            };
            let inner = if clips(c) {
                intersect(clip, c.padding_rect())
            } else {
                clip
            };
            let inner_cb = if c.style.is_positioned() {
                inner
            } else {
                cb_clip
            };
            match phase {
                Phase::Collect { .. } => {
                    // Floats inside an inline-level box paint with that box.
                    let next = if inline_level {
                        Phase::Collect { floats: false }
                    } else {
                        phase
                    };
                    self.flow(
                        c,
                        inner,
                        inner_cb,
                        op,
                        layer_style,
                        next,
                        collect.as_deref_mut(),
                    );
                }
                Phase::Blocks if inline_level => {}
                Phase::Blocks => {
                    self.paint_self(c, clip, op);
                    self.flow(c, inner, inner_cb, op, layer_style, phase, None);
                }
                Phase::Inlines if inline_level => self.atomic(c, clip, cb_clip, op, layer_style),
                Phase::Inlines => self.flow(c, inner, inner_cb, op, layer_style, phase, None),
            }
        }
    }

    fn push(&mut self, cmd: DisplayCommand) {
        self.list.commands.push(cmd);
    }

    fn paint_self(&mut self, f: &Fragment, clip: Option<Rect>, opacity: f32) {
        let s = &f.style;
        if s.visibility_hidden {
            return;
        }
        match &f.kind {
            FragmentKind::Box(edges) => {
                self.state(clip, opacity);
                self.box_decorations(f, *edges);
            }
            FragmentKind::Text(t) => {
                self.state(clip, opacity);
                self.push(DisplayCommand::Text {
                    text: t.text.clone(),
                    x: f.rect.x,
                    baseline: t.baseline,
                    font_size: t.font_size,
                    color: t.color,
                    run: t.run.clone(),
                });
                if t.decorations != 0 {
                    let thickness = (t.font_size / 14.0).max(1.0);
                    let line = |y: f32| DisplayCommand::FillRect {
                        rect: Rect::new(f.rect.x, y, f.rect.width, thickness),
                        color: t.decoration_color,
                    };
                    let mut cmds = Vec::new();
                    if t.decorations & UNDERLINE != 0 {
                        cmds.push(line(t.baseline + t.font_size * 0.1));
                    }
                    if t.decorations & OVERLINE != 0 {
                        cmds.push(line(f.rect.y));
                    }
                    if t.decorations & LINE_THROUGH != 0 {
                        cmds.push(line(t.baseline - t.font_size * 0.3));
                    }
                    self.list.commands.extend(cmds);
                }
            }
            FragmentKind::Image(image) => {
                self.state(clip, opacity);
                self.push(DisplayCommand::Image {
                    rect: f.rect,
                    image: image.clone(),
                });
            }
            FragmentKind::Shape(shape, color) => {
                self.state(clip, opacity);
                self.push(DisplayCommand::Shape {
                    rect: f.rect,
                    shape: *shape,
                    color: *color,
                });
            }
            FragmentKind::AbsPlaceholder(_) => {}
        }
    }

    fn box_decorations(&mut self, f: &Fragment, edges: BoxEdges) {
        let s = &f.style;
        let r = f.rect;
        if r.width <= 0.0 || r.height <= 0.0 {
            return;
        }
        let radius = border_radius(f, edges);
        // The color paints in the bottom layer's painting area.
        let color_clip =
            s.background.clip[(s.background.image.len() - 1) % s.background.clip.len()];
        if s.background_color.a > 0 && color_clip != BoxArea::Text {
            let rect = layers::box_area(f, color_clip);
            let radius = layers::area_radius(f, color_clip, radius);
            self.push(if radius > 0.0 {
                DisplayCommand::FillRoundedRect {
                    rect,
                    radius,
                    color: s.background_color,
                }
            } else {
                DisplayCommand::FillRect {
                    rect,
                    color: s.background_color,
                }
            });
        }
        if s.background.has_images() {
            for layer in layers::resolve(f, &s.background, radius, self.images) {
                self.push(DisplayCommand::ImageLayer(Box::new(layer)));
            }
        }
        let mut widths = s.border_widths();
        if !edges.left {
            widths[3] = 0.0;
        }
        if !edges.right {
            widths[1] = 0.0;
        }
        if widths.iter().any(|w| *w > 0.0) {
            let colors = [0, 1, 2, 3].map(|i| s.border[i].color);
            if colors.iter().zip(widths).any(|(c, w)| c.a > 0 && w > 0.0) {
                self.push(DisplayCommand::Border {
                    rect: r,
                    widths,
                    colors,
                    styles: [0, 1, 2, 3].map(|i| s.border[i].style),
                    radius,
                });
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Framebuffer {
    pub width: u32,
    pub height: u32,
    /// Pixels as `0xAABBGGRR` (red in the low byte).
    pub pixels: Vec<u32>,
}

impl Framebuffer {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0xff_ff_ff_ff; (width * height) as usize],
        }
    }

    pub fn fill(&mut self, color: u32) {
        self.pixels.fill(color);
    }

    pub fn put_pixel(&mut self, x: i32, y: i32, color: u32) {
        if x < 0 || y < 0 {
            return;
        }
        let x = x as u32;
        let y = y as u32;
        if x >= self.width || y >= self.height {
            return;
        }
        self.pixels[(y * self.width + x) as usize] = color;
    }

    pub fn to_ppm(&self) -> Vec<u8> {
        let mut out = format!("P6\n{} {}\n255\n", self.width, self.height).into_bytes();
        out.reserve((self.width * self.height * 3) as usize);
        for px in &self.pixels {
            out.push((px & 0xff) as u8);
            out.push(((px >> 8) & 0xff) as u8);
            out.push(((px >> 16) & 0xff) as u8);
        }
        out
    }
}

/// Rasterizes the page region starting at the page origin.
pub fn rasterize(list: &DisplayList, width: u32, height: u32) -> Framebuffer {
    rasterize_region(list, 0.0, 0.0, width, height)
}

/// Rasterizes the `width`×`height` page region whose top-left corner is at page
/// coordinates (`x`, `y`). Commands outside the region are skipped.
pub fn rasterize_region(
    list: &DisplayList,
    x: f32,
    y: f32,
    width: u32,
    height: u32,
) -> Framebuffer {
    let mut r = Raster {
        fb: Framebuffer::new(width, height),
        ox: x,
        oy: y,
        clip: DeviceClip::full(width, height),
        opacity: 1.0,
        groups: Vec::new(),
    };
    let region = Rect::new(x, y, width as f32, height as f32);
    for cmd in &list.commands {
        if let Some(b) = cmd.bounds() {
            if b.right() < region.x
                || b.x > region.right()
                || b.bottom() < region.y
                || b.y > region.bottom()
            {
                continue;
            }
        }
        r.command(cmd);
    }
    r.fb
}

/// Clip in device pixels, as fractional edges.
#[derive(Clone, Copy)]
struct DeviceClip {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

impl DeviceClip {
    fn full(w: u32, h: u32) -> Self {
        Self {
            x0: 0.0,
            y0: 0.0,
            x1: w as f32,
            y1: h as f32,
        }
    }
}

struct Raster {
    fb: Framebuffer,
    ox: f32,
    oy: f32,
    clip: DeviceClip,
    opacity: f32,
    groups: Vec<Group>,
}

/// An open group: the pixels its device rectangle held when it started.
struct Group {
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    backdrop: Vec<u32>,
}

/// Straight-alpha color of `image` at texel coordinates (`u`, `v`), bilinear.
fn bilinear(image: &RasterImage, u: f32, v: f32) -> Color {
    let (iw, ih) = (image.width as i32, image.height as i32);
    let texel = |x: i32, y: i32| -> [f32; 4] {
        let i = ((y.clamp(0, ih - 1) * iw + x.clamp(0, iw - 1)) * 4) as usize;
        let p = &image.rgba[i..i + 4];
        [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]
    };
    let (ux, fx) = (u.floor() as i32, u - u.floor());
    let (vy, fy) = (v.floor() as i32, v - v.floor());
    let (a, b, c, d) = (
        texel(ux, vy),
        texel(ux + 1, vy),
        texel(ux, vy + 1),
        texel(ux + 1, vy + 1),
    );
    let mut px = [0.0f32; 4];
    for k in 0..4 {
        let top = a[k] + (b[k] - a[k]) * fx;
        let bottom = c[k] + (d[k] - c[k]) * fx;
        px[k] = top + (bottom - top) * fy;
    }
    Color {
        r: px[0].round() as u8,
        g: px[1].round() as u8,
        b: px[2].round() as u8,
        a: px[3].round() as u8,
    }
}

/// Snaps an edge span to whole device pixels, keeping a visible span at least 1px.
fn snap(a0: f32, a1: f32) -> (f32, f32) {
    let s0 = a0.round();
    let s1 = a1.round();
    if s1 <= s0 && a1 > a0 {
        (s0, s0 + 1.0)
    } else {
        (s0, s1)
    }
}

/// Border widths snapped like box edges: whole pixels, and at least 1px when non-zero.
fn snap_width(w: f32) -> f32 {
    if w <= 0.0 {
        0.0
    } else {
        w.round().max(1.0)
    }
}

fn overlap(a0: f32, a1: f32, b0: f32, b1: f32) -> f32 {
    (a1.min(b1) - a0.max(b0)).clamp(0.0, 1.0)
}

fn shade(c: Color, factor: f32) -> Color {
    let f = |v: u8| (v as f32 * factor).round().clamp(0.0, 255.0) as u8;
    Color {
        r: f(c.r),
        g: f(c.g),
        b: f(c.b),
        a: c.a,
    }
}

fn lighten(c: Color) -> Color {
    let f = |v: u8| (v as f32 + (255.0 - v as f32) * 0.5) as u8;
    Color {
        r: f(c.r),
        g: f(c.g),
        b: f(c.b),
        a: c.a,
    }
}

/// Signed distance from (`px`, `py`) to a rounded rectangle (negative inside).
fn rounded_distance(px: f32, py: f32, x0: f32, y0: f32, x1: f32, y1: f32, radius: f32) -> f32 {
    let cx = (x0 + x1) / 2.0;
    let cy = (y0 + y1) / 2.0;
    let hx = (x1 - x0) / 2.0 - radius;
    let hy = (y1 - y0) / 2.0 - radius;
    let qx = (px - cx).abs() - hx;
    let qy = (py - cy).abs() - hy;
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - radius
}

fn segment_distance(px: f32, py: f32, (ax, ay): (f32, f32), (bx, by): (f32, f32)) -> f32 {
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    ((px - ax - t * dx).powi(2) + (py - ay - t * dy).powi(2)).sqrt()
}

/// Coverage of a convex polygon (clockwise in screen space) at a pixel centre.
fn polygon_coverage(px: f32, py: f32, pts: &[(f32, f32)]) -> f32 {
    let mut d = f32::MAX;
    for i in 0..pts.len() {
        let (ax, ay) = pts[i];
        let (bx, by) = pts[(i + 1) % pts.len()];
        let (ex, ey) = (bx - ax, by - ay);
        let len = (ex * ex + ey * ey).sqrt().max(1e-6);
        // Distance to the inside of the edge.
        d = d.min(((px - ax) * ey - (py - ay) * ex) / -len);
    }
    (d + 0.5).clamp(0.0, 1.0)
}

impl Raster {
    fn command(&mut self, cmd: &DisplayCommand) {
        match cmd {
            DisplayCommand::FillRect { rect, color } => self.fill_rect(*rect, *color),
            DisplayCommand::FillRoundedRect {
                rect,
                radius,
                color,
            } => self.fill_rounded(*rect, *radius, *color),
            DisplayCommand::Border {
                rect,
                widths,
                colors,
                styles,
                radius,
            } => self.border(*rect, *widths, *colors, *styles, *radius),
            DisplayCommand::Text {
                x,
                baseline,
                font_size,
                color,
                run,
                ..
            } => self.text(*x, *baseline, *font_size, *color, run),
            DisplayCommand::Image { rect, image } => self.image(*rect, image),
            DisplayCommand::Shape { rect, shape, color } => self.shape(*rect, *shape, *color),
            DisplayCommand::ImageLayer(layer) => self.image_layer(layer),
            DisplayCommand::PushGroup(bounds) => self.push_group(*bounds),
            DisplayCommand::PopGroup { mask } => self.pop_group(mask),
            DisplayCommand::SetClip(clip) => {
                let full = DeviceClip::full(self.fb.width, self.fb.height);
                self.clip = match clip {
                    None => full,
                    Some(c) => DeviceClip {
                        x0: (c.x - self.ox).round().max(full.x0),
                        y0: (c.y - self.oy).round().max(full.y0),
                        x1: (c.right() - self.ox).round().min(full.x1),
                        y1: (c.bottom() - self.oy).round().min(full.y1),
                    },
                };
            }
            DisplayCommand::SetOpacity(o) => self.opacity = o.clamp(0.0, 1.0),
        }
    }

    fn blend(&mut self, x: i32, y: i32, color: Color, coverage: f32) {
        if x < 0 || y < 0 || x as u32 >= self.fb.width || y as u32 >= self.fb.height {
            return;
        }
        let a = coverage * self.opacity * color.a as f32 / 255.0;
        if a <= 0.0 {
            return;
        }
        let idx = (y as u32 * self.fb.width + x as u32) as usize;
        if a >= 0.999 {
            self.fb.pixels[idx] =
                0xff00_0000 | (color.b as u32) << 16 | (color.g as u32) << 8 | color.r as u32;
            return;
        }
        let dst = self.fb.pixels[idx];
        let mix = |s: u8, d: u32| (s as f32 * a + (d & 0xff) as f32 * (1.0 - a)).round() as u32;
        let r = mix(color.r, dst);
        let g = mix(color.g, dst >> 8);
        let b = mix(color.b, dst >> 16);
        self.fb.pixels[idx] = 0xff00_0000 | b << 16 | g << 8 | r;
    }

    /// Integer pixel bounds of a device rectangle, limited by the clip.
    fn span(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> Option<(i32, i32, i32, i32)> {
        let c = self.clip;
        let (x0, y0, x1, y1) = (x0.max(c.x0), y0.max(c.y0), x1.min(c.x1), y1.min(c.y1));
        (x1 > x0 && y1 > y0).then(|| {
            (
                x0.floor() as i32,
                y0.floor() as i32,
                x1.ceil() as i32,
                y1.ceil() as i32,
            )
        })
    }

    /// Clip coverage of a pixel (fractional clip edges).
    fn clip_coverage(&self, x: i32, y: i32) -> f32 {
        let c = self.clip;
        let (fx, fy) = (x as f32, y as f32);
        overlap(fx, fx + 1.0, c.x0, c.x1) * overlap(fy, fy + 1.0, c.y0, c.y1)
    }

    /// Fills a device-space rectangle with anti-aliased edges.
    fn fill_device(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, color: Color) {
        let Some((ix0, iy0, ix1, iy1)) = self.span(x0, y0, x1, y1) else {
            return;
        };
        for y in iy0..iy1 {
            let cy = overlap(y as f32, y as f32 + 1.0, y0, y1);
            for x in ix0..ix1 {
                let cx = overlap(x as f32, x as f32 + 1.0, x0, x1);
                let cov = cx * cy * self.clip_coverage(x, y);
                self.blend(x, y, color, cov);
            }
        }
    }

    fn fill_rect(&mut self, r: Rect, color: Color) {
        if color.a == 0 {
            return;
        }
        let (x0, x1) = snap(r.x - self.ox, r.right() - self.ox);
        let (y0, y1) = snap(r.y - self.oy, r.bottom() - self.oy);
        self.fill_device(x0, y0, x1, y1, color);
    }

    /// Fills pixels of a device-space bounding box with `coverage(px, py)` at centres.
    fn fill_with(
        &mut self,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        color: Color,
        coverage: impl Fn(f32, f32) -> f32,
    ) {
        let Some((ix0, iy0, ix1, iy1)) = self.span(x0, y0, x1, y1) else {
            return;
        };
        for y in iy0..iy1 {
            for x in ix0..ix1 {
                let cov = coverage(x as f32 + 0.5, y as f32 + 0.5) * self.clip_coverage(x, y);
                self.blend(x, y, color, cov);
            }
        }
    }

    fn fill_rounded(&mut self, r: Rect, radius: f32, color: Color) {
        if color.a == 0 {
            return;
        }
        let (x0, x1) = snap(r.x - self.ox, r.right() - self.ox);
        let (y0, y1) = snap(r.y - self.oy, r.bottom() - self.oy);
        let radius = radius.min((x1 - x0) / 2.0).min((y1 - y0) / 2.0);
        self.fill_with(x0, y0, x1, y1, color, |px, py| {
            if (px > x0 + radius && px < x1 - radius) || (py > y0 + radius && py < y1 - radius) {
                overlap(px - 0.5, px + 0.5, x0, x1) * overlap(py - 0.5, py + 0.5, y0, y1)
            } else {
                (0.5 - rounded_distance(px, py, x0, y0, x1, y1, radius)).clamp(0.0, 1.0)
            }
        });
    }

    fn border(
        &mut self,
        r: Rect,
        widths: [f32; 4],
        colors: [Color; 4],
        styles: [BorderStyle; 4],
        radius: f32,
    ) {
        let (x0, x1) = snap(r.x - self.ox, r.right() - self.ox);
        let (y0, y1) = snap(r.y - self.oy, r.bottom() - self.oy);
        let widths = widths.map(snap_width);
        let [t, rt, b, l] = widths;
        let side_color = |i: usize| -> Color {
            let c = colors[i];
            let top_left = i == 0 || i == 3;
            match styles[i] {
                BorderStyle::Inset | BorderStyle::Groove if top_left => shade(c, 0.6),
                BorderStyle::Outset | BorderStyle::Ridge if !top_left => shade(c, 0.6),
                BorderStyle::Inset
                | BorderStyle::Outset
                | BorderStyle::Groove
                | BorderStyle::Ridge => lighten(c),
                _ => c,
            }
        };
        if radius > 0.0 {
            let (ix0, iy0, ix1, iy1) = (x0 + l, y0 + t, x1 - rt, y1 - b);
            let inner_r = (radius - widths.iter().cloned().fold(0.0, f32::max)).max(0.0);
            self.fill_with(x0, y0, x1, y1, side_color(0), |px, py| {
                let outer =
                    (0.5 - rounded_distance(px, py, x0, y0, x1, y1, radius)).clamp(0.0, 1.0);
                let inner = if ix1 > ix0 && iy1 > iy0 {
                    (0.5 - rounded_distance(px, py, ix0, iy0, ix1, iy1, inner_r)).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                (outer - inner).max(0.0)
            });
            return;
        }
        // Top and bottom span the full width; left and right fit between them.
        let sides = [
            (x0, y0, x1, y0 + t, true),
            (x1 - rt, y0 + t, x1, y1 - b, false),
            (x0, y1 - b, x1, y1, true),
            (x0, y0 + t, x0 + l, y1 - b, false),
        ];
        for (i, &(sx0, sy0, sx1, sy1, horizontal)) in sides.iter().enumerate() {
            let w = widths[i];
            if w <= 0.0 || colors[i].a == 0 {
                continue;
            }
            let c = side_color(i);
            match styles[i] {
                BorderStyle::None | BorderStyle::Hidden => {}
                BorderStyle::Double if w >= 3.0 => {
                    let third = w / 3.0;
                    if horizontal {
                        self.fill_device(sx0, sy0, sx1, sy0 + third, c);
                        self.fill_device(sx0, sy1 - third, sx1, sy1, c);
                    } else {
                        self.fill_device(sx0, sy0, sx0 + third, sy1, c);
                        self.fill_device(sx1 - third, sy0, sx1, sy1, c);
                    }
                }
                style @ (BorderStyle::Dashed | BorderStyle::Dotted) => {
                    let dash = if style == BorderStyle::Dotted {
                        w
                    } else {
                        (w * 3.0).max(3.0)
                    };
                    let len = if horizontal { sx1 - sx0 } else { sy1 - sy0 };
                    let mut pos = 0.0;
                    while pos < len {
                        let end = (pos + dash).min(len);
                        if horizontal {
                            self.fill_device(sx0 + pos, sy0, sx0 + end, sy1, c);
                        } else {
                            self.fill_device(sx0, sy0 + pos, sx1, sy0 + end, c);
                        }
                        pos += dash * 2.0;
                    }
                }
                _ => self.fill_device(sx0, sy0, sx1, sy1, c),
            }
        }
    }

    fn text(&mut self, x: f32, baseline: f32, font_size: f32, color: Color, run: &ShapedRun) {
        if color.a == 0 {
            return;
        }
        let c = self.clip;
        for g in &run.glyphs {
            let bm = axiom_text::glyph_bitmap(g, font_size);
            if bm.width == 0 {
                continue;
            }
            let gx = (x + g.x - self.ox).round() as i32 + bm.left;
            let gy = (baseline + g.y - self.oy).round() as i32 - bm.top;
            if gx as f32 > c.x1
                || gy as f32 > c.y1
                || ((gx + bm.width as i32) as f32) < c.x0
                || ((gy + bm.height as i32) as f32) < c.y0
            {
                continue;
            }
            for row in 0..bm.height as i32 {
                for col in 0..bm.width as i32 {
                    let cov = bm.coverage[(row * bm.width as i32 + col) as usize];
                    if cov == 0 {
                        continue;
                    }
                    let (px, py) = (gx + col, gy + row);
                    let clip = self.clip_coverage(px, py);
                    self.blend(px, py, color, cov as f32 / 255.0 * clip);
                }
            }
        }
    }

    fn image(&mut self, r: Rect, image: &RasterImage) {
        if image.width == 0 || image.height == 0 || r.width <= 0.0 || r.height <= 0.0 {
            return;
        }
        let (x0, x1) = snap(r.x - self.ox, r.right() - self.ox);
        let (y0, y1) = snap(r.y - self.oy, r.bottom() - self.oy);
        let Some((ix0, iy0, ix1, iy1)) = self.span(x0, y0, x1, y1) else {
            return;
        };
        let sx = image.width as f32 / (x1 - x0);
        let sy = image.height as f32 / (y1 - y0);
        for y in iy0..iy1 {
            let v = (y as f32 + 0.5 - y0) * sy - 0.5;
            for x in ix0..ix1 {
                let u = (x as f32 + 0.5 - x0) * sx - 0.5;
                let color = bilinear(image, u, v);
                self.blend(x, y, color, self.clip_coverage(x, y));
            }
        }
    }

    /// Coverage of the device pixel centred at (`px`, `py`) by a layer's painting area.
    fn area_coverage(&self, layer: &ImageLayer, px: f32, py: f32) -> f32 {
        let r = layer.clip;
        let (x0, x1) = snap(r.x - self.ox, r.right() - self.ox);
        let (y0, y1) = snap(r.y - self.oy, r.bottom() - self.oy);
        if layer.radius > 0.0 {
            let radius = layer.radius.min((x1 - x0) / 2.0).min((y1 - y0) / 2.0);
            (0.5 - rounded_distance(px, py, x0, y0, x1, y1, radius)).clamp(0.0, 1.0)
        } else {
            overlap(px - 0.5, px + 0.5, x0, x1) * overlap(py - 0.5, py + 0.5, y0, y1)
        }
    }

    /// Color of `layer` at the device pixel centred at (`px`, `py`); `None` between tiles.
    fn layer_sample(&self, layer: &ImageLayer, px: f32, py: f32) -> Option<Color> {
        let t = layer.tile;
        // Tiles start on whole device pixels, so images at their natural size stay sharp.
        let (tx, ty) = ((t.x - self.ox).round(), (t.y - self.oy).round());
        let wrap = |d: f32, len: f32, repeat: bool, gap: f32| -> Option<f32> {
            let d = if repeat { d.rem_euclid(len + gap) } else { d };
            (d >= 0.0 && d < len).then_some(d)
        };
        let u = wrap(px - tx, t.width, layer.repeat[0], layer.gap[0])?;
        let v = wrap(py - ty, t.height, layer.repeat[1], layer.gap[1])?;
        Some(match &layer.source {
            LayerSource::Image(img) => bilinear(
                img,
                u * img.width as f32 / t.width - 0.5,
                v * img.height as f32 / t.height - 0.5,
            ),
            LayerSource::Gradient(g) => g.color_at(u, v),
        })
    }

    fn image_layer(&mut self, layer: &ImageLayer) {
        let r = layer.clip;
        let Some((ix0, iy0, ix1, iy1)) = self.span(
            r.x - self.ox,
            r.y - self.oy,
            r.right() - self.ox,
            r.bottom() - self.oy,
        ) else {
            return;
        };
        for y in iy0..iy1 {
            for x in ix0..ix1 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let cov = self.area_coverage(layer, px, py) * self.clip_coverage(x, y);
                if cov <= 0.0 {
                    continue;
                }
                if let Some(color) = self.layer_sample(layer, px, py) {
                    self.blend(x, y, color, cov);
                }
            }
        }
    }

    fn push_group(&mut self, bounds: Rect) {
        let (w, h) = (self.fb.width as f32, self.fb.height as f32);
        let x0 = (bounds.x - self.ox).floor().clamp(0.0, w) as u32;
        let y0 = (bounds.y - self.oy).floor().clamp(0.0, h) as u32;
        let x1 = (bounds.right() - self.ox).ceil().clamp(0.0, w) as u32;
        let y1 = (bounds.bottom() - self.oy).ceil().clamp(0.0, h) as u32;
        let mut backdrop = Vec::with_capacity(((x1 - x0) * (y1.saturating_sub(y0))) as usize);
        for y in y0..y1 {
            let row = (y * self.fb.width) as usize;
            backdrop.extend_from_slice(&self.fb.pixels[row + x0 as usize..row + x1 as usize]);
        }
        self.groups.push(Group {
            x0,
            y0,
            x1,
            y1,
            backdrop,
        });
    }

    /// Composites the group through its mask: `backdrop + (painted - backdrop) × mask`,
    /// which is the group drawn source-over with its alpha scaled by the mask. The mask is
    /// the union of the layers' alpha, each within its painting area.
    fn pop_group(&mut self, mask: &[ImageLayer]) {
        let Some(g) = self.groups.pop() else {
            return;
        };
        let width = (g.x1 - g.x0) as usize;
        for y in g.y0..g.y1 {
            for x in g.x0..g.x1 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let clear = mask.iter().fold(1.0f32, |clear, layer| {
                    let a = self
                        .layer_sample(layer, px, py)
                        .map_or(0.0, |c| c.a as f32 / 255.0);
                    clear * (1.0 - a * self.area_coverage(layer, px, py))
                });
                let m = 1.0 - clear;
                if m >= 0.999 {
                    continue;
                }
                let idx = (y * self.fb.width + x) as usize;
                let back = g.backdrop[(y - g.y0) as usize * width + (x - g.x0) as usize];
                if m <= 0.001 {
                    self.fb.pixels[idx] = back;
                    continue;
                }
                let front = self.fb.pixels[idx];
                let ch = |shift: u32| {
                    let (b, f) = ((back >> shift) & 0xff, (front >> shift) & 0xff);
                    (b as f32 + (f as f32 - b as f32) * m).round() as u32
                };
                self.fb.pixels[idx] = 0xff00_0000 | ch(16) << 16 | ch(8) << 8 | ch(0);
            }
        }
    }

    fn shape(&mut self, r: Rect, shape: Shape, color: Color) {
        let (x0, y0) = (r.x - self.ox, r.y - self.oy);
        let (w, h) = (r.width, r.height);
        let (x1, y1) = (x0 + w, y0 + h);
        let (cx, cy) = (x0 + w / 2.0, y0 + h / 2.0);
        let radius = w.min(h) / 2.0;
        match shape {
            Shape::Disc | Shape::RadioDot => self.fill_with(x0, y0, x1, y1, color, |px, py| {
                (radius - ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() + 0.5).clamp(0.0, 1.0)
            }),
            Shape::Circle => {
                let stroke = (radius * 0.3).max(1.0);
                self.fill_with(x0, y0, x1, y1, color, |px, py| {
                    let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
                    (stroke / 2.0 - (d - (radius - stroke / 2.0)).abs() + 0.5).clamp(0.0, 1.0)
                })
            }
            Shape::Square => self.fill_device(x0, y0, x1, y1, color),
            Shape::DisclosureClosed => {
                let pts = [(x0, y0), (x1, cy), (x0, y1)];
                self.fill_with(x0, y0, x1, y1, color, |px, py| {
                    polygon_coverage(px, py, &pts)
                })
            }
            Shape::DisclosureOpen | Shape::DropdownArrow => {
                let top = y0 + h * 0.2;
                let pts = [(x0, top), (x1, top), (cx, y1 - h * 0.2)];
                self.fill_with(x0, y0, x1, y1, color, |px, py| {
                    polygon_coverage(px, py, &pts)
                })
            }
            Shape::Check => {
                let stroke = (w * 0.14).max(1.5);
                let a = (x0 + w * 0.18, y0 + h * 0.52);
                let b = (x0 + w * 0.42, y0 + h * 0.76);
                let c = (x0 + w * 0.84, y0 + h * 0.26);
                self.fill_with(x0, y0, x1, y1, color, |px, py| {
                    let d = segment_distance(px, py, a, b).min(segment_distance(px, py, b, c));
                    (stroke / 2.0 - d + 0.5).clamp(0.0, 1.0)
                })
            }
        }
    }
}

#[cfg(test)]
mod tests;
