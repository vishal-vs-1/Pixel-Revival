"""Build and package the shared Windows x64 graphics runtime."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import zipfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
DEFAULT_BUILD = ROOT / "build" / "native-graphics-u22"
DEFAULT_RUNTIME = ROOT / "build" / "common-runtime-u22"


def _cmake(explicit: str | None = None) -> str:
    candidates = [explicit, os.environ.get("J2ME_CMAKE"), shutil.which("cmake")]
    program_files = Path(os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)"))
    candidates.extend(str(path) for path in program_files.glob(
        "Microsoft Visual Studio/*/BuildTools/Common7/IDE/CommonExtensions/Microsoft/CMake/CMake/bin/cmake.exe"))
    candidates.extend((r"C:\Program Files\CMake\bin\cmake.exe",
                       r"C:\Program Files (x86)\CMake\bin\cmake.exe"))
    for candidate in candidates:
        if candidate and Path(candidate).is_file():
            return str(Path(candidate).resolve())
    raise RuntimeError("CMake 3.27 or newer is required; pass --cmake or add it to PATH.")


def _java_home(explicit: Path | None) -> Path:
    candidates = [explicit, Path(os.environ["JAVA_HOME"]) if os.environ.get("JAVA_HOME") else None,
                  Path(os.environ["JME_JAVA_HOME"]) if os.environ.get("JME_JAVA_HOME") else None,
                  Path(r"C:\Program Files\Java\jdk-25.0.2"), Path(r"C:\Program Files\Java\jdk-22")]
    for candidate in candidates:
        if candidate is not None and (candidate / "include" / "jni.h").is_file() and \
                (candidate / "include" / "win32" / "jni_md.h").is_file():
            return candidate.resolve()
    raise RuntimeError("A JDK with Windows JNI headers is required; pass --java-home.")


def _run(command: list[str], *, cwd: Path = ROOT) -> None:
    print("+ " + subprocess.list2cmdline(command), flush=True)
    subprocess.run(command, cwd=cwd, check=True)


def _source_bundle(target: Path) -> None:
    paths = [ROOT / "native-graphics", ROOT / "graphics", ROOT / "tools" / "build_graphics.py", ROOT / "LICENSE"]
    with zipfile.ZipFile(target, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
        for source in paths:
            files = sorted(source.rglob("*") if source.is_dir() else [source])
            for path in files:
                if not path.is_file():
                    continue
                if path.is_symlink() or path.is_junction():
                    raise RuntimeError(f"Source package cannot include redirected path: {path}")
                if "__pycache__" in path.parts:
                    continue
                archive.write(path, path.relative_to(ROOT).as_posix())


def _artifact_bin(build_dir: Path, configuration: str) -> Path:
    candidates = (build_dir / "bin" / configuration, build_dir / "bin", build_dir / configuration)
    for directory in candidates:
        if (directory / "revival_graphics.dll").is_file() and \
                (directory / "revival_graphics_jni.dll").is_file():
            return directory
    raise FileNotFoundError("CMake did not produce both common graphics DLLs in its runtime output folder.")


def build(args: argparse.Namespace) -> Path:
    if os.name != "nt":
        raise RuntimeError("The common graphics runtime currently builds on Windows x64 only.")
    cmake = _cmake(args.cmake)
    ctest = shutil.which("ctest") or str(Path(cmake).with_name("ctest.exe"))
    if not Path(ctest).is_file():
        raise RuntimeError("CTest must be installed with CMake to run native contract checks.")
    java_home = _java_home(args.java_home)
    build_dir = args.build_dir.resolve()
    runtime_root = args.runtime_root.resolve()
    if ROOT not in build_dir.parents or ROOT not in runtime_root.parents:
        raise ValueError("Build and runtime output folders must stay inside this repository.")
    build_dir.mkdir(parents=True, exist_ok=True)
    runtime_root.mkdir(parents=True, exist_ok=True)

    configure = [cmake, "-S", str(ROOT / "native-graphics"), "-B", str(build_dir),
                 "-DBUILD_TESTING=ON", "-DRV_BUILD_GL_HARNESS=OFF", f"-DJAVA_HOME={java_home}"]
    if args.generator:
        configure.extend(["-G", args.generator])
        if args.generator.startswith("Visual Studio"):
            configure.extend(["-A", "x64"])
    _run(configure)
    _run([cmake, "--build", str(build_dir), "--config", args.configuration, "--parallel", "1"])
    _run([ctest, "--test-dir", str(build_dir), "-C", args.configuration,
          "--output-on-failure"])

    abi_id = (build_dir / "abi-build-id.txt").read_text(encoding="ascii").strip()
    binary_dir = _artifact_bin(build_dir, args.configuration)
    common = runtime_root / "common"
    common.mkdir(parents=True, exist_ok=True)
    for name in ("revival_graphics.dll", "revival_graphics_jni.dll"):
        shutil.copy2(binary_dir / name, common / name)
    shutil.copy2(build_dir / "abi-build-id.txt", common / "abi-build-id.txt")
    shutil.copy2(ROOT / "native-graphics" / "dependencies.json", common / "dependencies.json")
    shutil.copy2(ROOT / "native-graphics" / "README.md", common / "BUILDING.md")
    shutil.copy2(ROOT / "LICENSE", runtime_root / "LICENSE")
    (runtime_root / "README.txt").write_text(
        "JAR Revival shared graphics runtime (Windows x64)\n\n"
        "The common folder contains one shared renderer DLL and its Java JNI shim.\n"
        "The jme and nes folders contain portable manifests that point to that same DLL.\n"
        "This package does not include an emulator, game, ROM, save, or game artwork.\n"
        "See common/BUILDING.md and common/dependencies.json for build requirements.\n",
        encoding="utf-8")
    source = runtime_root / "source" / "native-graphics-source.zip"
    source.parent.mkdir(parents=True, exist_ok=True)
    _source_bundle(source)

    from graphics.runtime_package import validate_runtime_tree, write_consumer_manifests

    write_consumer_manifests(runtime_root, abi_id)
    identity = validate_runtime_tree(runtime_root)
    package = runtime_root / "JAR-Revival-CommonGraphics-Windows-x64.zip"
    with zipfile.ZipFile(package, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
        for folder in ("common", "jme", "nes", "source"):
            for path in sorted((runtime_root / folder).rglob("*")):
                if path.is_file():
                    archive.write(path, f"JAR-Revival-CommonGraphics/{path.relative_to(runtime_root).as_posix()}")
        for name in ("LICENSE", "README.txt"):
            archive.write(runtime_root / name, f"JAR-Revival-CommonGraphics/{name}")
    from graphics.runtime_package import validate_runtime_archive
    validate_runtime_archive(package)
    digest = hashlib.sha256(package.read_bytes()).hexdigest()
    package.with_name(package.name + ".sha256").write_text(f"{digest}  {package.name}\n", encoding="ascii")
    print(f"Common graphics package validated: {package}")
    print(json.dumps(identity, sort_keys=True))
    return package


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cmake")
    parser.add_argument("--java-home", type=Path)
    parser.add_argument("--generator", help="Optional CMake generator, for example Visual Studio 17 2022")
    parser.add_argument("--configuration", default="Release", choices=("Debug", "Release"))
    parser.add_argument("--build-dir", type=Path, default=DEFAULT_BUILD)
    parser.add_argument("--runtime-root", type=Path, default=DEFAULT_RUNTIME)
    args = parser.parse_args()
    build(args)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
