//! Actor state (owned by the host) and agent tuning parameters.

use crate::ids::TeamId;
use glam::Vec3;

/// Body posture.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum Stance {
    /// Standing upright.
    #[default]
    Stand,
    /// Crouched (hidden behind low cover).
    Crouch,
}

/// Physical state of a combatant, authored by the host game every frame.
///
/// The AI never mutates this; it only reads it. The host is authoritative for
/// position, health and ammo.
#[derive(Copy, Clone, Debug)]
pub struct ActorState {
    /// Feet position (+Y up).
    pub position: Vec3,
    /// Current velocity (used to lead/penalize aim and extrapolate memory).
    pub velocity: Vec3,
    /// Facing direction (normalized; the Y component is ignored for vision cones).
    pub forward: Vec3,
    /// Team; different teams are hostile.
    pub team: TeamId,
    /// Current health.
    pub health: f32,
    /// Maximum health.
    pub max_health: f32,
    /// Rounds in the current magazine.
    pub ammo: u16,
    /// Magazine capacity.
    pub mag_size: u16,
    /// Current stance (affects eye height).
    pub stance: Stance,
    /// Eye height when standing.
    pub eye_height: f32,
    /// Eye height when crouched.
    pub crouch_eye_height: f32,
}

impl ActorState {
    /// A healthy, fully loaded actor at `position` facing +Z.
    pub fn new(position: Vec3, team: TeamId) -> Self {
        Self {
            position,
            velocity: Vec3::ZERO,
            forward: Vec3::Z,
            team,
            health: 100.0,
            max_health: 100.0,
            ammo: 30,
            mag_size: 30,
            stance: Stance::Stand,
            eye_height: 1.6,
            crouch_eye_height: 1.0,
        }
    }

    /// Builder-style facing setter.
    pub fn facing(mut self, forward: Vec3) -> Self {
        self.forward = forward.normalize_or_zero();
        self
    }

    /// Whether the actor is alive.
    #[inline]
    pub fn is_alive(&self) -> bool {
        self.health > 0.0
    }

    /// World-space eye position for the current stance.
    #[inline]
    pub fn eye(&self) -> Vec3 {
        let h = match self.stance {
            Stance::Stand => self.eye_height,
            Stance::Crouch => self.crouch_eye_height,
        };
        self.position + Vec3::Y * h
    }

    /// Center of mass, used as the default aim/visibility point.
    #[inline]
    pub fn chest(&self) -> Vec3 {
        let h = match self.stance {
            Stance::Stand => self.eye_height,
            Stance::Crouch => self.crouch_eye_height,
        };
        self.position + Vec3::Y * (h * 0.75)
    }

    /// Health in `[0, 1]`.
    #[inline]
    pub fn health_fraction(&self) -> f32 {
        if self.max_health > 0.0 {
            (self.health / self.max_health).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// Ammo in `[0, 1]`.
    #[inline]
    pub fn ammo_fraction(&self) -> f32 {
        if self.mag_size > 0 {
            self.ammo as f32 / self.mag_size as f32
        } else {
            1.0
        }
    }
}

/// Tuning parameters shared by all agents of an [`crate::Archetype`].
#[derive(Clone, Debug)]
pub struct AgentConfig {
    // --- perception ---
    /// Full central field of view in degrees (fast detection).
    pub fov_deg: f32,
    /// Full peripheral field of view in degrees (slow detection).
    pub peripheral_fov_deg: f32,
    /// Maximum sight distance in meters.
    pub view_range: f32,
    /// Targets closer than this are detected instantly regardless of facing.
    pub proximity_range: f32,
    /// Detection-meter fill rate per second for a target at point-blank in the central FOV.
    pub detection_rate: f32,
    /// Multiplier on sound radii.
    pub hearing: f32,
    /// Seconds a confirmed target is remembered after losing sight of it.
    pub memory_span: f32,
    /// Max line-of-sight raycasts per perception update (others reuse cached results).
    pub max_los_checks: u8,
    /// Max hostile actors remembered at once; the least relevant are forgotten first.
    pub max_tracked_targets: u8,

    // --- combat ---
    /// Preferred engagement distance band `(min, max)` in meters.
    pub preferred_range: (f32, f32),
    /// Delay between confirming a target and the first shot.
    pub reaction_time: f32,
    /// Base aim spread (half-angle, degrees) once fully settled on a stationary
    /// target while calm. Shots land uniformly inside this cone, so the chance
    /// of hitting a target of radius `r` at distance `d` is roughly
    /// `(r / (d * tan(spread)))^2`. See [`AgentConfig::hit_chance`].
    pub accuracy_deg: f32,
    /// Seconds of continuous tracking to reach full accuracy.
    pub aim_settle_time: f32,
    /// Spread multiplier added while unsettled: a fresh target is shot at with
    /// `accuracy_deg * (1 + aim_unsettled_penalty)`, easing to `accuracy_deg`
    /// over `aim_settle_time`.
    pub aim_unsettled_penalty: f32,
    /// Extra spread (degrees) per m/s of target speed.
    pub aim_moving_target_penalty: f32,
    /// Extra spread (degrees) per m/s of the shooter's own speed (run and gun).
    pub aim_moving_self_penalty: f32,
    /// Extra spread (degrees) when fully under fire (scaled by the under-fire level).
    pub aim_under_fire_penalty: f32,
    /// Trigger discipline: aimed shots are only taken within the distance at
    /// which a settled, calm shot has at least this chance to hit (see
    /// [`AgentConfig::effective_range`]). Beyond it the agent repositions
    /// instead of wasting ammo. `0` disables the limit.
    pub min_hit_chance: f32,
    /// Seconds between shots within a burst.
    pub fire_interval: f32,
    /// Shots per burst.
    pub burst_length: u8,
    /// Pause between bursts `(min, max)` seconds.
    pub burst_pause: (f32, f32),
    /// Maximum rounds per second during suppressive fire at a position (never
    /// faster than `fire_interval` allows). Keeps suppression from emptying
    /// magazines into walls.
    pub suppression_rate: f32,
    /// 0 = cautious, 1 = reckless. Drives flanking and pushing.
    pub aggression: f32,
    /// 0 = flees early, 1 = fights to the death. Drives retreat.
    pub courage: f32,
    /// Seconds an agent tolerates having a target without dealing or taking
    /// damage before it grows impatient. Impatience then ramps from 0 to 1 over
    /// another `patience` seconds and pushes the agent's effective aggression
    /// and courage toward 1, so cautious agents eventually break a stand-off.
    /// `f32::INFINITY` disables it.
    pub patience: f32,

    // --- tactics ---
    /// Radius searched for cover / tactical positions.
    pub tactical_search_radius: f32,
    /// Radius around the spawn point used for patrolling.
    pub patrol_radius: f32,
    /// Distance at which a move order counts as arrived.
    pub arrive_radius: f32,
    /// Base seconds between decisions at full level of detail.
    pub think_interval: f32,
    /// After losing every target, keep patrolling around the last place an
    /// enemy was known to be for this many seconds before returning to the
    /// home position. `f32::INFINITY` means hunt forever (deathmatch-style);
    /// `0` returns home immediately (guard-style).
    pub hunt_time: f32,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            fov_deg: 100.0,
            peripheral_fov_deg: 180.0,
            view_range: 60.0,
            proximity_range: 2.5,
            detection_rate: 3.0,
            hearing: 1.0,
            memory_span: 15.0,
            max_los_checks: 6,
            max_tracked_targets: 8,
            preferred_range: (10.0, 30.0),
            reaction_time: 0.35,
            accuracy_deg: 1.1,
            aim_settle_time: 1.0,
            aim_unsettled_penalty: 1.0,
            aim_moving_target_penalty: 0.1,
            aim_moving_self_penalty: 0.1,
            aim_under_fire_penalty: 0.5,
            min_hit_chance: 0.1,
            fire_interval: 0.1,
            burst_length: 4,
            burst_pause: (0.35, 0.8),
            suppression_rate: 2.0,
            aggression: 0.5,
            courage: 0.5,
            patience: 20.0,
            tactical_search_radius: 25.0,
            patrol_radius: 12.0,
            arrive_radius: 0.75,
            think_interval: 0.1,
            hunt_time: 60.0,
        }
    }
}

impl AgentConfig {
    /// Aim spread half-angle in degrees for the given conditions, before the
    /// global [`crate::SystemConfig::accuracy_scale`] and attack-token penalty.
    ///
    /// * `settle`: `0` = just acquired the target, `1` = fully settled.
    /// * `target_speed`, `own_speed`: m/s.
    /// * `under_fire`: `0..=1`.
    pub fn spread_deg(
        &self,
        settle: f32,
        target_speed: f32,
        own_speed: f32,
        under_fire: f32,
    ) -> f32 {
        let settle = settle.clamp(0.0, 1.0);
        self.accuracy_deg * (1.0 + self.aim_unsettled_penalty * (1.0 - settle))
            + target_speed.max(0.0) * self.aim_moving_target_penalty
            + own_speed.max(0.0) * self.aim_moving_self_penalty
            + under_fire.clamp(0.0, 1.0) * self.aim_under_fire_penalty
    }

    /// Distance at which a settled, calm shot at a stationary target of
    /// `target_radius` still has `min_hit_chance` to hit. Infinite when the
    /// limit is disabled.
    pub fn effective_range(&self, target_radius: f32) -> f32 {
        let tan = self.accuracy_deg.max(1e-3).to_radians().tan();
        if self.min_hit_chance <= 0.0 {
            return f32::INFINITY;
        }
        target_radius.max(0.0) / (self.min_hit_chance.min(1.0).sqrt() * tan)
    }

    /// Probability that one shot with `spread_deg` hits a target presenting a
    /// disc of radius `target_radius` at `distance` meters (the aim model's
    /// distribution is uniform over the spread cone). Useful for tuning tables:
    ///
    /// ```
    /// use combat_ai::AgentConfig;
    /// let cfg = AgentConfig::default();
    /// let calm = cfg.spread_deg(1.0, 0.0, 0.0, 0.0);
    /// let p = AgentConfig::hit_chance(calm, 20.0, 0.3);
    /// assert!(p > 0.0 && p <= 1.0);
    /// ```
    pub fn hit_chance(spread_deg: f32, distance: f32, target_radius: f32) -> f32 {
        let r = distance.max(0.0) * spread_deg.max(0.0).to_radians().tan();
        if r <= target_radius {
            1.0
        } else {
            (target_radius / r).powi(2)
        }
    }
}
