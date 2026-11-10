from __future__ import annotations

import hashlib
import json
from pathlib import Path
import sys
import zipfile

import pytest
import shutil

from graphics.runtime import GraphicsRuntimeResolver
from graphics.runtime_package import (
    PACKAGE_ROOT,
    validate_runtime_archive,
    validate_runtime_tree,
    write_consumer_manifests,
)
from tools import package_windows


def _tree(root: Path) -> Path:
    runtime = root / "runtime"
    common = runtime / "common"
    common.mkdir(parents=True)
    (common / "revival_graphics.dll").write_bytes(b"common renderer")
    (common / "revival_graphics_jni.dll").write_bytes(b"java shim")
    (common / "abi-build-id.txt").write_text("a" * 64 + "\n", encoding="ascii")
    (common / "dependencies.json").write_text(json.dumps({
        "schema_version": 1, "target": "windows-x64",
        "nes_runtime": {"commit": "b" * 40},
    }), encoding="utf-8")
    (runtime / "LICENSE").write_text("license", encoding="utf-8")
    (runtime / "README.txt").write_text("common runtime", encoding="utf-8")
    source = runtime / "source" / "native-graphics-source.zip"
    source.parent.mkdir()
    with zipfile.ZipFile(source, "w") as archive:
        archive.writestr("native-graphics/CMakeLists.txt", "cmake")
        archive.writestr("native-graphics/include/revival_graphics.h", "header")
    write_consumer_manifests(runtime, "a" * 64)
    return runtime


def _archive(runtime: Path, archive_path: Path, extra=None):
    with zipfile.ZipFile(archive_path, "w", zipfile.ZIP_DEFLATED) as archive:
        for path in sorted(runtime.rglob("*")):
            if path.is_file():
                archive.write(path, f"{PACKAGE_ROOT}/{path.relative_to(runtime).as_posix()}")
        if extra:
            for name, value in extra:
                archive.writestr(f"{PACKAGE_ROOT}/{name}", value)
    return archive_path


def test_portable_manifests_resolve_one_shared_dll(tmp_path):
    runtime = _tree(tmp_path)
    identity = validate_runtime_tree(runtime)
    java = GraphicsRuntimeResolver(runtime).resolve(runtime / "jme" / "graphics-runtime.json")
    nes = GraphicsRuntimeResolver(runtime).resolve(runtime / "nes" / "graphics-runtime.json")
    assert java.dll == nes.dll == runtime / "common" / "revival_graphics.dll"
    assert identity["dll_sha256"] == hashlib.sha256(java.dll.read_bytes()).hexdigest()


def test_runtime_resolver_rejects_relative_path_escape(tmp_path):
    runtime = _tree(tmp_path)
    manifest = runtime / "nes" / "graphics-runtime.json"
    document = json.loads(manifest.read_text(encoding="utf-8"))
    document["dll"] = "../../outside.dll"
    manifest.write_text(json.dumps(document), encoding="utf-8")
    with pytest.raises(ValueError, match="outside"):
        GraphicsRuntimeResolver(runtime).resolve(manifest)


def test_runtime_archive_has_hash_matched_common_payload_and_sources(tmp_path):
    runtime = _tree(tmp_path)
    archive = _archive(runtime, tmp_path / "runtime.zip")
    identity = validate_runtime_archive(archive)
    assert identity["dll_sha256"] == hashlib.sha256(
        (runtime / "common" / "revival_graphics.dll").read_bytes()).hexdigest()


def test_runtime_archive_rejects_game_data(tmp_path):
    runtime = _tree(tmp_path)
    archive = _archive(runtime, tmp_path / "with-rom.zip", [("test.nes", b"ROM")])
    with pytest.raises(ValueError, match="game or save data"):
        validate_runtime_archive(archive)


def test_runtime_archive_rejects_mismatched_consumer_manifest(tmp_path):
    runtime = _tree(tmp_path)
    (runtime / "nes" / "graphics-runtime.json").write_text(json.dumps({
        "schema": 1, "protocol": "common-v1", "abi_version": 1,
        "build_id": "f" * 64, "dll": "../common/revival_graphics.dll",
    }), encoding="utf-8")
    archive = _archive(runtime, tmp_path / "bad.zip")
    with pytest.raises(ValueError, match="shared DLL"):
        validate_runtime_archive(archive)


def test_windows_packager_stages_only_verified_shared_graphics_runtime(tmp_path, monkeypatch):
    source = _tree(tmp_path)
    (tmp_path / "LICENSE").write_text("license", encoding="utf-8")
    expected_root = tmp_path / "build" / "common-runtime-u23-resize"
    expected_root.parent.mkdir(parents=True)
    shutil.copytree(source, expected_root)
    (expected_root / "common" / "BUILDING.md").write_text("build notes", encoding="utf-8")
    contents = tmp_path / "dist" / "JAR Revival" / "app"
    monkeypatch.setattr(package_windows, "ROOT", tmp_path)
    monkeypatch.setattr(package_windows, "CONTENTS", contents)

    package_windows.add_common_graphics_runtime()

    staged = contents / "tools" / "common-runtime"
    identity = validate_runtime_tree(staged)
    assert identity["dll_sha256"] == hashlib.sha256(
        (expected_root / "common" / "revival_graphics.dll").read_bytes()).hexdigest()
    assert sorted(path.name for path in (staged / "nes").iterdir()) == ["graphics-runtime.json"]
    assert not (staged / "nes" / "player").exists()
    assert (staged / "source" / "native-graphics-source.zip").is_file()


def test_portable_app_archive_checks_both_shared_renderer_manifests(tmp_path):
    runtime = _tree(tmp_path)
    readme = (
        "JAR Revival.exe Import Java ME demo Settings Shared GPU Legacy GPU CPU · compatibility "
        "Artwork tab AI setup Prepare data\\tools\\ai"
    )
    archive_path = tmp_path / "portable.zip"
    prefix = "JAR Revival/"
    with zipfile.ZipFile(archive_path, "w") as archive:
        archive.writestr(prefix + "JAR Revival.exe", b"exe")
        archive.writestr(prefix + "README.txt", readme)
        archive.writestr(prefix + "data/", b"")
        for name in (
            f"app/python{sys.version_info.major}{sys.version_info.minor}.dll",
            "app/build/jme-player.jar", "app/fixtures/RevivalDemo.jar",
            "app/tools/runtime/bin/java.exe", "app/tools/gpu/lwjgl-3.3.6.jar",
            "app/LICENSE", "app/THIRD-PARTY-LICENSES/Python-LICENSE.txt",
        ):
            archive.writestr(prefix + name, b"fixture")
        for path in sorted(runtime.rglob("*")):
            if path.is_file():
                archive.write(path, prefix + "app/tools/common-runtime/" + path.relative_to(runtime).as_posix())

    package_windows.verify_archive(archive_path)
