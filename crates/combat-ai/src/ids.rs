//! Strongly typed handles.

/// Handle to an actor (AI agent or externally controlled combatant such as a player).
///
/// Generational: a stale handle to a removed actor never aliases a new one.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ActorId {
    pub(crate) index: u32,
    pub(crate) generation: u32,
}

impl ActorId {
    /// Slot index (stable for the actor's lifetime). Useful as a key in host-side arrays.
    pub fn index(self) -> u32 {
        self.index
    }
}

/// Team membership. Actors on different teams are hostile to each other.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct TeamId(pub u16);

/// Handle to a squad created with [`crate::AiSystem::create_squad`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SquadId(pub(crate) u32);

/// Handle to a registered cover point.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CoverId(pub(crate) u32);

impl CoverId {
    /// Index into the order in which cover points were added.
    pub fn index(self) -> u32 {
        self.0
    }
}
