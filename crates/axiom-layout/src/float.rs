//! Float placement within one block formatting context (CSS 2 §9.5.1).
//!
//! Horizontal coordinates are absolute; vertical ones are relative to the border-box
//! top of the box that established the formatting context.

use axiom_style::Clear;

#[derive(Debug, Clone, Copy)]
struct Placed {
    left: bool,
    /// Margin box.
    x0: f32,
    x1: f32,
    y0: f32,
    y1: f32,
}

#[derive(Debug, Default)]
pub(crate) struct Floats {
    placed: Vec<Placed>,
    /// A float's top may not be above the top of an earlier float.
    last_top: f32,
}

impl Floats {
    pub fn is_empty(&self) -> bool {
        self.placed.is_empty()
    }

    /// Left and right edges of the space next to the floats for content occupying
    /// `[y, y + h)` inside `[x0, x1]`. The right edge may be less than the left one when
    /// floats leave no room.
    pub fn band(&self, y: f32, h: f32, x0: f32, x1: f32) -> (f32, f32) {
        let bottom = y + h.max(0.01);
        let (mut l, mut r) = (x0, x1);
        for f in &self.placed {
            if f.y0 < bottom && f.y1 > y {
                if f.left {
                    l = l.max(f.x1);
                } else {
                    r = r.min(f.x0);
                }
            }
        }
        (l, r)
    }

    /// The nearest float bottom edge below `y` (where the space next to floats may grow).
    pub fn next_bottom(&self, y: f32) -> Option<f32> {
        self.placed
            .iter()
            .map(|f| f.y1)
            .filter(|&b| b > y + 0.001)
            .min_by(f32::total_cmp)
    }

    /// Bottom of the floats `clear` moves a box below.
    pub fn clearance(&self, clear: Clear) -> Option<f32> {
        self.placed
            .iter()
            .filter(|f| match clear {
                Clear::None => false,
                Clear::Left => f.left,
                Clear::Right => !f.left,
                Clear::Both => true,
            })
            .map(|f| f.y1)
            .max_by(f32::total_cmp)
    }

    /// Bottom of all floats (0 without floats).
    pub fn bottom(&self) -> f32 {
        self.placed.iter().map(|f| f.y1).fold(0.0, f32::max)
    }

    /// Places a float whose margin box is `w`×`h` as high as possible, but not above
    /// `y_min`, inside `[x0, x1]`. Returns the margin-box origin.
    pub fn place(
        &mut self,
        left: bool,
        w: f32,
        h: f32,
        y_min: f32,
        x0: f32,
        x1: f32,
    ) -> (f32, f32) {
        let mut y = y_min.max(self.last_top);
        let (x, y) = loop {
            let (l, r) = self.band(y, h, x0, x1);
            let narrowed = l > x0 || r < x1;
            if r - l >= w - 0.01 || !narrowed {
                break (if left { l } else { r - w }, y);
            }
            match self.next_bottom(y) {
                Some(b) => y = b,
                None => break (if left { l } else { r - w }, y),
            }
        };
        self.placed.push(Placed {
            left,
            x0: x,
            x1: x + w,
            y0: y,
            y1: y + h,
        });
        self.last_top = y;
        (x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_stack_sideways_then_wrap_below() {
        let mut f = Floats::default();
        assert_eq!(f.place(true, 60.0, 20.0, 0.0, 0.0, 100.0), (0.0, 0.0));
        assert_eq!(f.place(false, 30.0, 40.0, 0.0, 0.0, 100.0), (70.0, 0.0));
        // No room beside both: below the shorter one.
        assert_eq!(f.place(true, 50.0, 10.0, 0.0, 0.0, 100.0), (0.0, 20.0));
        assert_eq!(f.band(5.0, 1.0, 0.0, 100.0), (60.0, 70.0));
        assert_eq!(f.band(25.0, 1.0, 0.0, 100.0), (50.0, 70.0));
        assert_eq!(f.band(45.0, 1.0, 0.0, 100.0), (0.0, 100.0));
        assert_eq!(f.clearance(Clear::Left), Some(30.0));
        assert_eq!(f.clearance(Clear::Right), Some(40.0));
        assert_eq!(f.next_bottom(0.0), Some(20.0));
        assert_eq!(f.bottom(), 40.0);
    }
}
