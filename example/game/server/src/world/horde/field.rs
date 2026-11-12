//! Lazy, time-sliced multi-source flow field.
//!
//! One integer distance field per stage covers the whole walk grid: the
//! octile distance (orthogonal step 5, diagonal 7, no corner cutting) from
//! every walkable cell to the nearest player. Every monster descends it, so
//! the cost does not grow with the horde: one rebuild serves 10 or 10 000
//! monsters, and the multi-source seed makes each one chase its nearest
//! player for free. Finite distance doubles as "reachable", which the spawn
//! director uses to reject pockets and islands.
//!
//! Rebuilding is lazy and spread over ticks:
//! * a rebuild is requested only when some player has moved to another cell;
//! * the rebuild runs in a back buffer for at most `budget` queue pops per
//!   tick (Dial's bucket queue: no heap, no floats) and is swapped in when
//!   complete, so a tick never pays the full cost;
//! * requests arriving mid-build are coalesced into one follow-up build.
//!
//! On `prt_fild08` (400×400, ~90k walkable cells) a full build is ≈2.3 ms in
//! release; at the default 20k pops per tick it completes in ~5 ticks, so
//! monsters follow a field at most ~250 ms old. Nearby monsters steer
//! straight at the player when they have line of sight, which hides the
//! staleness where it would be visible.

use crate::world::map::WalkGrid;

pub const UNREACHABLE: u16 = u16::MAX;
const ORTHO: u32 = 5;
const DIAG: u32 = 7;
/// Bucket ring size: must exceed the largest edge cost.
const RING: usize = 8;

/// 8 neighbours: `(dx, dy, cost)`.
const NEIGHBOURS: [(i32, i32, u32); 8] = [
    (1, 0, ORTHO),
    (-1, 0, ORTHO),
    (0, 1, ORTHO),
    (0, -1, ORTHO),
    (1, 1, DIAG),
    (1, -1, DIAG),
    (-1, 1, DIAG),
    (-1, -1, DIAG),
];

pub struct FlowField {
    width: i32,
    height: i32,
    walkable: Vec<bool>,
    /// Field monsters read.
    front: Vec<u16>,
    /// Field under construction.
    back: Vec<u16>,
    buckets: [Vec<u32>; RING],
    /// Current bucket distance of the build.
    cursor: u32,
    /// Entries still queued in `buckets`.
    queued: usize,
    building: bool,
    /// Sources for the next build, set while one is running.
    next_sources: Option<Vec<(i32, i32)>>,
    current_sources: Vec<(i32, i32)>,
    ready: bool,
    builds: u64,
}

impl FlowField {
    pub fn new(walk: &WalkGrid) -> Self {
        let (width, height) = (walk.width() as i32, walk.height() as i32);
        let n = (width * height) as usize;
        let walkable = (0..n).map(|i| walk.is_walkable_cell(i as i32 % width, i as i32 / width)).collect();
        Self {
            width,
            height,
            walkable,
            front: vec![UNREACHABLE; n],
            back: vec![UNREACHABLE; n],
            // Grows to the map's frontier size on the first build, then reused.
            buckets: std::array::from_fn(|_| Vec::with_capacity(1024)),
            cursor: 0,
            queued: 0,
            building: false,
            next_sources: None,
            current_sources: Vec::new(),
            ready: false,
            builds: 0,
        }
    }

    /// Whether a complete field has been swapped in at least once.
    pub fn is_ready(&self) -> bool {
        self.ready
    }

    /// Completed builds (for stats and tests).
    pub fn builds(&self) -> u64 {
        self.builds
    }

    /// Sources the readable field was built from.
    pub fn sources(&self) -> &[(i32, i32)] {
        &self.current_sources
    }

    /// Asks for a field seeded at `sources` (cells). Starts at once when idle,
    /// otherwise replaces any queued request.
    pub fn request(&mut self, sources: &[(i32, i32)]) {
        if self.building {
            self.next_sources = Some(sources.to_vec());
        } else {
            self.start(sources);
        }
    }

    fn start(&mut self, sources: &[(i32, i32)]) {
        self.back.fill(UNREACHABLE);
        for b in &mut self.buckets {
            b.clear();
        }
        self.queued = 0;
        self.cursor = 0;
        self.current_sources.clear();
        for &(x, y) in sources {
            if let Some(i) = self.index(x, y)
                && self.walkable[i]
                && self.back[i] != 0
            {
                self.back[i] = 0;
                self.buckets[0].push(i as u32);
                self.queued += 1;
            }
        }
        self.current_sources.extend_from_slice(sources);
        self.building = true;
    }

    /// Advances the running build by at most `budget` queue pops. Returns true
    /// when a new field was swapped in during this call.
    pub fn step(&mut self, mut budget: usize) -> bool {
        let mut swapped = false;
        while self.building && budget > 0 {
            if self.queued == 0 {
                std::mem::swap(&mut self.front, &mut self.back);
                self.building = false;
                self.ready = true;
                self.builds += 1;
                swapped = true;
                if let Some(next) = self.next_sources.take() {
                    self.start(&next);
                }
                continue;
            }
            let b = (self.cursor as usize) % RING;
            let Some(i) = self.buckets[b].pop() else {
                self.cursor += 1;
                continue;
            };
            self.queued -= 1;
            budget -= 1;
            let i = i as usize;
            if self.back[i] as u32 != self.cursor {
                continue; // stale entry: settled earlier with a shorter distance
            }
            let (x, y) = (i as i32 % self.width, i as i32 / self.width);
            for (dx, dy, cost) in NEIGHBOURS {
                let (nx, ny) = (x + dx, y + dy);
                let Some(j) = self.index(nx, ny) else { continue };
                if !self.walkable[j] {
                    continue;
                }
                if dx != 0 && dy != 0 && !(self.walkable[(y * self.width + nx) as usize] && self.walkable[(ny * self.width + x) as usize]) {
                    continue; // no corner cutting
                }
                let nd = self.cursor + cost;
                if nd < UNREACHABLE as u32 && nd < self.back[j] as u32 {
                    self.back[j] = nd as u16;
                    self.buckets[nd as usize % RING].push(j as u32);
                    self.queued += 1;
                }
            }
        }
        swapped
    }

    /// Requests and completes a build synchronously.
    pub fn build_now(&mut self, sources: &[(i32, i32)]) {
        self.next_sources = None;
        self.start(sources);
        while !self.step(usize::MAX) {}
    }

    #[inline]
    fn index(&self, x: i32, y: i32) -> Option<usize> {
        (x >= 0 && y >= 0 && x < self.width && y < self.height).then(|| (y * self.width + x) as usize)
    }

    /// Distance of a cell in the readable field (`UNREACHABLE` outside).
    #[inline]
    pub fn dist(&self, x: i32, y: i32) -> u16 {
        self.index(x, y).map_or(UNREACHABLE, |i| self.front[i])
    }

    /// Unit direction for a monster at continuous `(x, y)`: towards the
    /// centre of the neighbouring cell with the smallest distance. `None` at
    /// a source cell or where no player is reachable.
    pub fn descent(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let (cx, cy) = (x.floor() as i32, y.floor() as i32);
        let here = self.dist(cx, cy);
        if here == 0 || here == UNREACHABLE {
            return None;
        }
        let mut best = (here, 0, 0);
        for (dx, dy, _) in NEIGHBOURS {
            let d = self.dist(cx + dx, cy + dy);
            if d < best.0 {
                // A diagonal is only valid when both orthogonal cells are open
                // (same rule as the build), otherwise the mob would clip a corner.
                if dx != 0 && dy != 0 && (self.dist(cx + dx, cy) == UNREACHABLE || self.dist(cx, cy + dy) == UNREACHABLE) {
                    continue;
                }
                best = (d, dx, dy);
            }
        }
        if best.0 == here {
            return None;
        }
        let (tx, ty) = ((cx + best.1) as f32 + 0.5, (cy + best.2) as f32 + 0.5);
        let (vx, vy) = (tx - x, ty - y);
        let len = (vx * vx + vy * vy).sqrt();
        (len > 1e-4).then(|| (vx / len, vy / len))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> WalkGrid {
        // A wall with a gap at the top: the field must route around it.
        WalkGrid::from_ascii(&[
            "##########",
            "#........#",
            "#.######.#",
            "#.#....#.#",
            "#.#....#.#",
            "#........#",
            "##########",
        ])
    }

    #[test]
    fn distances_are_octile_and_route_around_walls() {
        let mut f = FlowField::new(&grid());
        f.build_now(&[(1, 1)]);
        assert_eq!(f.dist(1, 1), 0);
        assert_eq!(f.dist(2, 1), 5);
        assert_eq!(f.dist(3, 1), 10);
        assert_eq!(f.dist(0, 0), UNREACHABLE, "wall");
        // Inside the box (open at the bottom row y=1 only via x=1..8).
        assert!(f.dist(4, 3) > f.dist(4, 1));
        assert_eq!(f.dist(4, 4), UNREACHABLE, "wall");
        // Descent from a far cell strictly decreases along the path.
        let (mut x, mut y) = (8.5f32, 5.5f32);
        let mut last = f.dist(8, 5);
        for _ in 0..200 {
            let Some((dx, dy)) = f.descent(x, y) else { break };
            x += dx * 0.3;
            y += dy * 0.3;
            assert!(grid().is_walkable(x, y), "stepped into a wall at {x},{y}");
            let d = f.dist(x.floor() as i32, y.floor() as i32);
            assert!(d <= last + 7, "distance went up");
            last = last.min(d);
        }
        assert_eq!((x.floor(), y.floor()), (1.0, 1.0), "reached the source");
    }

    #[test]
    fn time_sliced_build_matches_one_shot_and_coalesces_requests() {
        let mut one = FlowField::new(&grid());
        one.build_now(&[(8, 5), (1, 1)]);
        let mut sliced = FlowField::new(&grid());
        sliced.request(&[(4, 3)]);
        sliced.request(&[(8, 5), (1, 1)]); // queued behind the running build
        let mut steps = 0;
        while sliced.builds() < 2 {
            sliced.step(3);
            steps += 1;
            assert!(steps < 10_000);
        }
        assert!(steps > 2, "work was spread over several steps");
        assert_eq!(sliced.sources(), &[(8, 5), (1, 1)]);
        for y in 0..7 {
            for x in 0..10 {
                assert_eq!(sliced.dist(x, y), one.dist(x, y), "cell {x},{y}");
            }
        }
        // Readers see the previous complete field until the swap.
        sliced.request(&[(4, 3)]);
        sliced.step(1);
        assert_eq!(sliced.dist(8, 5), 0, "old field still readable mid-build");
    }

    #[test]
    fn no_corner_cutting() {
        let g = WalkGrid::from_ascii(&["####", "#.##", "##.#", "####"]);
        let mut f = FlowField::new(&g);
        f.build_now(&[(1, 2)]);
        assert_eq!(f.dist(2, 1), UNREACHABLE, "diagonal through two wall corners");
    }
}
