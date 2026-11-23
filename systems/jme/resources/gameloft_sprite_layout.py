"""Bounded layout reader for palette-based Gameloft sprite blocks."""
from dataclasses import dataclass
from struct import Struct

SIGNATURE = b'\xdf\x05'
PALETTE_WIDTH = {0x8888: 4, 0x4444: 2, 0x5515: 2, 0x6505: 2, 0x8808: 3}
_U16 = Struct('<H')
_U32 = Struct('<I')
_MODULE_LIMIT = 4096
_PIXEL_LIMIT = 1_048_576

_PALETTE_BANK_INDEX = 0x00000004
_WIDE_DIMENSIONS = 0x00000010
_MODULE_POSITION = 0x00000020
_WIDE_PAYLOAD_SIZE = 0x00000080
_WIDE_FRAME_COORDINATES = 0x00000400
_FRAME_MODULE_EXTRA = 0x00004000
_FRAME_RECTS = 0x00008000
_WIDE_ANIMATION_COORDINATES = 0x00040000
_WIDE_ANIMATION_FRAME = 0x00200000
_INDEXED_PIXELS = 0x01000000
_MULTIPLE_PALETTE_BANKS = 0x40000000

# The remaining supported bits are present on this layout family and do not
# change its byte counts. Other families can store PNG atlases or extra tables.
_SUPPORTED_FLAGS = (
    0x00010101 | _PALETTE_BANK_INDEX | _WIDE_DIMENSIONS | _MODULE_POSITION |
    _WIDE_PAYLOAD_SIZE | _WIDE_FRAME_COORDINATES | _FRAME_MODULE_EXTRA |
    _FRAME_RECTS | _WIDE_ANIMATION_COORDINATES | _WIDE_ANIMATION_FRAME |
    _INDEXED_PIXELS | _MULTIPLE_PALETTE_BANKS
)

# Primitive and placeholder modules carry geometry, so the reader advances
# over them while only returning bitmap modules for artwork extraction.
_PRIMITIVES = {
    0xff: (4, True, 0), 0xfe: (4, True, 0), 0xfd: (0, True, 0),
    0xfc: (4, True, 4), 0xfb: (4, True, 4), 0xfa: (4, False, 8),
    0xf9: (4, False, 8), 0xf8: (4, True, 0), 0xf7: (9, True, 0),
}


class SpriteFormatError(ValueError):
    """The block is outside the supported sprite layout or is malformed."""


class SpriteReader:
    def __init__(self, data):
        self.data = memoryview(data)
        self.offset = 0

    @property
    def remaining(self):
        return len(self.data) - self.offset

    def take(self, size):
        if not 0 <= size <= self.remaining:
            raise SpriteFormatError('Truncated sprite data')
        start = self.offset
        self.offset += size
        return self.data[start:self.offset]

    def u8(self):
        return self.take(1)[0]

    def u16(self):
        return _U16.unpack(self.take(_U16.size))[0]

    def u32(self):
        return _U32.unpack(self.take(_U32.size))[0]

    def skip_table(self, row_bytes):
        self.take(self.u16() * row_bytes)


@dataclass(frozen=True)
class SpriteModule:
    index: int
    width: int
    height: int
    palette_bank: int
    pixels: memoryview


@dataclass(frozen=True)
class SpriteLayout:
    palette_format: int
    pixel_format: int
    palettes: tuple[tuple[memoryview, ...], ...]
    modules: tuple[SpriteModule, ...]


def _read_module_table(reader, count, flags):
    dimension = reader.u16 if flags & _WIDE_DIMENSIONS else reader.u8
    entries = []
    for _ in range(count):
        kind = reader.u8()
        if kind == 0:
            bank = reader.u8() if flags & _PALETTE_BANK_INDEX else 0
            if flags & _MODULE_POSITION:
                reader.take(4)
            entries.append((dimension(), dimension(), bank))
            continue
        metadata = _PRIMITIVES.get(kind)
        if metadata is None:
            raise SpriteFormatError('Unknown sprite module type')
        prefix, has_size, suffix = metadata
        reader.take(prefix)
        if has_size:
            dimension()
            dimension()
        reader.take(suffix)
        entries.append(None)
    return entries


def _skip_frame_tables(reader, flags):
    coordinate_bytes = 2 if flags & _WIDE_FRAME_COORDINATES else 1
    has_rects = bool(flags & _FRAME_RECTS)
    reader.skip_table(2 + 2 * coordinate_bytes + bool(flags & _FRAME_MODULE_EXTRA))
    if has_rects:
        reader.skip_table(4 * coordinate_bytes)
    frames = reader.u16()
    reader.take(frames * (3 + has_rects))
    reader.take(frames * 4 * coordinate_bytes)
    animation_frame_bytes = 2 if flags & _WIDE_ANIMATION_FRAME else 1
    animation_coord_bytes = 2 if flags & _WIDE_ANIMATION_COORDINATES else 1
    reader.skip_table(animation_frame_bytes + 2 * animation_coord_bytes + 2)
    reader.skip_table(3)


def _read_palettes(reader, flags, palette_format):
    color_bytes = PALETTE_WIDTH.get(palette_format)
    if color_bytes is None:
        raise SpriteFormatError('Unsupported palette format')
    banks = reader.u8() if flags & _MULTIPLE_PALETTE_BANKS else 1
    if not 1 <= banks <= 16:
        raise SpriteFormatError('Invalid palette bank count')

    palettes = []
    for _ in range(banks):
        variants, colors = reader.u8(), reader.u8() or 256
        if not 1 <= variants <= 16:
            raise SpriteFormatError('Invalid palette variant count')
        palettes.append(tuple(reader.take(colors * color_bytes) for _ in range(variants)))
    return tuple(palettes)


def read_sprite_layout(data):
    reader = SpriteReader(data)
    if reader.take(len(SIGNATURE)) != SIGNATURE:
        raise SpriteFormatError('Wrong sprite signature')
    flags, count = reader.u32(), reader.u16()
    if not flags & _INDEXED_PIXELS or flags & ~_SUPPORTED_FLAGS or count > _MODULE_LIMIT:
        raise SpriteFormatError('Unsupported sprite flags or module count')
    entries = _read_module_table(reader, count, flags)
    _skip_frame_tables(reader, flags)
    if not count:
        if reader.remaining:
            raise SpriteFormatError('Unexpected data in empty sprite block')
        return None

    palette_format = reader.u16()
    palettes = _read_palettes(reader, flags, palette_format)
    pixel_format = reader.u16()
    length = reader.u32 if flags & _WIDE_PAYLOAD_SIZE else reader.u16
    modules = []
    for index, entry in enumerate(entries):
        pixels = reader.take(length())
        if entry is None:
            continue
        width, height, bank = entry
        area = width * height
        if area > _PIXEL_LIMIT or bank >= len(palettes) or bool(area) != bool(pixels):
            raise SpriteFormatError('Invalid sprite module dimensions, size or palette bank')
        if area:
            modules.append(SpriteModule(index, width, height, bank, pixels))
    if reader.remaining:
        raise SpriteFormatError('Unexpected data after sprite modules')
    return SpriteLayout(palette_format, pixel_format, palettes, tuple(modules))
