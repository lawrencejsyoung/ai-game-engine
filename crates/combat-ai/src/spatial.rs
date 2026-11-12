//! Allocation-free (after warm-up) uniform hash grid on the XZ plane.
//!
//! Rebuilt with a counting sort in O(n); radius queries touch only the cells
//! overlapping the query circle. Used for actor neighbor queries every tick and
//! for static cover lookups.

use glam::Vec3;

#[derive(Copy, Clone, Debug)]
struct Entry {
    item: u32,
    cx: i32,
    cz: i32,
    pos: Vec3,
}

/// Uniform spatial hash grid over the XZ plane.
#[derive(Clone, Debug)]
pub struct SpatialGrid {
    cell_size: f32,
    inv_cell: f32,
    mask: u32,
    starts: Vec<u32>,
    entries: Vec<Entry>,
    scratch: Vec<(u32, Entry)>,
}

impl SpatialGrid {
    /// Creates an empty grid with the given cell size (meters).
    pub fn new(cell_size: f32) -> Self {
        assert!(cell_size > 0.0, "cell size must be positive");
        Self {
            cell_size,
            inv_cell: 1.0 / cell_size,
            mask: 0,
            starts: Vec::new(),
            entries: Vec::new(),
            scratch: Vec::new(),
        }
    }

    /// Cell size in meters.
    pub fn cell_size(&self) -> f32 {
        self.cell_size
    }

    /// Number of items in the grid.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the grid is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[inline]
    fn cell_of(&self, p: Vec3) -> (i32, i32) {
        (
            (p.x * self.inv_cell).floor() as i32,
            (p.z * self.inv_cell).floor() as i32,
        )
    }

    #[inline]
    fn hash(&self, cx: i32, cz: i32) -> u32 {
        ((cx as u32).wrapping_mul(0x8DA6_B343) ^ (cz as u32).wrapping_mul(0xD816_3841)) & self.mask
    }

    /// Replaces the grid contents with `items` (`(id, position)` pairs).
    pub fn rebuild(&mut self, items: impl IntoIterator<Item = (u32, Vec3)>) {
        self.scratch.clear();
        for (item, pos) in items {
            let (cx, cz) = self.cell_of(pos);
            self.scratch.push((0, Entry { item, cx, cz, pos }));
        }
        let n = self.scratch.len();
        let table = (n * 2).next_power_of_two().max(64);
        self.mask = (table - 1) as u32;
        self.starts.clear();
        self.starts.resize(table + 1, 0);
        for i in 0..n {
            let e = self.scratch[i].1;
            let h = self.hash(e.cx, e.cz);
            self.scratch[i].0 = h;
            self.starts[h as usize + 1] += 1;
        }
        for i in 1..=table {
            self.starts[i] += self.starts[i - 1];
        }
        self.entries.clear();
        self.entries.resize(
            n,
            Entry {
                item: 0,
                cx: 0,
                cz: 0,
                pos: Vec3::ZERO,
            },
        );
        // Place entries; `starts[h]` is used as a write cursor then restored.
        for &(h, e) in &self.scratch {
            let slot = &mut self.starts[h as usize];
            self.entries[*slot as usize] = e;
            *slot += 1;
        }
        for i in (1..=table).rev() {
            self.starts[i] = self.starts[i - 1];
        }
        self.starts[0] = 0;
    }

    /// Calls `f(id, position)` for every item within `radius` (XZ distance) of `center`.
    #[inline]
    pub fn query(&self, center: Vec3, radius: f32, mut f: impl FnMut(u32, Vec3)) {
        if self.entries.is_empty() {
            return;
        }
        let r2 = radius * radius;
        let (x0, z0) = self.cell_of(center - Vec3::splat(radius));
        let (x1, z1) = self.cell_of(center + Vec3::splat(radius));
        for cz in z0..=z1 {
            for cx in x0..=x1 {
                let h = self.hash(cx, cz) as usize;
                let (s, e) = (self.starts[h] as usize, self.starts[h + 1] as usize);
                for entry in &self.entries[s..e] {
                    // Filter hash collisions so each item is visited exactly once.
                    if entry.cx != cx || entry.cz != cz {
                        continue;
                    }
                    let dx = entry.pos.x - center.x;
                    let dz = entry.pos.z - center.z;
                    if dx * dx + dz * dz <= r2 {
                        f(entry.item, entry.pos);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Rng;

    #[test]
    fn query_matches_brute_force() {
        let mut rng = Rng::new(7);
        let pts: Vec<Vec3> = (0..2000)
            .map(|_| Vec3::new(rng.range(-200.0, 200.0), 0.0, rng.range(-200.0, 200.0)))
            .collect();
        let mut grid = SpatialGrid::new(8.0);
        grid.rebuild(pts.iter().enumerate().map(|(i, p)| (i as u32, *p)));
        for _ in 0..50 {
            let c = Vec3::new(rng.range(-200.0, 200.0), 0.0, rng.range(-200.0, 200.0));
            let r = rng.range(1.0, 60.0);
            let mut got = Vec::new();
            grid.query(c, r, |id, _| got.push(id));
            got.sort_unstable();
            let want: Vec<u32> = pts
                .iter()
                .enumerate()
                .filter(|(_, p)| crate::math::dist2_xz(**p, c) <= r * r)
                .map(|(i, _)| i as u32)
                .collect();
            assert_eq!(got, want);
        }
    }
}
