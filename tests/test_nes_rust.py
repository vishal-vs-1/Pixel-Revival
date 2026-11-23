import tkinter as tk
from pathlib import Path
from unittest.mock import Mock

import pytest

from systems.nes import rust_core
from systems.nes.rust_core import RustCore
from systems.nes.rust_player import RustAudioOut, RustNesRuntime

DLL = rust_core.find_dll()
needs_dll = pytest.mark.skipif(DLL is None, reason="rust-nes DLL is not built")


def minimal_nrom():
    header = bytearray(16)
    header[0:4] = b"NES\x1a"
    header[4] = 1  # 1 x 16 KiB PRG
    header[5] = 1  # 1 x 8 KiB CHR
    return bytes(header) + bytes(16384) + bytes(8192)


@needs_dll
def test_garbage_rom_is_rejected_with_a_message():
    core = RustCore(DLL)
    with pytest.raises(Exception, match="[Rr]ejected|nrecognized|nvalid"):
        core.create(b"not a rom")


@needs_dll
def test_minimal_nrom_runs_frames_and_round_trips_state():
    core = RustCore(DLL)
    handle = core.create(minimal_nrom())
    try:
        assert core.region(handle) == 0
        assert core.mapper_id(handle) == 0
        assert core.battery_size(handle) == -1
        for _ in range(5):
            core.run_frame(handle)
        pixels, width, height = core.framebuffer(handle)
        assert (width, height) == (256, 240)
        assert len(pixels) == 256 * 240 * 4
        blob = core.save_state(handle)
        assert len(blob) > 100
        count = core.frame_count(handle)
        assert count == 5
        core.run_frame(handle)
        assert core.frame_count(handle) == count + 1
        core.load_state(handle, blob)
        assert core.frame_count(handle) == count
        assert core.is_alive(handle) and not core.is_alive(0)
    finally:
        core.destroy(handle)
    core.destroy(None)


@needs_dll
def test_audio_feeder_opens_and_closes_without_raising(tmp_path):
    del tmp_path
    out = RustAudioOut()
    out.start()
    try:
        out.feed(bytes(2048 * 2))
    finally:
        out.stop()


def _display():
    try:
        root = tk.Tk()
    except tk.TclError as error:
        pytest.skip(f"Tk display unavailable: {error}")
    root.withdraw()
    return root


@needs_dll
def test_rust_player_runs_inside_its_own_tk_window(tmp_path):
    import time

    root = _display()
    try:
        rom_path = tmp_path / "game.nes"
        rom_path.write_bytes(minimal_nrom())
        workspace = tmp_path / "workspace"
        (workspace / "saves").mkdir(parents=True)
        runtime = RustNesRuntime()
        game = {"id": "a" * 64, "name": "Tiny NROM", "render_scale": 4}
        player, log = runtime.launch(game, rom_path, workspace, original=True,
                                     host_parent=root)
        try:
            deadline = time.monotonic() + 10
            while player.poll() is None and time.monotonic() < deadline:
                root.update()
                time.sleep(0.02)
                if player._frames >= 20:
                    break
            assert player.poll() is None, "player died during startup"
            assert player._frames >= 20, "no frames pumped"
            assert player.window.winfo_exists()
            player.toggle_pause()
            player.toggle_pause()
        finally:
            player.shutdown(timeout=10)
        assert player.poll() == 0
        assert not (workspace / "saves" / "mesen" / "battery" / "game.sav").exists()
    finally:
        root.destroy()


def test_enhanced_launch_uses_the_rust_runtime(tmp_path):
    from systems.contracts import NES, ImportRequest
    from systems.nes.library import NesLibrary

    source = tmp_path / "cart.nes"
    source.write_bytes(b"NES\x1a\x01" + bytes(11) + bytes(16384) + b"extra")
    play = Mock()
    play.supports_enhanced = True
    play.available.return_value = True
    library = NesLibrary(tmp_path / "nes", asset_service=Mock(), runtime=play)
    game = library.import_game(ImportRequest(NES, source, "pixel"))
    process = Mock()
    process.poll.return_value = None
    play.launch.return_value = (process, tmp_path / "play.log")

    library.launch(game, original=True, host_parent=Mock())
    assert play.launch.called
    process.poll.return_value = 0

    play.launch.reset_mock()
    library.launch(game, original=False, host_parent=Mock())
    assert play.launch.called


def test_control_profile_keys_map_to_actions():
    from systems.nes.rust_player import _controls_to_keys

    assert _controls_to_keys({"A": "KEY_Q", "UP": "ARROW_DOWN"}) == {
        "q": "A", "down": "UP"}


def test_pack_page_names_cannot_escape_the_pack_folder(tmp_path):
    import json

    from systems.nes.rust_hd import load_pack_tiles

    workspace = tmp_path / "workspace"
    generation = "c" * 32
    pack = workspace / "nes" / "packs" / generation
    pack.mkdir(parents=True)
    (workspace / ".runtime").mkdir()
    (workspace / ".runtime" / "nes-pack.json").write_text(json.dumps({
        "schema": 1, "generation": generation, "background_priority": 2,
        "manifest": f"nes/packs/{generation}/manifest.json", "pack_scale": 4,
    }), encoding="utf-8")
    (pack / "manifest.json").write_text(json.dumps({
        "schema": 1, "generation": generation, "pack_scale": 4,
        "pages": ["../outside.png"],
        "tiles": [{"binding_id": "rom:0,00000000|background",
                   "selector": "rom:0,00000000", "page": 0,
                   "x": 0, "y": 0, "size": 32}],
    }), encoding="utf-8")
    (workspace / "nes" / "outside.png").write_bytes(b"not a page")
    assert load_pack_tiles(workspace) is None


def test_pack_selector_parsing_matches_pack_grammar():
    from systems.nes.rust_hd import parse_selector

    assert parse_selector("rom:7FFFFFFF,FFFFFFFF") == ("rom", 0x7FFFFFFF, 0xFFFFFFFF)
    assert parse_selector("rom:0,00000000") == ("rom", 0, 0)
    assert parse_selector("ram:" + "ab" * 16 + ",01020304") == ("ram", bytes.fromhex("ab" * 16), 0x01020304)
    assert parse_selector("rom:ZZZZ,00000000") is None
    assert parse_selector("ram:abcd,00000000") is None
    assert parse_selector("bogus") is None
    assert parse_selector("") is None


def _synthetic_pack(workspace, scale=4):
    from PIL import Image
    import json

    generation = "c" * 32
    pack = workspace / "nes" / "packs" / generation
    pack.mkdir(parents=True)
    tile = 8 * scale
    Image.new("RGBA", (tile, tile), (10, 20, 30, 255)).save(pack / "page-000.png")
    (pack / "hires.txt").write_text(
        f"<ver>109\n<scale>{scale}\n<img>page-000.png\n"
        f"<tile>0,7FFFFFFF,FFFFFFFF,0,0,1,N\n", encoding="ascii")
    manifest = {
        "schema": 1, "generation": generation, "pack_scale": scale,
        "recipe_key": "43e266fc57b618934894035b58726f6ad1556ad0cd985d809e323dba1d9b1f63",
        "pages": ["page-000.png"],
        "tiles": [{"binding_id": "rom:7FFFFFFF,FFFFFFFF|background",
                   "selector": "rom:7FFFFFFF,FFFFFFFF",
                   "source_id": "d" * 64, "page": 0, "x": 0, "y": 0,
                   "size": tile}],
        "bindings": [],
    }
    (pack / "manifest.json").write_text(json.dumps(manifest), encoding="utf-8")
    (workspace / ".runtime").mkdir(parents=True, exist_ok=True)
    (workspace / ".runtime" / "nes-pack.json").write_text(json.dumps({
        "schema": 1, "generation": generation, "background_priority": 2,
        "manifest": f"nes/packs/{generation}/manifest.json",
        "recipe_key": "43e266fc57b618934894035b58726f6ad1556ad0cd985d809e323dba1d9b1f63", "pack_scale": scale,
    }), encoding="utf-8")


@needs_dll
def test_pack_loader_feeds_the_hd_table(tmp_path):
    from systems.nes.rust_hd import load_pack_tiles

    workspace = tmp_path / "workspace"
    (workspace / "saves").mkdir(parents=True)
    assert load_pack_tiles(workspace) is None
    _synthetic_pack(workspace)
    loaded = load_pack_tiles(workspace)
    assert loaded is not None
    entries, scale = loaded
    assert scale == 4 and len(entries) == 1
    assert entries[0].image.size == (32, 32)

    from systems.nes.rust_core import RustCore

    core = RustCore(DLL)
    hd = core.hd_create(4)
    try:
        entry = entries[0]
        assert core.hd_add_tile(
            hd, sprite=entry.sprite, chr_index=entry.chr_index,
            palette=entry.palette, rgba=entry.image.tobytes(),
            size=entry.image.width) == 1
        width, height = core.hd_output_size(hd)
        assert (width, height) == (1024, 960)
    finally:
        core.hd_destroy(hd)


@needs_dll
def test_hd_run_frame_without_pack_matches_scaled_native(tmp_path):
    from systems.nes.rust_core import RustCore

    core = RustCore(DLL)
    handle = core.create(minimal_nrom())
    hd = core.hd_create(2)
    try:
        out_w, out_h = core.hd_output_size(hd)
        assert (out_w, out_h) == (512, 480)
        out = bytearray(out_w * out_h * 4)
        assert core.hd_run_frame(handle, hd, out) == len(out)
        # An empty table composites nothing: output must equal scaled native.
        for _ in range(5):
            core.hd_run_frame(handle, hd, out)
        from PIL import Image
        native = Image.frombytes("RGB", (256, 240), core.framebuffer(handle)[0],
                                 "raw", "BGRX")
        expected = native.resize((512, 480), Image.NEAREST)
        got = Image.frombytes("RGB", (512, 480), bytes(out), "raw", "BGRX")
        assert got.tobytes() == expected.tobytes()
    finally:
        core.hd_destroy(hd)
        core.destroy(handle)


def test_dll_lookup_honours_an_explicit_app_root(tmp_path):
    from systems.nes.rust_core import find_dll

    # A caller-supplied root is authoritative: tests and portable builds must
    # not silently borrow a DLL from an unrelated checkout.
    assert find_dll(tmp_path) is None


def test_framebuffer_decode_matches_xrgb_values():
    """The core exposes 0x00RRGGBB; our display must not swap red and blue."""
    import struct
    from PIL import Image

    value = 0x00142836  # R=0x14, G=0x28, B=0x36
    data = struct.pack("<I", value)
    image = Image.frombytes("RGB", (1, 1), data, "raw", "BGRX")
    assert image.getpixel((0, 0)) == (0x14, 0x28, 0x36)


@needs_dll
def test_comparison_opens_two_in_tool_players(tmp_path):
    import time

    root = _display()
    try:
        data = tmp_path / "nes"
        workspace = data / "rendered-games" / ("e" * 64)
        workspace.mkdir(parents=True)
        (workspace / "saves").mkdir()
        _synthetic_pack(workspace)
        rom = tmp_path / "cart.nes"
        rom.write_bytes(minimal_nrom())
        runtime = RustNesRuntime(app_root=Path.cwd())
        game = {"id": "e" * 64, "name": "Tiny NROM", "render_scale": 4,
                "backend": "pixel", "aspect": "4:3"}
        pair, _log = runtime.launch_comparison(game, rom, workspace,
                                               host_parent=root)
        try:
            assert pair.owns_window and type(pair).__name__ == "RustComparisonPlayers"
            original, enhanced = pair.players
            assert type(original).__name__ == type(enhanced).__name__ == "RustNesPlayer"
            assert original._hd is None, "Original pane must not composite a pack"
            assert enhanced._hd is not None, "Enhanced pane did not load the pack"
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline and min(p._frames for p in pair.players) < 5:
                root.update()
                time.sleep(0.02)
            assert pair.poll() is None, pair.startup_error
            assert min(p._frames for p in pair.players) >= 5
            # Both panes must use the same physical scale for a fair comparison.
            assert all(player._fit_exact for player in pair.players)
            root.update()
            assert pair.players[0]._painted_size == pair.players[1]._painted_size
        finally:
            pair.close()
            root.update()
        assert pair.poll() == 0
        assert all(player.poll() == 0 for player in pair.players)
    finally:
        root.destroy()
