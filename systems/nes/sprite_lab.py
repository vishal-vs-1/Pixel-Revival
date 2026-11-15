"""Read opt-in Mesen sprite placements and prepare a selected pose replacement."""

from collections import Counter, defaultdict
from dataclasses import dataclass
from hashlib import sha256
from itertools import islice
import json
from pathlib import Path
import re

from PIL import Image

from .capture import CAPTURE_DIRECTORY, owned_path
from .tiles import Tile

MAX_FRAMES = 32
MAX_PLACEMENTS = 128
_FRAME_BYTES = 16 * 1024
_ID = re.compile(r"[0-9a-f]{64}")
_SESSION = re.compile(r"[0-9a-f]{32}")
_PATTERN = re.compile(r"[0-9a-f]{32}")
_PALETTE = re.compile(r"[0-9a-f]{384}")
_OFFSETS = ((0, 0), (8, 0), (0, 8), (8, 8))


@dataclass(frozen=True)
class Placement:
    x: int
    y: int
    mirrors: int
    tile: Tile


@dataclass(frozen=True)
class Pose:
    key: str
    parts: tuple[Placement, ...]
    palette: bytes
    positions: tuple[tuple[int, int, int], ...]

    def image(self):
        canvas = Image.new("RGBA", (16, 16))
        for part, position in zip(self.parts, _OFFSETS):
            image = part.tile.image(self.palette)
            canvas.paste(_flip(image, part.mirrors), position)
        return canvas


def _read_ascii(path, maximum):
    if path.is_symlink() or path.is_junction() or not path.is_file():
        raise ValueError(f"Missing or redirected sprite capture: {path.name}")
    with path.open("rb") as stream:
        data = stream.read(maximum + 1)
    if len(data) > maximum:
        raise ValueError("Sprite capture exceeds its size limit")
    return data.decode("ascii")


def _read_session(workspace, game_id, session):
    if not _ID.fullmatch(game_id) or not _SESSION.fullmatch(session):
        raise ValueError("Invalid sprite capture identity")
    inbox = owned_path(Path(workspace), CAPTURE_DIRECTORY)
    marker = _read_ascii(inbox / f"{session}.rnl-done", 160).split()
    if len(marker) != 4 or marker[:3] != ["RNL2", game_id, session]:
        raise ValueError("Invalid completed sprite capture")
    count = int(marker[3])
    if not 1 <= count <= MAX_FRAMES:
        raise ValueError("Invalid sprite frame count")
    previous = -1
    palette = None
    for index in range(count):
        lines = _read_ascii(inbox / f"{session}-{index:08x}.rnl", _FRAME_BYTES).splitlines()
        if not 2 <= len(lines) <= 1 + MAX_PLACEMENTS:
            raise ValueError("Invalid sprite placement count")
        header = lines[0].split()
        if len(header) != 5 or header[:3] != marker[:3] or not _PALETTE.fullmatch(header[4]):
            raise ValueError("Invalid sprite frame header")
        frame = int(header[3])
        colors = bytes.fromhex(header[4])
        if not previous < frame < 2**32 or (palette is not None and colors != palette):
            raise ValueError("Sprite frames are out of order or their palette changed")
        previous, palette = frame, colors
        placements = []
        for line in lines[1:]:
            fields = line.split()
            if len(fields) != 7 or not _PATTERN.fullmatch(fields[6]):
                raise ValueError("Invalid sprite placement")
            x, y, mirrors, tile_index, packed, ram = map(int, fields[:6])
            if (not -7 <= x < 256 or not -7 <= y < 240 or mirrors not in range(4)
                    or ram not in (0, 1) or not 0 <= packed < 2**32):
                raise ValueError("Invalid sprite placement values")
            tile = Tile(frame, tile_index, packed, bytes.fromhex(fields[6]), bool(ram), True, True)
            placements.append(Placement(x, y, mirrors, tile))
        yield frame, palette, placements


def _groups(placements):
    by_origin = defaultdict(list)
    for part in placements:
        by_origin[part.x, part.y].append(part)
    selectors = Counter(part.tile.selector for part in placements)
    candidates = []
    for x, y in sorted(by_origin):
        if not (0 <= x <= 240 and 0 <= y <= 224):
            continue
        slots = [by_origin.get((x + dx, y + dy), ()) for dx, dy in _OFFSETS]
        if any(len(slot) != 1 for slot in slots):
            continue
        parts = tuple(slot[0] for slot in slots)
        if all(selectors[part.tile.selector] == 1 for part in parts):
            candidates.append(parts)
    shared = Counter(part for parts in candidates for part in parts)
    return (parts for parts in candidates if all(shared[part] == 1 for part in parts))


def moving_poses(workspace, game_id, session):
    """Find exact, unambiguous poses that recur at different screen positions."""
    observed = {}
    for frame, palette, placements in _read_session(workspace, game_id, session):
        for parts in _groups(placements):
            identity = [game_id, palette.hex(),
                        [(p.tile.selector, p.tile.pattern.hex(), p.mirrors) for p in parts]]
            key = sha256(json.dumps(identity, separators=(",", ":")).encode("ascii")).hexdigest()
            _, _, positions = observed.setdefault(key, (parts, palette, []))
            positions.append((frame, parts[0].x, parts[0].y))
    poses = [Pose(key, parts, palette, tuple(positions))
             for key, (parts, palette, positions) in observed.items()
             if len({(x, y) for _, x, y in positions}) >= 2]
    poses.sort(key=lambda pose: (-len(pose.positions), pose.key))
    return tuple(islice(poses, 32))


def _flip(image, mirrors):
    if mirrors & 1:
        image = image.transpose(Image.Transpose.FLIP_LEFT_RIGHT)
    if mirrors & 2:
        image = image.transpose(Image.Transpose.FLIP_TOP_BOTTOM)
    return image


def replacement_atlas(pose, artwork, scale):
    """Convert a PNG to the four source-oriented cells expected by Mesen."""
    if scale not in (2, 4):
        raise ValueError("Sprite replacement scale must be 2 or 4")
    if not 0 < artwork.width <= 4096 or not 0 < artwork.height <= 4096:
        raise ValueError("Replacement image exceeds its size limit")
    size = 16 * scale
    factor = min(size / artwork.width, size / artwork.height)
    dimensions = tuple(max(1, round(length * factor)) for length in artwork.size)
    fitted = artwork.convert("RGBA").resize(dimensions, Image.Resampling.LANCZOS)
    canvas = Image.new("RGBA", (size, size))
    canvas.paste(fitted, ((size - fitted.width) // 2, (size - fitted.height) // 2))
    if canvas.getchannel("A").getbbox() is None:
        raise ValueError("Replacement PNG is fully transparent")
    page = Image.new("RGBA", canvas.size)
    side = 8 * scale
    cells = []
    for part, (x, y) in zip(pose.parts, _OFFSETS):
        x, y = x * scale, y * scale
        cell = canvas.crop((x, y, x + side, y + side))
        page.paste(_flip(cell, part.mirrors), (x, y))
        cells.append({"selector": part.tile.selector, "mirrors": part.mirrors,
                      "page": 0, "x": x, "y": y})
    return page, cells
