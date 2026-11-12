//! Prints the aim model's expected hit chance per preset, by distance and
//! condition, plus each preset's effective range. Use it to tune
//! `AgentConfig` aim fields (and `SystemConfig::accuracy_scale`) for your
//! game's hitbox size before play-testing.
//!
//! `cargo run -p combat-ai --example aim_table -- [hitbox_radius_m] [accuracy_scale]`

use combat_ai::{AgentConfig, Archetype};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let radius: f32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0.3);
    let scale: f32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1.0);
    let distances = [5.0, 10.0, 20.0, 30.0, 50.0, 80.0];
    println!("hitbox radius {radius} m, accuracy_scale {scale}\n");
    for a in [
        Archetype::rifleman(),
        Archetype::assault(),
        Archetype::sniper(),
    ] {
        let c = &a.config;
        println!(
            "{} (accuracy {:.2} deg, effective range {:.0} m)",
            a.name,
            c.accuracy_deg,
            c.effective_range(radius) / scale
        );
        print!("  {:<34}", "condition \\ distance (m)");
        for d in distances {
            print!("{d:>7.0}");
        }
        println!();
        let rows: [(&str, f32, f32, f32, f32); 4] = [
            ("settled, both still, calm", 1.0, 0.0, 0.0, 0.0),
            ("fresh target, both still", 0.0, 0.0, 0.0, 0.0),
            ("settled, target running 4.5 m/s", 1.0, 4.5, 0.0, 0.0),
            ("settled, both running, under fire", 1.0, 4.5, 4.5, 1.0),
        ];
        for (label, settle, ts, os, uf) in rows {
            let spread = c.spread_deg(settle, ts, os, uf) * scale;
            print!("  {label:<34}");
            for d in distances {
                print!(
                    "{:>6.0}%",
                    100.0 * AgentConfig::hit_chance(spread, d, radius)
                );
            }
            println!();
        }
        println!();
    }
}
