//! Uniform spatial hash for monster-vs-monster push-out.
//!
//! Rebuilt from scratch every tick with a counting sort (count per bucket,
//! prefix sum, scatter), which leaves each bucket's slots contiguous in one
//! flat array: no per-bucket vectors, no allocation after the first tick,
//! and neighbour scans read memory linearly.
//!
//! Buckets are 2×2 cells (`shift = 1`). Coarser buckets (the classic 16 m
//! sectors) put hundreds of monsters in one bucket once they clump around a
//! player, which makes the pair loop quadratic exactly at peak load.

pub struct SpatialGrid {
    shift: u32,
    cols: i32,
    rows: i32,
    /// `starts[b]..starts[b + 1]` indexes `items` for bucket `b`.
    starts: Vec<u32>,
    items: Vec<u16>,
}

impl SpatialGrid {
    pub fn new(map_width: u32, map_height: u32, shift: u32, capacity: usize) -> Self {
        let cols = ((map_width >> shift) + 1) as i32;
        let rows = ((map_height >> shift) + 1) as i32;
        Self {
            shift,
            cols,
            rows,
            starts: vec![0; (cols * rows) as usize + 1],
            items: vec![0; capacity],
        }
    }

    /// Bucket edge in cells.
    pub fn bucket_size(&self) -> f32 {
        (1u32 << self.shift) as f32
    }

    #[inline]
    pub fn bucket_of(&self, x: f32, y: f32) -> (i32, i32) {
        let bx = ((x.max(0.0) as i32) >> self.shift).min(self.cols - 1);
        let by = ((y.max(0.0) as i32) >> self.shift).min(self.rows - 1);
        (bx, by)
    }

    /// Rebuilds from `(slot, x, y)` entries. `entries` is iterated twice.
    pub fn rebuild<I>(&mut self, entries: I)
    where
        I: Iterator<Item = (u16, f32, f32)> + Clone,
    {
        self.starts.fill(0);
        for (_, x, y) in entries.clone() {
            let (bx, by) = self.bucket_of(x, y);
            self.starts[(by * self.cols + bx) as usize + 1] += 1;
        }
        for b in 1..self.starts.len() {
            self.starts[b] += self.starts[b - 1];
        }
        // Scatter using `starts[b]` as a cursor, then shift back by one bucket.
        for (slot, x, y) in entries {
            let (bx, by) = self.bucket_of(x, y);
            let b = (by * self.cols + bx) as usize;
            let at = self.starts[b] as usize;
            self.items[at] = slot;
            self.starts[b] += 1;
        }
        for b in (1..self.starts.len()).rev() {
            self.starts[b] = self.starts[b - 1];
        }
        self.starts[0] = 0;
    }

    /// Slots in bucket `(bx, by)`; empty outside the grid.
    #[inline]
    pub fn bucket(&self, bx: i32, by: i32) -> &[u16] {
        if bx < 0 || by < 0 || bx >= self.cols || by >= self.rows {
            return &[];
        }
        let b = (by * self.cols + bx) as usize;
        &self.items[self.starts[b] as usize..self.starts[b + 1] as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counting_sort_buckets_every_entry_once() {
        let mut g = SpatialGrid::new(10, 10, 1, 16);
        let pts = [(0u16, 0.5f32, 0.5f32), (1, 1.9, 1.2), (2, 2.1, 0.0), (3, 9.9, 9.9), (4, -3.0, 50.0)];
        g.rebuild(pts.iter().copied());
        assert_eq!(g.bucket(0, 0), &[0, 1]);
        assert_eq!(g.bucket(1, 0), &[2]);
        assert_eq!(g.bucket(4, 4), &[3]);
        assert_eq!(g.bucket(0, 5), &[4], "out-of-map positions clamp to the edge bucket");
        assert!(g.bucket(-1, 0).is_empty());
        g.rebuild(std::iter::empty());
        assert!(g.bucket(0, 0).is_empty());
    }
}
