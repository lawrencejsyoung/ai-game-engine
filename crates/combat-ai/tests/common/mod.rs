#![allow(dead_code)]
use combat_ai::prelude::*;

/// Open field with axis-aligned box obstacles.
#[derive(Default)]
pub struct BoxWorld {
    pub boxes: Vec<(Vec3, Vec3)>,
}

impl BoxWorld {
    pub fn with_wall(min: Vec3, max: Vec3) -> Self {
        Self {
            boxes: vec![(min, max)],
        }
    }
}

fn segment_hits_box(a: Vec3, b: Vec3, min: Vec3, max: Vec3) -> bool {
    let d = b - a;
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for i in 0..3 {
        let (o, dir, lo, hi) = (a[i], d[i], min[i], max[i]);
        if dir.abs() < 1e-9 {
            if o < lo || o > hi {
                return false;
            }
        } else {
            let (mut ta, mut tb) = ((lo - o) / dir, (hi - o) / dir);
            if ta > tb {
                std::mem::swap(&mut ta, &mut tb);
            }
            t0 = t0.max(ta);
            t1 = t1.min(tb);
            if t0 > t1 {
                return false;
            }
        }
    }
    true
}

impl WorldQuery for BoxWorld {
    fn line_of_sight(&self, from: Vec3, to: Vec3) -> bool {
        !self
            .boxes
            .iter()
            .any(|(lo, hi)| segment_hits_box(from, to, *lo, *hi))
    }
}

/// Moves agents toward their move targets and faces them along their aim, like a minimal host.
pub fn apply(ai: &mut AiSystem, dt: f32) {
    let cmds: Vec<AgentCommand> = ai.commands().to_vec();
    for c in cmds {
        let s = ai.actor_state_mut(c.actor).unwrap();
        if let Some(t) = c.move_to {
            let to = Vec3::new(t.x - s.position.x, 0.0, t.z - s.position.z);
            let d = to.length();
            let step = (4.0 * dt).min(d);
            if d > 1e-4 {
                s.position += to / d * step;
                s.velocity = to / d * 4.0;
            }
        } else {
            s.velocity = Vec3::ZERO;
        }
        s.forward = Vec3::new(c.aim_dir.x, 0.0, c.aim_dir.z).normalize_or(s.forward);
        s.stance = c.stance;
        if c.fire && s.ammo > 0 {
            s.ammo -= 1;
        }
        if c.reload {
            s.ammo = s.mag_size;
        }
    }
}

pub fn run(ai: &mut AiSystem, world: &impl WorldQuery, seconds: f32) {
    let dt = 1.0 / 30.0;
    let n = (seconds / dt).round() as usize;
    for _ in 0..n {
        ai.tick(dt, world);
        apply(ai, dt);
    }
}
