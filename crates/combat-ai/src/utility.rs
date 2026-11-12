//! Utility AI: scores candidate actions from normalized inputs through response curves.
//!
//! Every decision, the engine fills a flat [`Inputs`] array once per agent; each
//! [`Consideration`] is then just an array load plus a curve evaluation, so
//! scoring dozens of considerations costs nanoseconds.

use crate::bt::BehaviorTree;

/// Normalized decision inputs, each in `[0, 1]`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Input {
    /// Own health fraction.
    Health,
    /// Own magazine fraction.
    Ammo,
    /// 1 if a confirmed target exists.
    HasTarget,
    /// 1 if the primary target is currently visible.
    TargetVisible,
    /// How well the target distance fits the preferred range band (1 = ideal).
    TargetInRange,
    /// Target distance / view range.
    TargetDistance,
    /// Seconds since the target was last seen / 10, clamped.
    TimeSinceTargetSeen,
    /// Number of confirmed threats / 4, clamped.
    ThreatCount,
    /// Recent incoming damage / near misses, decaying.
    UnderFire,
    /// 1 if standing at a claimed cover point.
    InCover,
    /// 1 if there is something to investigate (unconfirmed suspicion or lost target).
    HasInvestigatePoint,
    /// 1 if squad role is Flanker.
    RoleFlanker,
    /// 1 if squad role is Suppressor.
    RoleSuppressor,
    /// 1 if squad role is Assault (or not in a squad).
    RoleAssault,
    /// Archetype aggression.
    Aggression,
    /// Archetype courage.
    Courage,
    /// 1 if holding an attack token on the primary target.
    HasAttackToken,
    /// Seconds since the current action started / 10, clamped.
    TimeInAction,
    /// Constant 1 (useful for baseline actions).
    One,
}

/// Number of [`Input`] variants.
pub const INPUT_COUNT: usize = Input::One as usize + 1;

/// Pre-computed inputs for one agent's decision.
#[derive(Copy, Clone, Debug)]
pub struct Inputs(pub [f32; INPUT_COUNT]);

impl Default for Inputs {
    fn default() -> Self {
        let mut v = [0.0; INPUT_COUNT];
        v[Input::One as usize] = 1.0;
        Inputs(v)
    }
}

impl Inputs {
    /// Reads an input.
    #[inline]
    pub fn get(&self, i: Input) -> f32 {
        self.0[i as usize]
    }
    /// Writes an input (clamped to `[0, 1]`).
    #[inline]
    pub fn set(&mut self, i: Input, v: f32) {
        self.0[i as usize] = v.clamp(0.0, 1.0);
    }
}

/// Response curve mapping an input in `[0, 1]` to a score in `[0, 1]`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Curve {
    /// `m * x + b`.
    Linear {
        /// Slope.
        m: f32,
        /// Intercept.
        b: f32,
    },
    /// `1 - x`.
    Inverse,
    /// `x ^ exp`.
    Power {
        /// Exponent.
        exp: f32,
    },
    /// `1 / (1 + e^(-k (x - mid)))`. Negative `k` inverts.
    Logistic {
        /// Steepness.
        k: f32,
        /// Midpoint.
        mid: f32,
    },
    /// `hi` if `x >= threshold`, else `lo`.
    Step {
        /// Threshold.
        threshold: f32,
        /// Value below the threshold.
        lo: f32,
        /// Value at or above the threshold.
        hi: f32,
    },
    /// Identity.
    Identity,
}

impl Curve {
    /// Evaluates the curve, clamping the result to `[0, 1]`.
    #[inline]
    pub fn eval(&self, x: f32) -> f32 {
        let y = match *self {
            Curve::Linear { m, b } => m * x + b,
            Curve::Inverse => 1.0 - x,
            Curve::Power { exp } => x.max(0.0).powf(exp),
            Curve::Logistic { k, mid } => 1.0 / (1.0 + (-k * (x - mid)).exp()),
            Curve::Step { threshold, lo, hi } => {
                if x >= threshold {
                    hi
                } else {
                    lo
                }
            }
            Curve::Identity => x,
        };
        y.clamp(0.0, 1.0)
    }
}

/// One factor of an action's utility.
#[derive(Copy, Clone, Debug)]
pub struct Consideration {
    /// Input read.
    pub input: Input,
    /// Curve applied.
    pub curve: Curve,
}

impl Consideration {
    /// Creates a consideration.
    pub const fn new(input: Input, curve: Curve) -> Self {
        Self { input, curve }
    }
    /// Requires the boolean input to be set (score 0 otherwise).
    pub const fn require(input: Input) -> Self {
        Self::new(
            input,
            Curve::Step {
                threshold: 0.5,
                lo: 0.0,
                hi: 1.0,
            },
        )
    }
    /// Requires the boolean input to be clear (score 0 otherwise).
    pub const fn forbid(input: Input) -> Self {
        Self::new(
            input,
            Curve::Step {
                threshold: 0.5,
                lo: 1.0,
                hi: 0.0,
            },
        )
    }
}

/// High-level tactical action identifiers.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default)]
pub enum ActionKind {
    /// Nothing selected yet.
    #[default]
    Idle,
    /// Wander around the home position.
    Patrol,
    /// Go check out a suspicious position / last known target position.
    Investigate,
    /// Shoot the target, repositioning for line of sight if needed.
    Engage,
    /// Move to protective cover.
    TakeCover,
    /// Move to a position on the target's flank.
    Flank,
    /// Fire at the target's last known position to pin it down.
    Suppress,
    /// Reload the weapon.
    Reload,
    /// Fall back away from threats.
    Retreat,
    /// User-defined action.
    Custom(u16),
}

/// An action candidate: its utility considerations and the behavior tree that executes it.
#[derive(Clone, Debug)]
pub struct ActionDef {
    /// Identifier.
    pub kind: ActionKind,
    /// Score multiplier (relative priority).
    pub weight: f32,
    /// All factors (multiplied, with compensation for count).
    pub considerations: Vec<Consideration>,
    /// Tree executed while this action is selected.
    pub tree: BehaviorTree,
}

impl ActionDef {
    /// Creates an action.
    pub fn new(
        kind: ActionKind,
        weight: f32,
        considerations: Vec<Consideration>,
        tree: BehaviorTree,
    ) -> Self {
        Self {
            kind,
            weight,
            considerations,
            tree,
        }
    }

    /// Scores this action. Uses the multiplicative scheme with a compensation
    /// factor so actions with many considerations aren't unfairly penalized.
    /// Stops early once the score reaches zero.
    #[inline]
    pub fn score(&self, inputs: &Inputs) -> f32 {
        let n = self.considerations.len();
        if n == 0 {
            return self.weight;
        }
        let modification = 1.0 - 1.0 / n as f32;
        let mut score = 1.0;
        for c in &self.considerations {
            let s = c.curve.eval(inputs.get(c.input));
            let s = s + (1.0 - s) * modification * s;
            score *= s;
            if score <= 0.0 {
                return 0.0;
            }
        }
        score * self.weight
    }
}

/// Picks the best action with hysteresis: the current action gets a
/// `momentum` bonus and is kept for at least `min_commit` seconds unless
/// another action's score dominates it by `override_ratio`.
pub(crate) fn select(
    actions: &[ActionDef],
    inputs: &Inputs,
    current: Option<usize>,
    time_in_action: f32,
    scores_out: &mut [f32],
    blocked: impl Fn(usize) -> bool,
) -> Option<usize> {
    const MOMENTUM: f32 = 0.25;
    const MIN_COMMIT: f32 = 0.6;
    const OVERRIDE_RATIO: f32 = 1.6;
    const MIN_SCORE: f32 = 0.01;

    let mut best: Option<(usize, f32)> = None;
    for (i, a) in actions.iter().enumerate() {
        let mut s = if blocked(i) { 0.0 } else { a.score(inputs) };
        if i < scores_out.len() {
            scores_out[i] = s;
        }
        if Some(i) == current {
            s *= 1.0 + MOMENTUM;
        }
        // Ignore negligible scores: "nothing to do" beats a pointless action.
        if s > MIN_SCORE && best.is_none_or(|(_, b)| s > b) {
            best = Some((i, s));
        }
    }
    let (bi, bs) = best?;
    if let Some(ci) = current {
        if ci != bi && time_in_action < MIN_COMMIT {
            let cs = scores_out.get(ci).copied().unwrap_or(0.0) * (1.0 + MOMENTUM);
            if cs > 0.0 && bs < cs * OVERRIDE_RATIO {
                return Some(ci);
            }
        }
    }
    Some(bi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bt::{Bt, Task};

    fn action(kind: ActionKind, input: Input) -> ActionDef {
        ActionDef::new(
            kind,
            1.0,
            vec![Consideration::new(input, Curve::Identity)],
            Bt::task(Task::Hold).build(),
        )
    }

    #[test]
    fn curves() {
        assert_eq!(Curve::Inverse.eval(0.25), 0.75);
        assert!(Curve::Logistic { k: 10.0, mid: 0.5 }.eval(0.9) > 0.95);
        assert_eq!(
            Curve::Step {
                threshold: 0.5,
                lo: 0.1,
                hi: 0.9
            }
            .eval(0.6),
            0.9
        );
        assert_eq!(Curve::Linear { m: 2.0, b: 0.0 }.eval(0.8), 1.0);
    }

    #[test]
    fn selection_has_hysteresis() {
        let actions = vec![
            action(ActionKind::Engage, Input::Health),
            action(ActionKind::TakeCover, Input::Ammo),
        ];
        let mut inputs = Inputs::default();
        inputs.set(Input::Health, 0.5);
        inputs.set(Input::Ammo, 0.55);
        let mut scores = [0.0; 2];
        // From nothing: the strictly higher score wins.
        assert_eq!(
            select(&actions, &inputs, None, 0.0, &mut scores, |_| false),
            Some(1)
        );
        // Already engaging: a slightly better alternative doesn't cause dithering.
        assert_eq!(
            select(&actions, &inputs, Some(0), 5.0, &mut scores, |_| false),
            Some(0)
        );
        // A much better alternative wins.
        inputs.set(Input::Ammo, 1.0);
        assert_eq!(
            select(&actions, &inputs, Some(0), 5.0, &mut scores, |_| false),
            Some(1)
        );
    }

    #[test]
    fn zero_consideration_vetoes() {
        let a = ActionDef::new(
            ActionKind::Engage,
            1.0,
            vec![
                Consideration::require(Input::HasTarget),
                Consideration::new(Input::Health, Curve::Identity),
            ],
            Bt::task(Task::Hold).build(),
        );
        let mut inputs = Inputs::default();
        inputs.set(Input::Health, 1.0);
        assert_eq!(a.score(&inputs), 0.0);
        inputs.set(Input::HasTarget, 1.0);
        assert!(a.score(&inputs) > 0.9);
    }
}
