//! Stress test: prints per-tick engine counters for a large battle.
//! `cargo run -p combat-ai --release --example stress -- 2000`

use combat_ai::prelude::*;

/// Field of pillars on a 20 m lattice; LOS = analytic segment-vs-cylinder tests
/// against the few pillars near the segment (cheap, but not free, like a real raycast).
struct Pillars {
    spacing: f32,
    radius: f32,
}

impl WorldQuery for Pillars {
    fn line_of_sight(&self, from: Vec3, to: Vec3) -> bool {
        let (x0, x1) = (from.x.min(to.x), from.x.max(to.x));
        let (z0, z1) = (from.z.min(to.z), from.z.max(to.z));
        let s = self.spacing;
        let d = Vec3::new(to.x - from.x, 0.0, to.z - from.z);
        let len2 = d.length_squared().max(1e-6);
        let mut gx = (x0 / s).floor() as i32;
        while (gx as f32) * s <= x1 + s {
            let mut gz = (z0 / s).floor() as i32;
            while (gz as f32) * s <= z1 + s {
                let c = Vec3::new(gx as f32 * s, 0.0, gz as f32 * s);
                let t = ((c - Vec3::new(from.x, 0.0, from.z)).dot(d) / len2).clamp(0.0, 1.0);
                let p = Vec3::new(from.x, 0.0, from.z) + d * t;
                if p.distance_squared(c) < self.radius * self.radius {
                    return false;
                }
                gz += 1;
            }
            gx += 1;
        }
        true
    }
}

fn setup(n: usize, lod: bool) -> AiSystem {
    let mut ai = AiSystem::new(SystemConfig::default());
    let side = (n as f32).sqrt() * 6.0 + 40.0;
    // Cover on all four sides of each pillar.
    let s = 20.0;
    let mut x = -side;
    while x <= side {
        let mut z = -side;
        while z <= side {
            for dir in [Vec3::X, -Vec3::X, Vec3::Z, -Vec3::Z] {
                ai.add_cover_point(CoverPoint::new(
                    Vec3::new(x, 0.0, z) - dir * 1.6,
                    dir,
                    CoverKind::High,
                ));
            }
            z += s;
        }
        x += s;
    }
    let mut squad = ai.create_squad();
    for i in 0..n {
        let team = (i % 2) as u16;
        let row = (i / 2) as f32;
        let per_row = (n as f32).sqrt().ceil();
        let px = (row % per_row) * 5.0 - side * 0.5 + 2.5;
        let pz = (row / per_row).floor() * 5.0 * if team == 0 { 1.0 } else { -1.0 }
            + if team == 0 { 15.0 } else { -15.0 };
        let facing = if team == 0 { -Vec3::Z } else { Vec3::Z };
        let arch = match i % 5 {
            0..=2 => Archetype::rifleman(),
            3 => Archetype::assault(),
            _ => Archetype::sniper(),
        };
        let id = ai.spawn_agent(
            ActorState::new(Vec3::new(px, 0.0, pz), TeamId(team)).facing(facing),
            arch,
        );
        if i % 16 == 15 {
            squad = ai.create_squad();
        }
        ai.assign_squad(id, squad);
    }
    if lod {
        ai.set_lod_focus(&[Vec3::ZERO]);
    }
    ai
}

fn apply(ai: &mut AiSystem, dt: f32) {
    let cmds: Vec<AgentCommand> = ai.commands().to_vec();
    for c in cmds {
        let s = ai.actor_state_mut(c.actor).unwrap();
        if let Some(t) = c.move_to {
            let to = Vec3::new(t.x - s.position.x, 0.0, t.z - s.position.z);
            let d = to.length();
            if d > 1e-3 {
                s.position += to / d * (4.0 * dt).min(d);
            }
        }
        s.forward = Vec3::new(c.aim_dir.x, 0.0, c.aim_dir.z).normalize_or(s.forward);
        if c.fire && s.ammo > 0 {
            s.ammo -= 1;
        }
        if c.reload {
            s.ammo = s.mag_size;
        }
    }
}

fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(500);
    let world = Pillars {
        spacing: 20.0,
        radius: 1.2,
    };
    let mut ai = setup(n, false);
    let dt = 1.0 / 60.0;
    let mut acc = [0.0f64; 4];
    for frame in 1..=600 {
        ai.tick(dt, &world);
        apply(&mut ai, dt);
        let s = ai.stats();
        acc[0] += s.micros as f64;
        acc[1] += s.thinks as f64;
        acc[2] += s.raycasts as f64;
        acc[3] += s.path_queries as f64;
        if frame % 120 == 0 {
            let alive = ai.commands().len();
            let mem: usize = ai
                .actors()
                .filter_map(|a| ai.memory(a))
                .map(|m| m.entries().len())
                .max()
                .unwrap_or(0);
            let f = 120.0;
            println!(
                "t={:5.1}s alive={alive:5} tick={:8.1}us thinks={:6.1} rays={:7.1} paths={:6.1} max_mem={mem}",
                frame as f32 * dt, acc[0] / f, acc[1] / f, acc[2] / f, acc[3] / f
            );
            acc = [0.0; 4];
        }
    }
}
