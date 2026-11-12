//! Headless two-team firefight driven entirely by `combat-ai`.
//!
//! The "game" here is deliberately simple (grid level, hitscan weapons, no
//! physics) — it plays the role of the host engine: it owns transforms,
//! health, ammo and pathing, feeds state/stimuli to the AI, and applies the
//! commands it gets back.
//!
//! Usage: `cargo run -p arena-demo --release -- [--seed N] [--per-team N] [--quiet] [--swap-sides]`

mod map;

use combat_ai::prelude::*;
use combat_ai::ActionKind;
use map::{Map, CELL, LAYOUT};
use std::collections::HashMap;
use std::sync::Arc;

const DT: f32 = 1.0 / 60.0;
const MAX_TIME: f32 = 240.0;

struct Weapon {
    damage: f32,
    reload_time: f32,
}

struct Body {
    id: ActorId,
    label: char,
    team: u16,
    archetype: &'static str,
    weapon: Weapon,
    path: Vec<Vec3>,
    path_goal: Option<Vec3>,
    reload_left: f32,
    shots: u32,
    hits: u32,
    kills: u32,
}

fn arg<T: std::str::FromStr>(args: &[String], name: &str, default: T) -> T {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seed: u64 = arg(&args, "--seed", 7);
    let per_team: usize = arg(&args, "--per-team", 6);
    let quiet = args.iter().any(|a| a == "--quiet");
    let swap = args.iter().any(|a| a == "--swap-sides");

    let map = Map::parse(LAYOUT);
    let mut ai = AiSystem::new(SystemConfig {
        seed,
        ..SystemConfig::default()
    });
    for c in map.cover_points() {
        ai.add_cover_point(c);
    }

    // --- spawn two teams on opposite sides ---
    let roster: [(&'static str, Arc<Archetype>, Weapon); 3] = [
        (
            "rifleman",
            Archetype::rifleman(),
            Weapon {
                damage: 14.0,
                reload_time: 2.0,
            },
        ),
        (
            "assault",
            Archetype::assault(),
            Weapon {
                damage: 9.0,
                reload_time: 2.4,
            },
        ),
        (
            "sniper",
            Archetype::sniper(),
            Weapon {
                damage: 55.0,
                reload_time: 3.0,
            },
        ),
    ];
    let pick = |i: usize| match i % 6 {
        0 | 2 | 4 => 0,
        1 | 3 => 1,
        _ => 2,
    };
    let mut bodies: Vec<Body> = Vec::new();
    let mut rng = combat_ai::math::Rng::new(seed);
    for team in 0..2u16 {
        let squads = [ai.create_squad(), ai.create_squad()];
        for i in 0..per_team {
            let (name, arch, w) = &roster[pick(i)];
            let left = (team == 0) != swap;
            let x = if left { 4 } else { map.w - 5 };
            let (mut cx, mut cz);
            loop {
                cx = x + rng.range(-2.0, 3.0) as i32;
                cz = 2 + (rng.f32() * (map.h - 4) as f32) as i32;
                if map.walkable(cx, cz) {
                    break;
                }
            }
            let facing = if left { Vec3::X } else { -Vec3::X };
            let mut state = ActorState::new(Map::center(cx, cz), TeamId(team)).facing(facing);
            state.mag_size = match *name {
                "sniper" => 5,
                "assault" => 24,
                _ => 30,
            };
            state.ammo = state.mag_size;
            let id = ai.spawn_agent(state, arch.clone());
            ai.assign_squad(id, squads[i % 2]);
            let label = if team == 0 {
                (b'A' + i as u8) as char
            } else {
                (b'a' + i as u8) as char
            };
            bodies.push(Body {
                id,
                label,
                team,
                archetype: name,
                weapon: Weapon {
                    damage: w.damage,
                    reload_time: w.reload_time,
                },
                path: Vec::new(),
                path_goal: None,
                reload_left: 0.0,
                shots: 0,
                hits: 0,
                kills: 0,
            });
        }
    }
    let teams: Vec<(ActorId, u16)> = bodies.iter().map(|b| (b.id, b.team)).collect();
    let index_of: HashMap<ActorId, usize> =
        bodies.iter().enumerate().map(|(i, b)| (b.id, i)).collect();

    println!(
        "arena: {}x{} m, {} cover points, {} agents per team, seed {seed}",
        map.w as f32 * CELL,
        map.h as f32 * CELL,
        ai.cover_points().len(),
        per_team
    );

    let mut time = 0.0f32;
    let mut frame = 0u64;
    let mut total_us = 0.0f64;
    let mut max_us = 0.0f32;
    let mut total_thinks = 0u64;
    let mut total_rays = 0u64;
    let mut total_paths = 0u64;
    let mut action_time: HashMap<ActionKind, f32> = HashMap::new();
    let mut barks = 0u32;
    let mut explosion_done = false;

    let winner = loop {
        time += DT;
        frame += 1;

        // A distant explosion at the center draws both teams in.
        if !explosion_done && time > 1.0 {
            explosion_done = true;
            let center = Vec3::new(map.w as f32 * CELL * 0.5, 0.0, map.h as f32 * CELL * 0.5);
            ai.report_sound(SoundEvent {
                position: center,
                radius: 200.0,
                source: None,
                kind: SoundKind::Explosion,
            });
            if !quiet {
                println!("[{time:6.2}s] *** explosion at the center of the map ***");
            }
        }

        let commands: Vec<AgentCommand> = ai.tick(DT, &map).to_vec();
        let st = ai.stats();
        total_us += st.micros as f64;
        max_us = max_us.max(st.micros);
        total_thinks += st.thinks as u64;
        total_rays += st.raycasts as u64;
        total_paths += st.path_queries as u64;

        // --- apply commands (the "game" side) ---
        // Resolve everything against a snapshot taken before anyone moves, so
        // command order doesn't favor either team.
        let snapshot: HashMap<ActorId, ActorState> = teams
            .iter()
            .map(|(id, _)| (*id, *ai.actor_state(*id).unwrap()))
            .collect();
        let mut damage_events = Vec::new();
        let mut impact_sounds = Vec::new();
        for cmd in &commands {
            let bi = index_of[&cmd.actor];
            *action_time.entry(cmd.action).or_default() += DT;
            if let Some(b) = cmd.bark {
                barks += 1;
                if !quiet {
                    println!(
                        "[{time:6.2}s] {} ({}) {:?}  [{:?}]",
                        bodies[bi].label, bodies[bi].archetype, b, cmd.action
                    );
                }
            }

            // Locomotion along an A* path.
            let state = snapshot[&cmd.actor];
            let body = &mut bodies[bi];
            match cmd.move_to {
                Some(goal) => {
                    let replan = body.path_goal.is_none_or(|g| g.distance(goal) > 0.5);
                    if replan {
                        body.path = map.find_path(state.position, goal).unwrap_or_default();
                        body.path_goal = Some(goal);
                    }
                }
                None => {
                    body.path.clear();
                    body.path_goal = None;
                }
            }
            let speed = match cmd.move_speed {
                MoveSpeed::Walk => 2.0,
                MoveSpeed::Run => 4.5,
                MoveSpeed::Sprint => 6.5,
            };
            let mut pos = state.position;
            let mut vel = Vec3::ZERO;
            let mut budget = speed * DT;
            while budget > 0.0 {
                let Some(&wp) = body.path.first() else { break };
                let to = wp - pos;
                let d = to.length();
                if d <= budget {
                    pos = wp;
                    budget -= d;
                    body.path.remove(0);
                } else {
                    pos += to / d * budget;
                    vel = to / d * speed;
                    budget = 0.0;
                }
            }

            // Reloading.
            let mut ammo = state.ammo;
            if body.reload_left > 0.0 {
                body.reload_left -= DT;
                if body.reload_left <= 0.0 {
                    ammo = state.mag_size;
                }
            } else if cmd.reload && ammo < state.mag_size {
                body.reload_left = body.weapon.reload_time;
            }

            // Hitscan fire.
            if cmd.fire && ammo > 0 && body.reload_left <= 0.0 {
                ammo -= 1;
                body.shots += 1;
                let eye = state.eye();
                let mut best: Option<(f32, ActorId)> = None;
                let mut near_miss: Option<(f32, Vec3, ActorId)> = None;
                for &(oid, oteam) in &teams {
                    if oteam == body.team {
                        continue;
                    }
                    let o = &snapshot[&oid];
                    if !o.is_alive() {
                        continue;
                    }
                    let c = o.chest();
                    let t = (c - eye).dot(cmd.aim_dir);
                    if t <= 0.0 || t > 150.0 {
                        continue;
                    }
                    let closest = eye + cmd.aim_dir * t;
                    let miss = closest.distance(c);
                    if miss < 0.45 && map.line_of_sight(eye, closest) {
                        if best.is_none_or(|(bt, _)| t < bt) {
                            best = Some((t, oid));
                        }
                    } else if miss < 2.5 && near_miss.is_none_or(|(m, _, _)| miss < m) {
                        near_miss = Some((miss, closest, oid));
                    }
                }
                if let Some((_, victim)) = best {
                    body.hits += 1;
                    damage_events.push((victim, cmd.actor, body.weapon.damage, eye));
                } else if let Some((_, p, _)) = near_miss {
                    impact_sounds.push((p, cmd.actor));
                }
            }

            let st = ai.actor_state_mut(cmd.actor).unwrap();
            st.position = pos;
            st.velocity = vel;
            st.forward = Vec3::new(cmd.aim_dir.x, 0.0, cmd.aim_dir.z).normalize_or(st.forward);
            st.stance = if vel.length_squared() > 0.01 {
                Stance::Stand
            } else {
                cmd.stance
            };
            st.ammo = ammo;
        }
        for (victim, attacker, dmg, origin) in damage_events {
            let st = ai.actor_state_mut(victim).unwrap();
            if !st.is_alive() {
                continue;
            }
            st.health -= dmg;
            let died = !st.is_alive();
            ai.report_damage(DamageEvent {
                victim,
                attacker: Some(attacker),
                amount: dmg,
                origin,
            });
            if died {
                bodies[index_of[&attacker]].kills += 1;
                if !quiet {
                    println!(
                        "[{time:6.2}s] {} ({}) killed {} ({})",
                        bodies[index_of[&attacker]].label,
                        bodies[index_of[&attacker]].archetype,
                        bodies[index_of[&victim]].label,
                        bodies[index_of[&victim]].archetype
                    );
                }
            }
        }
        for (p, shooter) in impact_sounds {
            ai.report_sound(SoundEvent {
                position: p,
                radius: 4.0,
                source: Some(shooter),
                kind: SoundKind::BulletImpact,
            });
        }

        if !quiet && frame % (60 * 15) == 0 {
            print_snapshot(&map, &ai, &bodies, time);
        }

        let alive = |team: u16| {
            bodies
                .iter()
                .filter(|b| b.team == team && ai.actor_state(b.id).unwrap().is_alive())
                .count()
        };
        let (a, b) = (alive(0), alive(1));
        if a == 0 || b == 0 || time >= MAX_TIME {
            break if a > b {
                Some(0)
            } else if b > a {
                Some(1)
            } else {
                None
            };
        }
    };

    print_snapshot(&map, &ai, &bodies, time);
    println!("\n=== result after {time:.1}s ===");
    match winner {
        Some(t) => println!("team {} wins", if t == 0 { "UPPER" } else { "lower" }),
        None => println!("draw"),
    }
    println!(
        "\n{:<6}{:<10}{:>8}{:>7}{:>7}{:>7}",
        "agent", "type", "health", "shots", "hits", "kills"
    );
    for b in &bodies {
        let s = ai.actor_state(b.id).unwrap();
        println!(
            "{:<6}{:<10}{:>8.0}{:>7}{:>7}{:>7}",
            b.label,
            b.archetype,
            s.health.max(0.0),
            b.shots,
            b.hits,
            b.kills
        );
    }
    let mut actions: Vec<_> = action_time.into_iter().collect();
    actions.sort_by(|a, b| b.1.total_cmp(&a.1));
    println!("\naction time share (agent-seconds):");
    for (k, t) in actions {
        println!("  {:<12}{:>8.1}", format!("{k:?}"), t);
    }
    println!(
        "\nperf: {} frames, AI tick avg {:.1} us, max {:.1} us | per frame: {:.2} thinks, {:.1} raycasts, {:.2} path queries | {} barks",
        frame,
        total_us / frame as f64,
        max_us,
        total_thinks as f64 / frame as f64,
        total_rays as f64 / frame as f64,
        total_paths as f64 / frame as f64,
        barks
    );
}

fn print_snapshot(map: &Map, ai: &AiSystem, bodies: &[Body], time: f32) {
    let mut grid: Vec<Vec<char>> = (0..map.h)
        .map(|z| {
            (0..map.w)
                .map(|x| match map.tile(x, z) {
                    map::Tile::Wall => '#',
                    map::Tile::Low => '=',
                    map::Tile::Floor => ' ',
                })
                .collect()
        })
        .collect();
    for b in bodies {
        let s = ai.actor_state(b.id).unwrap();
        let (x, z) = Map::cell_of(s.position);
        if x >= 0 && z >= 0 && x < map.w && z < map.h {
            grid[z as usize][x as usize] = if s.is_alive() { b.label } else { 'x' };
        }
    }
    println!("\n--- t = {time:.1}s ---");
    for row in grid {
        println!("{}", row.into_iter().collect::<String>());
    }
    let mut line = String::new();
    for b in bodies {
        if let Some(d) = ai.debug(b.id) {
            if ai.actor_state(b.id).unwrap().is_alive() {
                line.push_str(&format!("{}:{:?}/{:?} ", b.label, d.action, d.role));
            }
        }
    }
    println!("{line}");
    if std::env::var("ARENA_DEBUG").is_ok() {
        for b in bodies {
            if !ai.actor_state(b.id).unwrap().is_alive() {
                continue;
            }
            if let Some(d) = ai.debug(b.id) {
                let mem = ai.memory(b.id).unwrap();
                println!(
                    "  {} {:?} vis={} cover={:?} mem={:?}",
                    b.label,
                    d.scores
                        .iter()
                        .map(|(k, s)| format!("{k:?}={s:.2}"))
                        .collect::<Vec<_>>(),
                    d.target_visible,
                    d.cover,
                    mem.entries()
                        .iter()
                        .map(|e| format!(
                            "aw{:.1} vis{} age{:.1} src{:?}",
                            e.awareness,
                            e.visible,
                            time - e.last_update,
                            e.source
                        ))
                        .collect::<Vec<_>>()
                );
            }
        }
    }
}
