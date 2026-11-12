//! # combat-ai
//!
//! An engine-agnostic, data-oriented AI engine for combative NPCs in
//! first-person shooters.
//!
//! The host game owns all physical state (transforms, health, ammo, pathing,
//! animation). Every frame it:
//!
//! 1. updates actor state ([`AiSystem::actor_state_mut`]),
//! 2. reports stimuli ([`AiSystem::report_sound`], [`AiSystem::report_damage`]),
//! 3. calls [`AiSystem::tick`] with a [`WorldQuery`] implementation, and
//! 4. applies the returned [`AgentCommand`]s to its own characters.
//!
//! Internally each agent runs a *perceive → remember → decide → act* loop:
//! vision cones with line-of-sight and a detection meter, hearing, a decaying
//! target memory, utility-AI action selection with hysteresis, and compact
//! arena-allocated behavior trees that execute the chosen action. Squads share
//! knowledge, assign roles (assault / suppress / flank), reserve cover so
//! agents don't stack up, and limit how many agents may shoot at one target
//! at a time (attack tokens).
//!
//! ```no_run
//! use combat_ai::prelude::*;
//!
//! struct OpenField;
//! impl WorldQuery for OpenField {
//!     fn line_of_sight(&self, _from: Vec3, _to: Vec3) -> bool { true }
//! }
//!
//! let mut ai = AiSystem::new(SystemConfig::default());
//! let player = ai.add_actor(ActorState::new(Vec3::ZERO, TeamId(0)));
//! let grunt = ai.spawn_agent(
//!     ActorState::new(Vec3::new(20.0, 0.0, 0.0), TeamId(1)),
//!     Archetype::rifleman(),
//! );
//! let squad = ai.create_squad();
//! ai.assign_squad(grunt, squad);
//!
//! for cmd in ai.tick(1.0 / 60.0, &OpenField) {
//!     // move / aim / fire your character according to `cmd`
//!     let _ = (cmd.actor, cmd.move_to, cmd.aim_dir, cmd.fire);
//! }
//! # let _ = player;
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod agent;
pub mod archetype;
pub mod bt;
pub mod command;
pub mod ids;
pub mod math;
pub mod memory;
pub mod spatial;
pub mod stimuli;
pub mod system;
pub mod tactics;
pub mod utility;
pub mod world;

mod aim;
mod brain;
mod perception;
mod squad;
mod tasks;

pub use agent::{ActorState, AgentConfig, Stance};
pub use archetype::Archetype;
pub use bt::{BehaviorTree, Bt, Condition, Task};
pub use command::{AgentCommand, Bark, MoveSpeed};
pub use glam::{Vec2, Vec3};
pub use ids::{ActorId, CoverId, SquadId, TeamId};
pub use squad::SquadRole;
pub use stimuli::{DamageEvent, SoundEvent, SoundKind};
pub use system::{AgentDebug, AiSystem, SystemConfig, TickStats};
pub use tactics::{CoverKind, CoverPoint};
pub use utility::{ActionDef, ActionKind, Consideration, Curve, Input};
pub use world::WorldQuery;

/// The README's code examples, compiled as doctests.
#[doc = include_str!("../../../README.md")]
#[cfg(doctest)]
pub struct ReadmeDoctests;

/// Convenient glob import of the most commonly used types.
pub mod prelude {
    pub use crate::{
        ActionDef, ActionKind, ActorId, ActorState, AgentCommand, AgentConfig, AiSystem, Archetype,
        Bark, Bt, Condition, Consideration, CoverId, CoverKind, CoverPoint, Curve, DamageEvent,
        Input, MoveSpeed, SoundEvent, SoundKind, SquadId, SquadRole, Stance, SystemConfig, Task,
        TeamId, Vec3, WorldQuery,
    };
}
