import json
from pathlib import Path
from types import SimpleNamespace

from PIL import Image

from systems.nes.embedded import core, pack as embedded_pack
from systems.nes.embedded.api import RevivalHdPlaybackStatus


def make_pack(workspace: Path, scale=4):
    generation = "a" * 32
    folder = workspace / "nes" / "packs" / generation
    folder.mkdir(parents=True)
    tile_size = 8 * scale
    Image.new("RGBA", (tile_size, tile_size), (255, 20, 10, 255)).save(
        folder / "page-000.png", format="PNG")
    selector = "rom:1,00000001"
    binding = {
        "binding_id": selector + "|sprite",
        "selector": selector,
        "source_id": "b" * 64,
    }
    tile = {**binding, "page": 0, "x": 0, "y": 0, "size": tile_size}
    (folder / "hires.txt").write_text(
        f"<ver>109\n<scale>{scale}\n<img>page-000.png\n"
        "<tile>0,1,00000001,0,0,1,N\n", encoding="ascii")
    (folder / "manifest.json").write_text(json.dumps({
        "schema": 1, "recipe_key": "recipe", "pack_scale": scale,
        "bindings": [binding], "generation": generation,
        "pages": ["page-000.png"], "tiles": [tile],
    }), encoding="utf-8")
    return folder / "manifest.json"


def test_pack_validation_and_staging_are_bounded(tmp_path):
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    pack = make_pack(workspace)
    files = embedded_pack._pack_files(pack, workspace, 4)
    assert [name for name, _path in files] == ["hires.txt", "page-000.png"]

    home = tmp_path / "home"
    home.mkdir()
    rom = tmp_path / "game.nes"
    rom.write_bytes(b"rom")
    staged = embedded_pack._stage_pack(pack, workspace, home, rom, 4)
    assert (staged / "hires.txt").is_file()
    assert (staged / "page-000.png").read_bytes() == (pack.parent / "page-000.png").read_bytes()
    assert not (staged / "manifest.json").exists()


def test_pack_rejects_definition_that_disagrees_with_manifest(tmp_path):
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    pack = make_pack(workspace)
    (pack.parent / "hires.txt").write_text(
        "<ver>109\n<scale>4\n<img>page-000.png\n<tile>0,1,00000001,32,0,1,N\n",
        encoding="ascii")
    try:
        embedded_pack._pack_files(pack, workspace, 4)
    except ValueError as error:
        assert "definitions" in str(error)
    else:
        raise AssertionError("a pack with a mismatched tile placement was accepted")


class FakeDirectory:
    def close(self):
        pass


class FakeMesenApi:
    def __init__(self):
        self.calls = []

    def __getattr__(self, name):
        if name in ("SetRevivalSaveFolder", "SetRevivalNesControls", "LoadRom",
                    "StartRevivalNesCapture"):
            return lambda *args: self._true(name, *args)
        if name == "GetKeyCode":
            return lambda value: self._key_code(value)
        if name == "hd_playback_status":
            return self._status
        return lambda *args: self.calls.append((name, args))

    def _true(self, name, *args):
        self.calls.append((name, args))
        return True

    def _key_code(self, value):
        self.calls.append(("GetKeyCode", value))
        return len([call for call in self.calls if call[0] == "GetKeyCode"])

    def _status(self):
        status = RevivalHdPlaybackStatus()
        status.struct_bytes = 640
        status.abi_version = 2
        status.pack_loaded = 1
        status.pack_scale = 4
        status.common_renderer_active = 1
        status.presented_frames = 1
        status.graphics_build_id = b"a" * 64
        return status


def test_native_host_reuses_core_until_application_shutdown(tmp_path, monkeypatch):
    monkeypatch.setattr(core.os, "name", "nt")
    monkeypatch.setattr(core.os, "add_dll_directory", lambda _path: FakeDirectory(), raising=False)
    api = FakeMesenApi()
    host = core.MesenCoreHost(tmp_path / "MesenCore.dll", tmp_path,
                                   api_factory=lambda _path: api)
    session = SimpleNamespace(
        home=tmp_path / ".runtime" / "sessions" / ("d" * 32) / "home",
        saves=tmp_path / "saves", rom_path=tmp_path / "game.nes",
        events=tmp_path / "events", controls={
            "UP": "ARROW_UP", "DOWN": "ARROW_DOWN", "LEFT": "ARROW_LEFT",
            "RIGHT": "ARROW_RIGHT", "A": "KEY_X", "B": "KEY_Z",
            "START": "ENTER", "SELECT": "SPACE",
        }, game_id="c" * 64, session_id="d" * 32, pack_scale=4,
    )
    session.home.mkdir(parents=True)
    session.saves.mkdir()
    session.events.mkdir()

    keys, status = host.start(session, 10, 11, original=True, capture=False).result(timeout=5)
    assert status is None and keys["A"] != keys["B"]
    host.stop().result(timeout=5)
    keys, status = host.start(session, 10, 11, original=False, capture=True).result(timeout=5)
    assert status.presented_frames == 1
    host.stop().result(timeout=5)
    host.shutdown(timeout=5)

    names = [name for name, _args in api.calls if name != "GetKeyCode"]
    assert names.count("InitDll") == 1
    assert names.count("LoadRom") == 2
    assert names.count("Release") == 1
    assert next(args[0] for name, args in api.calls if name == "StartRevivalNesCapture") == str(
        tmp_path / ".runtime" / "nes-capture")
