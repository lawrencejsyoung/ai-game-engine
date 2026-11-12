//! Per-agent runtime state and the think loop.

use crate::agent::{ActorState, Stance};
use crate::archetype::Archetype;
use crate::bt::{NodeId, NodeMem, Status};
use crate::command::{AgentCommand, Bark, MoveSpeed};
use crate::ids::{ActorId, CoverId, SquadId};
use crate::math::{band, dist2_xz, mix64, Rng};
use crate::memory::Memory;
use crate::spatial::SpatialGrid;
use crate::squad::{Squad, SquadRole};
use crate::stimuli::{DamageEvent, TimedSound};
use crate::system::SystemConfig;
use crate::tactics::CoverPoint;
use crate::utility::{self, Input, Inputs};
use crate::world::WorldQuery;
use glam::Vec3;
use smallvec::SmallVec;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

/// Storage slot for any actor (AI-driven or external).
#[derive(Clone, Debug)]
pub(crate) struct ActorSlot {
    pub generation: u32,
    pub occupied: bool,
    pub state: ActorState,
    /// Index into the brain array, or `u32::MAX` for external actors.
    pub brain: u32,
    pub squad: Option<SquadId>,
}

/// Read-only view of the world shared by all agents during the parallel phase.
pub(crate) struct Shared<'a> {
    pub time: f32,
    pub dt: f32,
    pub actors: &'a [ActorSlot],
    pub actor_grid: &'a SpatialGrid,
    pub covers: &'a [CoverPoint],
    pub cover_grid: &'a SpatialGrid,
    pub cover_claims: &'a [Option<ActorId>],
    pub squads: &'a [Squad],
    pub sounds: &'a [TimedSound],
    pub world: &'a dyn WorldQuery,
    pub config: &'a SystemConfig,
    pub lod_focus: &'a [Vec3],
    pub raycasts: AtomicU32,
    pub path_queries: AtomicU32,
}

impl Shared<'_> {
    #[inline]
    pub fn actor(&self, id: ActorId) -> Option<&ActorState> {
        let s = self.actors.get(id.index as usize)?;
        (s.occupied && s.generation == id.generation).then_some(&s.state)
    }

    #[inline]
    pub fn actor_id(&self, index: u32) -> ActorId {
        ActorId {
            index,
            generation: self.actors[index as usize].generation,
        }
    }

    /// Counted line-of-sight query.
    #[inline]
    pub fn los(&self, from: Vec3, to: Vec3) -> bool {
        self.raycasts.fetch_add(1, Ordering::Relaxed);
        self.world.line_of_sight(from, to)
    }
}

/// Where the agent looks.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) enum LookMode {
    /// Movement direction / current facing.
    Forward,
    /// The primary target (live position while visible, else last known).
    Target,
    /// A fixed point.
    Point(Vec3),
}

/// What the agent shoots at.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) enum FireMode {
    Hold,
    Target,
    Point(Vec3),
}

/// Persistent low-level intent produced by behavior tree tasks and executed every frame.
#[derive(Copy, Clone, Debug)]
pub(crate) struct Intent {
    pub move_to: Option<Vec3>,
    pub speed: MoveSpeed,
    pub look: LookMode,
    pub fire: FireMode,
    pub stance: Stance,
    pub reload: bool,
}

impl Default for Intent {
    fn default() -> Self {
        Self {
            move_to: None,
            speed: MoveSpeed::Walk,
            look: LookMode::Forward,
            fire: FireMode::Hold,
            stance: Stance::Stand,
            reload: false,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct AimState {
    pub target: Option<ActorId>,
    pub settle: f32,
    pub error: glam::Vec2,
    pub next_shot: f32,
    pub shots_in_burst: u8,
}

pub(crate) struct Brain {
    pub actor: ActorId,
    pub archetype: Arc<Archetype>,
    pub rng: Rng,
    pub home: Vec3,

    // perception
    pub memory: Memory,
    pub last_perception: f32,
    /// Highest sound sequence number already heard.
    pub heard_seq: u64,
    pub next_think: f32,
    pub think_interval: f32,
    pub pending_damage: SmallVec<[DamageEvent; 4]>,
    pub wake: bool,
    pub under_fire: f32,
    pub investigate: Option<Vec3>,
    /// Host-provided intel ([`crate::AiSystem::reveal`]): target, position, velocity.
    pub pending_intel: SmallVec<[(ActorId, Vec3, Vec3); 2]>,
    /// Where and when a confirmed enemy was last known to be (survives forgetting).
    pub last_contact: Option<(Vec3, f32)>,
    /// Seconds spent with a target but without exchanging damage (impatience).
    pub calm_time: f32,

    // decision
    pub target: Option<ActorId>,
    pub reaction_ready: f32,
    pub current_action: Option<usize>,
    pub action_finished: bool,
    pub action_start: f32,
    pub bt_mem: Vec<NodeMem>,
    pub bt_tick: u32,
    pub cooldowns: SmallVec<[(u16, NodeId, f32); 4]>,
    pub scores: SmallVec<[f32; 12]>,
    /// Actions that recently failed, blocked until the given time.
    pub suppressed: SmallVec<[(u16, f32); 4]>,
    pub intent: Intent,
    pub destination: Option<Vec3>,
    pub cover: Option<CoverId>,
    pub cover_since: f32,
    pub role: SquadRole,
    pub squad: Option<SquadId>,
    pub wants_token: Option<ActorId>,
    pub token: Option<ActorId>,

    // output
    pub aim: AimState,
    pub pending_bark: Option<Bark>,
    pub last_bark: f32,
    pub command: AgentCommand,
    pub thought: bool,
}

impl Brain {
    pub fn new(actor: ActorId, archetype: Arc<Archetype>, state: &ActorState, seed: u64) -> Self {
        // Hash seed and handle together: a plain `seed ^ index` would give agent 0
        // exactly the stream a host gets from `Rng::new(seed)`, correlating the
        // AI's dice with the host's (e.g. spawn placement).
        let mut rng = Rng::new(mix64(
            mix64(seed) ^ ((actor.index as u64) << 32 | actor.generation as u64),
        ));
        let interval = archetype.config.think_interval;
        let n_actions = archetype.actions.len();
        // Stagger first thinks so agents spawned together don't all think on one frame.
        let first_think = rng.range(0.0, interval);
        Self {
            actor,
            rng,
            home: state.position,
            memory: Memory::default(),
            last_perception: f32::NEG_INFINITY,
            heard_seq: 0,
            next_think: first_think,
            think_interval: interval,
            pending_damage: SmallVec::new(),
            wake: false,
            under_fire: 0.0,
            investigate: None,
            pending_intel: SmallVec::new(),
            last_contact: None,
            calm_time: 0.0,
            target: None,
            reaction_ready: 0.0,
            current_action: None,
            action_finished: false,
            action_start: 0.0,
            bt_mem: Vec::new(),
            bt_tick: 0,
            cooldowns: SmallVec::new(),
            scores: smallvec::smallvec![0.0; n_actions],
            suppressed: SmallVec::new(),
            intent: Intent::default(),
            destination: None,
            cover: None,
            cover_since: 0.0,
            role: SquadRole::Assault,
            squad: None,
            wants_token: None,
            token: None,
            aim: AimState::default(),
            pending_bark: None,
            last_bark: f32::NEG_INFINITY,
            command: AgentCommand::idle(actor, state.forward),
            thought: false,
            archetype,
        }
    }

    pub fn bark(&mut self, now: f32, bark: Bark) {
        if now - self.last_bark >= 2.5 {
            self.pending_bark = Some(bark);
            self.last_bark = now;
        }
    }

    /// Stand-off frustration in `[0, 1]`.
    pub fn impatience(&self) -> f32 {
        let p = self.archetype.config.patience;
        if p.is_nan() || p == f32::INFINITY {
            0.0
        } else if p <= 0.0 {
            1.0
        } else {
            ((self.calm_time - p) / p).clamp(0.0, 1.0)
        }
    }

    /// Center for patrolling: the last contact while hunting, else home.
    pub fn patrol_center(&self, now: f32) -> Vec3 {
        match self.last_contact {
            Some((p, t)) if now - t <= self.archetype.config.hunt_time => p,
            _ => self.home,
        }
    }

    /// Releases tactical resources (on death / removal).
    pub fn release(&mut self) {
        self.cover = None;
        self.token = None;
        self.wants_token = None;
        self.destination = None;
    }

    pub fn in_cover(&self, me: &ActorState, sh: &Shared) -> bool {
        let Some(c) = self.cover else { return false };
        let cover = &sh.covers[c.0 as usize];
        let r = self.archetype.config.arrive_radius.max(1.0);
        if dist2_xz(me.position, cover.position) > r * r {
            return false;
        }
        match self.target.and_then(|t| self.memory.get(t)) {
            Some(t) => cover.protection_from(t.last_known_pos) > 0.3,
            None => true,
        }
    }

    fn update_lod(&mut self, me: &ActorState, sh: &Shared) {
        let base = self.archetype.config.think_interval.max(1e-3);
        let engaged = self.under_fire > 0.0
            || self
                .target
                .and_then(|t| self.memory.get(t))
                .is_some_and(|m| m.visible);
        if engaged || sh.lod_focus.is_empty() {
            self.think_interval = base;
            return;
        }
        let d2 = sh
            .lod_focus
            .iter()
            .map(|f| dist2_xz(*f, me.position))
            .fold(f32::INFINITY, f32::min);
        let lod = &sh.config.lod_distances;
        self.think_interval = if d2 < lod.0 * lod.0 {
            base
        } else if d2 < lod.1 * lod.1 {
            base * 2.5
        } else {
            base * 6.0
        };
    }

    fn select_target(&mut self, me: &ActorState, now: f32) {
        let prev = self.target;
        let mut best: Option<(ActorId, f32)> = None;
        for e in self.memory.entries.iter().filter(|e| e.confirmed()) {
            let d = e.last_known_pos.distance(me.position);
            let mut s = 1.0 / (1.0 + d * 0.05);
            if e.visible {
                s += 2.0;
            }
            if now - e.last_damaged_us < 3.0 {
                s += 1.5;
            }
            if Some(e.target) == prev {
                s += 0.75;
            }
            if best.is_none_or(|(_, b)| s > b) {
                best = Some((e.target, s));
            }
        }
        self.target = best.map(|b| b.0);
        if self.target != prev && self.target.is_some() {
            let rt = self.archetype.config.reaction_time;
            self.reaction_ready = now + rt * self.rng.range(0.8, 1.25);
        }
    }

    fn compute_inputs(&self, me: &ActorState, sh: &Shared) -> Inputs {
        let cfg = &self.archetype.config;
        let now = sh.time;
        let mut x = Inputs::default();
        x.set(Input::Health, me.health_fraction());
        x.set(Input::Ammo, me.ammo_fraction());
        x.set(Input::UnderFire, self.under_fire);
        let imp = self.impatience();
        x.set(Input::Impatience, imp);
        x.set(
            Input::Aggression,
            cfg.aggression + (1.0 - cfg.aggression) * imp,
        );
        x.set(Input::Courage, cfg.courage + (1.0 - cfg.courage) * imp);
        x.set(
            Input::ThreatCount,
            self.memory.confirmed_count() as f32 / 4.0,
        );
        x.set(Input::InCover, self.in_cover(me, sh) as u8 as f32);
        x.set(Input::TimeInAction, (now - self.action_start) / 10.0);
        let role = |r| (self.role == r) as u8 as f32;
        x.set(Input::RoleFlanker, role(SquadRole::Flanker));
        x.set(Input::RoleSuppressor, role(SquadRole::Suppressor));
        x.set(Input::RoleAssault, role(SquadRole::Assault));

        let mut investigate = self.investigate.is_some()
            || self
                .memory
                .entries
                .iter()
                .any(|e| !e.confirmed() && e.awareness > 0.25 && !e.searched);
        if let Some(t) = self.target.and_then(|t| self.memory.get(t)) {
            x.set(Input::HasTarget, 1.0);
            x.set(Input::TargetVisible, t.visible as u8 as f32);
            let d = t.last_known_pos.distance(me.position);
            x.set(
                Input::TargetInRange,
                band(d, cfg.preferred_range.0, cfg.preferred_range.1, 10.0),
            );
            x.set(Input::TargetDistance, d / cfg.view_range);
            let seen = if t.last_seen.is_finite() {
                t.last_seen
            } else {
                t.last_update
            };
            x.set(Input::TimeSinceTargetSeen, (now - seen) / 10.0);
            let max_tokens = sh.config.max_attackers_per_player;
            let target_is_external = sh
                .actors
                .get(t.target.index as usize)
                .is_some_and(|s| s.brain == u32::MAX);
            let has_token = max_tokens == 0 || !target_is_external || self.token == Some(t.target);
            x.set(Input::HasAttackToken, has_token as u8 as f32);
            investigate |= !t.visible && !t.searched;
        }
        x.set(Input::HasInvestigatePoint, investigate as u8 as f32);
        x
    }

    /// Runs a full perceive → decide → behave step.
    pub fn think(&mut self, me: &ActorState, sh: &Shared) {
        let now = sh.time;
        crate::perception::perceive(self, me, sh);
        self.select_target(me, now);
        self.update_lod(me, sh);

        let inputs = self.compute_inputs(me, sh);
        let arch = Arc::clone(&self.archetype);
        let current = if self.action_finished {
            None
        } else {
            self.current_action
        };
        self.suppressed.retain(|s| s.1 > now);
        let suppressed = self.suppressed.clone();
        let chosen = utility::select(
            &arch.actions,
            &inputs,
            current,
            now - self.action_start,
            &mut self.scores,
            |i| suppressed.iter().any(|s| s.0 as usize == i),
        );
        if chosen != self.current_action || self.action_finished {
            if chosen != self.current_action {
                self.action_start = now;
                self.intent.fire = FireMode::Hold;
                self.intent.reload = false;
                self.intent.look = LookMode::Forward;
            }
            self.current_action = chosen;
            self.action_finished = false;
            // Skip a tick number so every node is treated as freshly entered.
            self.bt_tick = self.bt_tick.wrapping_add(2);
            if let Some(i) = chosen {
                self.bt_mem.clear();
                self.bt_mem
                    .resize(arch.actions[i].tree.len(), NodeMem::default());
            }
        }
        if let Some(i) = self.current_action {
            self.bt_tick = self.bt_tick.wrapping_add(1);
            let mut mem = std::mem::take(&mut self.bt_mem);
            let tick = self.bt_tick;
            let mut ctx = crate::tasks::Ctx {
                b: self,
                me,
                sh,
                action: i as u16,
            };
            let status = arch.actions[i].tree.tick(&mut ctx, &mut mem, tick);
            self.bt_mem = mem;
            if status != Status::Running {
                self.action_finished = true;
            }
            if status == Status::Failure {
                // Don't immediately retry an action that just failed (e.g. no cover found).
                self.suppressed.push((i as u16, now + 1.5));
            }
        }
        self.thought = true;
    }

    /// Per-frame update: think if due, then produce this frame's command.
    pub fn update(&mut self, me: &ActorState, sh: &Shared) {
        self.thought = false;
        if !me.is_alive() {
            self.release();
            return;
        }
        if sh.time >= self.next_think || self.wake {
            self.wake = false;
            self.think(me, sh);
            self.next_think = sh.time + self.think_interval;
        }
        crate::aim::act(self, me, sh);
    }
}
