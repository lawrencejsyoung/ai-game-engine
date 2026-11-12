//! Archetypes bundle tuning parameters with a set of utility actions and their trees.
//!
//! Presets ([`Archetype::rifleman`], [`Archetype::assault`], [`Archetype::sniper`])
//! share the same standard action set ([`Archetype::standard_actions`]) and differ
//! in tuning; build your own by editing either.

use crate::agent::{AgentConfig, Stance};
use crate::bt::{Bt, Condition as C, Task as T};
use crate::command::{Bark, MoveSpeed};
use crate::utility::{ActionDef, ActionKind, Consideration, Curve, Input};
use std::sync::Arc;

/// A reusable NPC definition, shared (via `Arc`) by every agent spawned from it.
#[derive(Clone, Debug)]
pub struct Archetype {
    /// Display name.
    pub name: String,
    /// Tuning.
    pub config: AgentConfig,
    /// Candidate actions.
    pub actions: Vec<ActionDef>,
}

fn lin(input: Input, m: f32, b: f32) -> Consideration {
    Consideration::new(input, Curve::Linear { m, b })
}

fn step(input: Input, lo: f32, hi: f32) -> Consideration {
    Consideration::new(
        input,
        Curve::Step {
            threshold: 0.5,
            lo,
            hi,
        },
    )
}

impl Archetype {
    /// Creates an archetype from parts.
    pub fn new(name: impl Into<String>, config: AgentConfig, actions: Vec<ActionDef>) -> Self {
        Self {
            name: name.into(),
            config,
            actions,
        }
    }

    /// Wraps in an `Arc` for spawning.
    pub fn shared(self) -> Arc<Self> {
        Arc::new(self)
    }

    /// Looks up an action by kind for tweaking.
    pub fn action_mut(&mut self, kind: ActionKind) -> Option<&mut ActionDef> {
        self.actions.iter_mut().find(|a| a.kind == kind)
    }

    /// The standard FPS combat action set.
    pub fn standard_actions() -> Vec<ActionDef> {
        use Input::*;
        let reload_if_empty = Bt::sequence([Bt::cond(C::AmmoEmpty), Bt::task(T::Reload)]);
        vec![
            // Shoot the target; if it isn't visible, reposition for a shot or push its last known spot.
            ActionDef::new(
                ActionKind::Engage,
                1.0,
                vec![
                    Consideration::require(HasTarget),
                    step(TargetVisible, 0.4, 1.0),
                    step(InCover, 0.45, 1.0),
                    lin(TargetInRange, 0.4, 0.6),
                    step(HasAttackToken, 0.75, 1.0),
                    Consideration::new(
                        Ammo,
                        Curve::Step {
                            threshold: 0.01,
                            lo: 0.3,
                            hi: 1.0,
                        },
                    ),
                ],
                Bt::selector([
                    reload_if_empty.clone(),
                    Bt::sequence([Bt::cond(C::TargetVisible), Bt::task(T::ShootTarget)]),
                    Bt::sequence([
                        Bt::task(T::FindAttackPosition),
                        Bt::task(T::MoveToDestination(MoveSpeed::Run)),
                    ]),
                    Bt::sequence([
                        Bt::task(T::SetDestinationToLastKnown),
                        Bt::task(T::MoveToDestination(MoveSpeed::Run)),
                    ]),
                ])
                .build(),
            ),
            // Get into protective cover; when already there, duck and top up.
            ActionDef::new(
                ActionKind::TakeCover,
                1.5,
                vec![
                    Consideration::require(HasTarget),
                    step(InCover, 1.0, 0.35),
                    lin(UnderFire, 0.5, 0.5),
                    lin(Aggression, -0.7, 1.0),
                    lin(Health, -0.4, 1.0),
                    // Hiding from someone we haven't seen in a while is pointless; go hunt instead.
                    Consideration::new(TimeSinceTargetSeen, Curve::Logistic { k: -12.0, mid: 0.6 }),
                ],
                Bt::selector([
                    Bt::sequence([
                        Bt::cond(C::InCover),
                        Bt::task(T::SetStance(Stance::Crouch)),
                        Bt::selector([
                            Bt::sequence([
                                Bt::cond(C::AmmoBelow(0.6)),
                                Bt::task(T::Bark(Bark::Reloading)),
                                Bt::task(T::Reload),
                            ]),
                            Bt::wait(1.0),
                        ]),
                    ]),
                    // Relocating has a cooldown so an agent whose cover gets
                    // outflanked fights for a while instead of cover-hopping forever.
                    Bt::sequence([
                        Bt::task(T::FindCover),
                        Bt::task(T::Bark(Bark::MovingToCover)),
                        Bt::task(T::MoveToDestination(MoveSpeed::Sprint)),
                        Bt::task(T::SetStance(Stance::Crouch)),
                        Bt::wait(0.4),
                    ])
                    .cooldown(6.0),
                ])
                .build(),
            ),
            // Squad flanker: circle to the target's side.
            ActionDef::new(
                ActionKind::Flank,
                1.4,
                vec![
                    Consideration::require(HasTarget),
                    Consideration::require(RoleFlanker),
                    lin(Aggression, 0.5, 0.5),
                    lin(Health, 0.6, 0.4),
                    step(TargetVisible, 1.0, 0.7),
                ],
                Bt::sequence([
                    Bt::task(T::FindFlankPosition),
                    Bt::task(T::Bark(Bark::Flanking)),
                    Bt::task(T::MoveToDestination(MoveSpeed::Run)),
                ])
                .cooldown(10.0)
                .build(),
            ),
            // Squad suppressor: keep a recently seen target's head down.
            ActionDef::new(
                ActionKind::Suppress,
                1.2,
                vec![
                    Consideration::require(HasTarget),
                    Consideration::require(RoleSuppressor),
                    step(TargetVisible, 1.0, 0.5),
                    Consideration::new(TimeSinceTargetSeen, Curve::Logistic { k: -15.0, mid: 0.4 }),
                    lin(Ammo, 0.5, 0.5),
                ],
                Bt::selector([
                    reload_if_empty,
                    Bt::sequence([
                        Bt::task(T::Bark(Bark::Suppressing)),
                        Bt::task(T::SuppressTarget(2.5)),
                    ]),
                ])
                .build(),
            ),
            // Tactical reload when the magazine runs low and nobody is in sight.
            ActionDef::new(
                ActionKind::Reload,
                1.0,
                vec![
                    Consideration::new(Ammo, Curve::Logistic { k: -14.0, mid: 0.3 }),
                    step(TargetVisible, 1.0, 0.6),
                    step(InCover, 0.7, 1.0),
                ],
                Bt::sequence([
                    Bt::cond(C::AmmoBelow(1.0)),
                    Bt::task(T::SetStance(Stance::Crouch)),
                    Bt::task(T::Bark(Bark::Reloading)),
                    Bt::task(T::Reload),
                ])
                .build(),
            ),
            // Fall back when badly hurt, scaled by (lack of) courage.
            ActionDef::new(
                ActionKind::Retreat,
                1.3,
                vec![
                    Consideration::require(HasTarget),
                    Consideration::new(
                        Health,
                        Curve::Logistic {
                            k: -12.0,
                            mid: 0.35,
                        },
                    ),
                    lin(Courage, -0.8, 1.0),
                    lin(ThreatCount, 0.5, 0.5),
                    step(InCover, 1.0, 0.5),
                ],
                Bt::sequence([
                    Bt::task(T::FindRetreatPosition),
                    Bt::task(T::Bark(Bark::Retreating)),
                    Bt::task(T::MoveToDestination(MoveSpeed::Sprint)),
                    Bt::task(T::SetStance(Stance::Crouch)),
                    Bt::selector([
                        Bt::sequence([Bt::cond(C::AmmoBelow(0.99)), Bt::task(T::Reload)]),
                        Bt::wait(2.0),
                    ]),
                ])
                .build(),
            ),
            // Go look where the enemy was last known / something suspicious happened.
            ActionDef::new(
                ActionKind::Investigate,
                0.8,
                vec![
                    Consideration::forbid(TargetVisible),
                    Consideration::require(HasInvestigatePoint),
                    lin(Aggression, 0.4, 0.6),
                    lin(TimeSinceTargetSeen, 4.0, 0.2),
                ],
                Bt::sequence([
                    Bt::task(T::SetDestinationToLastKnown),
                    Bt::task(T::Bark(Bark::Searching)).cooldown(8.0).optional(),
                    Bt::task(T::MoveToDestination(MoveSpeed::Run)),
                    Bt::task(T::Search(3.0)),
                ])
                .build(),
            ),
            // Nothing going on: wander around home.
            ActionDef::new(
                ActionKind::Patrol,
                0.08,
                vec![
                    Consideration::forbid(HasTarget),
                    Consideration::forbid(HasInvestigatePoint),
                ],
                Bt::sequence([
                    Bt::task(T::FindPatrolPoint),
                    Bt::task(T::MoveToDestination(MoveSpeed::Walk)),
                    Bt::wait(2.5),
                ])
                .build(),
            ),
        ]
    }

    /// Balanced mid-range infantry.
    pub fn rifleman() -> Arc<Self> {
        Self::new("rifleman", AgentConfig::default(), Self::standard_actions()).shared()
    }

    /// Aggressive close-quarters fighter (shotgun / SMG): pushes and flanks.
    pub fn assault() -> Arc<Self> {
        let config = AgentConfig {
            view_range: 45.0,
            preferred_range: (3.0, 12.0),
            accuracy_deg: 3.0,
            fire_interval: 0.08,
            burst_length: 6,
            burst_pause: (0.25, 0.5),
            aggression: 0.85,
            courage: 0.75,
            reaction_time: 0.3,
            tactical_search_radius: 20.0,
            ..AgentConfig::default()
        };
        Self::new("assault", config, Self::standard_actions()).shared()
    }

    /// Long-range marksman: slow, accurate, cautious.
    pub fn sniper() -> Arc<Self> {
        let config = AgentConfig {
            fov_deg: 70.0,
            peripheral_fov_deg: 150.0,
            view_range: 110.0,
            detection_rate: 2.5,
            preferred_range: (30.0, 90.0),
            reaction_time: 0.6,
            accuracy_deg: 0.35,
            aim_settle_time: 2.0,
            fire_interval: 1.4,
            burst_length: 1,
            burst_pause: (1.2, 2.2),
            aggression: 0.15,
            courage: 0.4,
            tactical_search_radius: 35.0,
            ..AgentConfig::default()
        };
        Self::new("sniper", config, Self::standard_actions()).shared()
    }
}
