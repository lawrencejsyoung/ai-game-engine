# ai-game-engine

A reusable, high-performance AI engine for combative NPCs in first-person shooters, written in Rust.

`combat-ai` is engine-agnostic: it doesn't own transforms, physics, animation or pathing.
The host game feeds it actor state and stimuli each frame, implements two world queries
(line of sight and, optionally, path distance), and gets back one command per agent:
where to move, where to aim, when to fire, reload, crouch or bark.

```text
crates/
  combat-ai/      the engine (Rust library)
  combat-ai-ffi/  C ABI + C header for C/C++ engines (Unreal, Unity native plugins, custom)
  arena-demo/     headless 6v6 firefight on a grid level, driven entirely by the engine
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
- **Cover fire**: agents in cover duck, top up their magazine, then pop up and return fire
  before ducking again.
- **Stand-off breaking**: an agent that has a target but trades no damage for
  `patience` seconds grows impatient, which pushes its effective aggression and courage
  up until it goes and finds a fight. Agents that lose every target hunt around the last
  contact (`hunt_time`) rather than walking back to their spawn. Hosts can also
  `reveal` a target to a team (radar ping, alarm, director).
- **Squads**: shared target knowledge (call-outs), role assignment (assault / suppressor /
  flanker), and cover reservation so agents never stack on one spot.
- **Attack tokens**: at most N agents shoot accurately at a given player at once. The rest
  still fire, but deliberately miss (the classic fairness trick for FPS AI).
- **Aim model**: reaction time, an accuracy that settles while tracking the same target,
  penalties when the target or the shooter is moving or the shooter is under fire, and
  burst fire. Agents aim at whatever part of the body is exposed, hold aimed fire beyond
  their effective range (and close in instead), and rate-limit suppressive fire. Every
  knob is on `AgentConfig`, with a global `SystemConfig::accuracy_scale` for difficulty.
  See [Tuning aim](#tuning-aim).
- **Performance**: spatial hash grid rebuilt in O(n) each tick, per-think raycast budget
  with round-robin, think-rate LOD by distance to the player, staggered thinks, and a
  parallel perceive/decide phase on rayon (feature `parallel`, on by default).
  No allocation in the hot path after warm-up.
- **Deterministic**: same seed and same inputs give bit-identical commands, including in the
  multi-threaded build. Shared resources (cover, tokens) are resolved serially after the
  parallel phase, so thread scheduling can't change the outcome. Each agent's random
  stream is derived by hashing the seed with its handle, so it is independent of any RNG
  the host seeds with the same value.
- **C/C++ interface**: `combat-ai-ffi` builds a static and a dynamic library with a
  plain-C header. See [C and C++](#c-and-c).

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
player gunfire, footsteps and damage yourself. Report damage with the `attacker` set:
it's how agents learn who shot them, and landing a hit also resets the attacker's
impatience.

`AgentCommand::aim_target` names the actor being aimed at (`None` while firing means
suppressive fire at a position). Resolve the shot along `aim_dir`, which already includes
the aim error.

`tick` treats a non-finite or negative `dt` as `0`. Simulation time is an `f32`: its
resolution stays below 1 ms for the first ~2 hours (2^13 s) of continuous simulation and
degrades after that, so recreate the system per match or map rather than running one
instance for days.

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

### Tuning aim

Shots land uniformly inside a cone of half-angle

```text
spread = accuracy_deg * (1 + aim_unsettled_penalty * (1 - settle))
       + target_speed * aim_moving_target_penalty
       + own_speed    * aim_moving_self_penalty
       + under_fire   * aim_under_fire_penalty          (then * accuracy_scale)
```

where `settle` goes from 0 to 1 over `aim_settle_time` of tracking the same target. So the
chance of hitting a target of radius `r` at distance `d` is about `(r / (d tan spread))^2`,
which is `AgentConfig::hit_chance`. Agents take aimed shots only inside
`AgentConfig::effective_range` (where a calm, settled shot still has `min_hit_chance`).
Beyond that they reposition. Attack tokens add `no_token_spread_deg` for agents shooting
at a player without a token, which is how you make a crowd of NPCs fair to fight.

To tune, start from your game's hitbox radius (`SystemConfig::target_radius`) and print
the table for each preset:

```bash
cargo run -p combat-ai --example aim_table -- 0.3
```

Then change `accuracy_deg` (base skill), the penalties (how much movement and pressure
matter), and `accuracy_scale` (difficulty). In the arena demo the presets land 37–47% of
aimed shots (see [Arena demo](#arena-demo)); against a human player, the attack-token
penalty brings this down for everyone but the token holders.

### Debugging

`ai.debug(id)` returns an agent's current action, every action's utility score, target,
visibility, squad role, claimed cover, destination, LOD think interval, token and
impatience. `ai.memory(id)`
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
command application. Measured on a 4-core cloud VM (these VMs vary by ±20% from run to
run):

| Agents | Time per tick |
| ---: | ---: |
| 100 | ~0.13 ms |
| 500 | ~0.7 ms |
| 2000 | ~2.7 ms |

This benchmark is close to a worst case. Every agent is in a dense, active fight, so LOD
never kicks in (engaged agents always think at full rate). `cargo run -p combat-ai --release
--example stress -- 2000` prints live counters for the same scenario.

## C and C++

`crates/combat-ai-ffi` exposes the engine through a C ABI. The header is
[`crates/combat-ai-ffi/include/combat_ai.h`](crates/combat-ai-ffi/include/combat_ai.h).

```bash
cargo build -p combat-ai-ffi --release
```

This produces `target/release/libcombat_ai_ffi.a` (static) and `libcombat_ai_ffi.so`
(`.dylib` on macOS; `combat_ai_ffi.lib` / `.dll` on Windows). When linking the static
library, add the platform libraries that Rust's standard library needs. To list them
exactly for your target, run:

```bash
cargo rustc -p combat-ai-ffi --release --crate-type staticlib -- --print native-static-libs
```

On Linux the list is `-lpthread -ldl -lm`.

```c
#include "combat_ai.h"

static bool los(void *physics, CaiVec3 from, CaiVec3 to) { /* raycast */ return true; }

if (!CAI_ABI_CHECK()) abort();                       /* header matches the library */
CaiSystem *ai = cai_system_new(NULL);                /* NULL = default config */
CaiArchetype *rifleman = cai_archetype_preset(CAI_PRESET_RIFLEMAN);
CaiActorState s;
cai_actor_state_default((CaiVec3){0, 0, 30}, /*team*/ 1, &s);
CaiActorId npc = cai_spawn_agent(ai, &s, rifleman);

CaiWorld world = {my_physics, los, NULL};            /* NULL path_distance = straight line */
const CaiAgentCommand *cmds; size_t n;
cai_tick(ai, dt, &world, &cmds, &n);                 /* every frame */
for (size_t i = 0; i < n; i++) { /* cmds[i].move_to, aim_dir, flags & CAI_CMD_FIRE, ... */ }
```

- All structs are plain data with fixed-width fields. Enums are integer `CAI_*` constants.
- Every entry point checks for null pointers and catches panics. Failures come back as
  `CAI_ERR_*`, `NULL` or `CAI_INVALID_ACTOR`, with the reason in `cai_last_error()`.
- Use a `CaiSystem` from one thread at a time. World callbacks may run concurrently on
  worker threads once the agent count reaches `parallel_threshold`. Set it to `UINT32_MAX`
  to keep them on the calling thread. Callbacks must not throw.
- The engine is +Y up, in meters. For Unreal (Z-up, centimeters), swap Y and Z and scale
  by 0.01 at the boundary.
- Custom actions and behavior trees are only available from Rust. From C you can tune any
  `CaiAgentConfig` field of the standard action set (`cai_archetype_new`).

[`examples/c/firefight.c`](crates/combat-ai-ffi/examples/c/firefight.c) is a complete host
loop, and [`examples/cpp/smoke.cpp`](crates/combat-ai-ffi/examples/cpp/smoke.cpp) shows RAII
ownership in C++. `cargo test -p combat-ai-ffi` compiles and runs both against the static
library when a C/C++ compiler is available.

## Arena demo

```bash
cargo run -p arena-demo --release
```

```bash
cargo run -p arena-demo --release -- --seed 3 --per-team 6 --quiet --swap-sides
```

```bash
cargo run -p arena-demo --release -- --batch 1000 --seed 1 --check
```

A 90 m × 39 m grid level with walls and low walls, mirror-symmetric so neither side has an
advantage. Cover points are generated automatically
from the geometry, A* is used for pathing, and weapons are hitscan. An explosion at the
center draws both teams in. The demo prints a live log of barks and kills, ASCII
snapshots, and a final scoreboard with time spent per action and AI cost. Set `ARENA_DEBUG=1`
to also print each survivor's utility scores and memory in the snapshots.

`--batch N` plays N matches (alternating sides) and prints the win split by team and by
side, how many matches hit the 240 s cap, and hit rates per archetype. The hit rates are
split into aimed shots and suppressive fire. `--check` exits non-zero if more than 2% of
matches time out or either team or side wins significantly more than half; CI runs it.
A typical run of 1000 matches:

```text
1000 matches: UPPER 517 / lower 480 / draw 3 | 2 hit the 240s cap (0%) | avg length 64.5s

type          shots    hit %     aimed %  suppress %
assault      113767    25.4%       36.6%       31.1%
rifleman     169436    27.4%       40.6%       32.9%
sniper         8580    46.9%       46.9%        0.0%
overall      291783    27.2%       39.3%       31.2%
```

Shots resolve against a capsule hitbox (0.3 m radius, shins to the top of the head), and
running or sprinting characters make footstep sounds.

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

```bash
cargo test -p combat-ai-ffi
```

The minimum supported Rust version is 1.82 for the libraries and the demo; the benchmarks'
dev-dependencies need a newer toolchain. CI (`.github/workflows/ci.yml`) runs formatting, clippy, the tests with and without the
`parallel` feature, the C/C++ examples, docs with warnings denied, and the arena
balance check.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE), at your option.
