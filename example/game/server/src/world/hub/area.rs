//! Ground areas of skills and attacks (COMBAT.md §4.3), on the 2D plane in
//! cells: circles, oriented boxes (squares, walls, piercing lines) and
//! cones. Every test inflates the shape by the target's body radius.
//!
//! Monster queries never scan the horde: the area's bounding box picks the
//! buckets of the horde's spatial index ([`Horde::near_box`]) and only
//! those candidates run the exact test.

use crate::world::horde::{in_radius, Horde};
use crate::world::protocol::Telegraph;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Area {
    Circle { x: f32, y: f32, r: f32 },
    /// Box centred on `(x, y)`: `half_len` along the unit `dir`,
    /// `half_wid` across it.
    Rect { x: f32, y: f32, dir: (f32, f32), half_len: f32, half_wid: f32 },
    /// Cone from the apex `(x, y)` along `dir`; `arc_cos` is the cosine of
    /// half its angle (−1 = a full circle).
    Arc { x: f32, y: f32, dir: (f32, f32), r: f32, arc_cos: f32 },
}

/// Something touching the apex of a cone counts as inside it (the swing
/// starts at the body, not at a point).
const APEX: f32 = 0.4;

impl Area {
    /// A box from `(ox, oy)` along `dir`.
    pub fn line(ox: f32, oy: f32, dir: (f32, f32), length: f32, width: f32) -> Self {
        let half_len = length.max(0.0) / 2.0;
        Area::Rect { x: ox + dir.0 * half_len, y: oy + dir.1 * half_len, dir, half_len, half_wid: width.max(0.0) / 2.0 }
    }

    /// Where it is anchored: the centre (circles, boxes) or the apex (cones).
    pub fn centre(&self) -> (f32, f32) {
        match *self {
            Area::Circle { x, y, .. } | Area::Rect { x, y, .. } | Area::Arc { x, y, .. } => (x, y),
        }
    }

    /// Bounding box `(x0, y0, x1, y1)`.
    pub fn bounds(&self) -> (f32, f32, f32, f32) {
        match *self {
            Area::Circle { x, y, r } | Area::Arc { x, y, r, .. } => (x - r, y - r, x + r, y + r),
            Area::Rect { x, y, dir, half_len, half_wid } => {
                let ex = (dir.0 * half_len).abs() + (dir.1 * half_wid).abs();
                let ey = (dir.1 * half_len).abs() + (dir.0 * half_wid).abs();
                (x - ex, y - ey, x + ex, y + ey)
            }
        }
    }

    /// Whether a body of radius `body` at `(mx, my)` touches the area.
    pub fn contains(&self, mx: f32, my: f32, body: f32) -> bool {
        match *self {
            Area::Circle { x, y, r } => in_radius(mx - x, my - y, r + body),
            Area::Rect { x, y, dir, half_len, half_wid } => {
                let (dx, dy) = (mx - x, my - y);
                let along = dx * dir.0 + dy * dir.1;
                let across = dx * dir.1 - dy * dir.0;
                along.abs() <= half_len + body && across.abs() <= half_wid + body
            }
            Area::Arc { x, y, dir, r, arc_cos } => {
                let (dx, dy) = (mx - x, my - y);
                if !in_radius(dx, dy, r + body) {
                    return false;
                }
                if arc_cos <= -1.0 {
                    return true;
                }
                let d = (dx * dx + dy * dy).sqrt();
                d <= body + APEX || dx * dir.0 + dy * dir.1 >= arc_cos * d
            }
        }
    }

    /// The same shape anchored at `(nx, ny)` (homing hits, auras).
    pub fn moved_to(self, nx: f32, ny: f32) -> Self {
        match self {
            Area::Circle { r, .. } => Area::Circle { x: nx, y: ny, r },
            Area::Rect { dir, half_len, half_wid, .. } => Area::Rect { x: nx, y: ny, dir, half_len, half_wid },
            Area::Arc { dir, r, arc_cos, .. } => Area::Arc { x: nx, y: ny, dir, r, arc_cos },
        }
    }

    /// Sort key of a target: distance from the anchor; for a box, how far
    /// along it (a non-piercing arrow stops at the first one).
    pub fn order(&self, mx: f32, my: f32) -> f32 {
        match *self {
            Area::Rect { x, y, dir, half_len, .. } => (mx - x) * dir.0 + (my - y) * dir.1 + half_len,
            _ => {
                let (x, y) = self.centre();
                (mx - x).powi(2) + (my - y).powi(2)
            }
        }
    }

    /// Unit direction a hit pushes a monster at `(mx, my)`: away from the
    /// centre, or along the aim for a box.
    pub fn push_dir(&self, mx: f32, my: f32) -> (f32, f32) {
        match *self {
            Area::Rect { dir, .. } => dir,
            Area::Circle { x, y, .. } | Area::Arc { x, y, .. } => {
                let (dx, dy) = (mx - x, my - y);
                let l = (dx * dx + dy * dy).sqrt();
                if l > 1e-4 { (dx / l, dy / l) } else { (1.0, 0.0) }
            }
        }
    }

    /// Where a body inside must go to be just outside (barriers): the
    /// nearest side of a box, radially out of a circle. `None` if outside.
    pub fn eject(&self, mx: f32, my: f32, body: f32) -> Option<(f32, f32)> {
        const GAP: f32 = 0.05;
        if !self.contains(mx, my, body) {
            return None;
        }
        match *self {
            Area::Rect { x, y, dir, half_len, half_wid } => {
                let (dx, dy) = (mx - x, my - y);
                let along = dx * dir.0 + dy * dir.1;
                let across = dx * dir.1 - dy * dir.0;
                let out_len = half_len + body - along.abs();
                let out_wid = half_wid + body - across.abs();
                // Leave by the closer side, keeping the other coordinate.
                if out_len <= out_wid {
                    let a = (half_len + body + GAP) * along.signum();
                    Some((x + dir.0 * a + dir.1 * across, y + dir.1 * a - dir.0 * across))
                } else {
                    let c = (half_wid + body + GAP) * if across < 0.0 { -1.0 } else { 1.0 };
                    Some((x + dir.0 * along + dir.1 * c, y + dir.1 * along - dir.0 * c))
                }
            }
            Area::Circle { x, y, r } | Area::Arc { x, y, r, .. } => {
                let (ux, uy) = self.push_dir(mx, my);
                let d = r + body + GAP;
                Some((x + ux * d, y + uy * d))
            }
        }
    }

    /// Live monsters touching the area, through the horde index.
    pub fn monsters(&self, horde: &Horde, out: &mut Vec<u16>) {
        let (x0, y0, x1, y1) = self.bounds();
        let at = out.len();
        horde.near_box(x0, y0, x1, y1, out);
        let store = horde.store();
        let catalog = horde.catalog();
        let mut keep = at;
        for k in at..out.len() {
            let i = out[k] as usize;
            if self.contains(store.x[i], store.y[i], catalog.get(store.kind[i]).radius) {
                out[keep] = out[k];
                keep += 1;
            }
        }
        out.truncate(keep);
    }

    /// The shape for clients (`id` 0 = a one-off strike).
    pub fn telegraph(&self, id: u32, look: u8, span_ms: u32, follow: u32) -> Telegraph {
        let angle = |d: (f32, f32)| {
            let a = d.1.atan2(d.0).rem_euclid(std::f32::consts::TAU);
            ((a / std::f32::consts::TAU * 256.0).round() as u32 % 256) as u8
        };
        let (shape, x, y, dir, a, b) = match *self {
            Area::Circle { x, y, r } => (0, x, y, 0, r, 0.0),
            Area::Rect { x, y, dir, half_len, half_wid } => (1, x, y, angle(dir), half_len * 2.0, half_wid * 2.0),
            Area::Arc { x, y, dir, r, arc_cos } => (2, x, y, angle(dir), r, arc_cos.clamp(-1.0, 1.0).acos().to_degrees() * 2.0),
        };
        Telegraph { id, shape, look, x, y, dir, a, b, span_ms, follow }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_contain_bodies_on_the_plane() {
        let c = Area::Circle { x: 10.0, y: 10.0, r: 2.0 };
        assert!(c.contains(12.3, 10.0, 0.5) && !c.contains(12.6, 10.0, 0.5));
        // A 4×1 box along +x from (0, 0): a piercing line.
        let l = Area::line(0.0, 0.0, (1.0, 0.0), 4.0, 1.0);
        assert_eq!(l.centre(), (2.0, 0.0));
        assert!(l.contains(3.9, 0.4, 0.0) && !l.contains(3.9, 0.6, 0.0) && !l.contains(-0.2, 0.0, 0.1));
        assert!(l.order(1.0, 0.0) < l.order(3.0, 0.0), "the nearest along the line comes first");
        // Rotated: a wall across +y, 4 long, 0.5 thick.
        let w = Area::Rect { x: 0.0, y: 5.0, dir: (0.0, 1.0), half_len: 0.25, half_wid: 2.0 };
        assert!(w.contains(1.9, 5.0, 0.0) && !w.contains(0.0, 5.6, 0.2));
        let (x0, y0, x1, y1) = w.bounds();
        assert!((x0 + 2.0).abs() < 1e-5 && (x1 - 2.0).abs() < 1e-5 && (y0 - 4.75).abs() < 1e-5 && (y1 - 5.25).abs() < 1e-5);
        // A 90° cone toward +x.
        let a = Area::Arc { x: 0.0, y: 0.0, dir: (1.0, 0.0), r: 3.0, arc_cos: std::f32::consts::FRAC_PI_4.cos() };
        assert!(a.contains(2.0, 1.0, 0.0) && !a.contains(1.0, 2.0, 0.0) && !a.contains(-2.0, 0.0, 0.0));
        assert!(a.contains(-0.3, 0.0, 0.2), "touching the apex counts");
    }

    #[test]
    fn barriers_eject_to_the_nearest_side() {
        let w = Area::Rect { x: 0.0, y: 0.0, dir: (0.0, 1.0), half_len: 0.5, half_wid: 3.0 };
        let (x, y) = w.eject(1.0, -0.2, 0.3).unwrap();
        assert!((x - 1.0).abs() < 1e-5 && y < -0.8, "out through the near long side: ({x}, {y})");
        assert!(!w.contains(x, y, 0.3));
        let c = Area::Circle { x: 0.0, y: 0.0, r: 2.0 };
        let (x, y) = c.eject(0.5, 0.0, 0.3).unwrap();
        assert!(x > 2.3 && y.abs() < 1e-5);
        assert_eq!(c.eject(5.0, 0.0, 0.3), None);
    }

    #[test]
    fn telegraphs_carry_the_resolved_geometry() {
        let t = Area::line(0.0, 0.0, (0.0, 1.0), 8.0, 2.0).telegraph(3, 1, 500, 0);
        assert_eq!((t.shape, t.dir, t.a, t.b, t.x, t.y), (1, 64, 8.0, 2.0, 0.0, 4.0));
        let a = Area::Arc { x: 1.0, y: 1.0, dir: (-1.0, 0.0), r: 2.0, arc_cos: 0.0 }.telegraph(0, 0, 0, 7);
        assert_eq!((a.shape, a.dir, a.follow), (2, 128, 7));
        assert!((a.b - 180.0).abs() < 1e-3);
    }
}
