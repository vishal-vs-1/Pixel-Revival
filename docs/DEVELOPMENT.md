# Development

The active architecture is [Architecture and HLD](PROJECT_FLOW.md), with contracts in [Low-level design](LOW_LEVEL_DESIGN.md); unresolved behavior is tracked in [Status](STATUS.md). Read repository `AGENTS.md` before modifying code.

## Java ME setup

Follow [source setup](../README.md#run-from-source). `tools/build.py` fetches the pinned FreeJ2ME-Plus source, applies integration patches and prepares the Java player/demo/runtime. Java ME uses the compatibility GPU presenter or per-game CPU mode.

## NES setup

Build the vendored C ABI library from the project root:

```powershell
cargo build --release --manifest-path rust-nes/Cargo.toml -p mesen-player --target-dir build/rust-nes-target
python app.py
```

`systems/nes/rust_core.py` loads the newest existing `mesen_player.dll` or `mesen_player_next.dll` under `build/rust-nes-target/release/`. `RUST_NES_DLL` overrides discovery for an explicit build. A running process keeps its loaded DLL; restart to pick up changes. Do not build or edit the external upstream checkout. New vendored-core changes require the approval specified in `AGENTS.md`.

Base ABI is 1. Optional pose protocol 2 reports actual fetched sprites; old libraries retain the debug-read fallback. Journals write version 2 and read versions 1/2. Pose observation does not change save-state serialization. See [Rust build notes](../rust-nes/README.md).

## Source layout

| Path | Ownership |
|---|---|
| `launcher/` | Tk views, operations, playback choices and UI events |
| `systems/application.py` | System routing boundary |
| `systems/jme/` | JAR import, preparation, pose data and JVM launch |
| `emulator-src/`, `tools/patches/` | Java integration and pinned-source patches |
| `systems/nes/` | Cartridge library, captures, HD packs, pose pipeline, input/audio/display |
| `rust-nes/mesen-core/`, `rust-nes/mesen-player/` | Vendored NES core and Python-facing C ABI |
| `runtime_assets/` | Queue/storage, scheduling, converters and publication |
| `systems/generated_poses.py` | Generated pose-set storage and catalog validation |
| `storage/`, `controls/`, `graphics/` | Paths, profiles and device/runtime policy |

The NES emulator worker owns native calls; the display worker consumes bounded immutable frames. Backend calls must not execute under the application routing lock. Tk widget ownership stays on the UI thread.

## Checks

Use focused checks appropriate to a code change. Documentation cleanup needs link/reference review, not gameplay or benchmarks.

```powershell
python -m pip install -r requirements-dev.txt
python -m pytest -q tests/test_nes_pose.py tests/test_nes_pose_tracking.py tests/test_nes_player_safety.py
```

Rust checks use `cargo test --manifest-path rust-nes/Cargo.toml --target-dir build/rust-nes-target`. Native Python integration needs a built DLL; Java checks need a JDK. Inspect selected test fixtures/markers before running: some checks open Tk or player windows. Default discovery is restricted to `tests/`; `local-work/` experiments are not part of the suite. Respect the user's current authorization for runtime tests and desktop interaction.

## Portable Windows ZIP

The existing packaging script still stages the shared native graphics runtime. Its presence in a ZIP is not evidence that active players use it. Until dependencies are audited, build that staging dependency as required by the script:

```powershell
python tools/build_graphics.py --generator 'Visual Studio 17 2022'
python -m pip install pyinstaller==6.22.3
python tools/package_windows.py
```

The native build requires CMake, Visual Studio 2022 x64 C++ tools/Windows SDK and a JDK with JNI headers (`JAVA_HOME`). Build the Java player and Rust library first. The packager copies the standard `mesen_player.dll`, so ensure it is the intended build rather than relying on a newer `_next.dll`. Output goes to `build/windows-release/Pixel-Revival-Windows-x64-preview.zip`, containing `Pixel Revival/Pixel Revival.exe`; optional AI setup remains separate. Earlier previews retain their original filenames until rebuilt. Preserve the existing portable `data/` directory when upgrading.

Keep generated binaries under `build/` and upstream checkouts under `vendor/`. Preserve games, saves, captures, caches and permanent `local-work/` history. Do not stage or publish without explicit instruction.
