//! Tactical position evaluation: cover, flanking, attack and retreat positions.
//!
//! Each query runs in two stages: a cheap analytic pass scores every candidate
//! in the search radius (spatial-grid lookup, dot products, no raycasts), then
//! only the best few are validated with world queries (`path_distance` and a
//! line-of-sight ray). This bounds expensive queries to a small constant per
//! decision regardless of level density.

use crate::brain::Shared;
use crate::ids::{ActorId, CoverId};
use crate::math::{band, dist2_xz, flat_dir, smoothstep, Rng};
use glam::Vec3;

/// Kind of cover.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CoverKind {
    /// Waist-high: protects when crouched, shoot by standing up.
    Low,
    /// Full height: protects when standing, shoot by leaning/stepping out.
    High,
}

/// A designer- or tool-placed cover location.
#[derive(Copy, Clone, Debug)]
pub struct CoverPoint {
    /// Where the agent stands (on the navmesh).
    pub position: Vec3,
    /// Horizontal unit direction from `position` toward the obstacle, i.e. the
    /// direction this cover protects against.
    pub facing: Vec3,
    /// Cover height class.
    pub kind: CoverKind,
}

impl CoverPoint {
    /// Creates a cover point; `facing` is flattened and normalized.
    pub fn new(position: Vec3, facing: Vec3, kind: CoverKind) -> Self {
        Self {
            position,
            facing: flat_dir(facing, Vec3::Z),
            kind,
        }
    }

    /// Analytic protection against a threat at `threat` in `[0, 1]`.
    #[inline]
    pub fn protection_from(&self, threat: Vec3) -> f32 {
        if dist2_xz(self.position, threat) < 9.0 {
            return 0.0; // too close: the threat can just walk around it
        }
        let d = flat_dir(threat - self.position, self.facing);
        smoothstep(0.2, 0.75, d.dot(self.facing))
    }

    /// Height at which an agent hiding here is protected.
    #[inline]
    pub fn hide_height(&self) -> f32 {
        match self.kind {
            CoverKind::Low => 0.9,
            CoverKind::High => 1.5,
        }
    }
}

/// Result of a tactical query.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct TacticalPosition {
    /// Destination.
    pub position: Vec3,
    /// Cover at the destination, if any.
    pub cover: Option<CoverId>,
}

/// Inputs shared by tactical queries.
pub(crate) struct Query<'a> {
    pub me: ActorId,
    pub pos: Vec3,
    /// Positions of known threats (feet).
    pub threats: &'a [Vec3],
    /// Primary target last known position (feet).
    pub primary: Option<Vec3>,
    pub preferred: (f32, f32),
    pub radius: f32,
    /// Direction the squad is attacking from (target -> squad), for flanking.
    pub attack_axis: Option<Vec3>,
}

const SHORTLIST: usize = 4;

#[derive(Copy, Clone)]
struct Cand {
    pos: Vec3,
    cover: Option<CoverId>,
    score: f32,
}

/// Keeps the best `SHORTLIST` candidates without allocating.
struct Shortlist {
    items: [Option<Cand>; SHORTLIST],
}

impl Shortlist {
    fn new() -> Self {
        Self {
            items: [None; SHORTLIST],
        }
    }
    fn push(&mut self, c: Cand) {
        if c.score <= 0.0 {
            return;
        }
        let mut slot = None;
        let mut worst = f32::INFINITY;
        for (i, it) in self.items.iter().enumerate() {
            match it {
                None => {
                    slot = Some(i);
                    worst = f32::NEG_INFINITY;
                    break;
                }
                Some(x) if x.score < worst => {
                    worst = x.score;
                    slot = Some(i);
                }
                _ => {}
            }
        }
        if let Some(i) = slot {
            if c.score > worst {
                self.items[i] = Some(c);
            }
        }
    }
    fn sorted(mut self) -> impl Iterator<Item = Cand> {
        self.items.sort_by(|a, b| {
            let sa = a.map_or(f32::NEG_INFINITY, |c| c.score);
            let sb = b.map_or(f32::NEG_INFINITY, |c| c.score);
            sb.total_cmp(&sa)
        });
        self.items.into_iter().flatten()
    }
}

fn cover_available(sh: &Shared, me: ActorId, c: u32) -> bool {
    match sh.cover_claims.get(c as usize) {
        Some(Some(owner)) => *owner == me,
        _ => true,
    }
}

fn protection(cover: &CoverPoint, threats: &[Vec3]) -> f32 {
    if threats.is_empty() {
        return 0.0;
    }
    let mut min_p: f32 = 1.0;
    let mut sum = 0.0;
    for t in threats {
        let p = cover.protection_from(*t);
        min_p = min_p.min(p);
        sum += p;
    }
    // Mostly about the worst-covered angle; a little credit for the average.
    0.7 * min_p + 0.3 * sum / threats.len() as f32
}

fn reachable(sh: &Shared, from: Vec3, to: Vec3, max: f32) -> bool {
    sh.path_queries
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    matches!(sh.world.path_distance(from, to), Some(d) if d <= max)
}

fn hidden_from(sh: &Shared, threat: Vec3, cover: &CoverPoint) -> bool {
    !sh.los(
        threat + Vec3::Y * 1.6,
        cover.position + Vec3::Y * cover.hide_height(),
    )
}

/// Best protective cover near the agent.
pub(crate) fn find_cover(sh: &Shared, q: &Query) -> Option<TacticalPosition> {
    let mut list = Shortlist::new();
    sh.cover_grid.query(q.pos, q.radius, |ci, cpos| {
        if !cover_available(sh, q.me, ci) {
            return;
        }
        let cover = &sh.covers[ci as usize];
        let prot = protection(cover, q.threats);
        if prot < 0.3 {
            return;
        }
        let travel = 1.0 - (cpos.distance(q.pos) / q.radius).min(1.0);
        let range_fit = q.primary.map_or(0.5, |t| {
            band(cpos.distance(t), q.preferred.0, q.preferred.1, 10.0)
        });
        list.push(Cand {
            pos: cpos,
            cover: Some(CoverId(ci)),
            score: prot * 0.45 + travel * 0.35 + range_fit * 0.2,
        });
    });
    let primary = q.primary.or_else(|| q.threats.first().copied());
    list.sorted()
        .find(|c| {
            let cover = &sh.covers[c.cover.unwrap().0 as usize];
            reachable(sh, q.pos, c.pos, q.radius * 1.75)
                && primary.is_none_or(|t| hidden_from(sh, t, cover))
        })
        .map(|c| TacticalPosition {
            position: c.pos,
            cover: c.cover,
        })
}

/// Covered position far from threats.
pub(crate) fn find_retreat(sh: &Shared, q: &Query) -> Option<TacticalPosition> {
    let nearest_threat = |p: Vec3| {
        q.threats
            .iter()
            .map(|t| dist2_xz(*t, p))
            .fold(f32::INFINITY, f32::min)
            .sqrt()
    };
    let here = nearest_threat(q.pos);
    let radius = q.radius * 1.5;
    let mut list = Shortlist::new();
    sh.cover_grid.query(q.pos, radius, |ci, cpos| {
        if !cover_available(sh, q.me, ci) {
            return;
        }
        let away = nearest_threat(cpos);
        if away < here + 3.0 {
            return;
        }
        let prot = protection(&sh.covers[ci as usize], q.threats);
        let gain = ((away - here) / radius).min(1.0);
        let travel = 1.0 - (cpos.distance(q.pos) / radius).min(1.0);
        list.push(Cand {
            pos: cpos,
            cover: Some(CoverId(ci)),
            score: gain * 0.45 + prot * 0.35 + travel * 0.2,
        });
    });
    list.sorted()
        .find(|c| reachable(sh, q.pos, c.pos, radius * 1.75))
        .map(|c| TacticalPosition {
            position: c.pos,
            cover: c.cover,
        })
}

/// Position roughly perpendicular to the squad's line of attack on the target.
pub(crate) fn find_flank(sh: &Shared, q: &Query, rng: &mut Rng) -> Option<TacticalPosition> {
    let target = q.primary?;
    let axis = q
        .attack_axis
        .unwrap_or_else(|| flat_dir(q.pos - target, Vec3::Z));
    let mid = 0.5 * (q.preferred.0 + q.preferred.1);
    let mut list = Shortlist::new();
    let mut consider = |pos: Vec3, cover: Option<CoverId>, prot: f32| {
        let dir = flat_dir(pos - target, axis);
        let angle = dir.dot(axis).clamp(-1.0, 1.0).acos().to_degrees();
        let flank = band(angle, 60.0, 115.0, 30.0);
        if flank < 0.3 {
            return;
        }
        let range_fit = band(pos.distance(target), q.preferred.0, q.preferred.1, 8.0);
        let travel = 1.0 - (pos.distance(q.pos) / (q.radius * 2.0)).min(1.0);
        list.push(Cand {
            pos,
            cover,
            score: flank * 0.45 + range_fit * 0.2 + travel * 0.2 + prot * 0.15,
        });
    };
    sh.cover_grid
        .query(target, q.preferred.1 + 5.0, |ci, cpos| {
            if cover_available(sh, q.me, ci) {
                let prot = sh.covers[ci as usize].protection_from(target);
                consider(cpos, Some(CoverId(ci)), prot);
            }
        });
    // Open-ground samples on both sides, in case there's no suitable cover.
    let side = Vec3::new(-axis.z, 0.0, axis.x);
    for s in [-1.0f32, 1.0] {
        for jitter in [0.0f32, 0.35] {
            let d = (side * s + axis * (jitter + rng.range(-0.1, 0.1))).normalize();
            consider(target + d * mid, None, 0.0);
        }
    }
    list.sorted()
        .find(|c| reachable(sh, q.pos, c.pos, q.radius * 3.0))
        .map(|c| TacticalPosition {
            position: c.pos,
            cover: c.cover,
        })
}

/// Nearby position with line of sight to the target inside the preferred range.
pub(crate) fn find_attack_position(
    sh: &Shared,
    q: &Query,
    target_chest: Vec3,
    rng: &mut Rng,
) -> Option<TacticalPosition> {
    let target = q.primary?;
    let mut list = Shortlist::new();
    let mut consider = |pos: Vec3, cover: Option<CoverId>| {
        let range_fit = band(pos.distance(target), q.preferred.0, q.preferred.1, 10.0);
        let travel = 1.0 - (pos.distance(q.pos) / q.radius).min(1.0);
        let cover_bonus = if cover.is_some() { 1.0 } else { 0.0 };
        list.push(Cand {
            pos,
            cover,
            score: range_fit * 0.45 + travel * 0.4 + cover_bonus * 0.15 + 0.01,
        });
    };
    sh.cover_grid.query(q.pos, q.radius, |ci, cpos| {
        if cover_available(sh, q.me, ci) {
            consider(cpos, Some(CoverId(ci)));
        }
    });
    let to_target = flat_dir(target - q.pos, Vec3::Z);
    let side = Vec3::new(-to_target.z, 0.0, to_target.x);
    let step = rng.range(4.0, 8.0);
    consider(q.pos + side * step, None);
    consider(q.pos - side * step, None);
    let d = q.pos.distance(target);
    let mid = 0.5 * (q.preferred.0 + q.preferred.1);
    if d > mid {
        consider(q.pos + to_target * (d - mid).min(q.radius), None);
    }
    list.sorted()
        .find(|c| {
            sh.los(c.pos + Vec3::Y * 1.6, target_chest)
                && reachable(sh, q.pos, c.pos, q.radius * 1.75)
        })
        .map(|c| TacticalPosition {
            position: c.pos,
            cover: c.cover,
        })
}

/// Random reachable point around `home` (the patrol center).
pub(crate) fn find_patrol_point(
    sh: &Shared,
    from: Vec3,
    home: Vec3,
    radius: f32,
    rng: &mut Rng,
) -> Option<Vec3> {
    // The center may be far away (hunting around a distant last contact).
    let max_path = from.distance(home) * 2.0 + radius * 4.0;
    for _ in 0..4 {
        let o = rng.unit_disk() * radius;
        let p = home + Vec3::new(o.x, 0.0, o.y);
        if reachable(sh, from, p, max_path) {
            return Some(p);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protection_depends_on_threat_direction() {
        let c = CoverPoint::new(Vec3::ZERO, Vec3::Z, CoverKind::Low);
        assert!(c.protection_from(Vec3::new(0.0, 0.0, 20.0)) > 0.99);
        assert_eq!(c.protection_from(Vec3::new(0.0, 0.0, -20.0)), 0.0);
        assert_eq!(c.protection_from(Vec3::new(20.0, 0.0, 0.0)), 0.0);
        assert_eq!(
            c.protection_from(Vec3::new(0.0, 0.0, 2.0)),
            0.0,
            "too close"
        );
    }

    #[test]
    fn shortlist_keeps_best() {
        let mut l = Shortlist::new();
        for i in 0..10 {
            l.push(Cand {
                pos: Vec3::ZERO,
                cover: None,
                score: i as f32,
            });
        }
        let s: Vec<f32> = l.sorted().map(|c| c.score).collect();
        assert_eq!(s, [9.0, 8.0, 7.0, 6.0]);
    }
}
