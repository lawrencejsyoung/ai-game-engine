//! Squad-level knowledge sharing and role assignment.

use crate::ids::ActorId;
use glam::Vec3;
use smallvec::SmallVec;

/// Role an agent plays within its squad.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum SquadRole {
    /// Default: engage directly from cover.
    #[default]
    Assault,
    /// Pin the target down with fire at its last known position.
    Suppressor,
    /// Move around to the target's side.
    Flanker,
}

/// Max distinct targets a squad shares.
pub(crate) const SQUAD_MAX_TARGETS: usize = 8;

#[derive(Copy, Clone, Debug)]
pub(crate) struct SquadTarget {
    pub target: ActorId,
    pub pos: Vec3,
    pub vel: Vec3,
    pub last_seen: f32,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Squad {
    pub members: Vec<ActorId>,
    /// Confirmed targets known by any member (rebuilt each tick).
    pub targets: SmallVec<[SquadTarget; 8]>,
    /// Target most recently seen by the squad.
    pub primary: Option<ActorId>,
    /// Average member position.
    pub centroid: Vec3,
    /// Next time roles are re-evaluated.
    pub next_role_eval: f32,
    /// Primary target when roles were last assigned.
    pub role_target: Option<ActorId>,
}

impl Squad {
    pub(crate) fn merge_target(targets: &mut SmallVec<[SquadTarget; 8]>, t: SquadTarget) {
        if let Some(e) = targets.iter_mut().find(|e| e.target == t.target) {
            if t.last_seen > e.last_seen {
                *e = t;
            }
        } else if targets.len() < SQUAD_MAX_TARGETS {
            targets.push(t);
        } else if let Some(oldest) = targets
            .iter_mut()
            .min_by(|a, b| a.last_seen.total_cmp(&b.last_seen))
        {
            if t.last_seen > oldest.last_seen {
                *oldest = t;
            }
        }
    }
}
