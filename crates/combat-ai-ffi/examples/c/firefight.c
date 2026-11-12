/*
 * Minimal C host for combat-ai: one player behind a wall, a squad of four
 * riflemen. Shows the per-frame loop (push state -> tick -> apply commands)
 * and exits non-zero if the AI doesn't behave (used as an integration test).
 *
 *   cargo build -p combat-ai-ffi --release
 *   cc examples/c/firefight.c -Iinclude -L../../target/release \
 *      -l:libcombat_ai_ffi.a -lpthread -ldl -lm -o firefight && ./firefight
 */
#include "combat_ai.h"

#include <math.h>
#include <stdio.h>
#include <stdlib.h>

typedef struct Box {
    CaiVec3 min, max;
} Box;

typedef struct World {
    Box boxes[4];
    int count;
} World;

static int segment_hits_box(CaiVec3 a, CaiVec3 b, const Box *box) {
    float o[3] = {a.x, a.y, a.z}, d[3] = {b.x - a.x, b.y - a.y, b.z - a.z};
    float lo[3] = {box->min.x, box->min.y, box->min.z}, hi[3] = {box->max.x, box->max.y, box->max.z};
    float t0 = 0.0f, t1 = 1.0f;
    for (int i = 0; i < 3; i++) {
        if (fabsf(d[i]) < 1e-9f) {
            if (o[i] < lo[i] || o[i] > hi[i]) return 0;
        } else {
            float ta = (lo[i] - o[i]) / d[i], tb = (hi[i] - o[i]) / d[i];
            if (ta > tb) { float t = ta; ta = tb; tb = t; }
            if (ta > t0) t0 = ta;
            if (tb < t1) t1 = tb;
            if (t0 > t1) return 0;
        }
    }
    return 1;
}

/* Read-only, so it is safe to call from several threads at once. */
static bool line_of_sight(void *user_data, CaiVec3 from, CaiVec3 to) {
    const World *w = (const World *)user_data;
    for (int i = 0; i < w->count; i++)
        if (segment_hits_box(from, to, &w->boxes[i])) return false;
    return true;
}

static CaiVec3 v3(float x, float y, float z) {
    CaiVec3 v = {x, y, z};
    return v;
}

#define CHECK(cond)                                                                 \
    do {                                                                            \
        if (!(cond)) {                                                              \
            fprintf(stderr, "check failed: %s (line %d): %s\n", #cond, __LINE__, \
                    cai_last_error());                                              \
            return 1;                                                               \
        }                                                                           \
    } while (0)

int main(void) {
    CHECK(CAI_ABI_CHECK());
    printf("combat-ai %s (ABI %u)\n", cai_version_string(), cai_abi_version());

    CaiSystemConfig cfg;
    CHECK(cai_system_config_default(&cfg) == CAI_OK);
    cfg.seed = 42;
    CaiSystem *sys = cai_system_new(&cfg);
    CHECK(sys != NULL);

    /* Level: a low wall between the player and the squad, with cover behind it. */
    World world = {.count = 1};
    world.boxes[0].min = v3(-6.0f, 0.0f, 9.0f);
    world.boxes[0].max = v3(6.0f, 1.1f, 9.6f);
    for (int i = -2; i <= 2; i++) {
        CaiCoverPoint c = {v3((float)i * 2.5f, 0.0f, 10.2f), v3(0.0f, 0.0f, -1.0f), CAI_COVER_LOW};
        CHECK(cai_add_cover_point(sys, &c) != CAI_INVALID_INDEX);
    }

    CaiActorState st;
    CHECK(cai_actor_state_default(v3(0.0f, 0.0f, 0.0f), 0, &st) == CAI_OK);
    CaiActorId player = cai_add_actor(sys, &st);
    CHECK(player != CAI_INVALID_ACTOR);
    CHECK(!cai_is_agent(sys, player));

    /* Tune a preset: a slightly sharper rifleman. */
    CaiAgentConfig acfg;
    CHECK(cai_archetype_preset_config(CAI_PRESET_RIFLEMAN, &acfg) == CAI_OK);
    acfg.accuracy_deg *= 0.8f;
    CaiArchetype *rifleman = cai_archetype_new("sharp rifleman", &acfg);
    CHECK(rifleman != NULL);

    uint32_t squad = cai_create_squad(sys);
    CaiActorId agents[4];
    for (int i = 0; i < 4; i++) {
        CaiActorState a;
        cai_actor_state_default(v3((float)i * 3.0f - 4.5f, 0.0f, 30.0f), 1, &a);
        a.forward = v3(0.0f, 0.0f, -1.0f);
        agents[i] = cai_spawn_agent(sys, &a, rifleman);
        CHECK(agents[i] != CAI_INVALID_ACTOR);
        CHECK(cai_assign_squad(sys, agents[i], squad));
    }
    cai_archetype_free(rifleman); /* agents keep their own reference */

    /* Errors are reported, not crashes. */
    CHECK(cai_tick(sys, 0.016f, NULL, NULL, NULL) == CAI_ERR_NULL);
    CHECK(cai_last_error()[0] != '\0');
    CHECK(!cai_set_actor_state(sys, CAI_INVALID_ACTOR, &st));

    CaiWorld w = {&world, line_of_sight, NULL};
    const float dt = 1.0f / 60.0f;
    int shots = 0, moves = 0, barks = 0;
    for (int frame = 0; frame < 60 * 8; frame++) {
        /* The player strafes along the wall. */
        CaiActorState p;
        CHECK(cai_get_actor_state(sys, player, &p));
        p.position.x = 4.0f * sinf((float)frame * dt * 0.7f);
        p.velocity.x = 2.8f * cosf((float)frame * dt * 0.7f);
        CHECK(cai_set_actor_state(sys, player, &p));
        cai_set_lod_focus(sys, &p.position, 1);

        const CaiAgentCommand *cmds = NULL;
        size_t n = 0;
        CHECK(cai_tick(sys, dt, &w, &cmds, &n) == CAI_OK);
        CHECK(n == 4);
        for (size_t i = 0; i < n; i++) {
            const CaiAgentCommand *c = &cmds[i];
            CaiActorState s;
            CHECK(cai_get_actor_state(sys, c->actor, &s));
            if (c->flags & CAI_CMD_MOVE) {
                float dx = c->move_to.x - s.position.x, dz = c->move_to.z - s.position.z;
                float d = sqrtf(dx * dx + dz * dz);
                float speed = c->move_speed == CAI_SPEED_WALK ? 2.0f : 4.5f;
                if (d > 1e-3f) {
                    float step = fminf(d, speed * dt);
                    s.position.x += dx / d * step;
                    s.position.z += dz / d * step;
                    s.velocity = v3(dx / d * speed, 0.0f, dz / d * speed);
                }
                moves++;
            } else {
                s.velocity = v3(0.0f, 0.0f, 0.0f);
            }
            s.forward = v3(c->aim_dir.x, 0.0f, c->aim_dir.z);
            s.stance = c->stance;
            if ((c->flags & CAI_CMD_FIRE) && s.ammo > 0) {
                s.ammo--;
                shots++;
            }
            if (c->flags & CAI_CMD_RELOAD) s.ammo = s.mag_size;
            if (c->bark != CAI_BARK_NONE) barks++;
            CHECK(cai_set_actor_state(sys, c->actor, &s));
        }
    }

    CaiTickStats stats;
    CHECK(cai_get_stats(sys, &stats) == CAI_OK);
    CaiAgentDebug dbg;
    CHECK(cai_agent_debug(sys, agents[0], &dbg));
    printf("after %.1fs: %d shots, %d move-frames, %d barks; agent 0 action %u target %s; last tick %.1f us\n",
           cai_time(sys), shots, moves, barks, dbg.action,
           dbg.target == player ? "player" : "none", stats.micros);
    CHECK(shots > 0);
    CHECK(barks > 0);
    CHECK(dbg.target == player);

    CHECK(cai_remove_actor(sys, agents[3]));
    CHECK(!cai_remove_actor(sys, agents[3])); /* stale handle is inert */
    CHECK(cai_agent_count(sys) == 3);

    cai_system_free(sys);
    printf("ok\n");
    return 0;
}
