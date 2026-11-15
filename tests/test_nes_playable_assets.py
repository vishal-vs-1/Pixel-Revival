import json
import os
from unittest.mock import Mock
from zipfile import ZipFile

import pytest
from PIL import Image

from systems.contracts import ImportRequest, NES
from systems.nes.library import NesLibrary
from systems.nes.pose_pack import CELL, fit_image, write_pack
from systems.nes.pose_layouts import Cell, find_larger_checkpoint_sprite, identify_player, pose_key
from systems.nes.playable_assets import PlayableAssets
from systems.nes.portable_controls import set_keyboard
from systems.nes.playable_checkpoints import latest_checkpoint, read_checkpoint
from systems.nes.playable_recording import MAX_BYTES, read_recording


def test_player_pose_journal_keeps_complete_records_after_interrupted_write(tmp_path):
    journal = tmp_path / 'recorded-poses.txt'
    cell = b'13A1627182'
    journal.write_bytes(cell * 4 + b'\ninvalid\n' + cell * 8 + b'\r\n' + cell * 4)
    decoded = (0x13A, 'FF162718', 2)
    assert list(read_recording(journal)) == [(decoded,) * 4, (decoded,) * 8]
    journal.write_bytes(b'\0' * (MAX_BYTES + 1))
    with pytest.raises(ValueError, match='exceeds its size limit'):
        list(read_recording(journal))


def test_latest_checkpoint_uses_clean_close_or_newer_numbered_slot(tmp_path):
    def session(name, phase, identity, stamp):
        folder = tmp_path / '.runtime/sessions' / name
        recent = folder / 'home/RecentGames'
        recent.mkdir(parents=True)
        (folder / 'status.json').write_text(json.dumps({'phase': phase, 'session_id': name}))
        (folder / 'launch.json').write_text(json.dumps({'game_id': identity, 'session_id': name}))
        path = recent / 'game.rgd'
        with ZipFile(path, 'w') as archive:
            archive.writestr('Savestate.mss', b'MSS resume')
        os.utime(path, ns=(stamp, stamp))
        return path

    good = session('a' * 32, 'stopped', 'game', 20_000_000_000)
    session('b' * 32, 'flushing-saves', 'game', 30_000_000_000)
    session('c' * 32, 'stopped', 'different', 40_000_000_000)
    slot = tmp_path / 'saves/mesen/states/game_1.mss'
    slot.parent.mkdir(parents=True)
    slot.write_bytes(b'MSS slot')
    os.utime(slot, ns=(10_000_000_000, 10_000_000_000))
    assert latest_checkpoint(tmp_path, 'game') == good
    assert read_checkpoint(good) == b'MSS resume'
    os.utime(slot, ns=(25_000_000_000, 25_000_000_000))
    assert latest_checkpoint(tmp_path, 'game') == slot


def test_latest_checkpoint_reports_when_game_has_no_state(tmp_path):
    with pytest.raises(ValueError, match='close Mesen normally'):
        latest_checkpoint(tmp_path, 'game')


def test_controller_comparison_rejects_unrelated_motion_and_chooses_sprite_slots():
    def frame(x, other):
        return 8, 0, bytes(range(16)), [
            (100, 1, 0, x), (100, 2, 0, x + 8), (108, 3, 0, x), (108, 4, 0, x + 8),
            (144, 6, 0, other),
        ]

    raw = bytes([255]) * 8192
    traces = {direction: {i: frame(40 if i < 2 else x, 120 + i) for i in range(5)}
              for direction, x in [('neutral', 40), ('left', 32), ('right', 53)]}
    assert identify_player(traces, raw) == (0, 1, 2, 3)
    traces['right'] = traces['neutral']
    traces['left'] = traces['neutral']
    with pytest.raises(ValueError, match='Could not isolate'):
        identify_player(traces, raw)


def test_image_import_and_pack_preserve_transparency_and_mirrored_duplicate(tmp_path):
    source = tmp_path / 'art.webp'
    Image.new('RGBA', (10, 4), '#ffaa00').save(source, lossless=True)
    image = fit_image(source)
    assert image.size == (64, 64) and image.getpixel((32, 63)) == (255, 170, 0, 255)
    assert image.getpixel((32, 0))[3] == 0
    pose = tuple(Cell(index, 'FF162718', flip) for index, flip in [(0x3A, 0), (0x37, 0), (0x4F, 0), (0x4F, 1)])
    write_pack(tmp_path, (pose,), image, chr_rom=b'\xff' * 8192)
    pack = tmp_path / 'HdPacks' / 'game'
    definitions = (pack / 'hires.txt').read_text().splitlines()
    foot_lines = [line for line in definitions if '<tile>0,4F,' in line]
    assert len(foot_lines) == 2
    assert foot_lines[0].startswith('[!hmirror&') and foot_lines[1].startswith('[hmirror&')
    with Image.open(pack / 'player.png') as atlas:
        assert atlas.size == (2 * CELL, 4 * CELL)


def test_pack_fills_visible_pose_height_with_wide_artwork(tmp_path):
    small = tuple(Cell(index, 'FF162718', 0) for index in range(4))
    tall = tuple(Cell(index, 'FF162718', 0) for index in range(4, 12))
    # Each tile has transparent rows and columns, as do many real player layouts.
    chr_rom = bytes([0, 0x7E, 0x7E, 0x7E, 0x7E, 0x7E, 0x7E, 0] + [0] * 8) * 512
    image = Image.new('RGBA', (80, 30))
    image.paste('red', (0, 8, 80, 22))
    write_pack(tmp_path, (small, tall), image, chr_rom=chr_rom)
    with Image.open(tmp_path / 'HdPacks/game/player.png') as atlas:
        for i, height in enumerate((128, 64)):
            art = atlas.crop((i * 64, 0, (i + 1) * 64, height))
            assert art.getchannel('A').getbbox() == (4, 4, 60, height - 4)


def test_keyboard_settings_skip_the_first_run_mapping_reset():
    settings = {'ConfigUpgrade': 1, 'Nes': {'Port1': {'Mapping1': {'A': 4110}}}}
    set_keyboard(settings)
    saved = json.loads(json.dumps(settings))
    assert saved['ConfigUpgrade'] == 6
    assert saved['Nes']['Port1']['Mapping1'] == {'A': 4110}
    assert saved['Nes']['Port1']['Mapping2'] == {
        'Left': 23, 'Up': 24, 'Right': 25, 'Down': 26, 'Start': 6, 'Select': 18, 'A': 67, 'B': 69,
    }


def test_per_pose_artwork_overrides_default_and_can_be_cleared(tmp_path):
    service = PlayableAssets(tmp_path)
    small = tuple(Cell(index, 'FF162718', 0) for index in range(4))
    tall = tuple(Cell(index, 'FF162718', 0) for index in range(8))
    workspace = tmp_path / 'workspace'
    file = workspace / 'playable-assets/detected.json'
    file.parent.mkdir(parents=True)
    file.write_text(json.dumps({'game': 'game-id', 'poses': [
        [[cell.index, cell.palette, cell.mirrors] for cell in pose] for pose in (small, tall)]}))
    default, custom = tmp_path / 'default.png', tmp_path / 'tall.png'
    Image.new('RGBA', (64, 64), 'red').save(default)
    Image.new('RGBA', (64, 128), 'blue').save(custom)
    service.save_image('game-id', workspace, default)
    service.save_image('game-id', workspace, custom, key=pose_key(tall))
    saved = service._assigned_image(service.image_path(workspace))
    override = service._assigned_image(service.image_path(workspace, pose_key(tall)))
    write_pack(workspace, (small, tall), saved, {pose_key(tall): override}, chr_rom=b'\xff' * 8192)
    with Image.open(workspace / 'HdPacks/game/player.png') as atlas:
        assert atlas.getpixel((CELL, 0)) == (0, 0, 255, 255)
        assert atlas.getpixel((3 * CELL, 0)) == (255, 0, 0, 255)
    service.save_image('game-id', workspace, None, key=pose_key(tall))
    assert service.image_path(workspace).is_file()
    assert not service.image_path(workspace, pose_key(tall)).exists()
    service.save_image('game-id', workspace, custom, key=pose_key(tall))
    service.clear_overrides('game-id', workspace)
    assert service.image_path(workspace).is_file()
    assert not service.image_path(workspace, pose_key(tall)).exists()


def test_checkpoint_expansion_requires_player_anchor_and_unique_tall_layout():
    row = (8, 0, bytes(range(16)), [(100 + 8 * (i // 2), 1 + i, 0, 40 + 8 * (i % 2))
                                  for i in range(8)])
    raw = bytes([0xFF]) * 8192
    trace = dict.fromkeys(range(9), row)
    assert find_larger_checkpoint_sprite(trace, (4, 5, 6, 7), raw) == tuple(range(8))
    assert find_larger_checkpoint_sprite({0: row}, (4, 5, 6, 7), raw) is None


@pytest.mark.parametrize(('mode', 'options'), [
    ('player_replacement', {}), ('player_regenerated', {'regenerated': True}),
])
def test_explicit_player_replacement_routes_through_the_nes_library(tmp_path, mode, options):
    runtime = Mock()
    replacement = Mock()
    process = Mock()
    process.poll.return_value = None
    log = tmp_path / 'player.log'
    replacement.launch.return_value = (process, log)
    source = tmp_path / 'game.nes'
    source.write_bytes(b'NES\x1a\x01\x01' + bytes(10 + 16384 + 8192))
    library = NesLibrary(tmp_path / 'nes', asset_service=Mock(), runtime=runtime, playable_assets=replacement)
    game = library.import_game(ImportRequest(NES, source, 'pixel'))
    assert library.launch(game, **{mode: True}) == (process, log)
    runtime.launch.assert_not_called()
    assert replacement.launch.call_args.args[-1]['START'] == 'ENTER'
    assert replacement.launch.call_args.kwargs == options
    with pytest.raises(RuntimeError, match='Close the NES player'):
        library.delete_game(game['id'])
