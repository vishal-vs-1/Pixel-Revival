"""Identify, key, and reconstruct player poses from NES sprite tiles."""

from collections import defaultdict
from dataclasses import dataclass
from hashlib import sha256
import json
from pathlib import Path

from PIL import Image, ImageOps

from .rom import HEADER_BYTES, TRAINER_BYTES, NesRomInspector
from .tiles import Tile


# Default 2C02 palette from the pinned Mesen bundle's NesConfig.


MASTER_PALETTE = bytes.fromhex(
    '666666 002a88 1412a7 3b00a4 5c007e 6e0040 6c0600 561d00 333500 0b4800 005200 004f08 00404d 000000 000000 000000 '
    'adadad 155fd9 4240ff 7527fe a01acc b71e7b b53120 994e00 6b6d00 388700 0c9300 008f32 007c8d 000000 000000 000000 '
    'fffeff 64b0ff 9290ff c676ff f36aff fe6ecc fe8170 ea9e22 bcbe00 88d800 5ce430 45e082 48cdde 4f4f4f 000000 000000 '
    'fffeff c0dfff d3d2ff e8c8ff fbc2ff fec4ea feccc5 f7d8a5 e4e594 cfef96 bdf4ab b3f3cc b5ebf2 b8b8b8 000000 000000')


@dataclass(frozen=True)
class ReplayStart:
    first: int
    start: int
    throttle: str


REPLAYS = {
    '0e41150ac9e0dfeb5845cf20cf690c4bc3d6a53c408d15b387239be735ac10c9': ReplayStart(300, 80, 'b'),
    '4354eb1dac91038a5f7ca03de2fbcd0bcbf11e0236c247c82204b25ef6fbfd13': ReplayStart(700, 150, 'b'),
    '96095f10be7a1309fa2ec69fe20dbd2c841f2d0a7f61a23d6b13d13674ec04a5': ReplayStart(900, 80, 'both'),
}


@dataclass(frozen=True)
class Cell:
    index: int
    palette: str
    mirrors: int

    def orient(self, image):
        if self.mirrors & 1:
            image = ImageOps.mirror(image)
        if self.mirrors & 2:
            image = ImageOps.flip(image)
        return image


@dataclass(frozen=True)
class Detection:
    poses: tuple[tuple[Cell, ...], ...]
    preview: Path
    tags: tuple[str, ...] = ()
    slots: tuple[int, ...] = ()


def offsets_for(pose):
    return tuple((8 * (i % 2), 8 * (i // 2)) for i in range(len(pose)))


def pose_key(pose):
    encoded = json.dumps([(part.index, part.palette, part.mirrors) for part in pose], separators=(',', ':'))
    return sha256(encoded.encode('ascii')).hexdigest()[:32]


def read_chr_rom(source):
    info = NesRomInspector().inspect(source)
    if info.mapper != 0 or info.chr_rom_bytes != 8192:
        raise ValueError('Player detection currently needs mapper-0 NES cartridges with fixed sprite graphics.')
    with source.open('rb') as stream:
        stream.seek(HEADER_BYTES + (TRAINER_BYTES if info.has_trainer else 0) + info.prg_rom_bytes)
        return stream.read(8192)


def _pattern(raw, index):
    return raw[index * 16:(index + 1) * 16]


def _pose(entry, slots, raw):
    height, bank, colors, sprites = entry
    if height != 8 or bank not in (0, 4096):
        return None
    rows = [sprites[slot] for slot in slots]
    x, y = min(row[3] for row in rows), min(row[0] for row in rows)
    if (len(rows) not in (4, 8) or {(row[3] - x, row[0] - y) for row in rows} != set(offsets_for(rows))
            or x > 240 or y > 239 - 4 * len(rows)):
        return None
    result = []
    for _, index, flags, _ in sorted(rows, key=lambda row: (row[0], row[3])):
        index += bank // 16
        if not any(_pattern(raw, index)):
            return None
        offset = (flags & 3) * 4 + 1
        result.append(Cell(index, f'FF{colors[offset:offset + 3].hex().upper()}', flags >> 6))
    return tuple(result)


def identify_player(traces, raw, *, vertical=False):
    directions = ('climb_up', 'climb_down') if vertical else ('left', 'right')
    motion, anchor = (0, 3) if vertical else (3, 0)
    neutral, left, right = (traces[name] for name in ('neutral', *directions))
    frames = sorted(neutral)
    if len(frames) < 4 or not all(neutral[f] == left.get(f) == right.get(f) for f in frames[:2]):
        raise ValueError('The controller replays did not start from the same sprite state.')
    matches = defaultdict(int)
    for frame in frames[2:]:
        positions = defaultdict(list)
        samples = [trace[frame] for trace in (neutral, left, right)]
        for slot, (a, b, c) in enumerate(zip(*(sample[3] for sample in samples))):
            if a[anchor] == b[anchor] == c[anchor] and b[motion] <= a[motion] <= c[motion] and b[motion] < c[motion]:
                positions[(b[motion] - a[motion], c[motion] - a[motion])].append(slot)
        for slots in positions.values():
            visible = [slot for slot in slots if all(any(_pattern(raw, sample[1] // 16 + sample[3][slot][1]))
                                                      for sample in samples)]
            if len(visible) in (4, 8) and all(_pose(sample, visible, raw) for sample in samples):
                matches[tuple(visible)] += 1
    candidates = [slots for slots, count in matches.items() if count >= 3]
    if len(candidates) != 1:
        raise ValueError('Could not isolate one controllable 16×16 sprite across the replay. No replacement was made.')
    return candidates[0]


def collect_poses(traces, slots, raw):
    poses = {}
    for name, track in traces.items():
        previous = None
        for frame, entry in track.items():
            pose = _pose(entry, slots, raw)
            if pose is None:
                break
            x = min(entry[3][slot][3] for slot in slots)
            y = min(entry[3][slot][0] for slot in slots)
            # OAM slots can be reassigned after death, a screen change, or a cutscene.
            if previous and max(abs(x - previous[0]), abs(y - previous[1])) > 8:
                break
            delta = y - previous[1] if previous else 0
            neutral = traces.get('neutral', {}).get(frame)
            neutral_y = min(neutral[3][slot][0] for slot in slots) if neutral else y
            if name.startswith('climb') and delta and y != neutral_y:
                action = 'Climb'
            elif delta:
                action = 'Airborne / vertical motion'
            elif previous and x != previous[0]:
                action = 'Walk / turn'
            else:
                action = 'Stand'
            previous = (x, y)
            poses.setdefault(pose, set()).add(action)
    return poses


def find_larger_checkpoint_sprite(trace, anchors, raw):
    """Require a unique full rectangle that contains the tracked player's cells."""
    if len(anchors) != 4:
        return None
    previous = None
    consecutive = 0
    proven = set()
    for entry in trace.values():
        if _pose(entry, anchors, raw) is None:
            previous, consecutive = None, 0
            continue
        sprites = entry[3]
        x = min(sprites[i][3] for i in anchors)
        y = min(sprites[i][0] for i in anchors)
        candidates = set()
        for top in (y - 16, y - 8, y):
            points = {(x + dx, top + dy) for dx, dy in offsets_for(range(8))}
            slots = tuple(i for i, (sy, _, _, sx) in enumerate(sprites) if (sx, sy) in points)
            if len(slots) == 8 and set(anchors).issubset(slots) and _pose(entry, slots, raw):
                candidates.add(slots)
        if len(candidates) != 1:
            previous, consecutive = None, 0
            continue
        slots, = candidates
        consecutive = consecutive + 1 if slots == previous else 1
        previous = slots
        if consecutive >= 8:
            proven.add(slots)
    return next(iter(proven)) if len(proven) == 1 else None


def _pose_image(pose, chr_rom):
    image = Image.new('RGBA', (16, 4 * len(pose)))
    for part, (x, y) in zip(pose, offsets_for(pose)):
        tile = Tile(0, part.index, int(part.palette, 16), _pattern(chr_rom, part.index), False, True, True)
        image.paste(part.orient(tile.image(MASTER_PALETTE)), (x, y))
    return image


def owned_cartridge(source, identity):
    if NesRomInspector().inspect(source).id != identity:
        raise ValueError('The stored NES cartridge does not match the selected game.')
