// Checks that combat_ai.h compiles as C++ and shows RAII ownership of the
// opaque handles. Exits non-zero on failure.
#include "combat_ai.h"

#include <cstdio>
#include <memory>
#include <vector>

namespace {

struct SystemDeleter {
    void operator()(CaiSystem *s) const { cai_system_free(s); }
};
struct ArchetypeDeleter {
    void operator()(CaiArchetype *a) const { cai_archetype_free(a); }
};
using SystemPtr = std::unique_ptr<CaiSystem, SystemDeleter>;
using ArchetypePtr = std::unique_ptr<CaiArchetype, ArchetypeDeleter>;

// Captureless lambdas convert to C function pointers. Never let exceptions
// escape a callback.
bool open_field(void *, CaiVec3, CaiVec3) noexcept { return true; }

}  // namespace

int main() {
    if (!CAI_ABI_CHECK()) {
        std::fprintf(stderr, "ABI mismatch: %s\n", cai_last_error());
        return 1;
    }
    CaiSystemConfig cfg{};
    cai_system_config_default(&cfg);
    cfg.parallel_threshold = UINT32_MAX;  // keep callbacks on this thread
    SystemPtr sys(cai_system_new(&cfg));
    ArchetypePtr sniper(cai_archetype_preset(CAI_PRESET_SNIPER));
    if (!sys || !sniper) return 1;

    CaiActorState st{};
    cai_actor_state_default(CaiVec3{0, 0, 60}, 0, &st);
    CaiActorId player = cai_add_actor(sys.get(), &st);
    cai_actor_state_default(CaiVec3{0, 0, 0}, 1, &st);
    CaiActorId npc = cai_spawn_agent(sys.get(), &st, sniper.get());

    CaiWorld world{nullptr, open_field, nullptr};
    std::vector<CaiAgentCommand> last;
    for (int i = 0; i < 180; ++i) {
        const CaiAgentCommand *cmds = nullptr;
        size_t n = 0;
        if (cai_tick(sys.get(), 1.0f / 60.0f, &world, &cmds, &n) != CAI_OK) {
            std::fprintf(stderr, "tick failed: %s\n", cai_last_error());
            return 1;
        }
        last.assign(cmds, cmds + n);
    }
    CaiAgentDebug dbg{};
    if (!cai_agent_debug(sys.get(), npc, &dbg) || dbg.target != player) {
        std::fprintf(stderr, "sniper did not acquire the player\n");
        return 1;
    }
    std::printf("C++ ok: %zu command(s), sniper action %u\n", last.size(), unsigned(dbg.action));
    return 0;
}
