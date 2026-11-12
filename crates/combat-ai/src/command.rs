//! Output of the AI: per-agent commands the host applies.

use crate::agent::Stance;
use crate::ids::ActorId;
use crate::utility::ActionKind;
use glam::Vec3;

/// Locomotion speed requested for a move.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum MoveSpeed {
    /// Relaxed walk (patrol, investigate).
    #[default]
    Walk,
    /// Combat run.
    Run,
    /// Full sprint (usually can't fire).
    Sprint,
}

/// A voice line / gesture hint ("bark"). Purely presentational.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Bark {
    /// First sighting of an enemy.
    EnemySpotted,
    /// Heading for cover.
    MovingToCover,
    /// Reloading.
    Reloading,
    /// Starting a flank.
    Flanking,
    /// Laying down suppressive fire.
    Suppressing,
    /// Falling back.
    Retreating,
    /// Lost track of the enemy, searching.
    Searching,
}

/// What an agent wants its character to do this frame.
#[derive(Copy, Clone, Debug)]
pub struct AgentCommand {
    /// Agent this command is for.
    pub actor: ActorId,
    /// Navigation destination, or `None` to stand still.
    pub move_to: Option<Vec3>,
    /// Requested locomotion speed.
    pub move_speed: MoveSpeed,
    /// Normalized direction the agent wants to look / aim (includes aim error).
    pub aim_dir: Vec3,
    /// Point being aimed at, if aiming at something specific.
    pub aim_point: Option<Vec3>,
    /// Pull the trigger this frame (one shot). The host resolves hits along `aim_dir`.
    pub fire: bool,
    /// Begin (or continue) reloading.
    pub reload: bool,
    /// Desired stance.
    pub stance: Stance,
    /// Optional voice line to play.
    pub bark: Option<Bark>,
    /// The high-level action currently executing (for animation / debugging).
    pub action: ActionKind,
}

impl AgentCommand {
    pub(crate) fn idle(actor: ActorId, forward: Vec3) -> Self {
        Self {
            actor,
            move_to: None,
            move_speed: MoveSpeed::Walk,
            aim_dir: forward,
            aim_point: None,
            fire: false,
            reload: false,
            stance: Stance::Stand,
            bark: None,
            action: ActionKind::Idle,
        }
    }
}
