"""Geometry and pixel data for player poses observed during NES gameplay."""

from dataclasses import dataclass
from hashlib import sha256
import json
import re

from PIL import Image, ImageOps

from .tiles import Tile


@dataclass(frozen=True)
class LiveCell:
    x: int
    y: int
    index: int | None
    palette: str
    mirrors: int
    pattern: str

    def orient(self, image):
        if self.mirrors & 1:
            image = ImageOps.mirror(image)
        if self.mirrors & 2:
            image = ImageOps.flip(image)
        return image

    @property
    def selector(self):
        return self.pattern.upper() if self.index is None else f'{self.index:X}'


@dataclass(frozen=True)
class LivePose:
    cells: tuple[LiveCell, ...]

    def __len__(self):
        return len(self.cells)

    def __iter__(self):
        return iter(self.cells)

    def __getitem__(self, index):
        return self.cells[index]

    @property
    def size(self):
        return max(c.x for c in self.cells) + 8, max(c.y for c in self.cells) + 8

    @property
    def overlayable(self):
        # Whole-pose composition can handle layers that tile-pack selectors cannot.
        return len(self.cells) > 1

    @property
    def replaceable(self):
        # Mesen matches neighboring tiles by position and selector. Layered
        # parts can be previewed but are ambiguous as replacement destinations.
        positions = [(c.x, c.y) for c in self.cells]
        return len(positions) > 1 and all(
            abs(x - nx) >= 8 or abs(y - ny) >= 8
            for i, (x, y) in enumerate(positions) for nx, ny in positions[:i])

    def document(self):
        return [[c.x, c.y, c.index, c.palette, c.mirrors, c.pattern] for c in self.cells]

    @classmethod
    def parse(cls, records):
        if not isinstance(records, list) or not 1 <= len(records) <= 16:
            raise ValueError('Invalid live player pose layout.')
        cells = []
        for record in records:
            if not isinstance(record, list) or len(record) != 6:
                raise ValueError('Invalid live player pose tile.')
            x, y, index, palette, mirrors, pattern = record
            if (type(x) is not int or type(y) is not int or not 0 <= x <= 40 or not 0 <= y <= 56
                    or (index is not None and (type(index) is not int or not 0 <= index < 0x400000))
                    or not isinstance(palette, str) or not re.fullmatch(r'FF(?:[0-3][0-9A-F]){3}', palette)
                    or type(mirrors) is not int or mirrors not in range(4)
                    or not isinstance(pattern, str) or not re.fullmatch('[0-9a-f]{32}', pattern)):
                raise ValueError('Invalid live player pose tile.')
            cells.append(LiveCell(*record))
        if min(c.x for c in cells) or min(c.y for c in cells) or not any(int(c.pattern, 16) for c in cells):
            raise ValueError('Invalid live player pose bounds.')
        return cls(tuple(cells))

    def key(self):
        return sha256(json.dumps(['live', self.document()], separators=(',', ':')).encode('ascii')).hexdigest()[:32]

    def image(self, palette_rgb):
        image = Image.new('RGBA', self.size)
        for cell in reversed(self.cells):
            tile = Tile(0, cell.index or 0, int(cell.palette, 16), bytes.fromhex(cell.pattern),
                        cell.index is None, True, True)
            image.alpha_composite(cell.orient(tile.image(palette_rgb)), (cell.x, cell.y))
        return image
