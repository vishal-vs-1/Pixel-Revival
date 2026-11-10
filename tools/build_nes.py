"""Build the pinned managed Mesen runtime in an isolated build tree."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import uuid
import zipfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
UPSTREAM = ROOT / "nes-bridge" / "mesen" / "upstream.json"
DEFAULT_SOURCE = ROOT / "vendor" / "mesen-ce"
DEFAULT_SCRATCH = ROOT / "build" / "mesen-u22"
DEFAULT_RUNTIME = ROOT / "build" / "common-runtime-u22"


def _find_msbuild(explicit: str | None) -> str:
    candidates = [explicit, os.environ.get("MSBUILD_EXE"), shutil.which("MSBuild.exe"),
                  shutil.which("msbuild")]
    program_files = Path(os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)"))
    for version in program_files.glob("Microsoft Visual Studio/*"):
        for edition in ("BuildTools", "Community", "Professional", "Enterprise"):
            candidates.append(str(version / edition / "MSBuild" / "Current" / "Bin" / "amd64" / "MSBuild.exe"))
    for candidate in candidates:
        if candidate and Path(candidate).is_file():
            return str(Path(candidate).resolve())
    raise RuntimeError("64-bit MSBuild is required; pass --msbuild or install Visual Studio C++ build tools.")


def _find_dotnet(explicit: str | None) -> str:
    candidates = [explicit, os.environ.get("DOTNET_EXE"), shutil.which("dotnet"),
                  r"C:\Program Files\dotnet\dotnet.exe"]
    for candidate in candidates:
        if candidate and Path(candidate).is_file():
            return str(Path(candidate).resolve())
    raise RuntimeError("The x64 .NET 10 SDK is required; pass --dotnet or add dotnet to PATH.")


def _run(command: list[str], *, cwd: Path, env=None) -> None:
    print("+ " + subprocess.list2cmdline(command), flush=True)
    subprocess.run(command, cwd=cwd, env=env, check=True)


def _verify_checkout(source: Path) -> str:
    metadata = json.loads(UPSTREAM.read_text(encoding="utf-8"))
    expected = metadata["commit"]
    actual = subprocess.check_output(["git", "-C", str(source), "rev-parse", "HEAD"], text=True).strip()
    if actual != expected:
        raise RuntimeError(f"Pinned Mesen source mismatch: expected {expected}, got {actual}")
    return expected


def _copy_source(source: Path, destination: Path) -> None:
    if destination.exists() or destination.is_symlink():
        raise FileExistsError(f"Refusing to replace an existing build source tree: {destination}")

    def ignore(_directory: str, names: list[str]) -> set[str]:
        return {name for name in names if name in {".git", ".vs", "bin", "obj"}}

    shutil.copytree(source, destination, ignore=ignore)


def _apply_generated_patches(source: Path, output: Path) -> None:
    from tools.patch_mesen import generate

    metadata = json.loads(UPSTREAM.read_text(encoding="utf-8"))
    if output.exists():
        raise FileExistsError(f"Refusing to replace an existing generated patch folder: {output}")
    generate(source, output)
    for relative_text in metadata["generated_files"]:
        relative = Path(relative_text)
        if relative.is_absolute() or ".." in relative.parts:
            raise ValueError("Pinned bridge manifest contains an unsafe generated path")
        generated = output / relative
        destination = source / relative
        if not generated.is_file() or not destination.resolve().is_relative_to(source.resolve()):
            raise FileNotFoundError(f"Generated Mesen bridge patch is missing or outside source: {relative}")
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(generated, destination)


def _write_native_override(path: Path) -> None:
    path.write_text(
        '<Project xmlns="http://schemas.microsoft.com/developer/msbuild/2003">\n'
        '  <PropertyGroup Condition="\'$(RevivalProbeRoot)\' != \'\'">\n'
        '    <IntDir>$(RevivalProbeRoot)\\obj\\$(MSBuildProjectName)\\</IntDir>\n'
        '    <OutDir>$(RevivalProbeRoot)\\bin\\</OutDir>\n'
        '  </PropertyGroup>\n'
        '  <ItemDefinitionGroup><ClCompile><DebugInformationFormat>OldStyle</DebugInformationFormat></ClCompile></ItemDefinitionGroup>\n'
        '</Project>\n', encoding="utf-8")


def _finish_managed(args: argparse.Namespace, scratch: Path, runtime_root: Path,
                    core: Path, commit: str) -> Path:
    from graphics.runtime_package import validate_runtime_tree

    graphics_root = runtime_root / "common"
    identity = validate_runtime_tree(runtime_root)
    if hashlib.sha256((graphics_root / "revival_graphics.dll").read_bytes()).hexdigest() != identity["dll_sha256"]:
        raise ValueError("The shared graphics DLL changed after the native Mesen build.")
    dotnet = _find_dotnet(args.dotnet)
    info = subprocess.check_output([dotnet, "--list-sdks"], text=True)
    if not any(line.startswith("10.") for line in info.splitlines()):
        raise RuntimeError("The selected dotnet host does not expose a .NET 10 SDK.")
    player = runtime_root / "nes" / "player"
    if player.exists():
        raise FileExistsError(f"Refusing to overwrite an existing NES player folder: {player}")
    player.parent.mkdir(parents=True, exist_ok=True)
    staged_player = player.parent / (".player-build-" + uuid.uuid4().hex[:12])
    staged_player.mkdir()

    cache = args.nuget_cache
    offline_restore = cache is not None
    if cache is None:
        cache = Path(os.environ["J2ME_NUGET_CACHE"]) if os.environ.get("J2ME_NUGET_CACHE") else None
        offline_restore = cache is not None
    if cache is None:
        previous_cache = ROOT / "build" / "dotnet-home-u09-network" / "nuget-packages"
        offline_restore = previous_cache.is_dir()
        cache = previous_cache if offline_restore else scratch / "nuget-packages"
    cache = Path(cache).resolve()
    cache.mkdir(parents=True, exist_ok=True)

    environment = os.environ.copy()
    environment.update({
        "DOTNET_CLI_HOME": str(scratch / "dotnet-cli-home"),
        "NUGET_PACKAGES": str(cache),
        "APPDATA": str(scratch / "dotnet-appdata"),
        "DOTNET_SKIP_FIRST_TIME_EXPERIENCE": "1",
        "DOTNET_CLI_TELEMETRY_OPTOUT": "1",
        "DOTNET_NOLOGO": "1",
    })
    for key in ("DOTNET_CLI_HOME", "APPDATA"):
        Path(environment[key]).mkdir(parents=True, exist_ok=True)
    prepared = scratch / "workspace" / "vendor" / "mesen-ce"
    publish = [dotnet, "publish", str(prepared / "UI" / "UI.csproj"), "--configuration", "Release",
               "--runtime", "win-x64", "--self-contained", "false", "--output", str(staged_player),
               "--artifacts-path", str(scratch / "dotnet-artifacts"), "-p:NuGetAudit=false"]
    solution_output = scratch / "dotnet-solution"
    solution_output.mkdir(parents=True, exist_ok=True)
    publish.append(f"-p:SolutionDir={solution_output}{os.sep}")
    if offline_restore:
        # The workspace cache is also used as a local hierarchical NuGet feed when DNS is unavailable.
        publish.append(f"-p:RestoreSources={cache}")
    _run(publish, cwd=prepared, env=environment)
    shutil.copy2(core, staged_player / "MesenCore.dll")
    if not (staged_player / "Mesen.exe").is_file():
        raise FileNotFoundError("The managed build did not produce the x64 Mesen launcher.")

    source_archive = scratch / "mesence-bridge-source.zip"
    _make_mesen_source_bundle(prepared, source_archive)
    source_hash = hashlib.sha256(source_archive.read_bytes()).hexdigest()
    published_source = runtime_root / "source" / f"mesence-bridge-source-{commit[:12]}-{source_hash[:12]}.zip"
    published_source.parent.mkdir(parents=True, exist_ok=True)
    if not published_source.exists():
        shutil.copy2(source_archive, published_source)
    staged_player.rename(player)
    native_hash = hashlib.sha256(core.read_bytes()).hexdigest()
    (runtime_root / "nes" / "build-info.json").write_text(json.dumps({
        "schema": 1, "upstream_repository": "https://github.com/nesdev-org/MesenCE.git",
        "upstream_commit": commit, "native_core_sha256": native_hash,
        "graphics_dll_sha256": identity["dll_sha256"],
        "graphics_manifest": "graphics-runtime.json",
        "features": ["enhanced-pack-v1", "nes-tile-capture-rnt1"],
        "graphics_dll_loaded_from": "../common/revival_graphics.dll",
        "corresponding_source": f"../source/{published_source.name}",
    }, indent=2) + "\n", encoding="utf-8")
    print(f"Pinned Mesen runtime built locally at {player}")
    print("This is a build result only; no player window or game was launched.")
    return player


def resume_managed(args: argparse.Namespace) -> Path:
    if os.name != "nt":
        raise RuntimeError("The pinned managed Mesen runtime currently builds on Windows x64 only.")
    scratch = args.resume_managed_attempt.resolve()
    runtime_root = args.runtime_root.resolve()
    if ROOT not in scratch.parents or ROOT not in runtime_root.parents:
        raise ValueError("Resume and runtime folders must stay inside this repository.")
    marker_path = scratch / "source-pin.json"
    marker = json.loads(marker_path.read_text(encoding="utf-8"))
    expected = json.loads(UPSTREAM.read_text(encoding="utf-8"))["commit"]
    if marker.get("schema") != 1 or marker.get("upstream_commit") != expected:
        raise ValueError("Saved native build attempt is not from the currently pinned Mesen source.")
    prepared = scratch / "workspace" / "vendor" / "mesen-ce"
    if not (prepared / "UI" / "UI.csproj").is_file():
        raise FileNotFoundError("Saved managed source tree is incomplete.")
    core = scratch / "native" / "bin" / "MesenCore.dll"
    if not core.is_file() or b"revival_graphics.dll" not in core.read_bytes().lower():
        raise FileNotFoundError("Saved MesenCore DLL is missing or does not link the shared graphics runtime.")
    if hashlib.sha256(core.read_bytes()).hexdigest() != marker.get("native_core_sha256"):
        raise ValueError("Saved MesenCore DLL changed after the native build completed.")
    return _finish_managed(args, scratch, runtime_root, core, expected)


def build(args: argparse.Namespace) -> Path:
    if os.name != "nt":
        raise RuntimeError("The pinned managed Mesen runtime currently builds on Windows x64 only.")
    source = args.source_root.resolve()
    if not source.is_dir():
        raise FileNotFoundError(f"Pinned Mesen checkout is missing: {source}")
    commit = _verify_checkout(source)
    scratch_parent = args.scratch_root.resolve()
    runtime_root = args.runtime_root.resolve()
    if ROOT not in scratch_parent.parents or ROOT not in runtime_root.parents:
        raise ValueError("NES build and runtime outputs must stay inside this repository.")
    scratch_parent.mkdir(parents=True, exist_ok=True)
    scratch = scratch_parent / ("attempt-" + uuid.uuid4().hex[:12])
    scratch.mkdir()
    runtime_root.mkdir(parents=True, exist_ok=True)
    player = runtime_root / "nes" / "player"
    if player.exists():
        raise FileExistsError(f"Refusing to overwrite an existing NES player folder: {player}")

    graphics_build = args.graphics_build_dir.resolve()
    graphics_root = runtime_root / "common"
    from graphics.runtime_package import validate_runtime_tree
    validate_runtime_tree(runtime_root)
    graphics_lib_candidates = [graphics_build / "Release" / "revival_graphics.lib",
                               graphics_build / "lib" / "Release" / "revival_graphics.lib"]
    graphics_lib = next((path for path in graphics_lib_candidates if path.is_file()), None)
    if graphics_lib is None or not (graphics_root / "revival_graphics.dll").is_file():
        raise FileNotFoundError("Build the shared graphics runtime first with tools/build_graphics.py.")

    layout = scratch / "workspace"
    prepared = layout / "vendor" / "mesen-ce"
    layout.mkdir(parents=True, exist_ok=True)
    generated = scratch / "generated"
    _copy_source(source, prepared)
    for name in ("nes-bridge", "native-graphics"):
        shutil.copytree(ROOT / name, layout / name,
                        ignore=shutil.ignore_patterns("__pycache__", ".pytest_cache"))
    _apply_generated_patches(prepared, generated)

    native_build = scratch / "native"
    from tools.build_graphics import _cmake
    cmake = _cmake(args.cmake)
    ctest = shutil.which("ctest") or str(Path(cmake).with_name("ctest.exe"))
    if not Path(ctest).is_file():
        raise RuntimeError("CTest must be installed with CMake to run native bridge fixtures.")
    _run([cmake, "-S", str(ROOT / "nes-bridge"), "-B", str(scratch / "bridge-contracts"),
          f"-DREVIVAL_GRAPHICS_INCLUDE={ROOT / 'native-graphics' / 'include'}",
          f"-DMESEN_SOURCE_ROOT={prepared}", "-DBUILD_TESTING=ON"], cwd=ROOT)
    _run([cmake, "--build", str(scratch / "bridge-contracts"), "--config", "Release", "--parallel", "1"], cwd=ROOT)
    _run([ctest, "--test-dir", str(scratch / "bridge-contracts"), "-C", "Release",
          "--output-on-failure"], cwd=ROOT)

    msbuild = _find_msbuild(args.msbuild)
    override = scratch / "isolated-native-output.targets"
    _write_native_override(override)
    project = prepared / "InteropDLL" / "InteropDLL.vcxproj"
    _run([msbuild, str(project), "-t:Build", "-p:Configuration=Release", "-p:Platform=x64",
          f"-p:SolutionDir={prepared}{os.sep}", f"-p:RevivalProbeRoot={native_build}",
          f"-p:ForceImportAfterCppTargets={override}", f"-p:REVIVAL_GRAPHICS_LIB={graphics_lib}",
          "-p:CL_MPCount=1", "-m:1", "-nr:false", "-v:minimal", "-nologo"], cwd=ROOT)
    core = native_build / "bin" / "MesenCore.dll"
    if not core.is_file():
        raise FileNotFoundError(f"The pinned native build did not produce {core}")
    if b"revival_graphics.dll" not in core.read_bytes().lower():
        raise RuntimeError("MesenCore does not declare the shared graphics DLL dependency")
    (scratch / "source-pin.json").write_text(json.dumps({
        "schema": 1, "upstream_commit": commit,
        "native_core_sha256": hashlib.sha256(core.read_bytes()).hexdigest(),
    }, indent=2) + "\n", encoding="utf-8")
    return _finish_managed(args, scratch, runtime_root, core, commit)


def _make_mesen_source_bundle(source: Path, target: Path) -> None:
    layout = source.parents[1]
    with zipfile.ZipFile(target, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
        for path in sorted(source.rglob("*")):
            if path.is_file() and not path.is_symlink() and not path.is_junction():
                if any(part in {"bin", "obj", ".vs", ".git"} for part in path.parts):
                    continue
                if path.suffix.casefold() in {".nes", ".rom", ".sav", ".srm", ".state"}:
                    continue
                archive.write(path, f"mesen-source/vendor/mesen-ce/{path.relative_to(source).as_posix()}")
        for name in ("nes-bridge", "native-graphics"):
            component = layout / name
            for path in sorted(component.rglob("*")):
                if path.is_file() and not path.is_symlink() and not path.is_junction():
                    if path.suffix.casefold() in {".nes", ".rom", ".sav", ".srm", ".state"}:
                        continue
                    archive.write(path, f"mesen-source/{name}/{path.relative_to(component).as_posix()}")
        archive.write(ROOT / "tools" / "patch_mesen.py", "mesen-source/tools/patch_mesen.py")
        archive.write(ROOT / "LICENSE", "mesen-source/LICENSE-JAR-Revival")
        if not (source / "LICENSE").is_file():
            raise FileNotFoundError("Pinned Mesen source license is missing from the corresponding-source package")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-root", type=Path, default=DEFAULT_SOURCE)
    parser.add_argument("--scratch-root", type=Path, default=DEFAULT_SCRATCH)
    parser.add_argument("--runtime-root", type=Path, default=DEFAULT_RUNTIME)
    parser.add_argument("--graphics-build-dir", type=Path, default=ROOT / "build" / "native-graphics-u22")
    parser.add_argument("--cmake")
    parser.add_argument("--msbuild")
    parser.add_argument("--dotnet")
    parser.add_argument("--nuget-cache", type=Path)
    parser.add_argument("--resume-managed-attempt", type=Path,
                        help="Finish managed publish from an attempt whose native MesenCore build passed.")
    args = parser.parse_args()
    if args.resume_managed_attempt:
        resume_managed(args)
    else:
        build(args)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
