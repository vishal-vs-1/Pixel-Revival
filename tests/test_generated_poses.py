import json
from unittest.mock import Mock

from PIL import Image
import pytest

from systems.generated_poses import GeneratedPoses
from systems.jme.pose_artwork import PoseArtwork
from systems.nes.pose_pack import CELL
from systems.nes.pose_layouts import Cell, _pose_image, offsets_for, pose_key
from systems.nes.playable_assets import PlayableAssets
from systems.nes.rom import NesRomInspector


def test_failed_batch_keeps_the_previous_generation(tmp_path):
    store = GeneratedPoses(tmp_path, 'regenerated')
    first, second = 'a' * 32, 'b' * 32
    original = Image.new('RGBA', (16, 16), 'red')
    store.regenerate([first, second], lambda key: original)
    previous = store.current([second, first])

    def fail_on_second(key):
        if key == second:
            raise ValueError('Broken capture')
        return original

    with pytest.raises(ValueError, match='Broken capture'):
        store.regenerate([first, second], fail_on_second)
    assert store.current([first, second]) == previous
    assert previous.is_dir()
    assert {child.name for child in previous.parent.iterdir()} == {previous.name, 'current.json'}
    assert store.current([first]) is None


def test_java_generated_play_uses_only_current_confirmed_poses(tmp_path):
    artwork = PoseArtwork(tmp_path)
    selected, assigned, candidate = (char * 64 for char in 'abc')
    source = Image.new('RGBA', (9, 13))
    source.paste('red', (2, 3, 7, 11))
    artwork.folder('poses').mkdir(parents=True)
    for key in (selected, assigned, candidate):
        source.save(artwork.folder('poses') / f'{key}.png')
    artwork.set_selected(selected, True)
    artwork.assign(None, artwork.folder('poses') / f'{candidate}.png')
    artwork.assign(assigned, artwork.folder('poses') / f'{candidate}.png')
    default_before = artwork.assigned(None).read_bytes()
    override_before = artwork.assigned(assigned).read_bytes()

    assert artwork.regenerate() == 2
    folder = artwork.regenerated.current([selected, assigned])
    assert artwork.launch_arguments(False, regenerated=True) == [
        f'-Djme.playerPoses={folder}', '-Djme.recordPoses=false',
    ]
    assert artwork.launch_arguments(False)[0] == f'-Djme.playerPoses={artwork.root}'
    assert {path.stem for path in (folder / 'images').glob('*.png')} == {selected, assigned}
    with Image.open(folder / 'poses' / f'{selected}.png') as capture:
        assert capture.tobytes() == source.tobytes()
    with Image.open(folder / 'images' / f'{selected}.png') as generated:
        assert generated.getchannel('A').getbbox() == (8, 12, 28, 44)
    artwork.set_selected(selected, False)
    with pytest.raises(ValueError, match='Regenerate all confirmed'):
        artwork.launch_arguments(False, regenerated=True)
    assert artwork.assigned(None).read_bytes() == default_before
    assert artwork.assigned(assigned).read_bytes() == override_before


def test_nes_generated_launch_fits_small_and_tall_without_flipping_twice(tmp_path, monkeypatch):
    rom = tmp_path / 'game.nes'
    tile = bytes([0, 0x60, 0x60, 0x70, 0x70, 0x7E, 0x7E, 0] + [0] * 8)
    chr_rom = tile * 512
    rom.write_bytes(b'NES\x1a\x01\x01' + bytes(10 + 16384) + chr_rom)
    identity = NesRomInspector().inspect(rom).id
    workspace = tmp_path / 'workspace'
    catalog = workspace / 'playable-assets/detected.json'
    catalog.parent.mkdir(parents=True)
    tall = tuple(Cell(index, 'FF162718', 1) for index in range(8))
    small = tuple(Cell(index, 'FF162718', 1) for index in range(8, 12))
    poses = (tall, small)
    catalog.write_text(json.dumps({'game': identity, 'poses': [
        [[cell.index, cell.palette, cell.mirrors] for cell in pose] for pose in poses
    ]}))
    service = PlayableAssets(tmp_path)
    source_art = tmp_path / 'upload.png'
    Image.new('RGBA', (12, 9), '#22aacd').save(source_art)
    service.save_image(identity, workspace, source_art)
    uploaded = service.image_path(workspace).read_bytes()
    assert service.regenerate(identity, rom, workspace) == 2
    generated = service.regenerated(workspace).current(pose_key(pose) for pose in poses)

    home = tmp_path / 'portable'
    home.mkdir()
    monkeypatch.setattr(service, 'runtime', lambda: None)
    monkeypatch.setattr(service, '_player_home', lambda *args, **kwargs: home)
    service.launch(identity, rom, workspace, None, regenerated=True, process_factory=Mock())
    with Image.open(home / 'HdPacks/game/player.png') as atlas:
        for slot, pose in enumerate(poses):
            with Image.open(generated / 'images' / f'{pose_key(pose)}.png') as drawing:
                decoded = Image.new('RGBA', drawing.size)
                for part, (dx, dy) in zip(pose, offsets_for(pose)):
                    x = slot * 2 * CELL + 4 * dx
                    y = 4 * dy
                    decoded.paste(part.orient(atlas.crop((x, y, x + CELL, y + CELL))), (4 * dx, y))
                original_bounds = _pose_image(pose, chr_rom).getchannel('A').getbbox()
                assert decoded.getchannel('A').getbbox() == tuple(v * 4 for v in original_bounds)
                assert decoded.tobytes() == drawing.tobytes()
    assert service.image_path(workspace).read_bytes() == uploaded
