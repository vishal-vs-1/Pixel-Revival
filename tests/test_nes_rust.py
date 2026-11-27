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


def _sprite_test_nrom(large):
    """Draw asymmetric sprites with all OAM flip combinations on real PPU frames."""
    program = bytearray([0x78, 0xD8, 0xA2, 0xFF, 0x9A])

    def write(address, value):
        program.extend((0xA9, value, 0x8D, address & 255, address >> 8))

    write(0x2000, 0)
    write(0x2001, 0)
    write(0x2003, 0)
    # Clear all OAM, then install four visible instances.
    program.extend((0xA2, 0, 0xA9, 255, 0x8D, 4, 0x20, 0xE8, 0xD0, 0xFA))
    write(0x2003, 0)
    for index, flags in enumerate((0, 0x40, 0x80, 0xC0)):
        for value in (80, 2 if large else 1, flags, 32 + index * 48):
            write(0x2004, value)
    write(0x2006, 0x3F)
    write(0x2006, 0)
    palette = [15] * 32
    palette[17:20] = [22, 39, 24]
    for value in palette:
        write(0x2007, value)
    write(0x2000, 0x20 if large else 0)
    write(0x2001, 0x14)
    loop = 0x8000 + len(program)
    program.extend((0x4C, loop & 255, loop >> 8))
    prg = bytearray(16384)
    prg[:len(program)] = program
    prg[-6:] = bytes((0, 128, 0, 128, 0, 128))
    chr_data = bytearray(8192)
    for tile in (1, 2, 3):
        chr_data[tile * 16:tile * 16 + 16] = bytes(
            [((0x81 >> row) | (tile << row)) & 255 for row in range(8)] +
            [(0xC1 ^ (row * (tile + 5))) & 255 for row in range(8)])
    return minimal_nrom()[:16] + prg + chr_data


@needs_dll
@pytest.mark.parametrize('scale', [2, 4])
@pytest.mark.parametrize('large', [False, True])
def test_native_art_pack_preserves_real_sprite_pixels_and_all_flips(scale, large):
    from PIL import Image
    from systems.nes.tiles import Tile

    core = RustCore(DLL)
    handle = core.create(_sprite_test_nrom(large))
    hd = core.hd_create(scale)
    width, height = core.hd_output_size(hd)
    out = bytearray(width * height * 4)
    try:
        core.hd_arm(handle, True)
        core.capture_arm(handle, True)
        for _ in range(3):
            core.hd_run_frame(handle, hd, out)
        keys = core.capture_export(handle)
        assert any(key >> 63 for key in keys), 'fixture did not render sprites'
        palette = core.standard_palette()
        for key in keys:
            sprite, index, colors = bool(key >> 63), (key >> 32) & 0x7FFFFFFF, key & 0xFFFFFFFF
            tile = Tile(0, index, colors, core.tile_pattern(handle, index), False, sprite, sprite)
            art = tile.image(palette).resize((8 * scale, 8 * scale), Image.Resampling.NEAREST)
            core.hd_add_tile(hd, sprite=sprite, chr_index=index, palette=colors,
                             rgba=art.tobytes(), size=8 * scale)
        core.hd_run_frame(handle, hd, out)
        native = Image.frombytes('RGB', (256, 240), core.framebuffer(handle)[0], 'raw', 'BGRX')
        enhanced = Image.frombytes('RGB', (width, height), bytes(out), 'raw', 'BGRX')
        assert core.hd_stats(hd)[1] > 0
        assert enhanced.tobytes() == native.resize((width, height), Image.Resampling.NEAREST).tobytes()
    finally:
        core.hd_destroy(hd)
        core.destroy(handle)


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


@needs_dll
def test_blocked_native_present_does_not_stall_emulation_frames(tmp_path):
    import threading
    import time

    root = _display()
    player = None
    entered, release = threading.Event(), threading.Event()
    try:
        source = tmp_path / 'game.nes'
        source.write_bytes(minimal_nrom())
        runtime = RustNesRuntime()
        player, _ = runtime.launch({'id': 'd' * 64, 'name': 'Slow display fixture'},
                                   source, tmp_path / 'workspace', original=True,
                                   host_parent=root)
        assert player._native_view is not None
        deadline = time.monotonic() + 2
        while not player._direct_present and time.monotonic() < deadline:
            root.update()
            time.sleep(.005)
        assert player._direct_present, 'native display did not become ready'
        present = player._native_view.present

        def blocked(*args):
            entered.set()
            assert release.wait(4)
            present(*args)

        player._native_view.present = blocked
        assert entered.wait(1)
        initial = player._frames
        deadline = time.monotonic() + 1
        while player._frames < initial + 10 and time.monotonic() < deadline:
            root.update()
            time.sleep(.005)
        assert player._frames >= initial + 10, 'a screen blit stalled the emulator'
        assert not release.is_set()
    finally:
        release.set()
        if player is not None:
            player.close()
        root.destroy()


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
            assert not original._record_poses
            if runtime._core.observation_supported:
                assert enhanced._record_poses
                assert enhanced._pose_journal == workspace.resolve() / "playable-assets" / "live-recording.tsv"
            assert original._hd is None, "Original pane must not composite a pack"
            if runtime._core.capture_supported:
                assert original._capture_hd is None
                assert original._capture_workspace == workspace.resolve()
                assert enhanced._capture_workspace == workspace.resolve()
                assert enhanced._capture_armed
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


@needs_dll
@pytest.mark.parametrize('large', [False, True])
def test_pose_fetch_exports_the_rendered_original_patterns(large):
    core = RustCore(DLL)
    if not core.pose_observation_supported:
        pytest.skip('optional sprite-fetch observer not built')
    rom = _sprite_test_nrom(large)
    handle = core.create(rom)
    try:
        core.pose_arm(handle)
        for _ in range(4):
            core.run_frame(handle)
        cells = core.pose_frame(handle)
        assert len(cells) == (8 if large else 4)
        for cell in cells:
            index = int.from_bytes(cell[4:6], 'little')
            assert cell[9:25] == rom[-8192:][index * 16:index * 16 + 16]
            assert cell[6:9] == bytes((22, 39, 24))
        core.pose_arm(handle, False)
        assert core.pose_frame(handle) is None
    finally:
        core.destroy(handle)
