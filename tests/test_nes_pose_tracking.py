"""Animation continuity uses native sprites, independent of rendered artwork."""

from collections import Counter
import json

from PIL import Image
import pytest

from systems.nes.live_player_capture import Frame, Sprite
from systems.nes.live_player_tracking import _groups, _pose, find_poses
from systems.nes.playable_assets import PlayableAssets
from systems.nes.pose_observation import capture_frame
from systems.nes.pose_overlay import PoseOverlay
from systems.nes.pose_tracking import KnownPoseTracker
from systems.nes.rom import NesRomInspector


def player_frame(number, phase=0, x=74):
    patterns = ('ff', 'ee', ('aa', 'cc', '77')[phase], ('55', '33', '11')[phase])
    sprites = tuple(Sprite(x + column * 8, 100 + row * 8, 0,
                           (index + phase * 8, -2), patterns[index] * 16)
                    for index, (row, column) in enumerate(((0, 0), (0, 1), (1, 0), (1, 1))))
    # The head retains native identities while the running body changes.
    sprites = tuple(Sprite(s.x, s.y, s.flags, (i if i < 2 else s.indices[0], -2), s.pattern)
                    for i, s in enumerate(sprites))
    return Frame(number, 128, 8, '00162718' * 4, sprites)


def seed_pose():
    frame = player_frame(1)
    return _pose(frame, _groups(frame)[0], ram=False)


def test_known_player_learns_running_without_repeated_direction_presses():
    frames = [player_frame(1)] + [player_frame(n, 1 + n % 2, 74 + n)
                                  for n in range(2, 10)]
    diagnostics = Counter()
    poses = find_poses(frames, ram=False, max_poses=192,
                       known_poses=(seed_pose(),), diagnostics=diagnostics)
    assert len(poses) == 2
    assert diagnostics['learned_from_known_player'] == 2
    assert diagnostics['known_player_frames'] == len(frames)


def test_initial_control_proof_retains_recent_running_appearance():
    frames = []
    number, x = 0, 74
    for direction in (0, 128, 0, 64, 0):
        for sample in range(10):
            number += 3
            moving = direction != 0 and sample < 3
            if moving:
                x += 4 if direction == 128 else -4
            player = player_frame(number, int(moving), x)
            frames.append(Frame(number, direction, 8, player.palette, player.sprites))
    poses = find_poses(frames, ram=False, max_poses=192)
    # Both control episodes finish before their recurring animation is selected.
    assert {tuple(c.pattern for c in p) for p in poses} == {
        tuple(c.pattern for c in _pose(player_frame(1, phase),
                                     _groups(player_frame(1, phase))[0], ram=False))
        for phase in (0, 1)
    }


def test_camera_scroll_identifies_animated_player_without_selecting_fixed_hud():
    frames = []
    number = camera = 0
    for direction in (0, 128, 0, 64) * 3:
        for sample in range(10):
            number += 3
            camera = (camera + (3 if direction == 128 else -3 if direction == 64 else 0)) % 512
            player = player_frame(number, number // 6 % 2, 74)
            hud = tuple(Sprite(150 + column * 8, 20 + row * 8, 0,
                               (40 + row * 2 + column, -2), '99' * 16)
                        for row in range(2) for column in range(2))
            frames.append(Frame(number, direction, 8, player.palette,
                                player.sprites + hud, (camera, 120)))
    diagnostics = Counter()
    poses = find_poses(frames, ram=False, max_poses=192, diagnostics=diagnostics)
    assert len(poses) >= 2
    assert all(cell.index < 40 for pose in poses for cell in pose)
    assert diagnostics['static_camera_response'] > 0


@pytest.mark.parametrize('background', ['blue', 'green'])
def test_default_image_stays_applied_on_a_new_running_pose(background):
    seed = seed_pose()
    red = Image.new('RGBA', (64, 64), 'red')
    overlay = PoseOverlay(((seed, red),), default_art=red, known_poses=(seed,))
    overlay.bind(object(), 1)
    for frame in (player_frame(1), player_frame(2, 1), player_frame(3, 1)):
        native_or_enhanced = Image.new('RGB', (256, 240), background).tobytes('raw', 'BGRX')
        output = overlay.apply(native_or_enhanced, 256, 240, None, frame=frame)
        image = Image.frombytes('RGB', (256, 240), output, 'raw', 'BGRX')
        assert image.getpixel((80, 105)) == (255, 0, 0)
    assert len(overlay.learned_poses) == 1


def test_ambiguous_neighbor_does_not_take_over_the_player():
    tracker = KnownPoseTracker((seed_pose(),))
    assert tracker.observe(player_frame(1)) is not None
    left, right = player_frame(2, 1, 62), player_frame(2, 1, 86)
    ambiguous = Frame(2, 128, 8, left.palette, left.sprites + right.sprites)
    assert tracker.observe(ambiguous) is None
    assert not tracker.learned


def test_layered_pose_matches_all_native_parts_without_reusing_one_tile():
    from systems.nes.live_pose import LiveCell, LivePose
    body = LiveCell(0, 0, 1, 'FF162718', 0, 'ff' * 16)
    detail = LiveCell(0, 0, 2, 'FF162718', 0, 'aa' * 16)
    pose = LivePose((body, detail))
    assert pose.overlayable and not pose.replaceable
    assert list(PoseOverlay._matches(pose.cells, (body, detail))) == [(0, 0)]
    assert not list(PoseOverlay._matches(pose.cells, (body,)))
    duplicate = LivePose((body, body))
    assert not list(PoseOverlay._matches(duplicate.cells, (body,)))


def test_lost_body_tiles_are_not_learned_as_a_smaller_character():
    tracker = KnownPoseTracker((seed_pose(),))
    tracker.observe(player_frame(1))
    for number in (2, 3):
        frame = player_frame(number)
        assert tracker.observe(Frame(number, 128, 8, frame.palette, frame.sprites[:2])) is None
    assert not tracker.learned


def test_one_native_sample_reads_each_pattern_once():
    class Core:
        def __init__(self):
            self.reads = []

        def tile_pattern(self, _handle, index):
            self.reads.append(index)
            return b'\xff' * 16

    core = Core()
    oam = bytearray([255] * 256)
    oam[:8] = bytes((100, 0, 0, 50, 100, 0, 0, 58))
    frame = capture_frame(core, 1, 1, 128, (False, 0, oam, bytes(32)))
    assert len(frame.sprites) == 2
    assert core.reads == [0]


def test_fetched_sprite_palette_survives_journal_round_trip(tmp_path):
    from systems.nes.live_player_capture import RECORDING_VERSION, read_frames
    from systems.nes.pose_observation import frame_cells
    path = tmp_path / 'capture.tsv'
    path.write_bytes(RECORDING_VERSION + (
        '3\t128\t8\t' + '00' * 16 + '\t50,100,0,1,-2,' + 'ff' * 16 + ',FF162718\n'
    ).encode('ascii'))
    frame = tuple(read_frames(path))[0]
    assert frame_cells(frame)[0].palette == 'FF162718'
    assert frame.sprites[0].colors == 'FF162718'
    old = tmp_path / 'old.tsv'
    old.write_bytes(b'nes-live-player\t1\n' + (
        '3\t128\t8\t' + '00' * 16 + '\t50,100,0,1,-2,' + 'ff' * 16 + '\n'
    ).encode('ascii'))
    assert tuple(read_frames(old))[0].sprites[0].colors is None


def test_entirely_new_animation_needs_reconnection_before_it_is_saved():
    tracker = KnownPoseTracker((seed_pose(),))
    tracker.observe(player_frame(1))
    for number in (2, 3):
        original = player_frame(number)
        sprites = tuple(Sprite(s.x, s.y, s.flags, (64 + i, -2), pattern * 16)
                        for i, (s, pattern) in enumerate(zip(original.sprites, ('22', '44', '88', '99'))))
        tracker.observe(Frame(number, 128, 8, original.palette, sprites))
    assert not tracker.learned
    tracker.observe(player_frame(4))
    assert len(tracker.learned) == 1


def test_pose_seen_between_journal_samples_is_imported_and_deduplicated(tmp_path):
    source = tmp_path / 'game.nes'
    source.write_bytes(b'NES\x1a\x02\x02\x40' + bytes(9 + 32768 + 16384))
    identity = NesRomInspector().inspect(source).id
    workspace = tmp_path / 'workspace'
    folder = workspace / 'playable-assets'
    folder.mkdir(parents=True)
    from systems.nes.live_player_capture import RECORDING_VERSION
    (folder / 'live-recording.tsv').write_bytes(RECORDING_VERSION)
    frame = player_frame(2, 1)
    observed = _pose(frame, _groups(frame)[0], ram=False)
    service = PlayableAssets(tmp_path)
    assert service.import_recording(identity, source, workspace, observed_poses=(observed,)) == 1
    assert service.import_recording(identity, source, workspace, observed_poses=(observed,)) == 0
    report = json.loads((folder / 'recording-report.json').read_text())
    assert report['observations']['learned_during_play'] == 1
    assert len(json.loads((folder / 'live-poses.json').read_text())['poses']) == 1
