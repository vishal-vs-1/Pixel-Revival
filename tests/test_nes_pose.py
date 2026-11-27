import json
from pathlib import Path

from PIL import Image
import pytest

from systems.nes.live_player_capture import RECORDING_VERSION
from systems.nes.live_pose import LivePose
from systems.nes.playable_assets import PlayableAssets
from systems.nes.pose_overlay import PoseOverlay
from systems.nes.rom import NesRomInspector


def _rom(tmp_path):
    source = tmp_path / 'game.nes'
    source.write_bytes(b'NES\x1a\x02\x02\x40' + bytes(9 + 32768 + 16384))
    return source, NesRomInspector().inspect(source).id


def _journal(path):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(RECORDING_VERSION)
    palette = '00162718' * 4
    enemy = '55' * 32
    frame = 0
    with path.open('ab') as stream:
        for direction in (0, 128, 0, 64) * 2:
            x = y = 0
            for sample in range(9):
                frame += 4
                if sample >= 2:
                    x += 2 if direction & 128 else -2 if direction & 64 else 0
                player = ('ff' if frame // 16 % 2 else 'aa') * 32
                row = [frame, direction, 16, palette,
                       f'{68 + x},{110 + y},0,640,641,{player}',
                       f'{76 + x},{110 + y},0,642,643,{player}',
                       f'{176 + frame // 16},80,0,48,49,{enemy}']
                stream.write(('\t'.join(map(str, row)) + '\n').encode('ascii'))


def test_live_recording_detects_player_poses(tmp_path):
    source, identity = _rom(tmp_path)
    workspace = tmp_path / 'workspace'
    service = PlayableAssets(tmp_path)
    _journal(workspace / 'playable-assets' / 'live-recording.tsv')
    assert service.import_recording(identity, source, workspace) == 2
    report = json.loads((workspace / 'playable-assets/recording-report.json').read_text())
    assert report['detected'] == 2 and report['added'] == 2
    assert report['observations']['selected_track'] > 0
    detection = service.detection(identity, workspace)
    assert detection is not None
    assert len(detection.poses) == 2
    assert detection.preview.is_file()
    assert {cell.index for cell in detection.poses[0]} == {640, 641, 642, 643}
    assert service.import_recording(identity, source, workspace) == 0


def test_pose_overlay_paints_a_matching_layout(tmp_path):
    pose = LivePose.parse([
        [0, 0, None, 'FF162718', 0, 'ff' * 16],
        [8, 0, None, 'FF162718', 0, 'ff' * 16],
    ])
    art = Image.new('RGBA', (64, 32), (255, 0, 0, 255))

    class Core:
        @staticmethod
        def tile_pattern(_handle, _index):
            return b'\xff' * 16

    overlay = PoseOverlay(((pose, art),))
    overlay.bind(Core(), 1, True)
    palette = bytearray(32)
    palette[0x11:0x14] = bytes((0x16, 0x27, 0x18))
    oam = bytearray(256)
    oam[0:4] = bytes((100, 0, 0, 50))
    oam[4:8] = bytes((100, 1, 0, 58))
    output = overlay.apply(bytes(256 * 240 * 4), 256, 240,
                           (False, 0, bytes(oam), bytes(palette)))
    image = Image.frombytes('RGB', (256, 240), output, 'raw', 'BGRX')
    assert image.getpixel((50, 100)) == (0, 0, 0)
    assert image.getpixel((50, 101)) == (255, 0, 0)
    assert image.getpixel((60, 105)) == (255, 0, 0)


def test_pose_overlay_normalizes_palette_and_refreshes_banked_patterns():
    pose = LivePose.parse([[0, 0, 0, 'FF0F2A3C', 0, 'ff' * 16]])
    art = Image.new('RGBA', (32, 32), 'red')

    class Core:
        pattern = b'\xff' * 16

        def tile_pattern(self, _handle, _index):
            return self.pattern

    core = Core()
    overlay = PoseOverlay(((pose, art),))
    overlay.bind(core, 1)
    palette = bytearray(32)
    palette[17:20] = bytes((15, 42, 60))
    oam = bytearray([255] * 256)
    oam[:4] = bytes((100, 0, 0, 50))
    observation = (False, 0, oam, palette)
    frame = bytes(256 * 240 * 4)
    assert overlay.apply(frame, 256, 240, observation) != frame
    # Same PPU slot, different mapper bank: this is no longer the saved pose.
    core.pattern = b'\xaa' * 16
    assert overlay.apply(frame, 256, 240, observation) == frame


def test_pose_overlay_refreshes_chr_ram_each_frame():
    pose = LivePose.parse([[0, 0, None, 'FF162718', 0, 'ff' * 16]])
    art = Image.new('RGBA', (32, 32), 'red')
    core = type('Core', (), {'pattern': b'\xff' * 16,
                           'tile_pattern': lambda self, *_: self.pattern})()
    overlay = PoseOverlay(((pose, art),))
    overlay.bind(core, 1, True)
    palette = bytearray(32)
    palette[17:20] = bytes((22, 39, 24))
    oam = bytearray([255] * 256)
    oam[:4] = bytes((100, 0, 0, 50))
    frame = bytes(256 * 240 * 4)
    assert overlay.apply(frame, 256, 240, (False, 0, oam, palette)) != frame
    core.pattern = bytes(16)
    assert overlay.apply(frame, 256, 240, (False, 0, oam, palette)) == frame


@pytest.mark.parametrize('scale', [1, 2, 4])
def test_pose_region_compositor_matches_full_frame_alpha_blend_at_edges(scale):
    pose = LivePose.parse([[0, 0, None, 'FF162718', 0, 'ff' * 16]])
    art = Image.new('RGBA', (32, 32), (240, 80, 20, 128))
    core = type('Core', (), {'tile_pattern': lambda *_: b'\xff' * 16})()
    overlay = PoseOverlay(((pose, art),))
    overlay.bind(core, 1, True)
    palette = bytearray(32)
    palette[17:20] = bytes((22, 39, 24))
    oam = bytearray([255] * 256)
    oam[:4] = bytes((235, 0, 0, 252))
    size = (256 * scale, 240 * scale)
    background = Image.new('RGB', size, (10, 30, 90))
    data = background.tobytes('raw', 'BGRX')
    expected = background.convert('RGBA')
    expected.alpha_composite(art.resize((8 * scale, 8 * scale), Image.Resampling.NEAREST),
                             (252 * scale, 236 * scale))
    output = overlay.apply(data, *size, (False, 0, oam, palette))
    assert Image.frombytes('RGB', size, output, 'raw', 'BGRX').tobytes() == expected.convert('RGB').tobytes()
