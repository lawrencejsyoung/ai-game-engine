# Changelog

## Unreleased

### Added
- `combat-ai-ffi`: C ABI with a plain-C header (`include/combat_ai.h`), static and
  dynamic libraries, ABI size check (`CAI_ABI_CHECK()`), panic-safe entry points with
  `cai_last_error()`, a C example host and a C++ RAII example, both built and run by
  `cargo test`.
- Cover fire: agents in cover pop up and return fire, then duck again (`Task::Watch`).
- Stand-off breaking: `AgentConfig::patience` and `Input::Impatience`. Agents that hold
  a target without trading damage grow more aggressive and courageous over time.
- Hunting: `AgentConfig::hunt_time`. After losing every target, agents patrol around
  the last contact instead of going home.
- `AiSystem::reveal(target, team)` for radar pings, alarms and director logic.
- Aim tuning: `AgentConfig::{aim_unsettled_penalty, aim_moving_target_penalty,
  aim_moving_self_penalty, aim_under_fire_penalty, min_hit_chance, suppression_rate}`,
  `AgentConfig::{spread_deg, hit_chance, effective_range}`,
  `SystemConfig::{accuracy_scale, target_radius}`, and the `aim_table` example.
- `AgentCommand::aim_target`, `AgentDebug::impatience`.
- `ActorId::{INVALID, generation, to_bits, from_bits}`, `SquadId::{index, from_index}`,
  `CoverId::from_index`, `math::mix64`.
- Arena demo: `--batch N` statistics (win split by team and side, time-outs, aimed and
  suppressive hit rates) and `--check` for CI.
- CI workflow, license files.

### Changed
- Aim defaults retuned (rifleman 1.1°, assault 2.2°). Agents no longer take aimed shots
  beyond their effective range; they reposition instead.
- Suppressive fire is rate-limited (2 rounds/s by default) and range-limited. The
  suppressor role goes to agents suited for it (high rate of fire, large magazine).
- Retreat fades once the threat has been out of sight for a while.
- Agents aim at the head when only the head is exposed.
- Arena demo: the map is mirror-symmetric, spawn columns are mirrored, A* tie-breaking
  is mirror-invariant, hits resolve against a capsule, and characters make footsteps.

### Fixed
- Agent RNG streams are hashed from the seed and the handle. Agent 0 previously shared
  the exact stream of a host `Rng::new(seed)`. In the arena demo this made the
  second-spawned team win about 52.5% of matches.
- Sounds reported between ticks (including automatic gunshots) were missed by agents
  that had perceived on the tick the sound was stamped with.
- `tick` with a non-finite or negative `dt` no longer corrupts simulation time.
- Arena demo: about 1 in 5 matches ended at the 240 s cap with cautious survivors hiding
  from each other. It is now about 0.1%. The left spawn side won about 61% of matches
  because of map asymmetry; sides are now even.
