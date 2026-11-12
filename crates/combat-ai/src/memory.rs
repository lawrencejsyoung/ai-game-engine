//! Per-agent memory of hostile actors.

use crate::ids::ActorId;
use glam::Vec3;
use smallvec::SmallVec;

/// How the agent learned about a target most recently.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// Seen directly.
    Sight,
    /// Heard.
    Sound,
    /// Took damage from it.
    Damage,
    /// Told by a squad mate.
    Squad,
}

/// What an agent remembers about one hostile actor.
#[derive(Copy, Clone, Debug)]
pub struct TargetMemory {
    /// The remembered actor.
    pub target: ActorId,
    /// Last known (possibly estimated) position.
    pub last_known_pos: Vec3,
    /// Last known velocity.
    pub last_known_vel: Vec3,
    /// Simulation time this entry was last refreshed.
    pub last_update: f32,
    /// Simulation time the target was last directly seen (`-inf` if never).
    pub last_seen: f32,
    /// Detection meter in `[0, 1]`; `1` means confirmed hostile.
    pub awareness: f32,
    /// Currently in line of sight (as of the last perception update).
    pub visible: bool,
    /// Simulation time the target last damaged us.
    pub last_damaged_us: f32,
    /// The last-known position has been searched without finding the target.
    pub searched: bool,
    /// Most recent information source.
    pub source: Source,
    /// Only the head was in line of sight at the last check (chest occluded).
    pub(crate) head_only: bool,
    /// Time of last line-of-sight raycast (for round-robin budgeting).
    pub(crate) los_checked: f32,
    /// Inside the view cone at the last perception update.
    pub(crate) in_view: bool,
}

impl TargetMemory {
    pub(crate) fn new(target: ActorId, pos: Vec3, time: f32, source: Source) -> Self {
        Self {
            target,
            last_known_pos: pos,
            last_known_vel: Vec3::ZERO,
            last_update: time,
            last_seen: f32::NEG_INFINITY,
            awareness: 0.0,
            visible: false,
            last_damaged_us: f32::NEG_INFINITY,
            searched: false,
            source,
            head_only: false,
            los_checked: f32::NEG_INFINITY,
            in_view: false,
        }
    }

    /// How much this entry matters, used to decide what to forget when memory is full.
    pub fn relevance(&self, now: f32, me: Vec3, current: Option<ActorId>) -> f32 {
        let mut r = self.awareness;
        if self.visible {
            r += 4.0;
        }
        if self.confirmed() {
            r += 2.0;
        }
        if now - self.last_damaged_us < 5.0 {
            r += 3.0;
        }
        if Some(self.target) == current {
            r += 10.0;
        }
        r - (now - self.last_update) * 0.2 + 3.0 / (1.0 + self.last_known_pos.distance(me) * 0.05)
    }

    /// Whether the detection meter is full.
    #[inline]
    pub fn confirmed(&self) -> bool {
        self.awareness >= 1.0
    }
}

/// Fixed-capacity-inline set of remembered targets. Spills to the heap only
/// when an agent tracks more than 8 enemies at once.
#[derive(Clone, Debug, Default)]
pub struct Memory {
    pub(crate) entries: SmallVec<[TargetMemory; 8]>,
}

impl Memory {
    /// All remembered targets.
    pub fn entries(&self) -> &[TargetMemory] {
        &self.entries
    }

    /// Looks up a target.
    pub fn get(&self, id: ActorId) -> Option<&TargetMemory> {
        self.entries.iter().find(|e| e.target == id)
    }

    pub(crate) fn get_mut(&mut self, id: ActorId) -> Option<&mut TargetMemory> {
        self.entries.iter_mut().find(|e| e.target == id)
    }

    /// Finds or inserts an entry. When the memory is full (`cap` entries), a
    /// new target only gets in by evicting a less relevant one; `priority`
    /// estimates the new entry's relevance (see [`TargetMemory::relevance`]).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn entry(
        &mut self,
        id: ActorId,
        pos: Vec3,
        time: f32,
        source: Source,
        cap: usize,
        priority: f32,
        me: Vec3,
        current: Option<ActorId>,
    ) -> Option<&mut TargetMemory> {
        if let Some(i) = self.entries.iter().position(|e| e.target == id) {
            return Some(&mut self.entries[i]);
        }
        if self.entries.len() < cap.max(1) {
            self.entries.push(TargetMemory::new(id, pos, time, source));
            return self.entries.last_mut();
        }
        let (worst, worst_r) = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| (i, e.relevance(time, me, current)))
            .min_by(|a, b| a.1.total_cmp(&b.1))?;
        let new_r = priority + 3.0 / (1.0 + pos.distance(me) * 0.05);
        if new_r <= worst_r {
            return None;
        }
        self.entries[worst] = TargetMemory::new(id, pos, time, source);
        Some(&mut self.entries[worst])
    }

    /// Decays unconfirmed suspicion and forgets stale targets.
    pub(crate) fn decay(&mut self, now: f32, dt: f32, memory_span: f32) {
        self.entries.retain(|e| {
            if e.visible {
                return true;
            }
            if !e.confirmed() {
                e.awareness -= dt * 0.15;
                return e.awareness > 0.0;
            }
            now - e.last_update <= memory_span
        });
    }

    /// Number of confirmed targets.
    pub fn confirmed_count(&self) -> usize {
        self.entries.iter().filter(|e| e.confirmed()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_evicts_least_relevant() {
        let mut m = Memory::default();
        let far = Vec3::new(0.0, 0.0, 100.0);
        for i in 0..4 {
            let e = m
                .entry(id(i), far, 0.0, Source::Sound, 4, 1.0, Vec3::ZERO, None)
                .unwrap();
            e.awareness = 0.2;
        }
        // A low-priority distant newcomer is ignored...
        assert!(m
            .entry(id(9), far, 0.0, Source::Sound, 4, 0.0, Vec3::ZERO, None)
            .is_none());
        // ...but someone shooting us from close range gets in.
        assert!(m
            .entry(
                id(10),
                Vec3::new(0.0, 0.0, 5.0),
                0.0,
                Source::Damage,
                4,
                6.0,
                Vec3::ZERO,
                None
            )
            .is_some());
        assert_eq!(m.entries().len(), 4);
        assert!(m.get(id(10)).is_some());
    }

    fn id(i: u32) -> ActorId {
        ActorId {
            index: i,
            generation: 0,
        }
    }

    #[test]
    fn suspicion_decays_and_confirmed_targets_expire() {
        let mut m = Memory::default();
        m.entry(
            id(1),
            Vec3::ZERO,
            0.0,
            Source::Sound,
            8,
            1.0,
            Vec3::ZERO,
            None,
        )
        .unwrap()
        .awareness = 0.3;
        let e = m
            .entry(
                id(2),
                Vec3::ZERO,
                0.0,
                Source::Sight,
                8,
                1.0,
                Vec3::ZERO,
                None,
            )
            .unwrap();
        e.awareness = 1.0;
        for i in 1..=30 {
            m.decay(i as f32 * 0.1, 0.1, 10.0);
        }
        assert!(m.get(id(1)).is_none(), "suspicion should fade");
        assert!(m.get(id(2)).is_some(), "confirmed target still remembered");
        m.decay(11.0, 0.1, 10.0);
        assert!(m.get(id(2)).is_none(), "forgotten after memory span");
    }
}
