//! C ABI for [`combat_ai`].
//!
//! The matching header is `include/combat_ai.h`; see the crate README for
//! build and link instructions. Design rules:
//!
//! * Every type crossing the boundary is `#[repr(C)]` plain data with
//!   fixed-width fields. Enums are passed as integers, never as C `enum`s
//!   (whose size is implementation-defined).
//! * Objects (`CaiSystem`, `CaiArchetype`) are opaque heap pointers owned by
//!   the caller and released with their `_free` function.
//! * No Rust panic ever unwinds into C: every entry point catches it, and
//!   reports failure through its return value plus [`cai_last_error`].
//! * Null pointers are checked and reported as errors, never dereferenced.
//! * `CaiActorId` is a 64-bit generational handle; [`CAI_INVALID_ACTOR`] never
//!   refers to a live actor, and stale handles are inert.
//!
//! Thread safety: a `CaiSystem` must be used from one thread at a time (wrap
//! it in your own lock otherwise). During [`cai_tick`] the world callbacks may
//! be invoked concurrently from worker threads when the `parallel` feature is
//! on and the agent count reaches `parallel_threshold`; set the threshold to
//! `UINT32_MAX` to keep every callback on the calling thread.

// The C ABI is unsafe by nature; each function documents its contract.
#![allow(clippy::missing_safety_doc)]

use combat_ai::prelude::*;
use combat_ai::{AgentDebug, TickStats};
use std::cell::RefCell;
use std::ffi::{c_char, c_void, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Constants

/// Bumped on any incompatible change to the C ABI.
pub const CAI_ABI_VERSION: u32 = 1;

/// Packed handle that never refers to a live actor.
pub const CAI_INVALID_ACTOR: u64 = u64::MAX;
/// Returned instead of an index when there is none (squads, cover points).
pub const CAI_INVALID_INDEX: u32 = u32::MAX;

/// Success.
pub const CAI_OK: i32 = 0;
/// A required pointer argument was null.
pub const CAI_ERR_NULL: i32 = -1;
/// An argument was out of range or referred to something that doesn't exist.
pub const CAI_ERR_INVALID: i32 = -2;
/// The library panicked internally (a bug); see `cai_last_error`.
pub const CAI_ERR_PANIC: i32 = -3;

// ---------------------------------------------------------------------------
// Plain data

/// 3D vector, +Y up.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct CaiVec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl From<CaiVec3> for Vec3 {
    fn from(v: CaiVec3) -> Self {
        Vec3::new(v.x, v.y, v.z)
    }
}

impl From<Vec3> for CaiVec3 {
    fn from(v: Vec3) -> Self {
        CaiVec3 {
            x: v.x,
            y: v.y,
            z: v.z,
        }
    }
}

/// Mirror of [`combat_ai::SystemConfig`].
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct CaiSystemConfig {
    pub seed: u64,
    pub grid_cell_size: f32,
    pub max_attackers_per_player: u32,
    pub no_token_spread_deg: f32,
    pub accuracy_scale: f32,
    pub target_radius: f32,
    pub lod_full_distance: f32,
    pub lod_medium_distance: f32,
    pub auto_gunshot_radius: f32,
    pub sound_retention: f32,
    pub role_interval: f32,
    /// `UINT32_MAX` keeps the whole tick (and every callback) on the calling thread.
    pub parallel_threshold: u32,
    pub _reserved: u32,
}

impl From<&SystemConfig> for CaiSystemConfig {
    fn from(c: &SystemConfig) -> Self {
        Self {
            seed: c.seed,
            grid_cell_size: c.grid_cell_size,
            max_attackers_per_player: c.max_attackers_per_player as u32,
            no_token_spread_deg: c.no_token_spread_deg,
            accuracy_scale: c.accuracy_scale,
            target_radius: c.target_radius,
            lod_full_distance: c.lod_distances.0,
            lod_medium_distance: c.lod_distances.1,
            auto_gunshot_radius: c.auto_gunshot_radius,
            sound_retention: c.sound_retention,
            role_interval: c.role_interval,
            parallel_threshold: c.parallel_threshold.min(u32::MAX as usize) as u32,
            _reserved: 0,
        }
    }
}

impl From<&CaiSystemConfig> for SystemConfig {
    fn from(c: &CaiSystemConfig) -> Self {
        Self {
            seed: c.seed,
            grid_cell_size: c.grid_cell_size,
            max_attackers_per_player: c.max_attackers_per_player.min(u8::MAX as u32) as u8,
            no_token_spread_deg: c.no_token_spread_deg,
            accuracy_scale: c.accuracy_scale,
            target_radius: c.target_radius,
            lod_distances: (c.lod_full_distance, c.lod_medium_distance),
            auto_gunshot_radius: c.auto_gunshot_radius,
            sound_retention: c.sound_retention,
            role_interval: c.role_interval,
            parallel_threshold: if c.parallel_threshold == u32::MAX {
                usize::MAX
            } else {
                c.parallel_threshold as usize
            },
        }
    }
}

/// Mirror of [`combat_ai::AgentConfig`]. Field meanings are documented there.
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct CaiAgentConfig {
    // perception
    pub fov_deg: f32,
    pub peripheral_fov_deg: f32,
    pub view_range: f32,
    pub proximity_range: f32,
    pub detection_rate: f32,
    pub hearing: f32,
    pub memory_span: f32,
    pub max_los_checks: u32,
    pub max_tracked_targets: u32,
    // combat
    pub preferred_range_min: f32,
    pub preferred_range_max: f32,
    pub reaction_time: f32,
    pub accuracy_deg: f32,
    pub aim_settle_time: f32,
    pub aim_unsettled_penalty: f32,
    pub aim_moving_target_penalty: f32,
    pub aim_moving_self_penalty: f32,
    pub aim_under_fire_penalty: f32,
    pub min_hit_chance: f32,
    pub fire_interval: f32,
    pub burst_length: u32,
    pub burst_pause_min: f32,
    pub burst_pause_max: f32,
    pub suppression_rate: f32,
    pub aggression: f32,
    pub courage: f32,
    pub patience: f32,
    // tactics
    pub tactical_search_radius: f32,
    pub patrol_radius: f32,
    pub arrive_radius: f32,
    pub think_interval: f32,
    pub hunt_time: f32,
}

impl From<&AgentConfig> for CaiAgentConfig {
    fn from(c: &AgentConfig) -> Self {
        Self {
            fov_deg: c.fov_deg,
            peripheral_fov_deg: c.peripheral_fov_deg,
            view_range: c.view_range,
            proximity_range: c.proximity_range,
            detection_rate: c.detection_rate,
            hearing: c.hearing,
            memory_span: c.memory_span,
            max_los_checks: c.max_los_checks as u32,
            max_tracked_targets: c.max_tracked_targets as u32,
            preferred_range_min: c.preferred_range.0,
            preferred_range_max: c.preferred_range.1,
            reaction_time: c.reaction_time,
            accuracy_deg: c.accuracy_deg,
            aim_settle_time: c.aim_settle_time,
            aim_unsettled_penalty: c.aim_unsettled_penalty,
            aim_moving_target_penalty: c.aim_moving_target_penalty,
            aim_moving_self_penalty: c.aim_moving_self_penalty,
            aim_under_fire_penalty: c.aim_under_fire_penalty,
            min_hit_chance: c.min_hit_chance,
            fire_interval: c.fire_interval,
            burst_length: c.burst_length as u32,
            burst_pause_min: c.burst_pause.0,
            burst_pause_max: c.burst_pause.1,
            suppression_rate: c.suppression_rate,
            aggression: c.aggression,
            courage: c.courage,
            patience: c.patience,
            tactical_search_radius: c.tactical_search_radius,
            patrol_radius: c.patrol_radius,
            arrive_radius: c.arrive_radius,
            think_interval: c.think_interval,
            hunt_time: c.hunt_time,
        }
    }
}

impl From<&CaiAgentConfig> for AgentConfig {
    fn from(c: &CaiAgentConfig) -> Self {
        let u8c = |v: u32| v.min(u8::MAX as u32) as u8;
        Self {
            fov_deg: c.fov_deg,
            peripheral_fov_deg: c.peripheral_fov_deg,
            view_range: c.view_range,
            proximity_range: c.proximity_range,
            detection_rate: c.detection_rate,
            hearing: c.hearing,
            memory_span: c.memory_span,
            max_los_checks: u8c(c.max_los_checks),
            max_tracked_targets: u8c(c.max_tracked_targets),
            preferred_range: (c.preferred_range_min, c.preferred_range_max),
            reaction_time: c.reaction_time,
            accuracy_deg: c.accuracy_deg,
            aim_settle_time: c.aim_settle_time,
            aim_unsettled_penalty: c.aim_unsettled_penalty,
            aim_moving_target_penalty: c.aim_moving_target_penalty,
            aim_moving_self_penalty: c.aim_moving_self_penalty,
            aim_under_fire_penalty: c.aim_under_fire_penalty,
            min_hit_chance: c.min_hit_chance,
            fire_interval: c.fire_interval,
            burst_length: u8c(c.burst_length),
            burst_pause: (c.burst_pause_min, c.burst_pause_max),
            suppression_rate: c.suppression_rate,
            aggression: c.aggression,
            courage: c.courage,
            patience: c.patience,
            tactical_search_radius: c.tactical_search_radius,
            patrol_radius: c.patrol_radius,
            arrive_radius: c.arrive_radius,
            think_interval: c.think_interval,
            hunt_time: c.hunt_time,
        }
    }
}

/// Stance values.
pub const CAI_STANCE_STAND: u8 = 0;
pub const CAI_STANCE_CROUCH: u8 = 1;

/// Mirror of [`combat_ai::ActorState`].
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct CaiActorState {
    pub position: CaiVec3,
    pub velocity: CaiVec3,
    pub forward: CaiVec3,
    pub health: f32,
    pub max_health: f32,
    pub eye_height: f32,
    pub crouch_eye_height: f32,
    pub ammo: u16,
    pub mag_size: u16,
    pub team: u16,
    /// `CAI_STANCE_*`.
    pub stance: u8,
    pub _reserved: u8,
}

fn stance_to_c(s: Stance) -> u8 {
    match s {
        Stance::Stand => CAI_STANCE_STAND,
        Stance::Crouch => CAI_STANCE_CROUCH,
    }
}

impl From<&ActorState> for CaiActorState {
    fn from(s: &ActorState) -> Self {
        Self {
            position: s.position.into(),
            velocity: s.velocity.into(),
            forward: s.forward.into(),
            health: s.health,
            max_health: s.max_health,
            eye_height: s.eye_height,
            crouch_eye_height: s.crouch_eye_height,
            ammo: s.ammo,
            mag_size: s.mag_size,
            team: s.team.0,
            stance: stance_to_c(s.stance),
            _reserved: 0,
        }
    }
}

fn state_from_c(c: &CaiActorState) -> ActorState {
    let mut s = ActorState::new(c.position.into(), TeamId(c.team));
    s.velocity = c.velocity.into();
    s.forward = Vec3::from(c.forward).normalize_or(Vec3::Z);
    s.health = c.health;
    s.max_health = c.max_health;
    s.eye_height = c.eye_height;
    s.crouch_eye_height = c.crouch_eye_height;
    s.ammo = c.ammo;
    s.mag_size = c.mag_size;
    s.stance = if c.stance == CAI_STANCE_CROUCH {
        Stance::Crouch
    } else {
        Stance::Stand
    };
    s
}

/// Cover kinds.
pub const CAI_COVER_LOW: u32 = 0;
pub const CAI_COVER_HIGH: u32 = 1;

/// Mirror of [`combat_ai::CoverPoint`].
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct CaiCoverPoint {
    pub position: CaiVec3,
    /// Horizontal direction toward the obstacle (what the cover protects against).
    pub facing: CaiVec3,
    /// `CAI_COVER_*`.
    pub kind: u32,
}

/// Sound kinds.
pub const CAI_SOUND_GUNSHOT: u32 = 0;
pub const CAI_SOUND_MOVEMENT: u32 = 1;
pub const CAI_SOUND_EXPLOSION: u32 = 2;
pub const CAI_SOUND_BULLET_IMPACT: u32 = 3;

/// Mirror of [`combat_ai::SoundEvent`].
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct CaiSoundEvent {
    /// Actor that made the sound, or `CAI_INVALID_ACTOR`.
    pub source: u64,
    pub position: CaiVec3,
    pub radius: f32,
    /// `CAI_SOUND_*`.
    pub kind: u32,
    pub _reserved: u32,
}

/// Mirror of [`combat_ai::DamageEvent`].
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct CaiDamageEvent {
    pub victim: u64,
    /// Attacker, or `CAI_INVALID_ACTOR` if unknown.
    pub attacker: u64,
    pub origin: CaiVec3,
    pub amount: f32,
}

/// `CaiAgentCommand::flags` bits.
pub const CAI_CMD_MOVE: u32 = 1;
pub const CAI_CMD_HAS_AIM_POINT: u32 = 2;
pub const CAI_CMD_FIRE: u32 = 4;
pub const CAI_CMD_RELOAD: u32 = 8;

/// Move speeds.
pub const CAI_SPEED_WALK: u8 = 0;
pub const CAI_SPEED_RUN: u8 = 1;
pub const CAI_SPEED_SPRINT: u8 = 2;

/// Barks (`0` = none).
pub const CAI_BARK_NONE: u8 = 0;
pub const CAI_BARK_ENEMY_SPOTTED: u8 = 1;
pub const CAI_BARK_MOVING_TO_COVER: u8 = 2;
pub const CAI_BARK_RELOADING: u8 = 3;
pub const CAI_BARK_FLANKING: u8 = 4;
pub const CAI_BARK_SUPPRESSING: u8 = 5;
pub const CAI_BARK_RETREATING: u8 = 6;
pub const CAI_BARK_SEARCHING: u8 = 7;

/// Actions. `CAI_ACTION_CUSTOM` carries its number in `custom_action`.
pub const CAI_ACTION_IDLE: u8 = 0;
pub const CAI_ACTION_PATROL: u8 = 1;
pub const CAI_ACTION_INVESTIGATE: u8 = 2;
pub const CAI_ACTION_ENGAGE: u8 = 3;
pub const CAI_ACTION_TAKE_COVER: u8 = 4;
pub const CAI_ACTION_FLANK: u8 = 5;
pub const CAI_ACTION_SUPPRESS: u8 = 6;
pub const CAI_ACTION_RELOAD: u8 = 7;
pub const CAI_ACTION_RETREAT: u8 = 8;
pub const CAI_ACTION_CUSTOM: u8 = 255;

/// Squad roles.
pub const CAI_ROLE_ASSAULT: u8 = 0;
pub const CAI_ROLE_SUPPRESSOR: u8 = 1;
pub const CAI_ROLE_FLANKER: u8 = 2;

/// Mirror of [`combat_ai::AgentCommand`].
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct CaiAgentCommand {
    pub actor: u64,
    /// Actor being aimed at, or `CAI_INVALID_ACTOR` (suppressive fire / not aiming).
    pub aim_target: u64,
    /// Valid when `flags & CAI_CMD_MOVE`.
    pub move_to: CaiVec3,
    /// Normalized aim direction (includes aim error). Resolve shots along it.
    pub aim_dir: CaiVec3,
    /// Valid when `flags & CAI_CMD_HAS_AIM_POINT`.
    pub aim_point: CaiVec3,
    /// `CAI_CMD_*` bits.
    pub flags: u32,
    /// `CAI_SPEED_*`.
    pub move_speed: u8,
    /// `CAI_STANCE_*`.
    pub stance: u8,
    /// `CAI_BARK_*`.
    pub bark: u8,
    /// `CAI_ACTION_*`.
    pub action: u8,
    /// Number of a `CAI_ACTION_CUSTOM` action.
    pub custom_action: u16,
    pub _reserved: u16,
}

fn action_to_c(a: ActionKind) -> (u8, u16) {
    match a {
        ActionKind::Idle => (CAI_ACTION_IDLE, 0),
        ActionKind::Patrol => (CAI_ACTION_PATROL, 0),
        ActionKind::Investigate => (CAI_ACTION_INVESTIGATE, 0),
        ActionKind::Engage => (CAI_ACTION_ENGAGE, 0),
        ActionKind::TakeCover => (CAI_ACTION_TAKE_COVER, 0),
        ActionKind::Flank => (CAI_ACTION_FLANK, 0),
        ActionKind::Suppress => (CAI_ACTION_SUPPRESS, 0),
        ActionKind::Reload => (CAI_ACTION_RELOAD, 0),
        ActionKind::Retreat => (CAI_ACTION_RETREAT, 0),
        ActionKind::Custom(n) => (CAI_ACTION_CUSTOM, n),
    }
}

fn bark_to_c(b: Option<Bark>) -> u8 {
    match b {
        None => CAI_BARK_NONE,
        Some(Bark::EnemySpotted) => CAI_BARK_ENEMY_SPOTTED,
        Some(Bark::MovingToCover) => CAI_BARK_MOVING_TO_COVER,
        Some(Bark::Reloading) => CAI_BARK_RELOADING,
        Some(Bark::Flanking) => CAI_BARK_FLANKING,
        Some(Bark::Suppressing) => CAI_BARK_SUPPRESSING,
        Some(Bark::Retreating) => CAI_BARK_RETREATING,
        Some(Bark::Searching) => CAI_BARK_SEARCHING,
    }
}

fn role_to_c(r: SquadRole) -> u8 {
    match r {
        SquadRole::Assault => CAI_ROLE_ASSAULT,
        SquadRole::Suppressor => CAI_ROLE_SUPPRESSOR,
        SquadRole::Flanker => CAI_ROLE_FLANKER,
    }
}

impl From<&AgentCommand> for CaiAgentCommand {
    fn from(c: &AgentCommand) -> Self {
        let mut flags = 0;
        if c.move_to.is_some() {
            flags |= CAI_CMD_MOVE;
        }
        if c.aim_point.is_some() {
            flags |= CAI_CMD_HAS_AIM_POINT;
        }
        if c.fire {
            flags |= CAI_CMD_FIRE;
        }
        if c.reload {
            flags |= CAI_CMD_RELOAD;
        }
        let (action, custom_action) = action_to_c(c.action);
        Self {
            actor: c.actor.to_bits(),
            aim_target: c.aim_target.map_or(CAI_INVALID_ACTOR, ActorId::to_bits),
            move_to: c.move_to.unwrap_or(Vec3::ZERO).into(),
            aim_dir: c.aim_dir.into(),
            aim_point: c.aim_point.unwrap_or(Vec3::ZERO).into(),
            flags,
            move_speed: match c.move_speed {
                MoveSpeed::Walk => CAI_SPEED_WALK,
                MoveSpeed::Run => CAI_SPEED_RUN,
                MoveSpeed::Sprint => CAI_SPEED_SPRINT,
            },
            stance: stance_to_c(c.stance),
            bark: bark_to_c(c.bark),
            action,
            custom_action,
            _reserved: 0,
        }
    }
}

/// Mirror of [`combat_ai::TickStats`].
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct CaiTickStats {
    pub agents: u32,
    pub thinks: u32,
    pub raycasts: u32,
    pub path_queries: u32,
    pub micros: f32,
}

impl From<TickStats> for CaiTickStats {
    fn from(s: TickStats) -> Self {
        Self {
            agents: s.agents,
            thinks: s.thinks,
            raycasts: s.raycasts,
            path_queries: s.path_queries,
            micros: s.micros,
        }
    }
}

/// Subset of [`combat_ai::AgentDebug`].
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct CaiAgentDebug {
    /// Primary target, or `CAI_INVALID_ACTOR`.
    pub target: u64,
    /// Valid when `has_destination`.
    pub destination: CaiVec3,
    pub think_interval: f32,
    /// Claimed cover point index, or `CAI_INVALID_INDEX`.
    pub cover: u32,
    pub known_targets: u32,
    pub custom_action: u16,
    /// `CAI_ACTION_*`.
    pub action: u8,
    /// `CAI_ROLE_*`.
    pub role: u8,
    pub target_visible: u8,
    pub has_token: u8,
    pub has_destination: u8,
    pub _reserved: u8,
}

impl From<&AgentDebug> for CaiAgentDebug {
    fn from(d: &AgentDebug) -> Self {
        let (action, custom_action) = action_to_c(d.action);
        Self {
            target: d.target.map_or(CAI_INVALID_ACTOR, ActorId::to_bits),
            destination: d.destination.unwrap_or(Vec3::ZERO).into(),
            think_interval: d.think_interval,
            cover: d.cover.map_or(CAI_INVALID_INDEX, |c| c.index()),
            known_targets: d.known_targets.min(u32::MAX as usize) as u32,
            custom_action,
            action,
            role: role_to_c(d.role),
            target_visible: d.target_visible as u8,
            has_token: d.has_token as u8,
            has_destination: d.destination.is_some() as u8,
            _reserved: 0,
        }
    }
}

/// Line-of-sight callback: return `true` if nothing blocks `from -> to`.
pub type CaiLineOfSightFn =
    extern "C" fn(user_data: *mut c_void, from: CaiVec3, to: CaiVec3) -> bool;
/// Path-distance callback: write the navigation distance to `*out_distance`
/// and return `true`, or return `false` if `to` is unreachable.
pub type CaiPathDistanceFn = extern "C" fn(
    user_data: *mut c_void,
    from: CaiVec3,
    to: CaiVec3,
    out_distance: *mut f32,
) -> bool;

/// World queries supplied by the host for one tick.
#[repr(C)]
#[derive(Copy, Clone)]
pub struct CaiWorld {
    /// Passed back to every callback.
    pub user_data: *mut c_void,
    /// Required.
    pub line_of_sight: Option<CaiLineOfSightFn>,
    /// Optional (null = straight-line distance).
    pub path_distance: Option<CaiPathDistanceFn>,
}

struct FfiWorld {
    user_data: *mut c_void,
    los: CaiLineOfSightFn,
    path: Option<CaiPathDistanceFn>,
}

// SAFETY: the host promises (see the header) that its callbacks are safe to
// call from several threads at once when parallel ticking is enabled, or keeps
// ticking on one thread via `parallel_threshold = UINT32_MAX`.
unsafe impl Sync for FfiWorld {}

impl WorldQuery for FfiWorld {
    fn line_of_sight(&self, from: Vec3, to: Vec3) -> bool {
        (self.los)(self.user_data, from.into(), to.into())
    }

    fn path_distance(&self, from: Vec3, to: Vec3) -> Option<f32> {
        match self.path {
            None => Some(from.distance(to)),
            Some(f) => {
                let mut d = f32::NAN;
                (f(self.user_data, from.into(), to.into(), &mut d) && d.is_finite()).then_some(d)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Errors

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
}

fn set_error(msg: impl Into<String>) {
    let msg = msg.into().replace('\0', " ");
    LAST_ERROR.with(|e| *e.borrow_mut() = CString::new(msg).unwrap_or_default());
}

/// Runs `f`, converting a panic into `on_panic` and an error message.
fn guard<T>(on_panic: T, f: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => v,
        Err(e) => {
            let msg = e
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| e.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            set_error(format!("internal panic: {msg}"));
            on_panic
        }
    }
}

/// Message describing the last error on the calling thread (empty if none).
/// The pointer stays valid until the next failing call on this thread.
#[no_mangle]
pub extern "C" fn cai_last_error() -> *const c_char {
    LAST_ERROR.with(|e| e.borrow().as_ptr())
}

// ---------------------------------------------------------------------------
// Version / ABI

/// ABI version the library was built with (`CAI_ABI_VERSION`).
#[no_mangle]
pub extern "C" fn cai_abi_version() -> u32 {
    CAI_ABI_VERSION
}

/// Library version string, e.g. `"0.1.0"` (static storage).
#[no_mangle]
pub extern "C" fn cai_version_string() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

/// Checks that the header the caller compiled against matches the library:
/// pass `CAI_ABI_VERSION` and `sizeof` of each struct (the `CAI_ABI_CHECK()`
/// macro does this). Returns `true` if everything matches.
#[no_mangle]
pub extern "C" fn cai_abi_check(
    abi_version: u32,
    system_config: usize,
    agent_config: usize,
    actor_state: usize,
    agent_command: usize,
    sound_event: usize,
    damage_event: usize,
    cover_point: usize,
    agent_debug: usize,
    world: usize,
) -> bool {
    use std::mem::size_of;
    let ok = abi_version == CAI_ABI_VERSION
        && system_config == size_of::<CaiSystemConfig>()
        && agent_config == size_of::<CaiAgentConfig>()
        && actor_state == size_of::<CaiActorState>()
        && agent_command == size_of::<CaiAgentCommand>()
        && sound_event == size_of::<CaiSoundEvent>()
        && damage_event == size_of::<CaiDamageEvent>()
        && cover_point == size_of::<CaiCoverPoint>()
        && agent_debug == size_of::<CaiAgentDebug>()
        && world == size_of::<CaiWorld>();
    if !ok {
        set_error("ABI mismatch: combat_ai.h does not match the linked library");
    }
    ok
}

// ---------------------------------------------------------------------------
// Configs and archetypes

/// Archetype presets.
pub const CAI_PRESET_RIFLEMAN: u32 = 0;
pub const CAI_PRESET_ASSAULT: u32 = 1;
pub const CAI_PRESET_SNIPER: u32 = 2;

fn preset(p: u32) -> Option<Arc<Archetype>> {
    match p {
        CAI_PRESET_RIFLEMAN => Some(Archetype::rifleman()),
        CAI_PRESET_ASSAULT => Some(Archetype::assault()),
        CAI_PRESET_SNIPER => Some(Archetype::sniper()),
        _ => None,
    }
}

/// Fills `*out` with the default system configuration.
#[no_mangle]
pub unsafe extern "C" fn cai_system_config_default(out: *mut CaiSystemConfig) -> i32 {
    guard(CAI_ERR_PANIC, || {
        let Some(out) = (unsafe { out.as_mut() }) else {
            set_error("cai_system_config_default: out is null");
            return CAI_ERR_NULL;
        };
        *out = (&SystemConfig::default()).into();
        CAI_OK
    })
}

/// Fills `*out` with the default agent configuration (the rifleman tuning).
#[no_mangle]
pub unsafe extern "C" fn cai_agent_config_default(out: *mut CaiAgentConfig) -> i32 {
    guard(CAI_ERR_PANIC, || {
        let Some(out) = (unsafe { out.as_mut() }) else {
            set_error("cai_agent_config_default: out is null");
            return CAI_ERR_NULL;
        };
        *out = (&AgentConfig::default()).into();
        CAI_OK
    })
}

/// Fills `*out` with a preset's tuning (`CAI_PRESET_*`), e.g. to tweak it and
/// pass it to `cai_archetype_new`.
#[no_mangle]
pub unsafe extern "C" fn cai_archetype_preset_config(p: u32, out: *mut CaiAgentConfig) -> i32 {
    guard(CAI_ERR_PANIC, || {
        let Some(out) = (unsafe { out.as_mut() }) else {
            set_error("cai_archetype_preset_config: out is null");
            return CAI_ERR_NULL;
        };
        let Some(a) = preset(p) else {
            set_error(format!("unknown preset {p}"));
            return CAI_ERR_INVALID;
        };
        *out = (&a.config).into();
        CAI_OK
    })
}

/// Opaque archetype handle.
pub struct CaiArchetype(Arc<Archetype>);

/// Creates a preset archetype (`CAI_PRESET_*`). Null on error.
#[no_mangle]
pub extern "C" fn cai_archetype_preset(p: u32) -> *mut CaiArchetype {
    guard(std::ptr::null_mut(), || match preset(p) {
        Some(a) => Box::into_raw(Box::new(CaiArchetype(a))),
        None => {
            set_error(format!("unknown preset {p}"));
            std::ptr::null_mut()
        }
    })
}

/// Creates an archetype with the standard action set and the given tuning.
/// `name` may be null. Null on error.
#[no_mangle]
pub unsafe extern "C" fn cai_archetype_new(
    name: *const c_char,
    config: *const CaiAgentConfig,
) -> *mut CaiArchetype {
    guard(std::ptr::null_mut(), || {
        let Some(cfg) = (unsafe { config.as_ref() }) else {
            set_error("cai_archetype_new: config is null");
            return std::ptr::null_mut();
        };
        let name = if name.is_null() {
            "custom".to_string()
        } else {
            unsafe { CStr::from_ptr(name) }
                .to_string_lossy()
                .into_owned()
        };
        let a = Archetype::new(name, cfg.into(), Archetype::standard_actions());
        Box::into_raw(Box::new(CaiArchetype(a.shared())))
    })
}

/// Releases an archetype handle. Agents already spawned from it keep working.
/// Null is ignored.
#[no_mangle]
pub unsafe extern "C" fn cai_archetype_free(a: *mut CaiArchetype) {
    if !a.is_null() {
        guard((), || drop(unsafe { Box::from_raw(a) }));
    }
}

// ---------------------------------------------------------------------------
// System

/// Opaque AI system handle.
pub struct CaiSystem {
    ai: AiSystem,
    commands: Vec<CaiAgentCommand>,
}

macro_rules! sys_mut {
    ($p:expr, $err:expr, $fn:literal) => {
        match unsafe { $p.as_mut() } {
            Some(s) => s,
            None => {
                set_error(concat!($fn, ": system is null"));
                return $err;
            }
        }
    };
}

macro_rules! sys_ref {
    ($p:expr, $err:expr, $fn:literal) => {
        match unsafe { $p.as_ref() } {
            Some(s) => s,
            None => {
                set_error(concat!($fn, ": system is null"));
                return $err;
            }
        }
    };
}

/// Creates a system. `config` may be null for defaults. Null on error.
#[no_mangle]
pub unsafe extern "C" fn cai_system_new(config: *const CaiSystemConfig) -> *mut CaiSystem {
    guard(std::ptr::null_mut(), || {
        let cfg = match unsafe { config.as_ref() } {
            Some(c) => c.into(),
            None => SystemConfig::default(),
        };
        Box::into_raw(Box::new(CaiSystem {
            ai: AiSystem::new(cfg),
            commands: Vec::new(),
        }))
    })
}

/// Destroys a system. Null is ignored.
#[no_mangle]
pub unsafe extern "C" fn cai_system_free(sys: *mut CaiSystem) {
    if !sys.is_null() {
        guard((), || drop(unsafe { Box::from_raw(sys) }));
    }
}

/// Fills `*out` with a healthy, fully loaded actor at `position` on `team`.
#[no_mangle]
pub unsafe extern "C" fn cai_actor_state_default(
    position: CaiVec3,
    team: u16,
    out: *mut CaiActorState,
) -> i32 {
    guard(CAI_ERR_PANIC, || {
        let Some(out) = (unsafe { out.as_mut() }) else {
            set_error("cai_actor_state_default: out is null");
            return CAI_ERR_NULL;
        };
        *out = (&ActorState::new(position.into(), TeamId(team))).into();
        CAI_OK
    })
}

/// Registers an externally controlled actor (e.g. a player).
/// Returns `CAI_INVALID_ACTOR` on error.
#[no_mangle]
pub unsafe extern "C" fn cai_add_actor(sys: *mut CaiSystem, state: *const CaiActorState) -> u64 {
    guard(CAI_INVALID_ACTOR, || {
        let s = sys_mut!(sys, CAI_INVALID_ACTOR, "cai_add_actor");
        let Some(st) = (unsafe { state.as_ref() }) else {
            set_error("cai_add_actor: state is null");
            return CAI_INVALID_ACTOR;
        };
        s.ai.add_actor(state_from_c(st)).to_bits()
    })
}

/// Spawns an AI agent. The system keeps its own reference to the archetype.
/// Returns `CAI_INVALID_ACTOR` on error.
#[no_mangle]
pub unsafe extern "C" fn cai_spawn_agent(
    sys: *mut CaiSystem,
    state: *const CaiActorState,
    archetype: *const CaiArchetype,
) -> u64 {
    guard(CAI_INVALID_ACTOR, || {
        let s = sys_mut!(sys, CAI_INVALID_ACTOR, "cai_spawn_agent");
        let (Some(st), Some(a)) = (unsafe { state.as_ref() }, unsafe { archetype.as_ref() }) else {
            set_error("cai_spawn_agent: state or archetype is null");
            return CAI_INVALID_ACTOR;
        };
        s.ai.spawn_agent(state_from_c(st), Arc::clone(&a.0))
            .to_bits()
    })
}

/// Removes an actor. Returns `false` if the handle was not live.
#[no_mangle]
pub unsafe extern "C" fn cai_remove_actor(sys: *mut CaiSystem, actor: u64) -> bool {
    guard(false, || {
        let s = sys_mut!(sys, false, "cai_remove_actor");
        s.ai.remove_actor(ActorId::from_bits(actor))
    })
}

/// Whether `actor` is a live AI agent (as opposed to an external actor).
#[no_mangle]
pub unsafe extern "C" fn cai_is_agent(sys: *const CaiSystem, actor: u64) -> bool {
    guard(false, || {
        let s = sys_ref!(sys, false, "cai_is_agent");
        s.ai.is_agent(ActorId::from_bits(actor))
    })
}

/// Reads an actor's state. Returns `false` if the handle is not live.
#[no_mangle]
pub unsafe extern "C" fn cai_get_actor_state(
    sys: *const CaiSystem,
    actor: u64,
    out: *mut CaiActorState,
) -> bool {
    guard(false, || {
        let s = sys_ref!(sys, false, "cai_get_actor_state");
        let Some(out) = (unsafe { out.as_mut() }) else {
            set_error("cai_get_actor_state: out is null");
            return false;
        };
        match s.ai.actor_state(ActorId::from_bits(actor)) {
            Some(st) => {
                *out = st.into();
                true
            }
            None => false,
        }
    })
}

/// Writes an actor's state (do this every frame for every actor the host
/// moves). The team cannot be changed. Returns `false` if the handle is not live.
#[no_mangle]
pub unsafe extern "C" fn cai_set_actor_state(
    sys: *mut CaiSystem,
    actor: u64,
    state: *const CaiActorState,
) -> bool {
    guard(false, || {
        let s = sys_mut!(sys, false, "cai_set_actor_state");
        let Some(st) = (unsafe { state.as_ref() }) else {
            set_error("cai_set_actor_state: state is null");
            return false;
        };
        match s.ai.actor_state_mut(ActorId::from_bits(actor)) {
            Some(dst) => {
                let team = dst.team;
                *dst = state_from_c(st);
                dst.team = team;
                true
            }
            None => false,
        }
    })
}

/// Creates an empty squad and returns its index.
#[no_mangle]
pub unsafe extern "C" fn cai_create_squad(sys: *mut CaiSystem) -> u32 {
    guard(CAI_INVALID_INDEX, || {
        let s = sys_mut!(sys, CAI_INVALID_INDEX, "cai_create_squad");
        s.ai.create_squad().index()
    })
}

/// Moves an agent into a squad. Returns `false` for unknown agents or squads.
#[no_mangle]
pub unsafe extern "C" fn cai_assign_squad(sys: *mut CaiSystem, actor: u64, squad: u32) -> bool {
    guard(false, || {
        let s = sys_mut!(sys, false, "cai_assign_squad");
        s.ai.assign_squad(
            ActorId::from_bits(actor),
            combat_ai::SquadId::from_index(squad),
        )
    })
}

/// Registers a cover point and returns its index (`CAI_INVALID_INDEX` on error).
#[no_mangle]
pub unsafe extern "C" fn cai_add_cover_point(
    sys: *mut CaiSystem,
    cover: *const CaiCoverPoint,
) -> u32 {
    guard(CAI_INVALID_INDEX, || {
        let s = sys_mut!(sys, CAI_INVALID_INDEX, "cai_add_cover_point");
        let Some(c) = (unsafe { cover.as_ref() }) else {
            set_error("cai_add_cover_point: cover is null");
            return CAI_INVALID_INDEX;
        };
        let kind = if c.kind == CAI_COVER_HIGH {
            CoverKind::High
        } else {
            CoverKind::Low
        };
        s.ai.add_cover_point(CoverPoint::new(c.position.into(), c.facing.into(), kind))
            .index()
    })
}

/// Agent currently holding a cover point, or `CAI_INVALID_ACTOR`.
#[no_mangle]
pub unsafe extern "C" fn cai_cover_owner(sys: *const CaiSystem, cover: u32) -> u64 {
    guard(CAI_INVALID_ACTOR, || {
        let s = sys_ref!(sys, CAI_INVALID_ACTOR, "cai_cover_owner");
        s.ai.cover_owner(combat_ai::CoverId::from_index(cover))
            .map_or(CAI_INVALID_ACTOR, ActorId::to_bits)
    })
}

/// Sets the points (player cameras) level of detail is measured from.
/// `points` may be null when `count` is 0.
#[no_mangle]
pub unsafe extern "C" fn cai_set_lod_focus(
    sys: *mut CaiSystem,
    points: *const CaiVec3,
    count: usize,
) -> i32 {
    guard(CAI_ERR_PANIC, || {
        let s = sys_mut!(sys, CAI_ERR_NULL, "cai_set_lod_focus");
        if count == 0 {
            s.ai.set_lod_focus(&[]);
            return CAI_OK;
        }
        if points.is_null() {
            set_error("cai_set_lod_focus: points is null");
            return CAI_ERR_NULL;
        }
        let pts = unsafe { std::slice::from_raw_parts(points, count) };
        let v: Vec<Vec3> = pts.iter().map(|p| (*p).into()).collect();
        s.ai.set_lod_focus(&v);
        CAI_OK
    })
}

fn actor_opt(bits: u64) -> Option<ActorId> {
    (bits != CAI_INVALID_ACTOR).then(|| ActorId::from_bits(bits))
}

/// Reports a sound.
#[no_mangle]
pub unsafe extern "C" fn cai_report_sound(sys: *mut CaiSystem, ev: *const CaiSoundEvent) -> i32 {
    guard(CAI_ERR_PANIC, || {
        let s = sys_mut!(sys, CAI_ERR_NULL, "cai_report_sound");
        let Some(e) = (unsafe { ev.as_ref() }) else {
            set_error("cai_report_sound: event is null");
            return CAI_ERR_NULL;
        };
        let kind = match e.kind {
            CAI_SOUND_GUNSHOT => SoundKind::Gunshot,
            CAI_SOUND_MOVEMENT => SoundKind::Movement,
            CAI_SOUND_EXPLOSION => SoundKind::Explosion,
            CAI_SOUND_BULLET_IMPACT => SoundKind::BulletImpact,
            k => {
                set_error(format!("cai_report_sound: unknown kind {k}"));
                return CAI_ERR_INVALID;
            }
        };
        s.ai.report_sound(SoundEvent {
            position: e.position.into(),
            radius: e.radius,
            source: actor_opt(e.source),
            kind,
        });
        CAI_OK
    })
}

/// Reports damage dealt to an actor (health itself is set via `cai_set_actor_state`).
#[no_mangle]
pub unsafe extern "C" fn cai_report_damage(sys: *mut CaiSystem, ev: *const CaiDamageEvent) -> i32 {
    guard(CAI_ERR_PANIC, || {
        let s = sys_mut!(sys, CAI_ERR_NULL, "cai_report_damage");
        let Some(e) = (unsafe { ev.as_ref() }) else {
            set_error("cai_report_damage: event is null");
            return CAI_ERR_NULL;
        };
        s.ai.report_damage(DamageEvent {
            victim: ActorId::from_bits(e.victim),
            attacker: actor_opt(e.attacker),
            amount: e.amount,
            origin: e.origin.into(),
        });
        CAI_OK
    })
}

/// Tells every agent on `team` where `target` is now. Returns how many agents were told.
#[no_mangle]
pub unsafe extern "C" fn cai_reveal(sys: *mut CaiSystem, target: u64, team: u16) -> u32 {
    guard(0, || {
        let s = sys_mut!(sys, 0, "cai_reveal");
        s.ai.reveal(ActorId::from_bits(target), TeamId(team)) as u32
    })
}

/// Advances the AI by `dt` seconds. On success `*out_commands` points to
/// `*out_count` commands, valid until the next call that takes `sys` mutably
/// (or `cai_system_free`). Either output pointer may be null.
#[no_mangle]
pub unsafe extern "C" fn cai_tick(
    sys: *mut CaiSystem,
    dt: f32,
    world: *const CaiWorld,
    out_commands: *mut *const CaiAgentCommand,
    out_count: *mut usize,
) -> i32 {
    // Clear outputs first so callers never read stale pointers after an error.
    unsafe {
        if let Some(p) = out_commands.as_mut() {
            *p = std::ptr::null();
        }
        if let Some(n) = out_count.as_mut() {
            *n = 0;
        }
    }
    guard(CAI_ERR_PANIC, || {
        let s = sys_mut!(sys, CAI_ERR_NULL, "cai_tick");
        let Some(w) = (unsafe { world.as_ref() }) else {
            set_error("cai_tick: world is null");
            return CAI_ERR_NULL;
        };
        let Some(los) = w.line_of_sight else {
            set_error("cai_tick: world.line_of_sight is null");
            return CAI_ERR_NULL;
        };
        let fw = FfiWorld {
            user_data: w.user_data,
            los,
            path: w.path_distance,
        };
        let cmds = s.ai.tick(dt, &fw);
        s.commands.clear();
        s.commands.extend(cmds.iter().map(CaiAgentCommand::from));
        unsafe {
            if let Some(p) = out_commands.as_mut() {
                *p = s.commands.as_ptr();
            }
            if let Some(n) = out_count.as_mut() {
                *n = s.commands.len();
            }
        }
        CAI_OK
    })
}

/// Counters from the last tick.
#[no_mangle]
pub unsafe extern "C" fn cai_get_stats(sys: *const CaiSystem, out: *mut CaiTickStats) -> i32 {
    guard(CAI_ERR_PANIC, || {
        let s = sys_ref!(sys, CAI_ERR_NULL, "cai_get_stats");
        let Some(out) = (unsafe { out.as_mut() }) else {
            set_error("cai_get_stats: out is null");
            return CAI_ERR_NULL;
        };
        *out = s.ai.stats().into();
        CAI_OK
    })
}

/// Debug snapshot of an agent. Returns `false` if `actor` is not a live agent.
#[no_mangle]
pub unsafe extern "C" fn cai_agent_debug(
    sys: *const CaiSystem,
    actor: u64,
    out: *mut CaiAgentDebug,
) -> bool {
    guard(false, || {
        let s = sys_ref!(sys, false, "cai_agent_debug");
        let Some(out) = (unsafe { out.as_mut() }) else {
            set_error("cai_agent_debug: out is null");
            return false;
        };
        match s.ai.debug(ActorId::from_bits(actor)) {
            Some(d) => {
                *out = (&d).into();
                true
            }
            None => false,
        }
    })
}

/// Number of AI agents (alive or dead, until removed).
#[no_mangle]
pub unsafe extern "C" fn cai_agent_count(sys: *const CaiSystem) -> usize {
    guard(0, || sys_ref!(sys, 0, "cai_agent_count").ai.agent_count())
}

/// Simulation time in seconds (sum of all `dt`).
#[no_mangle]
pub unsafe extern "C" fn cai_time(sys: *const CaiSystem) -> f32 {
    guard(0.0, || sys_ref!(sys, 0.0, "cai_time").ai.time())
}
