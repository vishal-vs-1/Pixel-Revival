# Rust NES backend

Pixel Revival loads `mesen_player.dll` directly from Python. The previous managed MesenCE player and its frontend bridge are no longer the active runtime. This gives the application integration ownership of windows, input, comparison, audio delivery and artwork hooks. See [the architecture decision](../docs/PROJECT_FLOW.md#why-the-nes-player-changed).

## Provenance and boundaries

| Path | Responsibility |
|---|---|
| `mesen-core/` | Vendored Rust port of Mesen 0.9.9, reused as the emulation core |
| `mesen-player/` | Project C ABI library wrapping the core and exposing HD/capture operations |
| `../systems/nes/` | Project Python player, presentation, audio, controls, comparison and artwork pipeline |

Original Mesen C++ attribution belongs to Sour; the Rust port retains its contributor/license notices. This app did not rewrite the emulator core from scratch. Source is vendored from the project's `mesen-rs` checkout so builds do not require that external directory and project changes can be versioned with the app. The external checkout remains read-only.

The vendored core is no longer pristine: approved changes include HD recording/composition, sprite coordinate/mirroring corrections and original sprite-fetch observation. Keep ordinary UI/artwork behavior in the frontend; new core changes require the explicit approval specified in repository `AGENTS.md`. GPL-3.0-or-later notices are retained.

## Build

From the project root:

```powershell
cargo build --release --manifest-path rust-nes/Cargo.toml -p mesen-player --target-dir build/rust-nes-target
```

Output: `build/rust-nes-target/release/mesen_player.dll`. Python loads the standard DLL there. Experimental `_next` builds require explicit `RUST_NES_DLL` selection; timestamps do not change the active runtime. Player logs include its path, SHA-256 and ABI. Restart a running launcher to load a rebuilt library.

## Optional sprite observation

Base C ABI remains 1. Optional `rs_pose_version=2`, `rs_pose_arm` and `rs_pose_frame` expose original fetched sprite bytes independently of Enhanced output pixels. The observer defaults inactive, reuses existing bus fetches and retains at most 128 complete 8x8 cells per frame. It clears on reset/load and is excluded from save states.

The output is a four-byte little-endian camera header plus up to 128 records of 25 bytes: x, y, flags, reserved, index-u16, palette[3], pattern[16]. Camera periods are 512/480; unavailable coordinates are 65535. The Python recorder/overlay uses this only when supported and armed. [Current status](../docs/STATUS.md) describes remaining player-identification and acceptance limits.
