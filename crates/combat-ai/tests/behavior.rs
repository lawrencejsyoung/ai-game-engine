mod common;

use combat_ai::prelude::*;
use combat_ai::{ActionKind, Archetype as A};
use common::{run, BoxWorld};

fn cfg() -> SystemConfig {
    SystemConfig {
        seed: 1,
        ..SystemConfig::default()
    }
}

fn enemy_at(ai: &mut AiSystem, p: Vec3) -> ActorId {
    ai.add_actor(ActorState::new(p, TeamId(0)))
}

fn grunt_at(ai: &mut AiSystem, p: Vec3, facing: Vec3) -> ActorId {
    ai.spawn_agent(ActorState::new(p, TeamId(1)).facing(facing), A::rifleman())
}

#[test]
fn spots_and_shoots_visible_enemy() {
    let mut ai = AiSystem::new(cfg());
    let player = enemy_at(&mut ai, Vec3::new(0.0, 0.0, 20.0));
    let grunt = grunt_at(&mut ai, Vec3::ZERO, Vec3::Z);
    let world = BoxWorld::default();

    let mut fired = false;
    for _ in 0..60 {
        ai.tick(1.0 / 30.0, &world);
        fired |= ai.commands().iter().any(|c| c.fire);
        common::apply(&mut ai, 1.0 / 30.0);
    }
    let d = ai.debug(grunt).unwrap();
    assert_eq!(d.target, Some(player));
    assert!(d.target_visible);
    assert!(fired, "agent should open fire within 2 seconds");
}

#[test]
fn does_not_see_through_walls_or_behind() {
    let mut ai = AiSystem::new(cfg());
    enemy_at(&mut ai, Vec3::new(0.0, 0.0, 20.0)); // behind a wall
    enemy_at(&mut ai, Vec3::new(0.0, 0.0, -20.0)); // behind the agent
    let grunt = grunt_at(&mut ai, Vec3::ZERO, Vec3::Z);
    let world = BoxWorld::with_wall(Vec3::new(-5.0, 0.0, 9.0), Vec3::new(5.0, 3.0, 10.0));
    for _ in 0..60 {
        ai.tick(1.0 / 30.0, &world);
    }
    assert_eq!(ai.debug(grunt).unwrap().target, None);
    assert!(ai.memory(grunt).unwrap().entries().is_empty());
}

#[test]
fn detection_takes_time_at_range() {
    let mut ai = AiSystem::new(cfg());
    let p = enemy_at(&mut ai, Vec3::new(0.0, 0.0, 50.0));
    let grunt = grunt_at(&mut ai, Vec3::ZERO, Vec3::Z);
    let world = BoxWorld::default();
    ai.tick(0.12, &world);
    let aw = ai
        .memory(grunt)
        .unwrap()
        .get(p)
        .map_or(0.0, |m| m.awareness);
    assert!(aw > 0.0 && aw < 1.0, "partially aware at first: {aw}");
    run(&mut ai, &world, 2.0);
    assert!(ai.memory(grunt).unwrap().get(p).unwrap().confirmed());
}

#[test]
fn hears_gunshots_behind_and_turns_to_fight() {
    let mut ai = AiSystem::new(cfg());
    let p = enemy_at(&mut ai, Vec3::new(0.0, 0.0, -15.0));
    // A stationary sentry: the standard set minus Patrol.
    let mut sentry = (*A::rifleman()).clone();
    sentry.actions.retain(|a| a.kind != ActionKind::Patrol);
    let grunt = ai.spawn_agent(
        ActorState::new(Vec3::ZERO, TeamId(1)).facing(Vec3::Z),
        sentry.shared(),
    );
    let world = BoxWorld::default();
    run(&mut ai, &world, 0.5);
    assert_eq!(ai.debug(grunt).unwrap().target, None);
    ai.report_sound(SoundEvent {
        position: Vec3::new(0.0, 0.0, -15.0),
        radius: 60.0,
        source: Some(p),
        kind: SoundKind::Gunshot,
    });
    run(&mut ai, &world, 1.5);
    let d = ai.debug(grunt).unwrap();
    assert_eq!(d.target, Some(p));
    assert!(
        d.target_visible,
        "should have turned around and seen the shooter"
    );
}

#[test]
fn damage_reveals_attacker() {
    let mut ai = AiSystem::new(cfg());
    let p = enemy_at(&mut ai, Vec3::new(0.0, 0.0, -30.0));
    let grunt = grunt_at(&mut ai, Vec3::ZERO, Vec3::Z);
    let world = BoxWorld::default();
    run(&mut ai, &world, 0.3);
    ai.report_damage(DamageEvent {
        victim: grunt,
        attacker: Some(p),
        amount: 10.0,
        origin: Vec3::new(0.0, 1.5, -30.0),
    });
    ai.tick(1.0 / 30.0, &world);
    assert_eq!(ai.debug(grunt).unwrap().target, Some(p));
}

#[test]
fn takes_cover_that_protects_and_never_shares_it() {
    let mut ai = AiSystem::new(cfg());
    let p = enemy_at(&mut ai, Vec3::new(0.0, 0.0, 30.0));
    // A low wall at z = 5 with a single good cover point behind it, plus a useless one.
    let good = ai.add_cover_point(CoverPoint::new(
        Vec3::new(0.0, 0.0, 4.0),
        Vec3::Z,
        CoverKind::Low,
    ));
    let _bad = ai.add_cover_point(CoverPoint::new(
        Vec3::new(3.0, 0.0, 0.0),
        -Vec3::Z,
        CoverKind::Low,
    ));
    let world = BoxWorld::with_wall(Vec3::new(-1.5, 0.0, 4.6), Vec3::new(1.5, 1.1, 5.0));
    let g1 = grunt_at(&mut ai, Vec3::new(-1.0, 0.0, 0.0), Vec3::Z);
    let g2 = grunt_at(&mut ai, Vec3::new(1.0, 0.0, 0.0), Vec3::Z);
    for g in [g1, g2] {
        ai.report_damage(DamageEvent {
            victim: g,
            attacker: Some(p),
            amount: 5.0,
            origin: Vec3::new(0.0, 1.5, 30.0),
        });
    }
    let mut owners = vec![];
    for _ in 0..90 {
        ai.tick(1.0 / 30.0, &world);
        common::apply(&mut ai, 1.0 / 30.0);
        let c = [ai.debug(g1).unwrap().cover, ai.debug(g2).unwrap().cover];
        assert!(
            !(c[0].is_some() && c[0] == c[1]),
            "two agents claimed the same cover"
        );
        assert_ne!(c[0], Some(_bad));
        assert_ne!(c[1], Some(_bad));
        owners.push(ai.cover_owner(good));
    }
    assert!(
        owners.iter().any(|o| o.is_some()),
        "someone should use the good cover"
    );
}

#[test]
fn attack_tokens_limit_accurate_shooters() {
    let mut ai = AiSystem::new(cfg());
    enemy_at(&mut ai, Vec3::new(0.0, 0.0, 15.0));
    let grunts: Vec<ActorId> = (0..6)
        .map(|i| grunt_at(&mut ai, Vec3::new(i as f32 * 2.0 - 5.0, 0.0, 0.0), Vec3::Z))
        .collect();
    let world = BoxWorld::default();
    let mut max_tokens = 0;
    let mut any_tokens = false;
    for _ in 0..120 {
        ai.tick(1.0 / 30.0, &world);
        common::apply(&mut ai, 1.0 / 30.0);
        let n = grunts
            .iter()
            .filter(|g| ai.debug(**g).unwrap().has_token)
            .count();
        any_tokens |= n > 0;
        max_tokens = max_tokens.max(n);
    }
    assert!(any_tokens);
    assert!(
        max_tokens <= 2,
        "max_attackers_per_player exceeded: {max_tokens}"
    );
}

#[test]
fn squads_share_knowledge_and_assign_roles() {
    let mut ai = AiSystem::new(cfg());
    let p = enemy_at(&mut ai, Vec3::new(0.0, 0.0, 25.0));
    let squad = ai.create_squad();
    // One scout faces the enemy; the rest face away and can't see it.
    let mut members = vec![grunt_at(&mut ai, Vec3::ZERO, Vec3::Z)];
    for i in 1..4 {
        members.push(grunt_at(
            &mut ai,
            Vec3::new(i as f32 * 3.0, 0.0, -2.0),
            -Vec3::Z,
        ));
    }
    for m in &members {
        assert!(ai.assign_squad(*m, squad));
    }
    let world = BoxWorld::default();
    run(&mut ai, &world, 1.5);
    for m in &members {
        assert!(
            ai.memory(*m).unwrap().get(p).is_some_and(|e| e.confirmed()),
            "every squad member should know about the enemy"
        );
    }
    let roles: Vec<SquadRole> = members.iter().map(|m| ai.debug(*m).unwrap().role).collect();
    assert_eq!(
        roles.iter().filter(|r| **r == SquadRole::Flanker).count(),
        1,
        "{roles:?}"
    );
    assert!(roles.contains(&SquadRole::Suppressor), "{roles:?}");
}

#[test]
fn patrols_when_idle_and_investigates_noises() {
    let mut ai = AiSystem::new(cfg());
    let g = grunt_at(&mut ai, Vec3::ZERO, Vec3::Z);
    let world = BoxWorld::default();
    run(&mut ai, &world, 1.0);
    assert_eq!(ai.debug(g).unwrap().action, ActionKind::Patrol);
    ai.report_sound(SoundEvent {
        position: Vec3::new(30.0, 0.0, 0.0),
        radius: 80.0,
        source: None,
        kind: SoundKind::Explosion,
    });
    run(&mut ai, &world, 1.0);
    let d = ai.debug(g).unwrap();
    assert_eq!(d.action, ActionKind::Investigate);
    assert!(d.destination.unwrap().distance(Vec3::new(30.0, 0.0, 0.0)) < 1.0);
}

#[test]
fn lod_reduces_think_rate_far_from_focus() {
    let mut ai = AiSystem::new(cfg());
    let near = grunt_at(&mut ai, Vec3::new(5.0, 0.0, 0.0), Vec3::Z);
    let far = grunt_at(&mut ai, Vec3::new(500.0, 0.0, 0.0), Vec3::Z);
    ai.set_lod_focus(&[Vec3::ZERO]);
    run(&mut ai, &BoxWorld::default(), 1.0);
    let (n, f) = (ai.debug(near).unwrap(), ai.debug(far).unwrap());
    assert!(f.think_interval > n.think_interval * 4.0);
}

#[test]
fn removed_actors_are_forgotten_and_handles_go_stale() {
    let mut ai = AiSystem::new(cfg());
    let p = enemy_at(&mut ai, Vec3::new(0.0, 0.0, 10.0));
    let g = grunt_at(&mut ai, Vec3::ZERO, Vec3::Z);
    let world = BoxWorld::default();
    run(&mut ai, &world, 1.0);
    assert_eq!(ai.debug(g).unwrap().target, Some(p));
    assert!(ai.remove_actor(p));
    assert!(ai.actor_state(p).is_none());
    assert!(!ai.remove_actor(p));
    let reused = enemy_at(&mut ai, Vec3::new(100.0, 0.0, 100.0));
    assert_eq!(reused.index(), p.index());
    assert_ne!(reused, p);
    run(&mut ai, &world, 0.5);
    assert_eq!(ai.debug(g).unwrap().target, None);
    // Removing an agent keeps the others intact.
    let g2 = grunt_at(&mut ai, Vec3::new(3.0, 0.0, 0.0), Vec3::Z);
    assert!(ai.remove_actor(g));
    run(&mut ai, &world, 0.2);
    assert!(ai.debug(g2).is_some());
    assert_eq!(ai.commands().len(), 1);
}

#[test]
fn dead_agents_emit_no_commands() {
    let mut ai = AiSystem::new(cfg());
    let g = grunt_at(&mut ai, Vec3::ZERO, Vec3::Z);
    ai.actor_state_mut(g).unwrap().health = 0.0;
    ai.tick(0.1, &BoxWorld::default());
    assert!(ai.commands().is_empty());
}

#[test]
fn custom_archetype_with_custom_action() {
    // Mirrors the README example: a berserker that charges when it can see you.
    let mut arch = (*A::assault()).clone();
    arch.name = "berserker".into();
    arch.config.courage = 1.0;
    arch.actions.push(ActionDef::new(
        ActionKind::Custom(1),
        2.0,
        vec![
            Consideration::require(Input::TargetVisible),
            Consideration::new(Input::Health, Curve::Linear { m: 0.5, b: 0.5 }),
        ],
        Bt::sequence([
            Bt::task(Task::SetDestinationToLastKnown),
            Bt::task(Task::MoveToDestination(MoveSpeed::Sprint)),
        ])
        .build(),
    ));
    let mut ai = AiSystem::new(cfg());
    enemy_at(&mut ai, Vec3::new(0.0, 0.0, 10.0));
    let b = ai.spawn_agent(
        ActorState::new(Vec3::ZERO, TeamId(1)).facing(Vec3::Z),
        arch.shared(),
    );
    let world = BoxWorld::default();
    let mut charged = false;
    for _ in 0..60 {
        ai.tick(1.0 / 30.0, &world);
        charged |= ai.commands()[0].action == ActionKind::Custom(1);
        common::apply(&mut ai, 1.0 / 30.0);
    }
    assert!(
        charged,
        "custom action should be selected: {:?}",
        ai.debug(b)
    );
}
