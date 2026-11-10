from __future__ import annotations

import engine
import gpu_policy
from graphics.backend_preference import (
    COMMON_BACKEND, LEGACY_BACKEND, BackendPreference, GraphicsBackendPreferenceStore,
)
from graphics.jme_runtime import JmeGraphicsRuntime
from graphics.launch import GraphicsLaunchDecision, JmeGraphicsLaunchPolicy


class FakeProcess:
    pass


class FixedPreference:
    def __init__(self, backend):
        self.backend = backend

    def load(self):
        return BackendPreference(self.backend)


class FixedRuntimeLocator:
    def __init__(self, runtime):
        self.runtime = runtime
        self.last_error = None if runtime is not None else "runtime missing"

    def locate(self):
        return self.runtime


def test_renderer_preference_defaults_to_common_and_saves_only_known_values(tmp_path):
    store = GraphicsBackendPreferenceStore(tmp_path / "config" / "renderer.json")

    assert store.load() == BackendPreference(COMMON_BACKEND)
    assert store.save(LEGACY_BACKEND) == BackendPreference(LEGACY_BACKEND)
    assert store.load() == BackendPreference(LEGACY_BACKEND)


def test_invalid_renderer_preference_fails_closed_to_legacy(tmp_path):
    path = tmp_path / "renderer.json"
    path.write_text('{"schema":1,"backend":"unknown"}', encoding="utf-8")
    result = GraphicsBackendPreferenceStore(path).load()

    assert result.backend == LEGACY_BACKEND
    assert "invalid" in result.warning.lower()


def test_launch_policy_only_adds_common_runtime_to_gpu_sessions(tmp_path):
    runtime = JmeGraphicsRuntime(
        tmp_path, tmp_path / "jme" / "graphics-runtime.json",
        tmp_path / "common" / "revival_graphics.dll",
        tmp_path / "common" / "revival_graphics_jni.dll", "a" * 64)
    policy = JmeGraphicsLaunchPolicy(FixedPreference(COMMON_BACKEND), FixedRuntimeLocator(runtime))

    decision = policy.for_renderer("gpu")
    assert decision == GraphicsLaunchDecision(COMMON_BACKEND, (
        "-Djme.graphicsBackend=common-v1",
        f"-Djme.graphicsCore={runtime.core}",
        f"-Djme.graphicsShim={runtime.shim}",
    ))
    assert policy.for_renderer("cpu") is None


def test_missing_runtime_records_prestart_compatibility_fallback():
    locator = FixedRuntimeLocator(None)
    policy = JmeGraphicsLaunchPolicy(FixedPreference(COMMON_BACKEND), locator)
    decision = policy.for_renderer("gpu")

    assert decision.requested_backend == COMMON_BACKEND
    assert decision.arguments == ("-Djme.graphicsBackend=common-v1",)
    assert "Legacy GPU" in decision.fallback_reason


def test_library_applies_backend_policy_only_to_gpu_and_logs_fallback(tmp_path, monkeypatch):
    app_root = tmp_path / "app"
    (app_root / "build").mkdir(parents=True)
    (app_root / "build" / "jme-player.jar").touch()
    (app_root / "tools" / "gpu").mkdir(parents=True)
    (app_root / "tools" / "gpu" / "lwjgl-test.jar").touch()
    monkeypatch.setattr(engine, "ROOT", app_root)
    monkeypatch.setattr(engine, "locate_java", lambda *args: "java-cpu")
    monkeypatch.setattr(gpu_policy, "settings", lambda: {"render_gpu": "system"})
    monkeypatch.setattr(gpu_policy, "player_java", lambda mode: "java-gpu")
    monkeypatch.setattr(gpu_policy, "expected_device", lambda mode: None)

    captured = {}

    def popen(args, **kwargs):
        captured["args"] = args
        return FakeProcess()

    monkeypatch.setattr(engine.subprocess, "Popen", popen)

    class Policy:
        def for_renderer(self, renderer):
            if renderer == "cpu":
                return None
            return GraphicsLaunchDecision(
                COMMON_BACKEND, ("-Djme.graphicsBackend=common-v1",),
                "No verified runtime; Legacy GPU compatibility is used.")

    rendered = tmp_path / "rendered"
    library = engine.Library(tmp_path / "library", rendered,
                             graphics_launch_policy=Policy())
    folder = rendered / ("c" * 64)
    (folder / "saves").mkdir(parents=True)
    (folder / "logs").mkdir()
    game = {
        "id": "c" * 64, "name": "Fixture", "status": "Ready", "renderer": "gpu",
        "backend": "pixel", "width": 176, "height": 220,
        "startup": {"note": "fixture start"},
    }
    library.prepare_startup = lambda current: tmp_path / "game.jar"

    _, log = library._launch(game)
    assert "-Djme.graphicsBackend=common-v1" in captured["args"]
    assert "Graphics backend requested: common-v1" in log.read_text(encoding="utf-8")
    assert "Graphics backend fallback: No verified runtime" in log.read_text(encoding="utf-8")

    game["renderer"] = "cpu"
    library._launch(game)
    assert "-Djme.graphicsBackend=common-v1" not in captured["args"]
