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
    /// A handle that never refers to a live actor.
    pub const INVALID: ActorId = ActorId {
        index: u32::MAX,
        generation: u32::MAX,
    };

    /// Slot index (stable for the actor's lifetime). Useful as a key in host-side arrays.
    pub fn index(self) -> u32 {
        self.index
    }

    /// Generation of the slot (incremented each time the slot is reused).
    pub fn generation(self) -> u32 {
        self.generation
    }

    /// Packs the handle into 64 bits (generation in the high half), e.g. for
    /// FFI or serialization. Round-trips with [`ActorId::from_bits`].
    pub fn to_bits(self) -> u64 {
        (self.generation as u64) << 32 | self.index as u64
    }

    /// Rebuilds a handle from [`ActorId::to_bits`]. Any value is safe to pass:
    /// handles that don't match a live actor are simply inert.
    pub fn from_bits(bits: u64) -> Self {
        Self {
            index: bits as u32,
            generation: (bits >> 32) as u32,
        }
    }
}

/// Team membership. Actors on different teams are hostile to each other.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct TeamId(pub u16);

/// Handle to a squad created with [`crate::AiSystem::create_squad`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SquadId(pub(crate) u32);

impl SquadId {
    /// Index in creation order.
    pub fn index(self) -> u32 {
        self.0
    }

    /// Rebuilds a handle from [`SquadId::index`]. Out-of-range values are rejected
    /// by the APIs that take a squad.
    pub fn from_index(index: u32) -> Self {
        Self(index)
    }
}

/// Handle to a registered cover point.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CoverId(pub(crate) u32);

impl CoverId {
    /// Index into the order in which cover points were added.
    pub fn index(self) -> u32 {
        self.0
    }

    /// Rebuilds a handle from [`CoverId::index`].
    pub fn from_index(index: u32) -> Self {
        Self(index)
    }
}
