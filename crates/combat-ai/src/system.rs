//! The top-level AI system: storage, scheduling and the per-frame pipeline.

use crate::agent::ActorState;
use crate::archetype::Archetype;
use crate::brain::{ActorSlot, Brain, Shared};
use crate::command::AgentCommand;
use crate::ids::{ActorId, CoverId, SquadId};
use crate::spatial::SpatialGrid;
use crate::squad::{Squad, SquadRole, SquadTarget};
use crate::stimuli::{DamageEvent, SoundEvent, SoundKind, TimedSound};
use crate::tactics::CoverPoint;
use crate::utility::ActionKind;
use crate::world::WorldQuery;
use glam::Vec3;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// Global configuration.
#[derive(Clone, Debug)]
pub struct SystemConfig {
    /// Seed for all agent randomness (identical seeds + inputs give identical output).
    pub seed: u64,
    /// Cell size of the actor spatial grid (meters).
    pub grid_cell_size: f32,
    /// Max AI agents allowed to shoot accurately at one *external* actor (player)
    /// at a time. Agents without a token still shoot, but deliberately miss.
    /// `0` disables the token system.
    pub max_attackers_per_player: u8,
    /// Extra aim spread (degrees) for agents shooting without an attack token.
    pub no_token_spread_deg: f32,
    /// Approximate radius (m) of an actor's hitbox. Only used by agents to
    /// estimate their own chance to hit ([`crate::AgentConfig::min_hit_chance`]).
    pub target_radius: f32,
    /// Global multiplier on every agent's aim spread: a difficulty knob.
    /// `1` = archetype tuning as authored, `<1` = deadlier, `>1` = sloppier.
    pub accuracy_scale: f32,
    /// LOD distance thresholds `(full, medium)` from the nearest LOD focus point.
    /// Beyond `full` agents think 2.5x less often, beyond `medium` 6x less.
    pub lod_distances: (f32, f32),
    /// Automatically emit gunshot sounds of this radius when agents fire
    /// (`0` disables; then report them yourself via [`AiSystem::report_sound`]).
    pub auto_gunshot_radius: f32,
    /// How long reported sounds remain audible to agents that haven't perceived yet.
    pub sound_retention: f32,
    /// Seconds between squad role re-assignments.
    pub role_interval: f32,
    /// With the `parallel` feature, below this many agents the tick runs on the
    /// calling thread (thread-pool dispatch costs more than it saves).
    pub parallel_threshold: usize,
}

impl Default for SystemConfig {
    fn default() -> Self {
        Self {
            seed: 0xC0FFEE,
            grid_cell_size: 12.0,
            max_attackers_per_player: 2,
            no_token_spread_deg: 5.0,
            accuracy_scale: 1.0,
            target_radius: 0.3,
            lod_distances: (40.0, 90.0),
            auto_gunshot_radius: 70.0,
            sound_retention: 1.0,
            role_interval: 4.0,
            parallel_threshold: 256,
        }
    }
}

/// Per-tick performance counters.
#[derive(Copy, Clone, Debug, Default)]
pub struct TickStats {
    /// Live AI agents updated.
    pub agents: u32,
    /// Agents that ran a full think (perceive + decide) this tick.
    pub thinks: u32,
    /// Line-of-sight queries issued.
    pub raycasts: u32,
    /// Path-distance queries issued.
    pub path_queries: u32,
    /// Wall-clock duration of the tick in microseconds.
    pub micros: f32,
}

/// Debug snapshot of one agent's mind.
#[derive(Clone, Debug)]
pub struct AgentDebug {
    /// Current action.
    pub action: ActionKind,
    /// Last utility score of every action.
    pub scores: Vec<(ActionKind, f32)>,
    /// Primary target.
    pub target: Option<ActorId>,
    /// Whether the primary target is visible.
    pub target_visible: bool,
    /// Squad role.
    pub role: SquadRole,
    /// Claimed cover.
    pub cover: Option<CoverId>,
    /// Current move destination.
    pub destination: Option<Vec3>,
    /// Current think interval (LOD).
    pub think_interval: f32,
    /// Whether the agent holds an attack token.
    pub has_token: bool,
    /// Number of remembered targets.
    pub known_targets: usize,
    /// Stand-off frustration in `[0, 1]` (see [`crate::AgentConfig::patience`]).
    pub impatience: f32,
}

/// The AI engine. See the [crate docs](crate) for the integration loop.
pub struct AiSystem {
    config: SystemConfig,
    time: f32,
    actors: Vec<ActorSlot>,
    free: Vec<u32>,
    brains: Vec<Brain>,
    actor_grid: SpatialGrid,
    covers: Vec<CoverPoint>,
    cover_grid: SpatialGrid,
    cover_dirty: bool,
    cover_claims: Vec<Option<ActorId>>,
    squads: Vec<Squad>,
    sounds: Vec<TimedSound>,
    lod_focus: Vec<Vec3>,
    commands: Vec<AgentCommand>,
    stats: TickStats,
    // reusable scratch buffers
    claim_scratch: Vec<(u32, f32, u32)>,
    token_scratch: Vec<(u32, bool, f32, u32)>,
    sound_of: Vec<u32>,
    sound_seq: u64,
}

impl AiSystem {
    /// Creates an empty system.
    pub fn new(config: SystemConfig) -> Self {
        Self {
            actor_grid: SpatialGrid::new(config.grid_cell_size),
            cover_grid: SpatialGrid::new(8.0),
            config,
            time: 0.0,
            actors: Vec::new(),
            free: Vec::new(),
            brains: Vec::new(),
            covers: Vec::new(),
            cover_dirty: false,
            cover_claims: Vec::new(),
            squads: Vec::new(),
            sounds: Vec::new(),
            lod_focus: Vec::new(),
            commands: Vec::new(),
            stats: TickStats::default(),
            claim_scratch: Vec::new(),
            token_scratch: Vec::new(),
            sound_of: Vec::new(),
            sound_seq: 0,
        }
    }

    /// Current simulation time (sum of all `dt`).
    pub fn time(&self) -> f32 {
        self.time
    }

    /// Configuration.
    pub fn config(&self) -> &SystemConfig {
        &self.config
    }

    /// Counters from the last tick.
    pub fn stats(&self) -> TickStats {
        self.stats
    }

    /// Number of AI-driven agents.
    pub fn agent_count(&self) -> usize {
        self.brains.len()
    }

    // ------------------------------------------------------------------ actors

    fn alloc(&mut self, state: ActorState, brain: u32) -> ActorId {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.actors[index as usize];
            slot.generation = slot.generation.wrapping_add(1);
            slot.occupied = true;
            slot.state = state;
            slot.brain = brain;
            slot.squad = None;
            ActorId {
                index,
                generation: slot.generation,
            }
        } else {
            self.actors.push(ActorSlot {
                generation: 0,
                occupied: true,
                state,
                brain,
                squad: None,
            });
            ActorId {
                index: self.actors.len() as u32 - 1,
                generation: 0,
            }
        }
    }

    fn slot(&self, id: ActorId) -> Option<&ActorSlot> {
        self.actors
            .get(id.index as usize)
            .filter(|s| s.occupied && s.generation == id.generation)
    }

    fn brain_mut(&mut self, id: ActorId) -> Option<&mut Brain> {
        let b = self.slot(id)?.brain;
        self.brains.get_mut(b as usize)
    }

    fn brain(&self, id: ActorId) -> Option<&Brain> {
        let b = self.slot(id)?.brain;
        self.brains.get(b as usize)
    }

    /// Registers an externally controlled combatant (e.g. a player) that agents
    /// can see, hear and fight.
    pub fn add_actor(&mut self, state: ActorState) -> ActorId {
        self.alloc(state, u32::MAX)
    }

    /// Spawns an AI-controlled agent.
    pub fn spawn_agent(&mut self, state: ActorState, archetype: Arc<Archetype>) -> ActorId {
        let id = self.alloc(state, self.brains.len() as u32);
        let mut brain = Brain::new(id, archetype, &state, self.config.seed);
        brain.next_think += self.time;
        self.brains.push(brain);
        id
    }

    /// Removes an actor (agent or external). Stale handles become inert.
    pub fn remove_actor(&mut self, id: ActorId) -> bool {
        let Some(slot) = self.slot(id) else {
            return false;
        };
        let (bi, squad) = (slot.brain, slot.squad);
        if let Some(s) = squad {
            self.squads[s.0 as usize].members.retain(|m| *m != id);
        }
        if bi != u32::MAX {
            self.brains.swap_remove(bi as usize);
            if let Some(moved) = self.brains.get(bi as usize) {
                self.actors[moved.actor.index as usize].brain = bi;
            }
        }
        let slot = &mut self.actors[id.index as usize];
        slot.occupied = false;
        slot.brain = u32::MAX;
        self.free.push(id.index);
        true
    }

    /// The host-authored state of an actor.
    pub fn actor_state(&self, id: ActorId) -> Option<&ActorState> {
        self.slot(id).map(|s| &s.state)
    }

    /// Mutable access to update an actor's state (position, health, ammo, ...).
    pub fn actor_state_mut(&mut self, id: ActorId) -> Option<&mut ActorState> {
        let s = self.actors.get_mut(id.index as usize)?;
        (s.occupied && s.generation == id.generation).then_some(&mut s.state)
    }

    /// Iterates all live actor handles.
    pub fn actors(&self) -> impl Iterator<Item = ActorId> + '_ {
        self.actors
            .iter()
            .enumerate()
            .filter(|(_, s)| s.occupied)
            .map(|(i, s)| ActorId {
                index: i as u32,
                generation: s.generation,
            })
    }

    /// Whether `id` is an AI-driven agent.
    pub fn is_agent(&self, id: ActorId) -> bool {
        self.slot(id).is_some_and(|s| s.brain != u32::MAX)
    }

    // ------------------------------------------------------------------ squads

    /// Creates an empty squad.
    pub fn create_squad(&mut self) -> SquadId {
        self.squads.push(Squad::default());
        SquadId(self.squads.len() as u32 - 1)
    }

    /// Moves an agent into a squad (removing it from any previous squad).
    pub fn assign_squad(&mut self, id: ActorId, squad: SquadId) -> bool {
        if squad.0 as usize >= self.squads.len() || !self.is_agent(id) {
            return false;
        }
        if let Some(old) = self.slot(id).and_then(|s| s.squad) {
            self.squads[old.0 as usize].members.retain(|m| *m != id);
        }
        self.squads[squad.0 as usize].members.push(id);
        self.actors[id.index as usize].squad = Some(squad);
        if let Some(b) = self.brain_mut(id) {
            b.squad = Some(squad);
        }
        true
    }

    /// Members of a squad.
    pub fn squad_members(&self, squad: SquadId) -> &[ActorId] {
        self.squads
            .get(squad.0 as usize)
            .map_or(&[], |s| s.members.as_slice())
    }

    // ------------------------------------------------------------------ level data

    /// Registers a cover point.
    pub fn add_cover_point(&mut self, cover: CoverPoint) -> CoverId {
        self.covers.push(cover);
        self.cover_claims.push(None);
        self.cover_dirty = true;
        CoverId(self.covers.len() as u32 - 1)
    }

    /// All registered cover points.
    pub fn cover_points(&self) -> &[CoverPoint] {
        &self.covers
    }

    /// Current owner of a cover point.
    pub fn cover_owner(&self, cover: CoverId) -> Option<ActorId> {
        self.cover_claims.get(cover.0 as usize).copied().flatten()
    }

    /// Sets the points (usually player cameras) agents' level of detail is measured from.
    /// Empty = every agent thinks at full rate.
    pub fn set_lod_focus(&mut self, points: &[Vec3]) {
        self.lod_focus.clear();
        self.lod_focus.extend_from_slice(points);
    }

    // ------------------------------------------------------------------ stimuli

    /// Reports a sound.
    pub fn report_sound(&mut self, event: SoundEvent) {
        self.sound_seq += 1;
        self.sounds.push(TimedSound {
            event,
            time: self.time,
            seq: self.sound_seq,
        });
    }

    /// Reports damage to an actor. AI victims react on their next tick.
    pub fn report_damage(&mut self, event: DamageEvent) {
        // Landing a hit counts as progress for the attacker (resets impatience).
        if let Some(b) = event
            .attacker
            .filter(|a| *a != event.victim)
            .and_then(|a| self.brain_mut(a))
        {
            b.calm_time = 0.0;
        }
        if let Some(b) = self.brain_mut(event.victim) {
            b.pending_damage.push(event);
            b.wake = true;
        }
    }

    /// Tells every living agent on `team` where `target` currently is, as if a
    /// squad mate had called it out. Use it for radar pings, alarms, scripted
    /// reveals, or a "director" that breaks stalemates. Returns the number of
    /// agents informed (0 if `target` is unknown).
    pub fn reveal(&mut self, target: ActorId, team: crate::ids::TeamId) -> usize {
        let Some(t) = self.slot(target).map(|s| s.state) else {
            return 0;
        };
        let mut n = 0;
        for b in &mut self.brains {
            let s = &self.actors[b.actor.index as usize].state;
            if s.team != team || !s.is_alive() || b.actor == target || s.team == t.team {
                continue;
            }
            if let Some(e) = b.pending_intel.iter_mut().find(|e| e.0 == target) {
                *e = (target, t.position, t.velocity);
            } else {
                b.pending_intel.push((target, t.position, t.velocity));
            }
            n += 1;
        }
        n
    }

    // ------------------------------------------------------------------ debug

    /// Snapshot of an agent's decision state.
    pub fn debug(&self, id: ActorId) -> Option<AgentDebug> {
        let b = self.brain(id)?;
        Some(AgentDebug {
            action: b.command.action,
            scores: b
                .archetype
                .actions
                .iter()
                .zip(b.scores.iter())
                .map(|(a, s)| (a.kind, *s))
                .collect(),
            target: b.target,
            target_visible: b
                .target
                .and_then(|t| b.memory.get(t))
                .is_some_and(|m| m.visible),
            role: b.role,
            cover: b.cover,
            destination: b.destination,
            think_interval: b.think_interval,
            has_token: b.token.is_some(),
            known_targets: b.memory.entries().len(),
            impatience: b.impatience(),
        })
    }

    /// An agent's target memory.
    pub fn memory(&self, id: ActorId) -> Option<&crate::memory::Memory> {
        self.brain(id).map(|b| &b.memory)
    }

    /// Commands produced by the last tick.
    pub fn commands(&self) -> &[AgentCommand] {
        &self.commands
    }

    // ------------------------------------------------------------------ tick

    /// Advances the simulation by `dt` seconds and returns one command per live agent.
    ///
    /// A non-finite or negative `dt` is treated as `0` (the AI re-evaluates
    /// without advancing time).
    pub fn tick<W: WorldQuery>(&mut self, dt: f32, world: &W) -> &[AgentCommand] {
        let start = Instant::now();
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        self.time += dt;
        let now = self.time;

        // 1. Spatial structures.
        self.actor_grid.rebuild(
            self.actors
                .iter()
                .enumerate()
                .filter(|(_, s)| s.occupied && s.state.is_alive())
                .map(|(i, s)| (i as u32, s.state.position)),
        );
        if self.cover_dirty {
            self.cover_grid.rebuild(
                self.covers
                    .iter()
                    .enumerate()
                    .map(|(i, c)| (i as u32, c.position)),
            );
            self.cover_dirty = false;
        }
        let retention = self.config.sound_retention;
        self.sounds.retain(|s| now - s.time <= retention);

        // 2. Squad knowledge + roles (serial, cheap).
        self.update_squads();

        // 3. Perceive / decide / act for every agent (parallel).
        let shared = Shared {
            time: now,
            dt,
            actors: &self.actors,
            actor_grid: &self.actor_grid,
            covers: &self.covers,
            cover_grid: &self.cover_grid,
            cover_claims: &self.cover_claims,
            squads: &self.squads,
            sounds: &self.sounds,
            world,
            config: &self.config,
            lod_focus: &self.lod_focus,
            raycasts: AtomicU32::new(0),
            path_queries: AtomicU32::new(0),
        };
        let actors = &self.actors;
        let run = |b: &mut Brain| {
            let me = &actors[b.actor.index as usize].state;
            b.update(me, &shared);
        };
        #[cfg(feature = "parallel")]
        if self.brains.len() >= self.config.parallel_threshold {
            use rayon::prelude::*;
            self.brains.par_iter_mut().with_min_len(32).for_each(run);
        } else {
            self.brains.iter_mut().for_each(run);
        }
        #[cfg(not(feature = "parallel"))]
        self.brains.iter_mut().for_each(run);
        let raycasts = shared.raycasts.load(Ordering::Relaxed);
        let path_queries = shared.path_queries.load(Ordering::Relaxed);

        // 4. Resolve shared resources deterministically.
        self.resolve_cover_claims();
        self.resolve_tokens();

        // 5. Emit commands.
        self.commands.clear();
        let mut thinks = 0;
        for b in &self.brains {
            let s = &self.actors[b.actor.index as usize].state;
            if !s.is_alive() {
                continue;
            }
            thinks += b.thought as u32;
            self.commands.push(b.command);
        }
        if self.config.auto_gunshot_radius > 0.0 {
            // One live gunshot sound per shooter: a burst refreshes the shooter's
            // existing sound instead of adding more, bounding the sound buffer.
            self.sound_of.clear();
            self.sound_of.resize(self.actors.len(), u32::MAX);
            for (i, s) in self.sounds.iter().enumerate() {
                if let (SoundKind::Gunshot, Some(src)) = (s.event.kind, s.event.source) {
                    self.sound_of[src.index as usize] = i as u32;
                }
            }
            for c in &self.commands {
                if !c.fire {
                    continue;
                }
                let s = &self.actors[c.actor.index as usize].state;
                self.sound_seq += 1;
                let sound = TimedSound {
                    seq: self.sound_seq,
                    event: SoundEvent {
                        position: s.position,
                        radius: self.config.auto_gunshot_radius,
                        source: Some(c.actor),
                        kind: SoundKind::Gunshot,
                    },
                    time: now,
                };
                match self.sound_of[c.actor.index as usize] {
                    u32::MAX => {
                        self.sound_of[c.actor.index as usize] = self.sounds.len() as u32;
                        self.sounds.push(sound);
                    }
                    i => self.sounds[i as usize] = sound,
                }
            }
        }
        self.stats = TickStats {
            agents: self.commands.len() as u32,
            thinks,
            raycasts,
            path_queries,
            micros: start.elapsed().as_secs_f32() * 1e6,
        };
        &self.commands
    }

    fn update_squads(&mut self) {
        let now = self.time;
        let role_interval = self.config.role_interval;
        for si in 0..self.squads.len() {
            let mut squad = std::mem::take(&mut self.squads[si]);
            squad.targets.clear();
            let mut centroid = Vec3::ZERO;
            let mut alive = 0;
            let mut target_votes: smallvec::SmallVec<[(ActorId, u32, f32); 8]> = Default::default();
            let mut targets = std::mem::take(&mut squad.targets);
            for &m in &squad.members {
                let Some(slot) = self.slot(m) else { continue };
                if !slot.state.is_alive() || slot.brain == u32::MAX {
                    continue;
                }
                alive += 1;
                centroid += slot.state.position;
                let b = &self.brains[slot.brain as usize];
                for e in b.memory.entries().iter().filter(|e| e.confirmed()) {
                    Squad::merge_target(
                        &mut targets,
                        SquadTarget {
                            target: e.target,
                            pos: e.last_known_pos,
                            vel: e.last_known_vel,
                            last_seen: e.last_update,
                        },
                    );
                }
                if let Some(t) = b.target {
                    let seen = b.memory.get(t).map_or(f32::NEG_INFINITY, |e| e.last_update);
                    match target_votes.iter_mut().find(|v| v.0 == t) {
                        Some(v) => {
                            v.1 += 1;
                            v.2 = v.2.max(seen);
                        }
                        None => target_votes.push((t, 1, seen)),
                    }
                }
            }
            squad.targets = targets;
            squad.centroid = if alive > 0 {
                centroid / alive as f32
            } else {
                Vec3::ZERO
            };
            squad.primary = target_votes
                .iter()
                .max_by(|a, b| a.1.cmp(&b.1).then(a.2.total_cmp(&b.2)))
                .map(|v| v.0);

            if now >= squad.next_role_eval || squad.primary != squad.role_target {
                squad.next_role_eval = now + role_interval;
                squad.role_target = squad.primary;
                self.assign_roles(&squad);
            }
            self.squads[si] = squad;
        }
    }

    fn assign_roles(&mut self, squad: &Squad) {
        // Living members with brains, in stable order.
        // (brain, aggression, health, suppression suitability)
        let mut members: smallvec::SmallVec<[(u32, f32, f32, f32); 16]> = Default::default();
        for &m in &squad.members {
            if let Some(slot) = self.slot(m) {
                if slot.state.is_alive() && slot.brain != u32::MAX {
                    let cfg = &self.brains[slot.brain as usize].archetype.config;
                    // Good suppressors are cautious and have a high rate of fire
                    // and a big magazine (a bolt-action sniper is a poor fit).
                    let rate = (0.25 / cfg.fire_interval.max(1e-3)).min(1.0);
                    let mag = (slot.state.mag_size as f32 / 20.0).min(1.0);
                    members.push((
                        slot.brain,
                        cfg.aggression,
                        slot.state.health_fraction(),
                        (1.0 - cfg.aggression) * rate * mag,
                    ));
                }
            }
        }
        for &(bi, ..) in &members {
            self.brains[bi as usize].role = SquadRole::Assault;
        }
        if squad.primary.is_none() || members.len() < 2 {
            return;
        }
        let n = members.len();
        // Flankers: the most aggressive healthy members.
        let flankers = if n >= 6 {
            2
        } else if n >= 3 {
            1
        } else {
            0
        };
        let mut by_aggr = members.clone();
        by_aggr.sort_by(|a, b| (b.1 * b.2).total_cmp(&(a.1 * a.2)).then(a.0.cmp(&b.0)));
        for &(bi, _, h, _) in by_aggr.iter().take(flankers) {
            if h > 0.4 {
                self.brains[bi as usize].role = SquadRole::Flanker;
            }
        }
        // Suppressors: the best suited of the rest.
        let suppressors = (n / 3).clamp(1, 3);
        let mut by_supp = members.clone();
        by_supp.sort_by(|a, b| b.3.total_cmp(&a.3).then(a.0.cmp(&b.0)));
        let mut assigned = 0;
        for &(bi, _, _, s) in by_supp.iter() {
            if assigned >= suppressors || s <= 0.05 {
                break;
            }
            let b = &mut self.brains[bi as usize];
            if b.role == SquadRole::Assault {
                b.role = SquadRole::Suppressor;
                assigned += 1;
            }
        }
    }

    /// Cover conflicts: the earliest claimant keeps it; ties go to the lower index.
    fn resolve_cover_claims(&mut self) {
        self.claim_scratch.clear();
        for (bi, b) in self.brains.iter().enumerate() {
            if let Some(c) = b.cover {
                self.claim_scratch.push((c.0, b.cover_since, bi as u32));
            }
        }
        self.claim_scratch
            .sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)));
        self.cover_claims.iter_mut().for_each(|c| *c = None);
        let mut last = u32::MAX;
        for &(c, _, bi) in &self.claim_scratch {
            let b = &mut self.brains[bi as usize];
            if c == last {
                // Lost the race: drop the claim; the tree will re-plan.
                if b.destination == Some(self.covers[c as usize].position) {
                    b.destination = None;
                }
                b.cover = None;
            } else {
                self.cover_claims[c as usize] = Some(b.actor);
                last = c;
            }
        }
    }

    /// Attack tokens: at most `max_attackers_per_player` agents shoot accurately at
    /// each external actor. Current holders keep priority, then nearest first.
    fn resolve_tokens(&mut self) {
        let max = self.config.max_attackers_per_player as usize;
        self.token_scratch.clear();
        for (bi, b) in self.brains.iter_mut().enumerate() {
            let Some(t) = b.wants_token else {
                b.token = None;
                continue;
            };
            let Some(ts) = self.actors.get(t.index as usize) else {
                continue;
            };
            if ts.brain != u32::MAX || max == 0 {
                b.token = None; // AI-vs-AI fights are not token limited
                continue;
            }
            let me = &self.actors[b.actor.index as usize].state;
            let d = me.position.distance(ts.state.position);
            self.token_scratch
                .push((t.index, b.token == Some(t), d, bi as u32));
        }
        self.token_scratch.sort_unstable_by(|a, b| {
            a.0.cmp(&b.0)
                .then(b.1.cmp(&a.1))
                .then(a.2.total_cmp(&b.2))
                .then(a.3.cmp(&b.3))
        });
        let mut cur = u32::MAX;
        let mut granted = 0;
        for &(t, _, _, bi) in &self.token_scratch {
            if t != cur {
                cur = t;
                granted = 0;
            }
            let b = &mut self.brains[bi as usize];
            if granted < max {
                b.token = b.wants_token;
                granted += 1;
            } else {
                b.token = None;
            }
        }
    }
}
