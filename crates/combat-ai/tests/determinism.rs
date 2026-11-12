mod common;

use combat_ai::prelude::*;
use common::BoxWorld;

fn simulate(seed: u64) -> Vec<(u32, bool, [i64; 3], ActionKind)> {
    let mut ai = AiSystem::new(SystemConfig {
        seed,
        ..SystemConfig::default()
    });
    let world = BoxWorld {
        boxes: vec![
            (Vec3::new(-10.0, 0.0, 10.0), Vec3::new(10.0, 1.1, 11.0)),
            (Vec3::new(15.0, 0.0, -5.0), Vec3::new(16.0, 3.0, 5.0)),
        ],
    };
    for i in 0..20 {
        let x = i as f32 * 2.0 - 20.0;
        ai.add_cover_point(CoverPoint::new(
            Vec3::new(x, 0.0, 9.5),
            Vec3::Z,
            CoverKind::Low,
        ));
        ai.add_cover_point(CoverPoint::new(
            Vec3::new(x, 0.0, 11.5),
            -Vec3::Z,
            CoverKind::Low,
        ));
    }
    for team in 0..2u16 {
        let squad = ai.create_squad();
        for i in 0..40 {
            let z = if team == 0 { -10.0 } else { 30.0 };
            let s = ActorState::new(Vec3::new(i as f32 - 20.0, 0.0, z), TeamId(team))
                .facing(if team == 0 { Vec3::Z } else { -Vec3::Z });
            let arch = match i % 3 {
                0 => Archetype::rifleman(),
                1 => Archetype::assault(),
                _ => Archetype::sniper(),
            };
            let id = ai.spawn_agent(s, arch);
            ai.assign_squad(id, squad);
        }
    }
    let mut trace = Vec::new();
    for _ in 0..240 {
        ai.tick(1.0 / 30.0, &world);
        for c in ai.commands() {
            let q = |v: f32| (v * 1e4) as i64;
            trace.push((
                c.actor.index(),
                c.fire,
                [q(c.aim_dir.x), q(c.aim_dir.y), q(c.aim_dir.z)],
                c.action,
            ));
        }
        common::apply(&mut ai, 1.0 / 30.0);
    }
    trace
}

#[test]
fn same_seed_same_result() {
    let a = simulate(99);
    let b = simulate(99);
    assert!(a.iter().any(|t| t.1), "the scenario should include combat");
    assert_eq!(
        a, b,
        "simulation must be deterministic (incl. multi-threaded)"
    );
}

#[test]
fn different_seed_different_result() {
    assert_ne!(simulate(1), simulate(2));
}
