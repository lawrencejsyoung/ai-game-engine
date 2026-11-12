//! Headless two-team firefight driven entirely by `combat-ai`.
//!
//! The "game" here is deliberately simple (grid level, hitscan weapons, no
//! physics) — it plays the role of the host engine: it owns transforms,
//! health, ammo and pathing, feeds state/stimuli to the AI, and applies the
//! commands it gets back.
//!
//! Usage: `cargo run -p arena-demo --release -- [--seed N] [--per-team N] [--quiet] [--swap-sides] [--batch N [--check]]`
//!
//! `--batch N` plays N matches (seeds `seed..seed+N`, alternating sides) and
//! prints aggregate statistics instead of a single match log. `--check` makes
//! the process fail if too many matches time out or either team/side wins
//! significantly more than half (used in CI).

mod map;

use combat_ai::prelude::*;
use combat_ai::ActionKind;
use map::{Map, CELL, LAYOUT};
use std::collections::HashMap;
use std::sync::Arc;

const DT: f32 = 1.0 / 60.0;
const MAX_TIME: f32 = 240.0;
/// Radius of the body capsule used for hit tests.
const HITBOX_RADIUS: f32 = 0.3;

/// Closest approach between the ray `origin + t * dir` (`t >= 0`, `dir`
/// normalized) and the segment `a..b`. Returns `(t, distance)`, or `None` if
/// the closest point is behind the origin.
fn ray_segment(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3) -> Option<(f32, f32)> {
    let u = b - a;
    let w = origin - a;
    let (uu, ud) = (u.dot(u), u.dot(dir));
    let (uw, dw) = (u.dot(w), dir.dot(w));
    let denom = uu - ud * ud;
    // Parameter along the segment, clamped to it.
    let mut s = if denom > 1e-6 {
        (uw - ud * dw) / denom
    } else {
        0.0
    };
    s = s.clamp(0.0, 1.0);
    let p = a + u * s;
    let t = (p - origin).dot(dir);
    if t <= 0.0 {
        return None;
    }
    // Re-project onto the segment for the final distance.
    let q = origin + dir * t;
    let s2 = ((q - a).dot(u) / uu.max(1e-6)).clamp(0.0, 1.0);
    Some((t, q.distance(a + u * s2)))
}

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
    /// Shots fired at a tracked, visible target (excludes suppressive fire).
    aimed_shots: u32,
    aimed_hits: u32,
    kills: u32,
}

fn arg<T: std::str::FromStr>(args: &[String], name: &str, default: T) -> T {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Shot counters.
#[derive(Copy, Clone, Default)]
struct Accuracy {
    shots: u32,
    hits: u32,
    aimed_shots: u32,
    aimed_hits: u32,
}

impl Accuracy {
    fn add(&mut self, o: Accuracy) {
        self.shots += o.shots;
        self.hits += o.hits;
        self.aimed_shots += o.aimed_shots;
        self.aimed_hits += o.aimed_hits;
    }
}

fn pct(n: u32, d: u32) -> f32 {
    100.0 * n as f32 / d.max(1) as f32
}

/// Outcome of one match.
struct MatchResult {
    /// Winning team (0 = UPPER, 1 = lower), `None` for a draw.
    winner: Option<u16>,
    /// Match length in seconds.
    time: f32,
    /// Ended because the time cap was reached.
    timed_out: bool,
    /// Per archetype: shot counts.
    accuracy: HashMap<&'static str, Accuracy>,
    /// Survivors per team at the end.
    alive: [usize; 2],
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seed: u64 = arg(&args, "--seed", 7);
    let per_team: usize = arg(&args, "--per-team", 6);
    let quiet = args.iter().any(|a| a == "--quiet");
    let swap = args.iter().any(|a| a == "--swap-sides");
    let batch: u64 = arg(&args, "--batch", 0);
    let check = args.iter().any(|a| a == "--check");

    if batch == 0 {
        run_match(
            seed,
            per_team,
            swap,
            if quiet {
                Verbosity::Summary
            } else {
                Verbosity::Full
            },
        );
        return;
    }

    let mut wins = [0u32; 2];
    // Wins by starting side (0 = left, 1 = right), to catch map/spawn bias.
    let mut side_wins = [0u32; 2];
    let mut draws = 0;
    let mut timeouts = 0;
    let mut total_time = 0.0;
    let mut acc: HashMap<&'static str, Accuracy> = HashMap::new();
    for i in 0..batch {
        let s = seed + i;
        let sw = swap ^ (i % 2 == 1);
        let r = run_match(s, per_team, sw, Verbosity::Silent);
        match r.winner {
            Some(t) => {
                wins[t as usize] += 1;
                // Team 0 starts on the left unless sides are swapped.
                side_wins[((t == 0) == sw) as usize] += 1;
            }
            None => draws += 1,
        }
        timeouts += r.timed_out as u32;
        total_time += r.time;
        for (k, a) in r.accuracy {
            acc.entry(k).or_default().add(a);
        }
        println!(
            "seed {s:>4} {:<5} {:>6.1}s  {:<10} survivors {}v{}",
            if sw { "swap" } else { "" },
            r.time,
            match (r.winner, r.timed_out) {
                (_, true) => "TIMEOUT".to_string(),
                (Some(0), _) => "UPPER".to_string(),
                (Some(_), _) => "lower".to_string(),
                (None, _) => "draw".to_string(),
            },
            r.alive[0],
            r.alive[1]
        );
    }
    println!(
        "\n{batch} matches: UPPER {} / lower {} / draw {draws} | {timeouts} hit the {MAX_TIME:.0}s cap ({:.0}%) | avg length {:.1}s",
        wins[0],
        wins[1],
        100.0 * timeouts as f32 / batch as f32,
        total_time / batch as f32
    );
    println!("by side: left {} / right {}", side_wins[0], side_wins[1]);
    let mut acc: Vec<_> = acc.into_iter().collect();
    acc.sort_by_key(|a| a.0);
    let mut total = Accuracy::default();
    println!(
        "\n{:<10}{:>9}{:>9}{:>12}{:>12}",
        "type", "shots", "hit %", "aimed %", "suppress %"
    );
    for (k, a) in &acc {
        total.add(*a);
        print_accuracy(k, a);
    }
    print_accuracy("overall", &total);

    if check {
        // Regression gate for CI: few timeouts, and neither team nor side
        // wins significantly more than half (3 sigma of a fair coin).
        let sigma = |a: u32, b: u32| {
            let n = (a + b).max(1) as f32;
            (a as f32 - n / 2.0).abs() / (n.sqrt() / 2.0)
        };
        let mut failures = Vec::new();
        if timeouts as f32 > batch as f32 * 0.02 {
            failures.push(format!("{timeouts} of {batch} matches hit the time cap"));
        }
        if sigma(wins[0], wins[1]) > 3.0 {
            failures.push(format!("team imbalance {} vs {}", wins[0], wins[1]));
        }
        if sigma(side_wins[0], side_wins[1]) > 3.0 {
            failures.push(format!(
                "side imbalance {} vs {}",
                side_wins[0], side_wins[1]
            ));
        }
        if failures.is_empty() {
            println!("check passed");
        } else {
            for f in &failures {
                eprintln!("check failed: {f}");
            }
            std::process::exit(1);
        }
    }
}

fn print_accuracy(name: &str, a: &Accuracy) {
    println!(
        "{:<10}{:>9}{:>8.1}%{:>11.1}%{:>11.1}%",
        name,
        a.shots,
        pct(a.hits, a.shots),
        pct(a.aimed_hits, a.aimed_shots),
        pct(a.shots - a.aimed_shots, a.shots)
    );
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Verbosity {
    Silent,
    Summary,
    Full,
}

fn run_match(seed: u64, per_team: usize, swap: bool, verbosity: Verbosity) -> MatchResult {
    let quiet = verbosity < Verbosity::Full;
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
    // Spawn cells are drawn first, in a fixed order, so the order agents are
    // registered in can't change where anyone starts.
    let mut cells = [Vec::new(), Vec::new()];
    for (team, cells) in cells.iter_mut().enumerate() {
        let left = (team == 0) != swap;
        for _ in 0..per_team {
            loop {
                // Column 3..=6 from the team's own wall, mirrored for the right side.
                let col = 3 + (rng.f32() * 4.0) as i32;
                let cx = if left { col } else { map.w - 1 - col };
                let cz = 2 + (rng.f32() * (map.h - 4) as f32) as i32;
                if map.walkable(cx, cz) {
                    cells.push((cx, cz));
                    break;
                }
            }
        }
    }
    for team in 0..2u16 {
        let squads = [ai.create_squad(), ai.create_squad()];
        for i in 0..per_team {
            let (name, arch, w) = &roster[pick(i)];
            let left = (team == 0) != swap;
            let (cx, cz) = cells[team as usize][i];
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
                aimed_shots: 0,
                aimed_hits: 0,
                kills: 0,
            });
        }
    }
    let teams: Vec<(ActorId, u16)> = bodies.iter().map(|b| (b.id, b.team)).collect();
    let index_of: HashMap<ActorId, usize> =
        bodies.iter().enumerate().map(|(i, b)| (b.id, i)).collect();

    if verbosity > Verbosity::Silent {
        println!(
            "arena: {}x{} m, {} cover points, {} agents per team, seed {seed}",
            map.w as f32 * CELL,
            map.h as f32 * CELL,
            ai.cover_points().len(),
            per_team
        );
    }

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
        let mut footsteps = Vec::new();
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
                    // Body hitbox: a capsule from the shins to the top of the head.
                    let base = o.position + Vec3::Y * 0.15;
                    let top = o.eye() + Vec3::Y * 0.15;
                    let Some((t, miss)) = ray_segment(eye, cmd.aim_dir, base, top) else {
                        continue;
                    };
                    if t > 150.0 {
                        continue;
                    }
                    let point = eye + cmd.aim_dir * t;
                    if miss < HITBOX_RADIUS && map.line_of_sight(eye, point) {
                        if best.is_none_or(|(bt, _)| t < bt) {
                            best = Some((t, oid));
                        }
                    } else if miss < 2.5 && near_miss.is_none_or(|(m, _, _)| miss < m) {
                        near_miss = Some((miss, point, oid));
                    }
                }
                if cmd.aim_target.is_some() {
                    body.aimed_shots += 1;
                    body.aimed_hits += best.is_some() as u32;
                }
                if let Some((_, victim)) = best {
                    body.hits += 1;
                    damage_events.push((victim, cmd.actor, body.weapon.damage, eye));
                } else if let Some((_, p, _)) = near_miss {
                    impact_sounds.push((p, cmd.actor));
                }
            }

            // Footsteps: running and sprinting are audible, walking is quiet.
            let step_radius = match cmd.move_speed {
                _ if vel == Vec3::ZERO => 0.0,
                MoveSpeed::Walk => 0.0,
                MoveSpeed::Run => 10.0,
                MoveSpeed::Sprint => 16.0,
            };
            if step_radius > 0.0 && frame % 20 == bi as u64 % 20 {
                footsteps.push((pos, cmd.actor, step_radius));
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
        for (position, source, radius) in footsteps {
            ai.report_sound(SoundEvent {
                position,
                radius,
                source: Some(source),
                kind: SoundKind::Movement,
            });
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

    let alive = |team: u16| {
        bodies
            .iter()
            .filter(|b| b.team == team && ai.actor_state(b.id).unwrap().is_alive())
            .count()
    };
    let mut accuracy: HashMap<&'static str, Accuracy> = HashMap::new();
    for b in &bodies {
        accuracy.entry(b.archetype).or_default().add(Accuracy {
            shots: b.shots,
            hits: b.hits,
            aimed_shots: b.aimed_shots,
            aimed_hits: b.aimed_hits,
        });
    }
    let result = MatchResult {
        winner,
        time,
        timed_out: time >= MAX_TIME,
        accuracy,
        alive: [alive(0), alive(1)],
    };
    if verbosity == Verbosity::Silent {
        return result;
    }

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
    let mut acc: Vec<_> = result.accuracy.iter().collect();
    acc.sort_by_key(|a| *a.0);
    println!(
        "\n{:<10}{:>9}{:>9}{:>12}{:>12}",
        "type", "shots", "hit %", "aimed %", "suppress %"
    );
    for (k, a) in acc {
        print_accuracy(k, a);
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
    result
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
