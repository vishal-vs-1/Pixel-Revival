"""Native NES match keys and canonical pixels decoded from original CHR."""

from dataclasses import dataclass
from hashlib import sha256

from PIL import Image


@dataclass(frozen=True)
class Tile:
    frame: int
    index: int
    palette: int
    pattern: bytes
    ram: bool
    sprite: bool
    transparent: bool

    def __post_init__(self):
        colors = self.palette.to_bytes(4, "big")
        if (not 0 <= self.frame < 2**64 or not 0 <= self.index < 2**31 or
                len(self.pattern) != 16 or any(value > 63 for value in colors[1:]) or
                (colors[0] != 255 if self.sprite else colors[0] > 63)):
            raise ValueError("Invalid NES tile observation")

    @property
    def selector(self):
        # HD packs match RAM by contents and ROM by the absolute CHR tile number.
        token = self.pattern.hex() if self.ram else f"{self.index:X}"
        return f"{'ram' if self.ram else 'rom'}:{token},{self.palette:08X}"

    def image(self, palette_rgb, *, transparent=False):
        colors = self.palette.to_bytes(4, "big")
        pixels = bytearray()
        for low, high in zip(self.pattern[:8], self.pattern[8:]):
            for bit in range(7, -1, -1):
                color = ((low >> bit) & 1) | (((high >> bit) & 1) << 1)
                if color == 0 and (self.sprite or self.transparent or transparent):
                    pixels.extend(b"\0" * 4)
                else:
                    offset = 3 * colors[color]
                    pixels.extend(palette_rgb[offset:offset + 3] + b"\xff")
        return Image.frombytes("RGBA", (8, 8), bytes(pixels))


def palette_id(rgb):
    if not isinstance(rgb, bytes) or len(rgb) != 192:
        raise ValueError("A NES palette must contain 64 RGB entries")
    return sha256(b"nes-palette-v1\0" + rgb).hexdigest()
