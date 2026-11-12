//! Small math helpers and a fast deterministic RNG.

use glam::{Vec2, Vec3};

/// Horizontal (XZ-plane) distance squared. The engine treats +Y as up.
#[inline]
pub fn dist2_xz(a: Vec3, b: Vec3) -> f32 {
    let dx = a.x - b.x;
    let dz = a.z - b.z;
    dx * dx + dz * dz
}

/// Returns `v` flattened onto the XZ plane and normalized, or `fallback` if degenerate.
#[inline]
pub fn flat_dir(v: Vec3, fallback: Vec3) -> Vec3 {
    let f = Vec3::new(v.x, 0.0, v.z);
    let len2 = f.length_squared();
    if len2 > 1e-8 {
        f / len2.sqrt()
    } else {
        fallback
    }
}

/// Hermite smoothstep of `x` between `e0` and `e1`.
#[inline]
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// 1.0 inside `[lo, hi]`, falling off linearly to 0 over `falloff` outside.
#[inline]
pub fn band(x: f32, lo: f32, hi: f32, falloff: f32) -> f32 {
    if x < lo {
        (1.0 - (lo - x) / falloff).max(0.0)
    } else if x > hi {
        (1.0 - (x - hi) / falloff).max(0.0)
    } else {
        1.0
    }
}

/// 64-bit avalanche hash (SplitMix64 finalizer with a salt). Use it to derive
/// independent seeds: `Rng::new(mix64(seed ^ k))`.
#[inline]
pub fn mix64(x: u64) -> u64 {
    let mut z = x ^ 0xA076_1D64_78BD_642F;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// SplitMix64-based RNG: tiny, fast, and fully deterministic for a given seed.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    /// Creates a generator from a seed.
    pub fn new(seed: u64) -> Self {
        let mut r = Rng(seed ^ 0x5DEE_CE66_D1CE_4E5B);
        r.next_u64();
        r
    }

    /// Next 64 random bits.
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform float in `[0, 1)`.
    #[inline]
    pub fn f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 * (1.0 / (1u64 << 24) as f32)
    }

    /// Uniform float in `[lo, hi)`.
    #[inline]
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f32()
    }

    /// Uniform point in the unit disk.
    #[inline]
    pub fn unit_disk(&mut self) -> Vec2 {
        let r = self.f32().sqrt();
        let a = self.f32() * std::f32::consts::TAU;
        Vec2::new(a.cos() * r, a.sin() * r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_deterministic_and_in_range() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..1000 {
            let x = a.f32();
            assert_eq!(x, b.f32());
            assert!((0.0..1.0).contains(&x));
        }
    }

    #[test]
    fn band_shape() {
        assert_eq!(band(5.0, 4.0, 6.0, 2.0), 1.0);
        assert_eq!(band(3.0, 4.0, 6.0, 2.0), 0.5);
        assert_eq!(band(10.0, 4.0, 6.0, 2.0), 0.0);
    }
}
