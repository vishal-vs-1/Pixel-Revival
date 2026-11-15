from __future__ import annotations

from systems.jme import library as engine, playback
from graphics import gpu_policy as gpu_policy
class FakeProcess:
    pass


def test_library_uses_legacy_gpu_and_keeps_cpu_compatibility(tmp_path, monkeypatch):
    app_root = tmp_path / "app"
    (app_root / "build").mkdir(parents=True)
    (app_root / "build" / "jme-player.jar").touch()
    (app_root / "tools" / "gpu").mkdir(parents=True)
    (app_root / "tools" / "gpu" / "lwjgl-test.jar").touch()
    monkeypatch.setattr(playback, "ROOT", app_root)
    monkeypatch.setattr(playback, "locate_java", lambda *args: "java-cpu")
    monkeypatch.setattr(gpu_policy, "settings", lambda: {"render_gpu": "system"})
    monkeypatch.setattr(gpu_policy, "player_java", lambda mode: "java-gpu")
    monkeypatch.setattr(gpu_policy, "expected_device", lambda mode: None)

    captured = {}

    def popen(args, **kwargs):
        captured["args"] = args
        return FakeProcess()

    monkeypatch.setattr(playback.subprocess, "Popen", popen)

    rendered = tmp_path / "rendered"
    library = engine.Library(tmp_path / "library", rendered)
    folder = rendered / ("c" * 64)
    (folder / "saves").mkdir(parents=True)
    (folder / "logs").mkdir()
    game = {
        "id": "c" * 64, "name": "Fixture", "status": "Ready", "renderer": "gpu",
        "backend": "pixel", "width": 176, "height": 220,
        "startup": {"note": "fixture start"},
    }
    library.prepare_startup = lambda current: tmp_path / "game.jar"

    library._launch(game)
    assert "-Djme.graphicsBackend=legacy" in captured["args"]
    assert not any(arg.startswith("-Djme.playerPoses=") for arg in captured["args"])

    library._launch(game, pose_mode="record")
    assert f"-Djme.playerPoses={folder / 'player-poses'}" in captured["args"]
    assert "-Djme.recordPoses=true" in captured["args"]

    game["renderer"] = "cpu"
    library._launch(game)
    assert "-Djme.graphicsBackend=legacy" not in captured["args"]
