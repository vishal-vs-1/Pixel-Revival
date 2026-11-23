# Development

## Java ME setup

Use the source setup in the [README](../README.md#run-from-source). `tools/build.py` fetches the pinned FreeJ2ME-Plus revision, applies the Java integration, builds the synthetic demo, and prepares the LWJGL and Java runtimes. Install `requirements-dev.txt` to run the Python checks.

## Shared graphics runtime

Install CMake 3.27+, Visual Studio 2022 C++ x64 build tools with the v143 toolset and Windows SDK, and a JDK with JNI headers. Set `JAVA_HOME` to that JDK. From the repository root run:

~~~powershell
python tools/build_graphics.py --generator 'Visual Studio 17 2022'
~~~

This builds the Windows x64 renderer DLL and Java JNI shim, runs the native contracts, and stages the runtime under `build/common-runtime-u22/`. Java ME uses its compatibility GPU presenter.

## NES setup

The NES player is our own Rust library in `rust-nes/`. The Python app loads `mesen_player.dll` through `systems/nes/rust_core.py`; no external emulator process or MesenCE checkout is used.

Install the stable Rust toolchain, then build the library from the repository root:

~~~powershell
$env:CARGO_TARGET_DIR = "build/rust-nes-target"
cargo build --release --manifest-path rust-nes/Cargo.toml -p mesen-player
~~~

Run `python app.py` and select **NES**. The library is discovered under `build/rust-nes-target/release/mesen_player.dll`; set `RUST_NES_DLL` to an absolute DLL path only when testing another build. A fresh build can be placed beside a locked DLL as `mesen_player_next.dll`, which the app prefers until the running app stops.

## Portable Windows ZIP

Build the Java ME player and shared graphics runtime first. Install PyInstaller and build the ZIP from the repository root:

~~~powershell
python -m pip install pyinstaller==6.22.3
python tools/package_windows.py
~~~

The ZIP and SHA-256 checksum are written to `build/windows-release/`. The archive includes the Java ME runtime, shared graphics runtime, the Rust NES library, the synthetic demo, and the Windows guide. Real-ESRGAN is a separate setup step.

## Source layout

- `app.py` is the desktop entry point. `launcher/` owns Tk layout, catalog and settings views, background operation controls, and playback UI.
- `systems/jme/` owns the Java ME library, artwork preparation, and JVM launch. `systems/nes/` owns the NES library, HD packs, and the Rust player window. `rust-nes/` contains the vendored core and its C ABI crate.
- `runtime_assets/` separates scheduling, live admission, conversion execution, and result publication. The deterministic sprite painter is also available through `python -m runtime_assets.sprite_paint input.png output.png`.
- `tools/patches/` separates build-time patches by emulator and subsystem. `emulator-src/` groups Java integration sources by responsibility. The patcher installs them into the Java package expected by the pinned upstream player.

Follow [project flow](PROJECT_FLOW.md) for the active runtime architecture. The ignored `local-work/` tree preserves the detailed designs, retired experiments, and validation history.

## Checks

| Location | What runs there |
| --- | --- |
| `tests/test_*.py` | Python contracts, storage, and launcher behavior. `test_ui.py` creates a Tk test window. |
| `tests/fixtures/` | Synthetic data and pinned text fixtures. |
| `tests/java/` | Reusable Java player checks, compiled by the corresponding scripts in `tools/`. |
| `native-graphics/tests/` | Native graphics contracts and optional OpenGL checks. |
| `rust-nes/` | Rust NES core and `mesen-player` C ABI tests (`cargo test`). |

For headless Python checks run:

~~~powershell
python -m pip install -r requirements-dev.txt
python -m pytest tests -q --ignore=tests/test_ui.py
~~~

Use `python -m pytest tests/test_ui.py -q` for Tk layout and interaction on a machine where opening test windows is acceptable. The [CI workflow](../.github/workflows/ci.yml) runs the Python suite and builds the Windows native components in separate jobs.

With a built Java ME player, use `tools/check_control_profiles.py`, `tools/check_controls.py`, `tools/check_renderer.py`, `tools/check_lifecycle.py`, or `tools/check_runtime_assets.py` for focused checks. `tools/check_gpu.py` and `tools/check_input.py` need working graphics. For NES, run `python -m pytest -q tests/test_nes_rust.py` after building the Rust DLL. See the [performance guide](PERFORMANCE.md) for the supported profiling tools.

## Working tree

Keep generated outputs under `build/`, pinned source checkouts under `vendor/`, and downloaded runtimes in their designated tool directories. Keep imported games, saves, captures, caches, and local design and experiment history out of commits. The Java integration sources use the package paths expected by the FreeJ2ME-Plus patcher.
