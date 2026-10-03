//! Background and mask image layers (CSS Backgrounds 3 §3.6–3.10): each layer's tile is
//! sized and placed in its positioning area, then repeated across its painting area.
//! Gradients are resolved against the tile and evaluated per pixel.

use std::collections::HashMap;
use std::f32::consts::TAU;
use std::sync::Arc;

use axiom_layout::{Fragment, RasterImage, Rect};
use axiom_style::background::{
    Gradient, GradientKind, Image, Layer, LinearDirection, RadialExtent,
};
use axiom_style::{BgSize, BoxArea, Color, ImageLayers, Repeat};

/// Loaded `url()` images, keyed by the URL text as the style wrote it.
pub type CssImages = HashMap<String, Arc<RasterImage>>;

#[derive(Debug, Clone)]
pub struct ImageLayer {
    /// Painting area (page coordinates) and its corner radius.
    pub clip: Rect,
    pub radius: f32,
    /// One tile at its position; repeating axes tile from here in both directions.
    pub tile: Rect,
    pub repeat: [bool; 2],
    /// Extra distance between repeated tiles (`space`).
    pub gap: [f32; 2],
    pub source: LayerSource,
}

#[derive(Debug, Clone)]
pub enum LayerSource {
    Image(Arc<RasterImage>),
    Gradient(Arc<ResolvedGradient>),
}

/// A gradient resolved for its tile; coordinates are relative to the tile's corner.
#[derive(Debug, Clone)]
pub struct ResolvedGradient {
    pub shape: GradientShape,
    /// Offsets along the gradient line or ray (fractions, non-decreasing) and colors.
    pub stops: Vec<(f32, Color)>,
    pub repeating: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum GradientShape {
    Linear {
        start: (f32, f32),
        end: (f32, f32),
    },
    Radial {
        center: (f32, f32),
        radii: (f32, f32),
    },
    /// `from` in radians clockwise from "up".
    Conic {
        center: (f32, f32),
        from: f32,
    },
}

/// The border, padding or content box of `f`.
pub(crate) fn box_area(f: &Fragment, area: BoxArea) -> Rect {
    match area {
        BoxArea::BorderBox | BoxArea::Text => f.rect,
        BoxArea::PaddingBox => f.padding_rect(),
        BoxArea::ContentBox => {
            let p = f.padding_rect();
            let pad = &f.style.padding;
            let w = f.rect.width;
            let (t, r, b, l) = (
                pad.top.resolve_or(w, 0.0),
                pad.right.resolve_or(w, 0.0),
                pad.bottom.resolve_or(w, 0.0),
                pad.left.resolve_or(w, 0.0),
            );
            Rect::new(
                p.x + l,
                p.y + t,
                (p.width - l - r).max(0.0),
                (p.height - t - b).max(0.0),
            )
        }
    }
}

/// Corner radius of an inner box of `f`, given the border box's `radius`.
pub(crate) fn area_radius(f: &Fragment, area: BoxArea, radius: f32) -> f32 {
    if radius <= 0.0 || matches!(area, BoxArea::BorderBox | BoxArea::Text) {
        return radius;
    }
    let inset = f.rect.width - box_area(f, area).width;
    (radius - inset / 2.0).max(0.0)
}

/// The paintable layers of `layers` on `f`, bottom layer first. `none` layers and images
/// that have not loaded paint nothing and are left out.
pub(crate) fn resolve(
    f: &Fragment,
    layers: &ImageLayers,
    radius: f32,
    images: &CssImages,
) -> Vec<ImageLayer> {
    layers
        .layers()
        .rev()
        .filter_map(|l| resolve_layer(f, l, radius, images))
        .collect()
}

fn resolve_layer(f: &Fragment, l: Layer, radius: f32, images: &CssImages) -> Option<ImageLayer> {
    let (image, intrinsic) = match l.image {
        Image::None => return None,
        Image::Url(url) => {
            let img = images.get(&**url)?;
            let size = (img.width as f32, img.height as f32);
            (Some(img), (size.0 > 0.0 && size.1 > 0.0).then_some(size))
        }
        Image::Gradient(_) => (None, None),
    };
    let area = box_area(f, l.origin);
    let (pw, ph) = (area.width, area.height);
    let (mut tw, mut th) = match (l.size, intrinsic) {
        (BgSize::Cover | BgSize::Contain, Some((iw, ih))) => {
            let (sx, sy) = (pw / iw, ph / ih);
            let s = if *l.size == BgSize::Cover {
                sx.max(sy)
            } else {
                sx.min(sy)
            };
            (iw * s, ih * s)
        }
        (BgSize::Cover | BgSize::Contain, None) => (pw, ph),
        (BgSize::Explicit(w, h), intrinsic) => match (w.resolve(pw), h.resolve(ph), intrinsic) {
            (Some(w), Some(h), _) => (w, h),
            (Some(w), None, Some((iw, ih))) => (w, w * ih / iw),
            (None, Some(h), Some((iw, ih))) => (h * iw / ih, h),
            (None, None, Some(size)) => size,
            (Some(w), None, None) => (w, ph),
            (None, Some(h), None) => (pw, h),
            (None, None, None) => (pw, ph),
        },
    };
    for (axis, t) in [(0, &mut tw), (1, &mut th)] {
        let len = if axis == 0 { pw } else { ph };
        if l.repeat[axis] == Repeat::Round && *t > 0.0 && len > 0.0 {
            *t = len / (len / *t).round().max(1.0);
        }
    }
    if tw <= 0.0 || th <= 0.0 || !tw.is_finite() || !th.is_finite() {
        return None;
    }
    let mut repeat = [true; 2];
    let mut gap = [0.0; 2];
    let mut pos = [0.0; 2];
    for axis in 0..2 {
        let (start, len, t) = if axis == 0 {
            (area.x, pw, tw)
        } else {
            (area.y, ph, th)
        };
        let offset = l.position[axis].resolve(len - t).unwrap_or(0.0);
        pos[axis] = start + offset;
        match l.repeat[axis] {
            Repeat::NoRepeat => repeat[axis] = false,
            Repeat::Space => {
                let n = (len / t).floor();
                if n >= 2.0 {
                    gap[axis] = (len - n * t) / (n - 1.0);
                    pos[axis] = start;
                } else {
                    repeat[axis] = false;
                }
            }
            Repeat::Repeat | Repeat::Round => {}
        }
    }
    let source = match (image, l.image) {
        (Some(img), _) => LayerSource::Image(img.clone()),
        (None, Image::Gradient(g)) => LayerSource::Gradient(Arc::new(resolve_gradient(g, tw, th))),
        _ => return None,
    };
    Some(ImageLayer {
        clip: box_area(f, l.clip),
        radius: area_radius(f, l.clip, radius),
        tile: Rect::new(pos[0], pos[1], tw, th),
        repeat,
        gap,
        source,
    })
}

fn resolve_gradient(g: &Gradient, w: f32, h: f32) -> ResolvedGradient {
    let (shape, line) = match &g.kind {
        GradientKind::Linear(dir) => {
            let a = match *dir {
                LinearDirection::Angle(deg) => deg.to_radians(),
                // Perpendicular to the diagonal between the other two corners.
                LinearDirection::Corner(sx, sy) => (sx * h).atan2(-sy * w),
            };
            let (dx, dy) = (a.sin(), -a.cos());
            let len = (w * a.sin()).abs() + (h * a.cos()).abs();
            let (cx, cy) = (w / 2.0, h / 2.0);
            let half = len / 2.0;
            (
                GradientShape::Linear {
                    start: (cx - dx * half, cy - dy * half),
                    end: (cx + dx * half, cy + dy * half),
                },
                len,
            )
        }
        GradientKind::Radial {
            circle,
            extent,
            center,
        } => {
            let (cx, cy) = (
                center[0].resolve_or(w, w / 2.0),
                center[1].resolve_or(h, h / 2.0),
            );
            let (xs, ys) = ([cx.abs(), (w - cx).abs()], [cy.abs(), (h - cy).abs()]);
            let (near_x, far_x) = (xs[0].min(xs[1]), xs[0].max(xs[1]));
            let (near_y, far_y) = (ys[0].min(ys[1]), ys[0].max(ys[1]));
            let radii = match (circle, extent) {
                (true, RadialExtent::ClosestSide) => {
                    let r = near_x.min(near_y);
                    (r, r)
                }
                (true, RadialExtent::FarthestSide) => {
                    let r = far_x.max(far_y);
                    (r, r)
                }
                (true, RadialExtent::ClosestCorner) => {
                    let r = near_x.hypot(near_y);
                    (r, r)
                }
                (true, RadialExtent::FarthestCorner) => {
                    let r = far_x.hypot(far_y);
                    (r, r)
                }
                (true, RadialExtent::Size(r, _)) => {
                    let r = r.resolve_or(w, 0.0);
                    (r, r)
                }
                (false, RadialExtent::ClosestSide) => (near_x, near_y),
                (false, RadialExtent::FarthestSide) => (far_x, far_y),
                // The side ellipse scaled to pass through the corner keeps its ratio.
                (false, RadialExtent::ClosestCorner) => (
                    near_x * std::f32::consts::SQRT_2,
                    near_y * std::f32::consts::SQRT_2,
                ),
                (false, RadialExtent::FarthestCorner) => (
                    far_x * std::f32::consts::SQRT_2,
                    far_y * std::f32::consts::SQRT_2,
                ),
                (false, RadialExtent::Size(rx, ry)) => {
                    (rx.resolve_or(w, 0.0), ry.resolve_or(h, 0.0))
                }
            };
            (
                GradientShape::Radial {
                    center: (cx, cy),
                    radii,
                },
                radii.0,
            )
        }
        GradientKind::Conic { from, center } => (
            GradientShape::Conic {
                center: (
                    center[0].resolve_or(w, w / 2.0),
                    center[1].resolve_or(h, h / 2.0),
                ),
                from: from.to_radians(),
            },
            // Conic positions are fractions of a turn stored as percentages.
            100.0,
        ),
    };
    let line = line.max(1e-3);
    let raw: Vec<(Option<f32>, Color)> = g
        .stops
        .iter()
        .map(|s| {
            let at = s.position.as_ref().map(|p| p.resolve_or(line, 0.0) / line);
            (at, s.color)
        })
        .collect();
    ResolvedGradient {
        shape,
        stops: fix_stops(raw),
        repeating: g.repeating,
    }
}

/// CSS Images 3 §3.5.3: missing first and last positions are 0 and 1, positions never
/// go backwards, and runs of missing positions spread evenly between their neighbours.
fn fix_stops(raw: Vec<(Option<f32>, Color)>) -> Vec<(f32, Color)> {
    let n = raw.len();
    let mut pos: Vec<Option<f32>> = raw.iter().map(|s| s.0).collect();
    if n == 0 {
        return Vec::new();
    }
    pos[0].get_or_insert(0.0);
    if n > 1 {
        pos[n - 1].get_or_insert(1.0);
    }
    let mut max = f32::MIN;
    for p in pos.iter_mut().flatten() {
        *p = p.max(max);
        max = *p;
    }
    let mut i = 1;
    while i < n {
        if pos[i].is_some() {
            i += 1;
            continue;
        }
        let start = i - 1;
        let end = (i..n).find(|&j| pos[j].is_some()).unwrap_or(n - 1);
        let (a, b) = (pos[start].unwrap_or(0.0), pos[end].unwrap_or(1.0));
        let steps = (end - start) as f32;
        for (k, slot) in pos.iter_mut().enumerate().take(end).skip(i) {
            *slot = Some(a + (b - a) * (k - start) as f32 / steps);
        }
        i = end + 1;
    }
    pos.into_iter()
        .zip(raw)
        .map(|(p, (_, c))| (p.unwrap_or(0.0), c))
        .collect()
}

/// Interpolates in premultiplied alpha, so fading to `transparent` keeps the hue.
fn mix(a: Color, b: Color, t: f32) -> Color {
    let (aa, ba) = (a.a as f32 / 255.0, b.a as f32 / 255.0);
    let alpha = aa + (ba - aa) * t;
    if alpha <= 0.0 {
        return Color::TRANSPARENT;
    }
    let ch = |x: u8, y: u8| {
        let v = (x as f32 * aa + (y as f32 * ba - x as f32 * aa) * t) / alpha;
        v.round().clamp(0.0, 255.0) as u8
    };
    Color {
        r: ch(a.r, b.r),
        g: ch(a.g, b.g),
        b: ch(a.b, b.b),
        a: (alpha * 255.0).round().clamp(0.0, 255.0) as u8,
    }
}

impl ResolvedGradient {
    /// Color at (`u`, `v`) relative to the tile's corner.
    pub fn color_at(&self, u: f32, v: f32) -> Color {
        let stops = &self.stops;
        let (Some(first), Some(last)) = (stops.first(), stops.last()) else {
            return Color::TRANSPARENT;
        };
        let mut t = match self.shape {
            GradientShape::Linear { start, end } => {
                let (dx, dy) = (end.0 - start.0, end.1 - start.1);
                let len2 = dx * dx + dy * dy;
                if len2 > 0.0 {
                    ((u - start.0) * dx + (v - start.1) * dy) / len2
                } else {
                    0.0
                }
            }
            GradientShape::Radial { center, radii } => {
                if radii.0 <= 0.0 || radii.1 <= 0.0 {
                    return last.1;
                }
                ((u - center.0) / radii.0).hypot((v - center.1) / radii.1)
            }
            GradientShape::Conic { center, from } => {
                let a = (u - center.0).atan2(-(v - center.1));
                (a - from).rem_euclid(TAU) / TAU
            }
        };
        let span = last.0 - first.0;
        if self.repeating && span > 0.0 {
            t = first.0 + (t - first.0).rem_euclid(span);
        }
        if t <= first.0 {
            return first.1;
        }
        if t >= last.0 {
            return last.1;
        }
        let i = stops
            .partition_point(|s| s.0 <= t)
            .clamp(1, stops.len() - 1);
        let (a, b) = (stops[i - 1], stops[i]);
        let f = if b.0 > a.0 {
            (t - a.0) / (b.0 - a.0)
        } else {
            1.0
        };
        mix(a.1, b.1, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Color = Color {
        r: 255,
        g: 0,
        b: 0,
        a: 255,
    };
    const BLUE: Color = Color {
        r: 0,
        g: 0,
        b: 255,
        a: 255,
    };

    #[test]
    fn missing_stop_positions_spread_evenly_and_never_go_backwards() {
        let stops = fix_stops(vec![
            (None, RED),
            (Some(0.5), BLUE),
            (None, RED),
            (None, BLUE),
            (Some(0.2), RED),
        ]);
        let at: Vec<f32> = stops.iter().map(|s| s.0).collect();
        assert_eq!(at, [0.0, 0.5, 0.5, 0.5, 0.5]);
        let even = fix_stops(vec![(None, RED), (None, BLUE), (None, RED)]);
        assert_eq!(
            even.iter().map(|s| s.0).collect::<Vec<_>>(),
            [0.0, 0.5, 1.0]
        );
    }

    #[test]
    fn fading_to_transparent_keeps_the_hue() {
        let c = mix(RED, Color::TRANSPARENT, 0.5);
        assert_eq!((c.r, c.g, c.b), (255, 0, 0));
        assert!((126..=129).contains(&c.a));
    }

    #[test]
    fn corner_gradients_run_perpendicular_to_the_other_diagonal() {
        let g = Gradient {
            kind: GradientKind::Linear(LinearDirection::Corner(1.0, 1.0)),
            stops: vec![
                axiom_style::background::ColorStop {
                    color: RED,
                    position: None,
                },
                axiom_style::background::ColorStop {
                    color: BLUE,
                    position: None,
                },
            ],
            repeating: false,
        };
        let r = resolve_gradient(&g, 200.0, 100.0);
        // The other diagonal's corners sit on the midline: same color.
        let (tr, bl) = (r.color_at(200.0, 0.0), r.color_at(0.0, 100.0));
        assert_eq!(tr, bl);
        assert_eq!(r.color_at(0.0, 0.0), RED);
        assert_eq!(r.color_at(200.0, 100.0), BLUE);
    }
}
