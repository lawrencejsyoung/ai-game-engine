/*
 * combat_ai.h - C ABI for the combat-ai NPC combat engine.
 *
 * Link against libcombat_ai_ffi (static: libcombat_ai_ffi.a / combat_ai_ffi.lib,
 * dynamic: libcombat_ai_ffi.so / .dylib / combat_ai_ffi.dll). The static library
 * also needs the platform's system libraries (Linux: -lpthread -ldl -lm;
 * Windows: ws2_32 userenv ntdll bcrypt advapi32; macOS: nothing extra).
 *
 * Conventions
 * - All structs are plain data with fixed-width fields. Enum-like values are
 *   integers with CAI_* constants (never C enums, whose size varies).
 * - CaiSystem and CaiArchetype are opaque; release them with their _free
 *   function. Freeing an archetype after spawning agents from it is fine.
 * - CaiActorId is a 64-bit generational handle. CAI_INVALID_ACTOR never refers
 *   to a live actor and stale handles are inert (calls just return false).
 * - Functions returning int32_t return CAI_OK (0) or a negative CAI_ERR_*.
 *   Functions returning handles return null / CAI_INVALID_* on failure.
 *   cai_last_error() describes the most recent failure on the calling thread.
 * - No exception or panic crosses the boundary in either direction: the
 *   library catches its own panics, and your callbacks must not throw.
 *
 * Threading
 * - Use a CaiSystem from one thread at a time.
 * - During cai_tick, world callbacks can run concurrently on worker threads
 *   when the agent count reaches CaiSystemConfig.parallel_threshold. Make them
 *   thread-safe (read-only physics queries usually are), or set the threshold
 *   to UINT32_MAX to keep every callback on the calling thread.
 *
 * Call CAI_ABI_CHECK() once at startup: it returns false if this header does
 * not match the library you linked.
 */
#ifndef COMBAT_AI_H
#define COMBAT_AI_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define CAI_ABI_VERSION 1u

typedef uint64_t CaiActorId;
#define CAI_INVALID_ACTOR ((CaiActorId)UINT64_MAX)
#define CAI_INVALID_INDEX UINT32_MAX

/* Results */
#define CAI_OK 0
#define CAI_ERR_NULL (-1)
#define CAI_ERR_INVALID (-2)
#define CAI_ERR_PANIC (-3)

/* Archetype presets */
#define CAI_PRESET_RIFLEMAN 0u
#define CAI_PRESET_ASSAULT 1u
#define CAI_PRESET_SNIPER 2u

/* Stance */
#define CAI_STANCE_STAND 0u
#define CAI_STANCE_CROUCH 1u

/* Cover kinds */
#define CAI_COVER_LOW 0u
#define CAI_COVER_HIGH 1u

/* Sound kinds */
#define CAI_SOUND_GUNSHOT 0u
#define CAI_SOUND_MOVEMENT 1u
#define CAI_SOUND_EXPLOSION 2u
#define CAI_SOUND_BULLET_IMPACT 3u

/* CaiAgentCommand.flags */
#define CAI_CMD_MOVE 1u
#define CAI_CMD_HAS_AIM_POINT 2u
#define CAI_CMD_FIRE 4u
#define CAI_CMD_RELOAD 8u

/* Move speeds */
#define CAI_SPEED_WALK 0u
#define CAI_SPEED_RUN 1u
#define CAI_SPEED_SPRINT 2u

/* Barks */
#define CAI_BARK_NONE 0u
#define CAI_BARK_ENEMY_SPOTTED 1u
#define CAI_BARK_MOVING_TO_COVER 2u
#define CAI_BARK_RELOADING 3u
#define CAI_BARK_FLANKING 4u
#define CAI_BARK_SUPPRESSING 5u
#define CAI_BARK_RETREATING 6u
#define CAI_BARK_SEARCHING 7u

/* Actions (CAI_ACTION_CUSTOM carries its number in custom_action) */
#define CAI_ACTION_IDLE 0u
#define CAI_ACTION_PATROL 1u
#define CAI_ACTION_INVESTIGATE 2u
#define CAI_ACTION_ENGAGE 3u
#define CAI_ACTION_TAKE_COVER 4u
#define CAI_ACTION_FLANK 5u
#define CAI_ACTION_SUPPRESS 6u
#define CAI_ACTION_RELOAD 7u
#define CAI_ACTION_RETREAT 8u
#define CAI_ACTION_CUSTOM 255u

/* Squad roles */
#define CAI_ROLE_ASSAULT 0u
#define CAI_ROLE_SUPPRESSOR 1u
#define CAI_ROLE_FLANKER 2u

/* 3D vector, +Y up. Convert from your engine's convention at the boundary
 * (Unreal is Z-up, centimeters: swap Y/Z and scale by 0.01). */
typedef struct CaiVec3 {
    float x, y, z;
} CaiVec3;

/* Global configuration (see SystemConfig in the Rust docs). */
typedef struct CaiSystemConfig {
    uint64_t seed;
    float grid_cell_size;
    uint32_t max_attackers_per_player; /* 0 disables attack tokens */
    float no_token_spread_deg;
    float accuracy_scale;              /* difficulty: <1 deadlier, >1 sloppier */
    float target_radius;               /* hitbox radius used for hit-chance estimates */
    float lod_full_distance;
    float lod_medium_distance;
    float auto_gunshot_radius;         /* 0 disables automatic gunshot sounds */
    float sound_retention;
    float role_interval;
    uint32_t parallel_threshold;       /* UINT32_MAX = always tick on the calling thread */
    uint32_t _reserved;
} CaiSystemConfig;

/* Per-archetype tuning (see AgentConfig in the Rust docs for each field). */
typedef struct CaiAgentConfig {
    /* perception */
    float fov_deg;
    float peripheral_fov_deg;
    float view_range;
    float proximity_range;
    float detection_rate;
    float hearing;
    float memory_span;
    uint32_t max_los_checks;
    uint32_t max_tracked_targets;
    /* combat */
    float preferred_range_min;
    float preferred_range_max;
    float reaction_time;
    float accuracy_deg;
    float aim_settle_time;
    float aim_unsettled_penalty;
    float aim_moving_target_penalty;
    float aim_moving_self_penalty;
    float aim_under_fire_penalty;
    float min_hit_chance;
    float fire_interval;
    uint32_t burst_length;
    float burst_pause_min;
    float burst_pause_max;
    float suppression_rate;
    float aggression;
    float courage;
    float patience;
    /* tactics */
    float tactical_search_radius;
    float patrol_radius;
    float arrive_radius;
    float think_interval;
    float hunt_time;
} CaiAgentConfig;

/* Host-owned physical state of an actor. Write it every frame. */
typedef struct CaiActorState {
    CaiVec3 position; /* feet */
    CaiVec3 velocity;
    CaiVec3 forward;
    float health;
    float max_health;
    float eye_height;
    float crouch_eye_height;
    uint16_t ammo;
    uint16_t mag_size;
    uint16_t team;  /* different teams are hostile */
    uint8_t stance; /* CAI_STANCE_* */
    uint8_t _reserved;
} CaiActorState;

typedef struct CaiCoverPoint {
    CaiVec3 position;
    CaiVec3 facing; /* horizontal direction toward the obstacle */
    uint32_t kind;  /* CAI_COVER_* */
} CaiCoverPoint;

typedef struct CaiSoundEvent {
    CaiActorId source; /* or CAI_INVALID_ACTOR */
    CaiVec3 position;
    float radius;
    uint32_t kind; /* CAI_SOUND_* */
    uint32_t _reserved;
} CaiSoundEvent;

typedef struct CaiDamageEvent {
    CaiActorId victim;
    CaiActorId attacker; /* or CAI_INVALID_ACTOR */
    CaiVec3 origin;
    float amount;
} CaiDamageEvent;

/* What one agent wants its character to do this frame. */
typedef struct CaiAgentCommand {
    CaiActorId actor;
    CaiActorId aim_target; /* or CAI_INVALID_ACTOR (suppressive fire / not aiming) */
    CaiVec3 move_to;       /* valid if flags & CAI_CMD_MOVE */
    CaiVec3 aim_dir;       /* normalized, includes aim error: resolve shots along it */
    CaiVec3 aim_point;     /* valid if flags & CAI_CMD_HAS_AIM_POINT */
    uint32_t flags;        /* CAI_CMD_* */
    uint8_t move_speed;    /* CAI_SPEED_* */
    uint8_t stance;        /* CAI_STANCE_* */
    uint8_t bark;          /* CAI_BARK_* */
    uint8_t action;        /* CAI_ACTION_* */
    uint16_t custom_action;
    uint16_t _reserved;
} CaiAgentCommand;

typedef struct CaiTickStats {
    uint32_t agents;
    uint32_t thinks;
    uint32_t raycasts;
    uint32_t path_queries;
    float micros;
} CaiTickStats;

typedef struct CaiAgentDebug {
    CaiActorId target; /* or CAI_INVALID_ACTOR */
    CaiVec3 destination; /* valid if has_destination */
    float think_interval;
    uint32_t cover; /* cover point index or CAI_INVALID_INDEX */
    uint32_t known_targets;
    uint16_t custom_action;
    uint8_t action; /* CAI_ACTION_* */
    uint8_t role;   /* CAI_ROLE_* */
    uint8_t target_visible;
    uint8_t has_token;
    uint8_t has_destination;
    uint8_t _reserved;
} CaiAgentDebug;

/* Return true if nothing blocks the segment from -> to. Required. */
typedef bool (*CaiLineOfSightFn)(void *user_data, CaiVec3 from, CaiVec3 to);
/* Write the navigation distance and return true, or return false if `to` is
 * unreachable. Optional: null means straight-line distance. */
typedef bool (*CaiPathDistanceFn)(void *user_data, CaiVec3 from, CaiVec3 to, float *out_distance);

typedef struct CaiWorld {
    void *user_data;
    CaiLineOfSightFn line_of_sight;
    CaiPathDistanceFn path_distance;
} CaiWorld;

typedef struct CaiSystem CaiSystem;
typedef struct CaiArchetype CaiArchetype;

/* ---- version / errors ---- */
uint32_t cai_abi_version(void);
const char *cai_version_string(void);
bool cai_abi_check(uint32_t abi_version, size_t system_config, size_t agent_config,
                   size_t actor_state, size_t agent_command, size_t sound_event,
                   size_t damage_event, size_t cover_point, size_t agent_debug,
                   size_t world);
#define CAI_ABI_CHECK()                                                                  \
    cai_abi_check(CAI_ABI_VERSION, sizeof(CaiSystemConfig), sizeof(CaiAgentConfig),      \
                  sizeof(CaiActorState), sizeof(CaiAgentCommand), sizeof(CaiSoundEvent), \
                  sizeof(CaiDamageEvent), sizeof(CaiCoverPoint), sizeof(CaiAgentDebug),  \
                  sizeof(CaiWorld))
/* Valid until the next failing call on this thread. Never null. */
const char *cai_last_error(void);

/* ---- configuration / archetypes ---- */
int32_t cai_system_config_default(CaiSystemConfig *out);
int32_t cai_agent_config_default(CaiAgentConfig *out);
int32_t cai_archetype_preset_config(uint32_t preset, CaiAgentConfig *out);
CaiArchetype *cai_archetype_preset(uint32_t preset);
/* Standard action set with custom tuning. name may be null. */
CaiArchetype *cai_archetype_new(const char *name, const CaiAgentConfig *config);
void cai_archetype_free(CaiArchetype *archetype);

/* ---- system ---- */
/* config may be null for defaults. */
CaiSystem *cai_system_new(const CaiSystemConfig *config);
void cai_system_free(CaiSystem *sys);

/* ---- actors ---- */
int32_t cai_actor_state_default(CaiVec3 position, uint16_t team, CaiActorState *out);
/* Externally controlled combatant, e.g. a player. */
CaiActorId cai_add_actor(CaiSystem *sys, const CaiActorState *state);
CaiActorId cai_spawn_agent(CaiSystem *sys, const CaiActorState *state, const CaiArchetype *archetype);
bool cai_remove_actor(CaiSystem *sys, CaiActorId actor);
bool cai_is_agent(const CaiSystem *sys, CaiActorId actor);
bool cai_get_actor_state(const CaiSystem *sys, CaiActorId actor, CaiActorState *out);
/* The team is fixed at creation and is not changed by this call. */
bool cai_set_actor_state(CaiSystem *sys, CaiActorId actor, const CaiActorState *state);

/* ---- squads / level ---- */
uint32_t cai_create_squad(CaiSystem *sys);
bool cai_assign_squad(CaiSystem *sys, CaiActorId agent, uint32_t squad);
uint32_t cai_add_cover_point(CaiSystem *sys, const CaiCoverPoint *cover);
CaiActorId cai_cover_owner(const CaiSystem *sys, uint32_t cover);
int32_t cai_set_lod_focus(CaiSystem *sys, const CaiVec3 *points, size_t count);

/* ---- stimuli ---- */
int32_t cai_report_sound(CaiSystem *sys, const CaiSoundEvent *event);
int32_t cai_report_damage(CaiSystem *sys, const CaiDamageEvent *event);
/* Tell every agent on `team` where `target` is now (radar ping, alarm, director). */
uint32_t cai_reveal(CaiSystem *sys, CaiActorId target, uint16_t team);

/* ---- simulation ---- */
/* On success *out_commands points to *out_count commands, valid until the next
 * call that modifies sys. Either output pointer may be null. */
int32_t cai_tick(CaiSystem *sys, float dt, const CaiWorld *world,
                 const CaiAgentCommand **out_commands, size_t *out_count);
int32_t cai_get_stats(const CaiSystem *sys, CaiTickStats *out);
bool cai_agent_debug(const CaiSystem *sys, CaiActorId agent, CaiAgentDebug *out);
size_t cai_agent_count(const CaiSystem *sys);
float cai_time(const CaiSystem *sys);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* COMBAT_AI_H */
