//! Tests the C ABI from Rust, then compiles and runs the C and C++ examples
//! against the static library (skipped with a note if no C compiler exists).

use combat_ai_ffi::*;
use std::ffi::{c_void, CStr};
use std::path::{Path, PathBuf};
use std::process::Command;

extern "C" fn open_field(_: *mut c_void, _: CaiVec3, _: CaiVec3) -> bool {
    true
}

extern "C" fn unreachable(_: *mut c_void, _: CaiVec3, _: CaiVec3, _: *mut f32) -> bool {
    false
}

fn v(x: f32, y: f32, z: f32) -> CaiVec3 {
    CaiVec3 { x, y, z }
}

#[test]
fn abi_roundtrip() {
    unsafe {
        let mut cfg = std::mem::zeroed::<CaiSystemConfig>();
        assert_eq!(cai_system_config_default(&mut cfg), CAI_OK);
        cfg.parallel_threshold = u32::MAX;
        let sys = cai_system_new(&cfg);
        assert!(!sys.is_null());

        let mut st = std::mem::zeroed::<CaiActorState>();
        cai_actor_state_default(v(0.0, 0.0, 25.0), 0, &mut st);
        let player = cai_add_actor(sys, &st);
        let arch = cai_archetype_preset(CAI_PRESET_RIFLEMAN);
        cai_actor_state_default(v(0.0, 0.0, 0.0), 1, &mut st);
        let npc = cai_spawn_agent(sys, &st, arch);
        cai_archetype_free(arch);
        assert_ne!(npc, CAI_INVALID_ACTOR);
        assert!(cai_is_agent(sys, npc));

        // Handles survive a round trip through the state accessors; team is immutable.
        let mut got = std::mem::zeroed::<CaiActorState>();
        assert!(cai_get_actor_state(sys, npc, &mut got));
        got.team = 7;
        got.health = 55.0;
        assert!(cai_set_actor_state(sys, npc, &got));
        assert!(cai_get_actor_state(sys, npc, &mut got));
        assert_eq!((got.team, got.health), (1, 55.0));

        let world = CaiWorld {
            user_data: std::ptr::null_mut(),
            line_of_sight: Some(open_field),
            path_distance: Some(unreachable),
        };
        let mut fired = false;
        let mut aimed_at_player = false;
        for _ in 0..90 {
            let mut cmds = std::ptr::null();
            let mut n = 0usize;
            assert_eq!(cai_tick(sys, 1.0 / 30.0, &world, &mut cmds, &mut n), CAI_OK);
            assert_eq!(n, 1);
            let c = &*cmds;
            assert_eq!(c.actor, npc);
            fired |= c.flags & CAI_CMD_FIRE != 0;
            aimed_at_player |= c.aim_target == player;
        }
        assert!(fired && aimed_at_player);

        let mut dbg = std::mem::zeroed::<CaiAgentDebug>();
        assert!(cai_agent_debug(sys, npc, &mut dbg));
        assert_eq!(dbg.target, player);
        assert_eq!(dbg.target_visible, 1);

        // Errors are reported, not crashes.
        assert_eq!(
            cai_tick(
                sys,
                0.1,
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut()
            ),
            CAI_ERR_NULL
        );
        let msg = CStr::from_ptr(cai_last_error())
            .to_string_lossy()
            .into_owned();
        assert!(msg.contains("world is null"), "{msg}");
        let defaults = cai_system_new(std::ptr::null()); // null config = defaults
        assert!(!defaults.is_null());
        cai_system_free(defaults);
        assert!(cai_archetype_preset(99).is_null());
        assert!(!cai_assign_squad(sys, npc, 1234));
        assert!(!cai_get_actor_state(sys, CAI_INVALID_ACTOR, &mut got));
        let bad = CaiSoundEvent {
            source: CAI_INVALID_ACTOR,
            position: v(0.0, 0.0, 0.0),
            radius: 10.0,
            kind: 42,
            _reserved: 0,
        };
        assert_eq!(cai_report_sound(sys, &bad), CAI_ERR_INVALID);
        assert_eq!(cai_reveal(sys, player, 1), 1);

        assert!(cai_remove_actor(sys, npc));
        assert!(!cai_remove_actor(sys, npc));
        assert_eq!(cai_agent_count(sys), 0);
        cai_system_free(sys);
        cai_system_free(std::ptr::null_mut());
    }
}

#[test]
fn configs_round_trip_losslessly() {
    unsafe {
        for p in [CAI_PRESET_RIFLEMAN, CAI_PRESET_ASSAULT, CAI_PRESET_SNIPER] {
            let mut c = std::mem::zeroed::<CaiAgentConfig>();
            assert_eq!(cai_archetype_preset_config(p, &mut c), CAI_OK);
            let rust: combat_ai::AgentConfig = (&c).into();
            let back: CaiAgentConfig = (&rust).into();
            assert_eq!(format!("{c:?}"), format!("{back:?}"));
        }
        let mut s = std::mem::zeroed::<CaiSystemConfig>();
        cai_system_config_default(&mut s);
        let rust: combat_ai::SystemConfig = (&s).into();
        let back: CaiSystemConfig = (&rust).into();
        assert_eq!(format!("{s:?}"), format!("{back:?}"));
    }
}

fn target_dir() -> PathBuf {
    // tests run from target/<profile>/deps/<test-binary>
    let exe = std::env::current_exe().unwrap();
    exe.parent().unwrap().parent().unwrap().to_path_buf()
}

fn find_tool(env: &str, candidates: &[&str]) -> Option<String> {
    let mut list: Vec<String> = std::env::var(env).ok().into_iter().collect();
    list.extend(candidates.iter().map(|s| s.to_string()));
    list.into_iter().find(|c| {
        Command::new(c)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

/// Libraries Rust's std needs when linking the static library
/// (`--print native-static-libs`). macOS's default libSystem covers them.
fn system_libs() -> &'static [&'static str] {
    if cfg!(target_os = "linux") {
        &["-lpthread", "-ldl", "-lm"]
    } else {
        &[]
    }
}

/// `cargo test` only builds the rlib, so build the static library explicitly
/// (a no-op when it is up to date) into the same target directory and profile.
fn static_lib() -> PathBuf {
    let dir = target_dir();
    let profile_dir = dir.file_name().unwrap().to_string_lossy().into_owned();
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.args(["build", "--lib", "-p", "combat-ai-ffi", "--target-dir"])
        .arg(dir.parent().unwrap());
    if profile_dir == "release" {
        cmd.arg("--release");
    }
    let status = cmd.status().expect("failed to run cargo");
    assert!(status.success(), "building the static library failed");
    let lib = dir.join(if cfg!(windows) {
        "combat_ai_ffi.lib"
    } else {
        "libcombat_ai_ffi.a"
    });
    assert!(
        lib.exists(),
        "static library not found at {}",
        lib.display()
    );
    lib
}

fn build_and_run(compiler: &str, std: &str, src: &Path, out: &Path, lib: &Path) {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let status = Command::new(compiler)
        .arg(std)
        .args(["-Wall", "-Wextra", "-Werror"])
        .arg(src)
        .arg("-I")
        .arg(crate_dir.join("include"))
        .arg(lib)
        .args(system_libs())
        .arg("-o")
        .arg(out)
        .status()
        .expect("failed to run the compiler");
    assert!(
        status.success(),
        "{compiler} failed to build {}",
        src.display()
    );
    let run = Command::new(out)
        .output()
        .expect("failed to run the example");
    assert!(
        run.status.success(),
        "{} failed:\n{}{}",
        out.display(),
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
#[cfg_attr(windows, ignore = "the gcc-style flags below assume a Unix toolchain")]
fn c_and_cpp_examples() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let tmp = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let cc = find_tool("CC", &["cc", "gcc", "clang"]);
    let cxx = find_tool("CXX", &["c++", "g++", "clang++"]);
    if cc.is_none() && cxx.is_none() {
        eprintln!("note: no C/C++ compiler found, skipping the native examples");
        return;
    }
    let lib = static_lib();
    match cc {
        Some(cc) => build_and_run(
            &cc,
            "-std=c99",
            &crate_dir.join("examples/c/firefight.c"),
            &tmp.join("firefight"),
            &lib,
        ),
        None => eprintln!("note: no C compiler found, skipping the C example"),
    }
    match cxx {
        Some(cxx) => build_and_run(
            &cxx,
            "-std=c++11",
            &crate_dir.join("examples/cpp/smoke.cpp"),
            &tmp.join("smoke"),
            &lib,
        ),
        None => eprintln!("note: no C++ compiler found, skipping the C++ example"),
    }
}
