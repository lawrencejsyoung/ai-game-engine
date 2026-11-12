//! Stimuli the host reports to the AI.

use crate::ids::ActorId;
use glam::Vec3;

/// Category of a sound, which affects how agents react.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SoundKind {
    /// Weapon fire. Reveals the shooter's approximate position.
    Gunshot,
    /// Footsteps, doors, etc.
    Movement,
    /// Explosions. Loud, but rarely attributable.
    Explosion,
    /// A bullet impacting near the listener; makes agents feel under fire.
    BulletImpact,
}

/// A sound heard in the world.
#[derive(Copy, Clone, Debug)]
pub struct SoundEvent {
    /// Where the sound originated.
    pub position: Vec3,
    /// Audible radius in meters (scaled by each agent's hearing multiplier).
    pub radius: f32,
    /// Actor that caused the sound, if any.
    pub source: Option<ActorId>,
    /// Kind of sound.
    pub kind: SoundKind,
}

/// Damage dealt to an actor.
#[derive(Copy, Clone, Debug)]
pub struct DamageEvent {
    /// Actor that took damage.
    pub victim: ActorId,
    /// Who caused it, if known.
    pub attacker: Option<ActorId>,
    /// Damage amount (informational; health itself is owned by the host).
    pub amount: f32,
    /// Position the damage came from (e.g. the shooter's muzzle).
    pub origin: Vec3,
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct TimedSound {
    pub event: SoundEvent,
    pub time: f32,
    /// Monotonic sequence number: agents hear each sound exactly once, even
    /// when it is reported between ticks with the previous tick's timestamp.
    pub seq: u64,
}
