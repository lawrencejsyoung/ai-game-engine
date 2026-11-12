//! Per-frame execution: aim error model, burst fire and command output.

use crate::agent::ActorState;
use crate::brain::{Brain, FireMode, LookMode, Shared};
use crate::command::AgentCommand;
use crate::math::flat_dir;
use crate::utility::ActionKind;
use glam::Vec3;

pub(crate) fn act(b: &mut Brain, me: &ActorState, sh: &Shared) {
    let now = sh.time;
    let dt = sh.dt;
    let cfg = &b.archetype.config;
    let intent = b.intent;
    let eye = me.eye();

    let tmem = b.target.and_then(|t| b.memory.get(t)).copied();
    let live = tmem.filter(|m| m.visible).and_then(|m| sh.actor(m.target));

    // --- where to look ---
    let look_point = match intent.look {
        LookMode::Target => match (live, tmem) {
            // Aim at whatever part of the body is actually exposed.
            (Some(a), Some(m)) if m.head_only => Some(a.eye()),
            (Some(a), _) => Some(a.chest()),
            (None, Some(m)) => Some(m.last_known_pos + Vec3::Y * 1.2),
            _ => None,
        },
        LookMode::Point(p) => Some(p),
        // Not told where to look: glance toward whatever we half-noticed.
        LookMode::Forward => b
            .memory
            .entries()
            .iter()
            .filter(|e| !e.confirmed() && e.awareness > 0.05)
            .max_by(|x, y| x.awareness.total_cmp(&y.awareness))
            .map(|e| e.last_known_pos + Vec3::Y * 1.2),
    };

    // --- aim settling: accuracy improves while tracking the same visible target ---
    if b.aim.target != b.target {
        b.aim.target = b.target;
        b.aim.settle = 0.0;
    }
    if intent.look == LookMode::Target && live.is_some() {
        b.aim.settle = (b.aim.settle + dt / cfg.aim_settle_time.max(1e-3)).min(1.0);
    } else {
        b.aim.settle = (b.aim.settle - dt / cfg.aim_settle_time.max(1e-3) * 0.5).max(0.0);
    }

    // --- attack token bookkeeping (resolved serially after this phase) ---
    b.wants_token = match (intent.fire, live) {
        (FireMode::Target, Some(_)) => b.target,
        _ => None,
    };
    let target_is_external = b
        .target
        .and_then(|t| sh.actors.get(t.index as usize))
        .is_some_and(|s| s.brain == u32::MAX);
    let token_limited = sh.config.max_attackers_per_player > 0 && target_is_external;
    let has_token = !token_limited || (b.token.is_some() && b.token == b.target);

    // --- fire decision with bursts ---
    let range = cfg.effective_range(sh.config.target_radius) / sh.config.accuracy_scale.max(1e-3);
    let in_range = |a: &ActorState| a.position.distance_squared(me.position) <= range * range;
    let wants_fire = match intent.fire {
        FireMode::Hold => false,
        FireMode::Target => live.is_some_and(in_range) && now >= b.reaction_ready,
        // Suppression doesn't need to hit, but there's no point beyond the weapon's reach.
        FireMode::Point(p) => p.distance_squared(eye) <= (range * 1.5) * (range * 1.5),
    } && me.ammo > 0
        && !intent.reload;
    let mut fire = false;
    if wants_fire && now >= b.aim.next_shot {
        fire = true;
        b.aim.shots_in_burst += 1;
        let interval = if matches!(intent.fire, FireMode::Point(_)) {
            cfg.fire_interval.max(1.0 / cfg.suppression_rate.max(0.01))
        } else {
            cfg.fire_interval
        };
        b.aim.next_shot = now + interval;
        if b.aim.shots_in_burst >= cfg.burst_length.max(1) {
            b.aim.shots_in_burst = 0;
            b.aim.next_shot = now + b.rng.range(cfg.burst_pause.0, cfg.burst_pause.1);
        }
        b.aim.error = b.rng.unit_disk();
    } else if !wants_fire {
        b.aim.shots_in_burst = 0;
    }

    // --- aim direction with error ---
    let (aim_dir, aim_point) = match look_point {
        Some(p) => {
            let to = p - eye;
            let dist = to.length().max(0.01);
            let dir = to / dist;
            let target_speed = live.map_or(0.0, |a| a.velocity.length());
            let mut spread = cfg.spread_deg(
                b.aim.settle,
                target_speed,
                me.velocity.length(),
                b.under_fire,
            ) * sh.config.accuracy_scale.max(0.0);
            if matches!(intent.fire, FireMode::Point(_)) {
                spread += 2.5;
            } else if !has_token {
                spread += sh.config.no_token_spread_deg;
            }
            let right = dir.cross(Vec3::Y).normalize_or(Vec3::X);
            let up = right.cross(dir);
            let r = dist * spread.to_radians().tan();
            let aimed = p + (right * b.aim.error.x + up * b.aim.error.y) * r;
            ((aimed - eye).normalize_or(dir), Some(aimed))
        }
        None => {
            let d = match intent.move_to {
                Some(m) => flat_dir(m - me.position, me.forward),
                None => me.forward,
            };
            (d, None)
        }
    };

    let action = b
        .current_action
        .map_or(ActionKind::Idle, |i| b.archetype.actions[i].kind);
    b.command = AgentCommand {
        actor: b.actor,
        move_to: intent.move_to,
        move_speed: intent.speed,
        aim_dir,
        aim_point,
        aim_target: match (intent.look, intent.fire, live) {
            (LookMode::Target, FireMode::Hold | FireMode::Target, Some(_)) => b.target,
            _ => None,
        },
        fire,
        reload: intent.reload && me.ammo < me.mag_size,
        stance: intent.stance,
        bark: b.pending_bark.take(),
        action,
    };
}
