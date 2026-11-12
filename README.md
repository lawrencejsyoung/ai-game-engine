# ai-game-engine

A reusable, high-performance AI engine for combative NPCs in first-person shooters, written in Rust.

`combat-ai` is engine-agnostic: it doesn't own transforms, physics, animation or pathing.
The host game feeds it actor state and stimuli each frame, implements two world queries
(line of sight and, optionally, path distance), and gets back one command per agent:
where to move, where to aim, when to fire, reload, crouch or bark.

```text
crates/
  combat-ai/    the engine (library)
  arena-demo/   headless 6v6 firefight on a grid level, driven entirely by the engine
```

## Features

- **Perception**: vision cones (central + peripheral) with line of sight, a detection meter
  (so distant or peripheral targets take time to notice), instant proximity detection,
  hearing (gunshots, footsteps, explosions, bullet impacts), and damage awareness.
  Suspicious agents glance toward whatever they half-noticed.
- **Memory**: per-agent target memory with last-known position and velocity, confidence decay
  and forgetting, capped at a configurable size with relevance-based eviction.
- **Decisions: utility AI.** Each think scores actions (Engage, TakeCover, Flank, Suppress,
  Reload, Retreat, Investigate, Patrol, or your own `Custom(n)`) from normalized inputs
  passed through response curves. Hysteresis (momentum, minimum commitment) stops agents
  from dithering between actions, and an action that just failed is blocked for a moment.
- **Execution: behavior trees.** Each action runs a compact, arena-allocated tree
  (reactive selectors, sequences that resume where they left off, cooldowns, timeouts, waits).
  Trees are built once per archetype and shared, and ticking them allocates nothing.
- **Tactics**: cover selection scored against every known threat and then checked with a
  raycast; flank positions chosen relative to the squad's line of attack; attack positions
  with line of sight in the preferred range band; retreat positions. Each query scores
  candidates cheaply first and only validates a shortlist of 4 with world queries.
- **Squads**: shared target knowledge (call-outs), role assignment (assault / suppressor /
  flanker), and cover reservation so agents never stack on one spot.
- **Attack tokens**: at most N agents shoot accurately at a given player at once. The rest
  still fire, but deliberately miss (the classic fairness trick for FPS AI).
- **Aim model**: reaction time, an accuracy that settles while tracking the same target,
  penalties when the target is moving or the shooter is under fire, and burst fire.
- **Performance**: spatial hash grid rebuilt in O(n) each tick, per-think raycast budget
  with round-robin, think-rate LOD by distance to the player, staggered thinks, and a
  parallel perceive/decide phase on rayon (feature `parallel`, on by default).
  No allocation in the hot path after warm-up.
- **Deterministic**: same seed and same inputs give bit-identical commands, including in the
  multi-threaded build. Shared resources (cover, tokens) are resolved serially after the
  parallel phase, so thread scheduling can't change the outcome.

## Integration

```rust
use combat_ai::prelude::*;

struct MyWorld; // wraps your physics scene and navmesh
impl WorldQuery for MyWorld {
    fn line_of_sight(&self, from: Vec3, to: Vec3) -> bool {
        true // e.g. physics.raycast(from, to).is_none()
    }
    // Optional; the default is straight-line distance.
    fn path_distance(&self, from: Vec3, to: Vec3) -> Option<f32> {
        Some(from.distance(to)) // e.g. navmesh.path_length(from, to)
    }
}

let mut ai = AiSystem::new(SystemConfig::default());

// Level data: cover points (facing = direction the cover protects against).
ai.add_cover_point(CoverPoint::new(Vec3::new(4.0, 0.0, 9.0), Vec3::Z, CoverKind::Low));

// Combatants: players are external actors; NPCs are agents with an archetype.
let player = ai.add_actor(ActorState::new(Vec3::ZERO, TeamId(0)));
let squad = ai.create_squad();
for i in 0..4 {
    let npc = ai.spawn_agent(
        ActorState::new(Vec3::new(i as f32 * 2.0, 0.0, 30.0), TeamId(1)),
        Archetype::rifleman(),
    );
    ai.assign_squad(npc, squad);
}

// Every frame:
let world = MyWorld;
ai.set_lod_focus(&[Vec3::ZERO]);                 // player camera(s)
if let Some(p) = ai.actor_state_mut(player) {     // push host-authoritative state
    p.position = Vec3::new(0.0, 0.0, 1.0);
}
// ai.report_sound(..), ai.report_damage(..) as things happen
for cmd in ai.tick(1.0 / 60.0, &world) {
    // cmd.move_to / move_speed -> your navigation and locomotion
    // cmd.aim_dir / fire       -> your weapon (resolve hits along aim_dir)
    // cmd.reload, stance, bark, action -> animation and audio
}
```

The host stays authoritative. Each frame it writes position, velocity, facing, stance,
health and ammo through `actor_state_mut`. The AI only reads that state. Gunshot sounds
are emitted automatically when agents fire (`SystemConfig::auto_gunshot_radius`). Report
player gunfire, footsteps and damage yourself.

### Custom archetypes

```rust
use combat_ai::prelude::*;

let mut berserker = (*Archetype::assault()).clone();
berserker.config.courage = 1.0;
berserker.actions.push(ActionDef::new(
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
let berserker = berserker.shared(); // Arc<Archetype>, pass to spawn_agent
```

Presets: `Archetype::rifleman()`, `assault()` (close range, aggressive) and `sniper()`
(long range, cautious). All of them use `Archetype::standard_actions()`.

### Debugging

`ai.debug(id)` returns an agent's current action, every action's utility score, target,
visibility, squad role, claimed cover, destination, LOD think interval and token. `ai.memory(id)`
exposes what it remembers. `ai.stats()` gives per-tick counters (thinks, raycasts,
path queries, microseconds).

## Architecture

Per tick (`AiSystem::tick`):

1. **Spatial**: rebuild the actor hash grid (counting sort, O(n)). Rebuild the cover grid
   only when cover changes. Expire old sounds.
2. **Squads** (serial, cheap): merge members' confirmed targets, pick the squad's primary
   target, and periodically reassign roles.
3. **Agents** (parallel): each agent that is due to think (LOD and stagger) runs
   perceive → select target → compute inputs → utility select → tick behavior tree.
   Every agent then runs its per-frame aim/fire step against the persistent intent the
   tree produced. Everything shared is read-only in this phase.
4. **Resolve** (serial, deterministic): cover-claim conflicts (earliest claimant wins) and
   attack tokens (current holders first, then nearest).
5. **Emit** one `AgentCommand` per live agent, plus automatic gunshot sounds.

| Module | Responsibility |
| --- | --- |
| `system` | storage (generational ids, dense brain array), pipeline, squads, resolution |
| `perception` | vision, hearing, damage, squad call-outs |
| `memory` | target memory, decay, capped relevance-based eviction |
| `utility` | inputs, response curves, considerations, scoring with hysteresis |
| `bt` | behavior-tree DSL, flat compiled trees, runtime |
| `tasks` | behavior-tree leaves executed against an agent's brain |
| `tactics` | cover / flank / attack / retreat / patrol position queries |
| `aim` | aim error model, bursts, command output |
| `spatial` | uniform XZ hash grid |
| `archetype` | presets and the standard action set |

## Performance

`cargo bench -p combat-ai` runs the full pipeline: two teams in sustained close combat over
a field of pillars with real (analytic) raycasts. Times are per tick and include the host's
command application. Measured on a 4-core cloud VM:

| Agents | Time per tick |
| ---: | ---: |
| 100 | ~0.09 ms |
| 500 | ~0.55 ms |
| 2000 | ~2.5 ms |

This benchmark is close to a worst case. Every agent is in a dense, active fight, so LOD
never kicks in (engaged agents always think at full rate). `cargo run -p combat-ai --release
--example stress -- 2000` prints live counters for the same scenario.

## Arena demo

```bash
cargo run -p arena-demo --release
```

```bash
cargo run -p arena-demo --release -- --seed 3 --per-team 6 --quiet --swap-sides
```

A 90 m × 39 m grid level with walls and low walls. Cover points are generated automatically
from the geometry, A* is used for pathing, and weapons are hitscan. An explosion at the
center draws both teams in. The demo prints a live log of barks and kills, ASCII
snapshots, and a final scoreboard with time spent per action and AI cost. Set `ARENA_DEBUG=1`
to also print each survivor's utility scores and memory in the snapshots.

## Development

```bash
cargo test --workspace
```

```bash
cargo test -p combat-ai --no-default-features
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
cargo bench -p combat-ai
```
