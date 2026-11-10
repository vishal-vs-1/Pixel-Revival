# Development

## Java ME setup

Use the source setup in the [README](../README.md#run-from-source). `tools/build.py` fetches the pinned FreeJ2ME-Plus revision, applies the Java integration, builds the synthetic demo, and prepares the LWJGL and Java runtimes. Install `requirements-dev.txt` to run the Python checks.

## Shared graphics runtime

Install CMake 3.27+, Visual Studio 2022 C++ x64 build tools with the v143 toolset and Windows SDK, and a JDK with JNI headers. Set `JAVA_HOME` to that JDK. From the repository root run:

~~~powershell
python tools/build_graphics.py --generator 'Visual Studio 17 2022'
~~~

This builds the Windows x64 renderer DLL and Java JNI shim, runs the native contracts, and stages the runtime under `build/common-runtime-u22/`. The Java ME launcher can use its Legacy GPU presenter when the shared runtime is unavailable.

## Managed NES runtime

NES uses a patched, pinned MesenCE build. Build the shared graphics runtime above and install the x64 .NET 10 SDK. In a fresh source checkout, fetch and build the pinned Mesen source:

~~~powershell
$mesen = Get-Content nes-bridge/mesen/upstream.json -Raw | ConvertFrom-Json
git clone $mesen.repository vendor/mesen-ce
git -C vendor/mesen-ce checkout --detach $mesen.commit
git -C vendor/mesen-ce submodule update --init --recursive
python tools/build_nes.py
~~~

The build applies the bridge to an isolated copy and places the player under `build/common-runtime-u22/nes/player/`. Run `python app.py` and select **NES**. The source launcher discovers the runtime in its build tree.

For a portable install, place the complete managed runtime tree at `tools/nes-runtime/` beside the EXE, or set `J2ME_REVIVAL_NES_RUNTIME_ROOT` to the absolute runtime root before launching. Keep its `common/` and `nes/` directories together. The launcher checks for `nes/player/Mesen.exe`, its native dependencies, and the matching graphics manifest. A stock Mesen executable alone cannot read the managed launch manifest.

## Portable Windows ZIP

Build the Java ME player and shared graphics runtime first. Install PyInstaller and build the ZIP from the repository root:

~~~powershell
python -m pip install pyinstaller==6.22.3
python tools/package_windows.py
~~~

The ZIP and SHA-256 checksum are written to `build/windows-release/`. The archive includes the Java ME and shared graphics runtimes, the synthetic demo, and the Windows guide. Managed MesenCE and Real-ESRGAN are separate setup steps.

## Checks

| Location | What runs there |
| --- | --- |
| `tests/test_*.py` | Python contracts, storage, and launcher behavior. `test_ui.py` creates a Tk test window. |
| `tests/fixtures/` | Synthetic data and pinned text fixtures. |
| `tests/java/` | Reusable Java player checks, compiled by the corresponding scripts in `tools/`. |
| `native-graphics/tests/` | Native graphics contracts and optional OpenGL checks. |
| `nes-bridge/tests/` | Native NES bridge and managed launcher contracts. |

For headless Python checks run:

~~~powershell
python -m pip install -r requirements-dev.txt
python -m pytest tests -q --ignore=tests/test_ui.py
~~~

Use `python -m pytest tests/test_ui.py -q` for Tk layout and interaction on a machine where opening test windows is acceptable. The [CI workflow](../.github/workflows/ci.yml) runs the Python suite and builds the Windows native components in separate jobs.

With a built Java ME player, use `tools/check_control_profiles.py`, `tools/check_controls.py`, `tools/check_renderer.py`, `tools/check_lifecycle.py`, or `tools/check_runtime_assets.py` for focused checks. `tools/check_gpu.py` and `tools/check_input.py` need working graphics. `tools/check_nes_launch_reader.py` exercises the managed NES launch, save, and artwork pack contracts with fixtures. See the [performance guide](PERFORMANCE.md) for the supported profiling tools.

## Working tree

Keep generated outputs under `build/`, pinned source checkouts under `vendor/`, and downloaded runtimes in their designated tool directories. Keep imported games, saves, captures, caches, and local design and experiment history out of commits. The Java integration sources use the package paths expected by the FreeJ2ME-Plus patcher.
