//! Vision, hearing, damage awareness and squad knowledge intake.

use crate::agent::ActorState;
use crate::brain::{Brain, Shared};
use crate::command::Bark;
use crate::ids::ActorId;
use crate::math::flat_dir;
use crate::memory::Source;
use crate::stimuli::SoundKind;
use glam::Vec3;

const MAX_LOS_BUDGET: usize = 16;

#[derive(Copy, Clone)]
struct Candidate {
    id: ActorId,
    dist: f32,
    central: bool,
    priority: f32,
}

pub(crate) fn perceive(b: &mut Brain, me: &ActorState, sh: &Shared) {
    let now = sh.time;
    let cfg = &b.archetype.config;
    let dt = if b.last_perception.is_finite() {
        (now - b.last_perception).clamp(0.0, 1.0)
    } else {
        cfg.think_interval
    };
    let since = b.last_perception;
    let eye = me.eye();
    let fwd = flat_dir(me.forward, Vec3::Z);
    let cos_central = (cfg.fov_deg.to_radians() * 0.5).cos();
    let cos_periph = (cfg.peripheral_fov_deg.to_radians() * 0.5).cos();

    let cap = cfg.max_tracked_targets as usize;
    let current = b.target;

    // --- vision: gather hostile candidates in range and in the view cone ---
    // Only the top `max_los_checks` (by priority) get a raycast, so keep a
    // fixed-size top-K set instead of collecting everything (no allocation).
    let budget = (cfg.max_los_checks as usize).clamp(1, MAX_LOS_BUDGET);
    let mut top: [Option<Candidate>; MAX_LOS_BUDGET] = [None; MAX_LOS_BUDGET];
    for e in b.memory.entries.iter_mut() {
        // Re-established below if still in view.
        e.in_view = false;
    }
    let range2 = cfg.view_range * cfg.view_range;
    let prox2 = cfg.proximity_range * cfg.proximity_range;
    sh.actor_grid
        .query(me.position, cfg.view_range, |slot, pos| {
            let a = &sh.actors[slot as usize];
            if a.state.team == me.team || !a.state.is_alive() {
                return;
            }
            let to = pos - me.position;
            let dist2 = to.length_squared();
            if dist2 > range2 {
                return;
            }
            let flat = Vec3::new(to.x, 0.0, to.z);
            let flat_len = flat.length();
            let dot = flat.dot(fwd);
            let close = dist2 <= prox2;
            if !close && dot < cos_periph * flat_len {
                return;
            }
            let id = sh.actor_id(slot);
            let staleness = match b.memory.get_mut(id) {
                Some(m) => {
                    m.in_view = true;
                    (now - m.los_checked).min(10.0)
                }
                None => 10.0,
            };
            let dist = dist2.sqrt();
            // Round-robin: current target first, then the least recently raycast, then nearest.
            let priority = if current == Some(id) { 100.0 } else { 0.0 } + staleness - dist * 0.01;
            let cand = Candidate {
                id,
                dist,
                central: close || dot >= cos_central * flat_len,
                priority,
            };
            // Insert into the top-K set.
            let mut worst = 0;
            for i in 0..budget {
                match top[i] {
                    None => {
                        top[i] = Some(cand);
                        return;
                    }
                    Some(c) if c.priority < top[worst].map_or(f32::INFINITY, |w| w.priority) => {
                        worst = i
                    }
                    _ => {}
                }
            }
            if top[worst].is_some_and(|w| w.priority < cand.priority) {
                top[worst] = Some(cand);
            }
        });
    // Anything not in the view cone this update is not visible.
    for e in b.memory.entries.iter_mut() {
        e.visible &= e.in_view;
    }

    let mut newly_confirmed = false;
    for c in top[..budget].iter().flatten() {
        let Some(target) = sh.actor(c.id) else {
            continue;
        };
        let visible = sh.los(eye, target.chest()) || sh.los(eye, target.eye());
        if !visible {
            if let Some(m) = b.memory.get_mut(c.id) {
                m.visible = false;
                m.los_checked = now;
            }
            continue;
        }
        let rate = if c.dist <= cfg.proximity_range {
            f32::INFINITY
        } else {
            let falloff = (1.0 - c.dist / cfg.view_range).max(0.15);
            let cone = if c.central { 1.0 } else { 0.35 };
            // Moving targets are noticed faster.
            let motion = 1.0 + (target.velocity.length() * 0.15).min(0.6);
            cfg.detection_rate * falloff * cone * motion
        };
        let Some(m) = b.memory.entry(
            c.id,
            target.position,
            now,
            Source::Sight,
            cap,
            3.0,
            me.position,
            current,
        ) else {
            continue;
        };
        let was_confirmed = m.confirmed();
        m.awareness = (m.awareness + rate * dt).min(1.0);
        if m.awareness > 0.995 {
            m.awareness = 1.0;
        }
        m.visible = true;
        m.los_checked = now;
        m.last_known_pos = target.position;
        m.last_known_vel = target.velocity;
        m.last_seen = now;
        m.last_update = now;
        m.searched = false;
        m.source = Source::Sight;
        if !was_confirmed && m.confirmed() {
            newly_confirmed = true;
        }
    }
    // Visible-but-over-budget targets keep their cached visibility and live position.
    for e in b
        .memory
        .entries
        .iter_mut()
        .filter(|e| e.visible && e.los_checked < now)
    {
        if let Some(a) = sh.actor(e.target) {
            e.last_known_pos = a.position;
            e.last_known_vel = a.velocity;
            e.last_seen = now;
            e.last_update = now;
        }
    }

    // --- hearing ---
    for s in sh.sounds.iter().filter(|s| s.time > since && s.time <= now) {
        let ev = &s.event;
        let r = ev.radius * cfg.hearing;
        let d = ev.position.distance(me.position);
        if d > r {
            continue;
        }
        let hostile_src = ev.source.and_then(|src| {
            let a = sh.actor(src)?;
            (a.team != me.team && src != b.actor).then_some((src, a))
        });
        if ev.kind == SoundKind::BulletImpact {
            b.under_fire = (b.under_fire + 0.4).min(1.0);
        }
        match hostile_src {
            Some((src, actor)) => {
                // Estimated position gets better the closer the sound.
                let o = b.rng.unit_disk() * (d * 0.08);
                let est = actor.position + Vec3::new(o.x, 0.0, o.y);
                let gain = match ev.kind {
                    SoundKind::Gunshot | SoundKind::BulletImpact => {
                        if d < r * 0.5 {
                            1.0
                        } else {
                            0.6
                        }
                    }
                    SoundKind::Movement => 0.35,
                    SoundKind::Explosion => 0.5,
                };
                let prio = if ev.kind == SoundKind::Movement {
                    0.5
                } else {
                    1.0
                };
                let Some(m) = b.memory.entry(
                    src,
                    est,
                    now,
                    Source::Sound,
                    cap,
                    prio,
                    me.position,
                    current,
                ) else {
                    continue;
                };
                let was_confirmed = m.confirmed();
                m.awareness = (m.awareness + gain).min(1.0);
                if !m.visible {
                    m.last_known_pos = est;
                    m.last_known_vel = Vec3::ZERO;
                    m.last_update = now;
                    m.searched = false;
                    m.source = Source::Sound;
                }
                if !was_confirmed && m.confirmed() {
                    newly_confirmed = true;
                }
            }
            // Unattributed noises are worth checking out when nothing else is going on.
            None if (ev.source.is_none() || ev.kind == SoundKind::Explosion)
                && b.memory.confirmed_count() == 0 =>
            {
                b.investigate = Some(ev.position);
            }
            None => {}
        }
    }

    // --- damage ---
    let damage = std::mem::take(&mut b.pending_damage);
    for dmg in &damage {
        b.under_fire = 1.0;
        let Some(att) = dmg.attacker else {
            b.investigate.get_or_insert(dmg.origin);
            continue;
        };
        let Some(a) = sh.actor(att) else { continue };
        if a.team == me.team {
            continue;
        }
        let Some(m) = b.memory.entry(
            att,
            dmg.origin,
            now,
            Source::Damage,
            cap,
            6.0,
            me.position,
            current,
        ) else {
            continue;
        };
        if !m.confirmed() {
            newly_confirmed = true;
        }
        m.awareness = 1.0;
        m.last_damaged_us = now;
        if !m.visible {
            m.last_known_pos = a.position;
            m.last_update = now;
            m.searched = false;
            m.source = Source::Damage;
        }
    }
    b.pending_damage = damage;
    b.pending_damage.clear();

    // --- squad call-outs ---
    if let Some(sq) = b.squad.and_then(|s| sh.squads.get(s.0 as usize)) {
        for t in &sq.targets {
            if sh.actor(t.target).is_none_or(|a| !a.is_alive()) {
                continue;
            }
            let Some(m) = b.memory.entry(
                t.target,
                t.pos,
                t.last_seen,
                Source::Squad,
                cap,
                2.2,
                me.position,
                current,
            ) else {
                continue;
            };
            if !m.visible && t.last_seen > m.last_update {
                m.last_known_pos = t.pos;
                m.last_known_vel = t.vel;
                m.last_update = t.last_seen;
                m.searched = false;
                m.source = Source::Squad;
            }
            m.awareness = 1.0;
        }
    }

    // --- forget the dead and the stale ---
    b.memory
        .entries
        .retain(|e| sh.actor(e.target).is_some_and(|a| a.is_alive()));
    b.memory.decay(now, dt, cfg.memory_span);
    b.under_fire = (b.under_fire - dt * 0.35).max(0.0);
    if b.memory.confirmed_count() > 0 {
        b.investigate = None;
    }
    if newly_confirmed {
        b.bark(now, Bark::EnemySpotted);
    }
    b.last_perception = now;
}
