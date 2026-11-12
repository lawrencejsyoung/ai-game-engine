//! A tiny grid level: walls, low walls, A* pathing and line of sight.

use combat_ai::{CoverKind, CoverPoint, Vec3, WorldQuery};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

pub const CELL: f32 = 1.5;
pub const LOW_WALL_HEIGHT: f32 = 1.15;

/// Mirror-symmetric left/right so neither spawn side has an advantage.
pub const LAYOUT: &str = "\
############################################################
#..........................................................#
#..........................................................#
#.....====.........#####............#####.........====.....#
#.............#..............................#.............#
#.....###.....#..............==..............#.....###.....#
#.#......................................................#.#
#.........====...........#........#...........====.........#
#........................#........#........................#
#..###.........###.......#........#.......###.........###..#
#...#............#..........====..........#............#...#
#...#...====.....#........................#.....====...#...#
#......................####......####......................#
#...==.........======..................======.........==...#
#......................####......####......................#
#...#...====.....#........................#.....====...#...#
#...#............#..........====..........#............#...#
#..###.........###.......#........#.......###.........###..#
#........................#........#........................#
#.........====...........#........#...........====.........#
#.#......................................................#.#
#.....###.....#..............==..............#.....###.....#
#.............#..............................#.............#
#.....====.........#####............#####.........====.....#
#..........................................................#
############################################################";

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum Tile {
    Floor,
    Wall,
    Low,
}

pub struct Map {
    pub w: i32,
    pub h: i32,
    tiles: Vec<Tile>,
}

impl Map {
    pub fn parse(layout: &str) -> Self {
        let rows: Vec<&str> = layout.lines().collect();
        let h = rows.len() as i32;
        let w = rows[0].len() as i32;
        let mut tiles = Vec::with_capacity((w * h) as usize);
        for r in &rows {
            assert_eq!(r.len() as i32, w, "ragged map row");
            for c in r.chars() {
                tiles.push(match c {
                    '#' => Tile::Wall,
                    '=' => Tile::Low,
                    _ => Tile::Floor,
                });
            }
        }
        Self { w, h, tiles }
    }

    #[inline]
    pub fn tile(&self, x: i32, z: i32) -> Tile {
        if x < 0 || z < 0 || x >= self.w || z >= self.h {
            Tile::Wall
        } else {
            self.tiles[(z * self.w + x) as usize]
        }
    }

    #[inline]
    pub fn cell_of(p: Vec3) -> (i32, i32) {
        ((p.x / CELL).floor() as i32, (p.z / CELL).floor() as i32)
    }

    #[inline]
    pub fn center(x: i32, z: i32) -> Vec3 {
        Vec3::new((x as f32 + 0.5) * CELL, 0.0, (z as f32 + 0.5) * CELL)
    }

    pub fn walkable(&self, x: i32, z: i32) -> bool {
        self.tile(x, z) == Tile::Floor
    }

    /// One cover point per (floor cell, adjacent obstacle) pair.
    pub fn cover_points(&self) -> Vec<CoverPoint> {
        let mut out = Vec::new();
        for z in 0..self.h {
            for x in 0..self.w {
                if !self.walkable(x, z) {
                    continue;
                }
                for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let kind = match self.tile(x + dx, z + dz) {
                        Tile::Wall => CoverKind::High,
                        Tile::Low => CoverKind::Low,
                        Tile::Floor => continue,
                    };
                    // Skip the outer boundary walls: they don't protect from anything inside.
                    let (nx, nz) = (x + dx, z + dz);
                    if nx == 0 || nz == 0 || nx == self.w - 1 || nz == self.h - 1 {
                        continue;
                    }
                    let c = Self::center(x, z);
                    let facing = Vec3::new(dx as f32, 0.0, dz as f32);
                    out.push(CoverPoint::new(c + facing * (CELL * 0.25), facing, kind));
                }
            }
        }
        out
    }

    /// A* over 8-connected walkable cells (no corner cutting). Returns world waypoints.
    pub fn find_path(&self, from: Vec3, to: Vec3) -> Option<Vec<Vec3>> {
        let (sx, sz) = self.nearest_walkable(Self::cell_of(from))?;
        let (gx, gz) = Self::cell_of(to);
        if !self.walkable(gx, gz) {
            return None;
        }
        let idx = |x: i32, z: i32| (z * self.w + x) as usize;
        let n = (self.w * self.h) as usize;
        let mut g = vec![f32::INFINITY; n];
        let mut parent = vec![u32::MAX; n];
        let mut open = BinaryHeap::new();
        let hfn = |x: i32, z: i32| {
            let (dx, dz) = ((x - gx).abs() as f32, (z - gz).abs() as f32);
            dx.max(dz) + (std::f32::consts::SQRT_2 - 1.0) * dx.min(dz)
        };
        g[idx(sx, sz)] = 0.0;
        // Ties on f are broken toward the goal (smaller h, then smaller |dx|, |dz|),
        // which is mirror-invariant: raw coordinates would bias routes to one side.
        let key = |f: f32, x: i32, z: i32| {
            (
                Reverse(ordered(f)),
                Reverse(ordered(hfn(x, z))),
                Reverse((x - gx).abs()),
                Reverse((z - gz).abs()),
                x,
                z,
            )
        };
        open.push(key(hfn(sx, sz), sx, sz));
        while let Some((_, _, _, _, x, z)) = open.pop() {
            if (x, z) == (gx, gz) {
                let mut cells = vec![(x, z)];
                let mut cur = idx(x, z);
                while parent[cur] != u32::MAX {
                    cur = parent[cur] as usize;
                    cells.push((cur as i32 % self.w, cur as i32 / self.w));
                }
                cells.reverse();
                let mut pts: Vec<Vec3> = cells
                    .iter()
                    .skip(1)
                    .map(|&(x, z)| Self::center(x, z))
                    .collect();
                if let Some(last) = pts.last_mut() {
                    *last = Vec3::new(to.x, 0.0, to.z);
                } else {
                    pts.push(Vec3::new(to.x, 0.0, to.z));
                }
                return Some(pts);
            }
            let gc = g[idx(x, z)];
            for dz in -1..=1 {
                for dx in -1..=1 {
                    if dx == 0 && dz == 0 {
                        continue;
                    }
                    let (nx, nz) = (x + dx, z + dz);
                    if !self.walkable(nx, nz) {
                        continue;
                    }
                    if dx != 0
                        && dz != 0
                        && (!self.walkable(x + dx, z) || !self.walkable(x, z + dz))
                    {
                        continue;
                    }
                    let cost = if dx != 0 && dz != 0 {
                        std::f32::consts::SQRT_2
                    } else {
                        1.0
                    };
                    let ng = gc + cost;
                    let ni = idx(nx, nz);
                    if ng < g[ni] {
                        g[ni] = ng;
                        parent[ni] = idx(x, z) as u32;
                        open.push(key(ng + hfn(nx, nz), nx, nz));
                    }
                }
            }
        }
        None
    }

    fn nearest_walkable(&self, (x, z): (i32, i32)) -> Option<(i32, i32)> {
        if self.walkable(x, z) {
            return Some((x, z));
        }
        for r in 1..3 {
            for dz in -r..=r {
                for dx in -r..=r {
                    if self.walkable(x + dx, z + dz) {
                        return Some((x + dx, z + dz));
                    }
                }
            }
        }
        None
    }
}

/// Total order wrapper for f32 heap keys (scaled to integer millimeters).
fn ordered(f: f32) -> i64 {
    (f * 1000.0) as i64
}

impl WorldQuery for Map {
    fn line_of_sight(&self, from: Vec3, to: Vec3) -> bool {
        let d = to - from;
        let len = d.length();
        let steps = (len / 0.3).ceil().max(1.0) as i32;
        for i in 1..steps {
            let p = from + d * (i as f32 / steps as f32);
            let (x, z) = Self::cell_of(p);
            match self.tile(x, z) {
                Tile::Wall => return false,
                Tile::Low if p.y < LOW_WALL_HEIGHT => return false,
                _ => {}
            }
        }
        true
    }

    fn path_distance(&self, from: Vec3, to: Vec3) -> Option<f32> {
        let path = self.find_path(from, to)?;
        let mut len = 0.0;
        let mut prev = Vec3::new(from.x, 0.0, from.z);
        for p in path {
            len += prev.distance(p);
            prev = p;
        }
        Some(len)
    }
}
