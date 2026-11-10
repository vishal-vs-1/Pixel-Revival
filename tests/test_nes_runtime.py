import hashlib
import json
import os
from pathlib import Path
import subprocess

import pytest

from graphics.gpu import apply_advisory_preference
from graphics.runtime import GraphicsHandshake, GraphicsRuntimeManifest, GraphicsRuntimeResolver
from runtime_assets.contracts import pixel_recipe
from systems.nes.pack import NesPackGenerator
from systems.nes.runtime import NesManagedRuntime, NesProcessRuntime, NesSessionFactory


def _graphics(tmp_path):
    root = tmp_path / "graphics"
    root.mkdir()
    dll = root / "revival_graphics.dll"
    dll.write_bytes(b"fixture graphics dll")
    build_id = hashlib.sha256(dll.read_bytes()).hexdigest()
    manifest = root / "graphics-runtime.json"
    manifest.write_text(json.dumps({
        "schema": 1, "protocol": "common-v1", "abi_version": 1,
        "build_id": build_id, "dll": str(dll),
    }), encoding="utf-8")
    return root, dll, manifest, build_id


def test_graphics_runtime_verifies_bytes_and_handshake(tmp_path):
    root, dll, manifest, build_id = _graphics(tmp_path)
    resolved = GraphicsRuntimeResolver(root).resolve(manifest)
    assert resolved.dll == dll.resolve()
    handshake = GraphicsHandshake(1, build_id, "NVIDIA GeForce RTX 2050")
    assert GraphicsRuntimeResolver.verify_handshake(resolved, handshake) == handshake
    dll.write_bytes(b"changed")
    with pytest.raises(ValueError, match="does not match"):
        GraphicsRuntimeResolver(root).resolve(manifest)


def test_session_factory_keeps_original_and_enhanced_modes_separate(tmp_path):
    root, dll, graphics_path, _build_id = _graphics(tmp_path)
    identity = "a" * 64
    nes_root = tmp_path / "nes"
    workspace = nes_root / "rendered-games" / identity
    source = nes_root / "library" / identity / "game.nes"
    source.parent.mkdir(parents=True)
    source.write_bytes(b"valid rom bytes")
    pack = workspace / "nes" / "packs" / "pack" / "manifest.json"
    pack.parent.mkdir(parents=True)
    pack.write_text("{}", encoding="utf-8")
    runtime_manifest = GraphicsRuntimeManifest(1, "common-v1", 1, _build_id, dll)
    factory = NesSessionFactory(session_id_factory=lambda: "b" * 32)
    original = factory.create(workspace, {"id": identity}, source, runtime_manifest,
                              original=True, graphics_manifest_path=graphics_path)
    assert original.mode == "original" and original.pack_manifest is None
    assert original.document()["controls"] == {
        "UP": "ARROW_UP", "DOWN": "ARROW_DOWN", "LEFT": "ARROW_LEFT",
        "RIGHT": "ARROW_RIGHT", "A": "KEY_X", "B": "KEY_Z",
        "START": "ENTER", "SELECT": "SPACE",
    }
    with pytest.raises(TypeError):
        original.controls["A"] = "KEY_Q"
    copied_document = original.document()
    copied_document["controls"]["A"] = "KEY_Q"
    assert original.controls["A"] == "KEY_X"
    with pytest.raises(ValueError, match="snapshot"):
        factory.create(workspace, {"id": identity}, source, runtime_manifest, original=False,
                       graphics_manifest_path=graphics_path)


def test_invalid_controls_fail_before_creating_session_directories(tmp_path):
    root, dll, graphics_path, build_id = _graphics(tmp_path)
    identity = "1" * 64
    workspace = tmp_path / "nes" / "rendered-games" / identity
    source = tmp_path / "nes" / "library" / identity / "game.nes"
    source.parent.mkdir(parents=True)
    source.write_bytes(b"rom")
    factory = NesSessionFactory(session_id_factory=lambda: "2" * 32)
    runtime_manifest = GraphicsRuntimeManifest(1, "common-v1", 1, build_id, dll)

    with pytest.raises(ValueError, match="Missing actions"):
        factory.create(workspace, {"id": identity}, source, runtime_manifest,
                       original=True, controls={"UP": "ARROW_UP"},
                       graphics_manifest_path=graphics_path)
    assert not (workspace / ".runtime" / "sessions").exists()


def test_separate_sessions_share_the_persistent_mesen_save_root(tmp_path):
    root, dll, graphics_path, build_id = _graphics(tmp_path)
    identity = "5" * 64
    workspace = tmp_path / "nes" / "rendered-games" / identity
    source = tmp_path / "nes" / "library" / identity / "game.nes"
    source.parent.mkdir(parents=True)
    source.write_bytes(b"rom")
    runtime_manifest = GraphicsRuntimeManifest(1, "common-v1", 1, build_id, dll)
    ids = iter(("6" * 32, "7" * 32))
    factory = NesSessionFactory(session_id_factory=lambda: next(ids))

    first = factory.create(workspace, {"id": identity}, source, runtime_manifest,
                           original=True, graphics_manifest_path=graphics_path)
    save = first.saves / "battery" / "progress.sav"
    save.parent.mkdir(parents=True)
    save.write_bytes(b"progress")
    second = factory.create(workspace, {"id": identity}, source, runtime_manifest,
                            original=True, graphics_manifest_path=graphics_path)

    assert first.session_id != second.session_id
    assert first.saves == second.saves == workspace / "saves" / "mesen"
    assert save.read_bytes() == b"progress"
    assert not (first.home.parent / "saves").exists()


def test_runtime_uses_argument_list_and_shell_disabled(tmp_path):
    root, dll, graphics_path, build_id = _graphics(tmp_path)
    identity = "c" * 64
    workspace = tmp_path / "nes" / "rendered-games" / identity
    source = tmp_path / "nes" / "library" / identity / "game.nes"
    source.parent.mkdir(parents=True)
    source.write_bytes(b"rom")
    factory = NesSessionFactory(session_id_factory=lambda: "d" * 32)
    manifest = factory.create(workspace, {"id": identity}, source,
                              GraphicsRuntimeManifest(1, "common-v1", 1, build_id, dll),
                              original=True, graphics_manifest_path=graphics_path)
    executable = tmp_path / "Mesen.exe"
    executable.write_bytes(b"exe")
    calls = []

    class Process:
        def wait(self, timeout):
            return 0

    def launch(args, **kwargs):
        calls.append((args, kwargs))
        return Process()

    process, _log = NesProcessRuntime(executable, process_factory=launch).launch(manifest)
    assert process is not None
    assert calls[0][0] == [str(executable.resolve()), f"--revival-launch={manifest.home.parent / 'launch.json'}"]
    assert calls[0][1]["shell"] is False
    assert NesProcessRuntime.close(process) == 0


def test_managed_runtime_uses_portable_shared_dll_search_path(tmp_path):
    install = tmp_path / "portable-runtime"
    common = install / "common"
    common.mkdir(parents=True)
    dll = common / "revival_graphics.dll"
    dll.write_bytes(b"shared renderer")
    (common / "revival_graphics_jni.dll").write_bytes(b"jni shim")
    nes_folder = install / "nes"
    nes_folder.mkdir()
    graphics_path = nes_folder / "graphics-runtime.json"
    graphics_path.write_text(json.dumps({
        "schema": 1, "protocol": "common-v1", "abi_version": 1,
        "build_id": hashlib.sha256(dll.read_bytes()).hexdigest(),
        "dll": "../common/revival_graphics.dll",
    }), encoding="utf-8")
    identity = "9" * 64
    nes_root = tmp_path / "nes-data"
    source = nes_root / "library" / identity / "game.nes"
    source.parent.mkdir(parents=True)
    source.write_bytes(b"rom")
    workspace = nes_root / "rendered-games" / identity
    workspace.mkdir(parents=True)
    executable = install / "player" / "Mesen.exe"
    executable.parent.mkdir()
    executable.write_bytes(b"exe")
    calls = []

    class Process:
        pass

    def launch(args, **kwargs):
        calls.append((args, kwargs))
        return Process()

    runtime = NesManagedRuntime(
        executable, graphics_path,
        session_factory=NesSessionFactory(session_id_factory=lambda: "8" * 32),
        process_factory=launch,
    )
    runtime.launch({"id": identity}, source, workspace, original=True)
    child_path = calls[0][1]["env"]["PATH"].split(os.pathsep)
    assert child_path[0] == str(common.resolve())
    assert "J2ME_REVIVAL_ENABLE_NES_ENHANCED" not in calls[0][1]["env"]
    assert "J2ME_REVIVAL_NES_CAPTURE" not in calls[0][1]["env"]
    assert calls[0][1]["shell"] is False


def test_capture_availability_requires_explicit_runtime_feature(tmp_path):
    _root, _dll, graphics_path, _build_id = _graphics(tmp_path)
    executable = tmp_path / "Mesen.exe"
    executable.write_bytes(b"exe")
    runtime = NesManagedRuntime(executable, graphics_path)
    build_info = graphics_path.parent / "build-info.json"

    assert not runtime.capture_available()
    build_info.write_text(json.dumps({
        "schema": 1, "features": ["enhanced-pack-v1", "nes-tile-capture-rnt1"],
    }), encoding="utf-8")
    assert runtime.capture_available()


def test_enhanced_launch_enables_only_requested_child_capture(tmp_path, monkeypatch):
    _root, _dll, graphics_path, _build_id = _graphics(tmp_path)
    identity = "a" * 64
    nes_root = tmp_path / "nes-data"
    source = nes_root / "library" / identity / "game.nes"
    source.parent.mkdir(parents=True)
    source.write_bytes(b"rom")
    workspace = nes_root / "rendered-games" / identity
    workspace.mkdir(parents=True)
    (graphics_path.parent / "build-info.json").write_text(json.dumps({
        "schema": 1, "features": ["nes-tile-capture-rnt1"],
    }), encoding="utf-8")
    NesPackGenerator(workspace).ensure_capture_pack(pixel_recipe(), pack_scale=4)
    executable = tmp_path / "Mesen.exe"
    executable.write_bytes(b"exe")
    calls = []
    ids = iter(("b" * 32, "c" * 32, "e" * 32))
    runtime = NesManagedRuntime(
        executable, graphics_path,
        session_factory=NesSessionFactory(session_id_factory=lambda: next(ids)),
        process_factory=lambda args, **kwargs: calls.append((args, kwargs)) or object(),
    )
    monkeypatch.setenv("J2ME_REVIVAL_ENABLE_NES_ENHANCED", "0")
    monkeypatch.setenv("J2ME_REVIVAL_NES_CAPTURE", "1")

    runtime.launch({"id": identity, "render_scale": 4}, source, workspace,
                   original=False, capture=False)
    enhanced_environment = calls[-1][1]["env"]
    assert enhanced_environment["J2ME_REVIVAL_ENABLE_NES_ENHANCED"] == "1"
    assert "J2ME_REVIVAL_NES_CAPTURE" not in enhanced_environment

    runtime.launch({"id": identity, "render_scale": 4}, source, workspace,
                   original=False, capture=True)
    capture_environment = calls[-1][1]["env"]
    assert capture_environment["J2ME_REVIVAL_ENABLE_NES_ENHANCED"] == "1"
    assert capture_environment["J2ME_REVIVAL_NES_CAPTURE"] == "1"

    runtime.launch({"id": identity, "render_scale": 4}, source, workspace, original=True)
    original_environment = calls[-1][1]["env"]
    assert "J2ME_REVIVAL_ENABLE_NES_ENHANCED" not in original_environment
    assert "J2ME_REVIVAL_NES_CAPTURE" not in original_environment


def test_enhanced_launch_rejects_pack_scale_until_rebuilt(tmp_path):
    _root, _dll, graphics_path, _build_id = _graphics(tmp_path)
    identity = "d" * 64
    nes_root = tmp_path / "nes-data"
    source = nes_root / "library" / identity / "game.nes"
    source.parent.mkdir(parents=True)
    source.write_bytes(b"rom")
    workspace = nes_root / "rendered-games" / identity
    workspace.mkdir(parents=True)
    NesPackGenerator(workspace).ensure_capture_pack(pixel_recipe(), pack_scale=2)
    executable = tmp_path / "Mesen.exe"
    executable.write_bytes(b"exe")
    runtime = NesManagedRuntime(executable, graphics_path,
                                process_factory=lambda *args, **kwargs: object())

    with pytest.raises(ValueError, match="being prepared"):
        runtime.launch({"id": identity, "render_scale": 4}, source, workspace,
                       original=False)
    assert not (workspace / ".runtime" / "sessions").exists()


def test_fallback_reason_is_flushed_before_child_starts(tmp_path):
    root, dll, graphics_path, build_id = _graphics(tmp_path)
    identity = "3" * 64
    workspace = tmp_path / "nes" / "rendered-games" / identity
    source = tmp_path / "nes" / "library" / identity / "game.nes"
    source.parent.mkdir(parents=True)
    source.write_bytes(b"rom")
    manifest = NesSessionFactory(session_id_factory=lambda: "4" * 32).create(
        workspace, {"id": identity}, source,
        GraphicsRuntimeManifest(1, "common-v1", 1, build_id, dll),
        original=True, graphics_manifest_path=graphics_path)
    executable = tmp_path / "Mesen.exe"
    executable.write_bytes(b"exe")
    log = manifest.home.parent / "runtime.log"

    def launch(_args, **_kwargs):
        assert log.read_text(encoding="utf-8") == (
            "NES controls fell back to Default: selected slot-3 is no longer saved\n"
        )
        return object()

    NesProcessRuntime(executable, process_factory=launch).launch(
        manifest, controls_fallback_reason="selected slot-3 is no longer saved")


def test_gpu_preference_targets_only_the_private_executable(tmp_path):
    executable = tmp_path / "runtime.exe"
    executable.write_bytes(b"exe")
    calls = []

    class Writer:
        def set(self, path, mode):
            calls.append(("set", path, mode))

        def clear(self, path):
            calls.append(("clear", path))

    result = apply_advisory_preference(executable, "dedicated", Writer())
    assert result.applied and calls == [("set", executable.resolve(), "dedicated")]


def test_close_timeout_is_reported_without_forced_kill(tmp_path):
    class Hung:
        def wait(self, timeout):
            raise subprocess.TimeoutExpired("Mesen", timeout)

    with pytest.raises(TimeoutError):
        NesProcessRuntime.close(Hung(), timeout=0.01)


def test_managed_runtime_requires_the_published_pack_snapshot(tmp_path):
    root, dll, graphics_path, build_id = _graphics(tmp_path)
    identity = "e" * 64
    workspace = tmp_path / "nes" / "rendered-games" / identity
    source = tmp_path / "nes" / "library" / identity / "game.nes"
    source.parent.mkdir(parents=True)
    source.write_bytes(b"rom")
    executable = tmp_path / "Mesen.exe"
    executable.write_bytes(b"exe")
    for name in ("MesenCore.dll", "libSkiaSharp.dll", "libHarfBuzzSharp.dll"):
        (tmp_path / name).write_bytes(b"managed runtime dependency")
    runtime = NesManagedRuntime(executable, graphics_path,
                                session_factory=NesSessionFactory(session_id_factory=lambda: "f" * 32),
                                process_factory=lambda *args, **kwargs: None)
    assert runtime.available()
    with pytest.raises(ValueError, match="pack snapshot"):
        runtime.launch({"id": identity}, source, workspace, original=False)


def test_managed_runtime_is_unavailable_without_required_frontend_libraries(tmp_path):
    _, _, graphics_path, _ = _graphics(tmp_path)
    executable = tmp_path / "Mesen.exe"
    executable.write_bytes(b"exe")
    (tmp_path / "MesenCore.dll").write_bytes(b"core")
    (tmp_path / "libSkiaSharp.dll").write_bytes(b"skia")

    runtime = NesManagedRuntime(executable, graphics_path)

    assert not runtime.available()
