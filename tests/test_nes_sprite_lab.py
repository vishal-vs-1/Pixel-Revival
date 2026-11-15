import pytest
from PIL import Image, ImageChops

from systems.nes.capture import CAPTURE_DIRECTORY
from systems.nes.sprite_lab import moving_poses, replacement_atlas

GAME = "a" * 64
SESSION = "b" * 32
PATTERN = "fe" * 8 + "00" * 8


def _part(x, y, index, mirrors=0):
    return f"{x} {y} {mirrors} {index} {0xFF010203} 0 {PATTERN}"


def _pose(x, y, first=1):
    offsets = ((0, 0), (8, 0), (0, 8), (8, 8))
    return [_part(x + dx, y + dy, first + index, index) for index, (dx, dy) in enumerate(offsets)]


def _capture(workspace, frames):
    inbox = workspace / CAPTURE_DIRECTORY
    inbox.mkdir(parents=True)
    palette = bytes(range(192)).hex()
    for index, placements in enumerate(frames):
        path = inbox / f"{SESSION}-{index:08x}.rnl"
        path.write_text(f"RNL2 {GAME} {SESSION} {120 * (index + 1)} {palette}\n" + "\n".join(placements),
                        encoding="ascii")
    marker = inbox / f"{SESSION}.rnl-done"
    marker.write_text(f"RNL2 {GAME} {SESSION} {len(frames)}\n", encoding="ascii")
    return marker


def test_selects_only_the_unambiguous_moving_pose(tmp_path):
    _capture(tmp_path, [_pose(20, 20) + _pose(80, 80, 5), _pose(39, 20) + _pose(80, 80, 5),
                        _pose(45, 20) + [_part(100, 100, 1)]])
    pose, = moving_poses(tmp_path, GAME, SESSION)
    assert pose.positions == ((120, 20, 20), (240, 39, 20))
    assert tuple(part.mirrors for part in pose.parts) == (0, 1, 2, 3)
    assert pose.image().getpixel((0, 0)) == (3, 4, 5, 255)
    assert pose.image().getpixel((8, 0)) == (0, 0, 0, 0)


def test_frame_without_complete_matching_marker_is_rejected(tmp_path):
    marker = _capture(tmp_path, [_pose(20, 20)])
    marker.write_text(f"RNL2 {'c' * 64} {SESSION} 1\n", encoding="ascii")
    with pytest.raises(ValueError, match="Invalid completed"):
        moving_poses(tmp_path, GAME, SESSION)
    marker.unlink()
    with pytest.raises(ValueError, match="Missing or redirected"):
        moving_poses(tmp_path, GAME, SESSION)


def test_png_matches_display_orientation_and_preserves_transparency(tmp_path):
    _capture(tmp_path, [_pose(20, 20), _pose(39, 20)])
    pose, = moving_poses(tmp_path, GAME, SESSION)
    source = Image.new("RGBA", (64, 64))
    for x, y in ((2, 3), (37, 10), (9, 39), (43, 50)):
        source.putpixel((x, y), (255, 64, 128, 255))
    atlas, cells = replacement_atlas(pose, source, 4)
    assert [cell["selector"] for cell in cells] == [f"rom:{i},FF010203" for i in range(1, 5)]
    canvas = Image.new("RGBA", (64, 64))
    for cell in cells:
        x, y = cell["x"], cell["y"]
        tile = atlas.crop((x, y, x + 32, y + 32))
        if cell["mirrors"] & 1:
            tile = tile.transpose(Image.Transpose.FLIP_LEFT_RIGHT)
        if cell["mirrors"] & 2:
            tile = tile.transpose(Image.Transpose.FLIP_TOP_BOTTOM)
        canvas.paste(tile, (x, y))
    assert ImageChops.difference(canvas, source).getbbox(alpha_only=False) is None


def test_small_png_scales_to_pose_and_empty_png_fails(tmp_path):
    _capture(tmp_path, [_pose(20, 20), _pose(39, 20)])
    pose, = moving_poses(tmp_path, GAME, SESSION)
    atlas, _ = replacement_atlas(pose, Image.new("RGB", (2, 2), "red"), 4)
    assert atlas.getcolors() == [(4096, (255, 0, 0, 255))]
    with pytest.raises(ValueError, match="fully transparent"):
        replacement_atlas(pose, Image.new("RGBA", (2, 2)), 4)
