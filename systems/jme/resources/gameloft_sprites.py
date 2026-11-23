"""Convert validated Gameloft bitmap modules to native RGBA images."""
from struct import iter_unpack

from PIL import Image

from systems.jme.resources.gameloft_sprite_layout import (
    PALETTE_WIDTH, SpriteFormatError, SpriteReader,
)

_BIT_DEPTH = {0x0200: 1, 0x0400: 2, 0x1600: 4}
_RAW_INDEX = 0x5602
_PACKED_RUN = 0x64f0
_INDEX_OR_REPEAT = 0x27f1
_LITERAL_OR_REPEAT = 0x56f2
_ALPHA_PAIRS = 0xa562
_ALPHA_RUN_FORMATS = {
    0xa256: _LITERAL_OR_REPEAT,
    0xa640: _PACKED_RUN,
    0xa127: _INDEX_OR_REPEAT,
}


def _palette_rgba(format_code, data):
    if format_code == 0x8888:
        colors = ((r, g, b, a) for b, g, r, a in iter_unpack('4B', data))
    elif format_code == 0x8808:
        colors = ((r, g, b, 0 if (r, g, b) == (255, 0, 255) else 255)
                  for b, g, r in iter_unpack('3B', data))
    elif format_code == 0x4444:
        colors = (tuple(((value >> shift) & 15) * 17 for shift in (8, 4, 0, 12))
                  for value, in iter_unpack('<H', data))
    else:
        colors = (_rgb16(format_code, value) for value, in iter_unpack('<H', data))
    return bytes(channel for color in colors for channel in color)


def _rgb16(format_code, value):
    if format_code == 0x5515:
        red, green = (value >> 7) & 248, (value >> 2) & 248
        alpha = 255 if value & 0x8000 else 0
    else:
        red, green = (value >> 8) & 248, (value >> 3) & 252
        alpha = 0 if value == 0xf81f else 255
    blue = (value & 31) << 3
    if (red, green, blue) == (248, 0, 248) and alpha == 0:
        return 255, 0, 255, 0
    return red, green, blue, alpha


def _append_run(output, run, area):
    if not run or len(run) > area - len(output):
        raise SpriteFormatError('Invalid sprite run length')
    output.extend(run)


def _read_index_runs(reader, area, format_code, palette_size):
    output = bytearray()
    bits = (palette_size - 1).bit_length()
    mask = (1 << bits) - 1
    while len(output) < area:
        token = reader.u8()
        if format_code == _PACKED_RUN:
            run = bytes((token & mask,)) * ((token >> bits) + 1)
        elif format_code == _INDEX_OR_REPEAT:
            run = (bytes((reader.u8(),)) * (token - 128)
                   if token > 127 else bytes((token,)))
        elif token >= 128:
            run = reader.take(token - 128)
        else:
            run = bytes((reader.u8(),)) * token
        _append_run(output, run, area)
    return bytes(output)


def _read_alpha_runs(reader, area):
    values = bytearray()
    while len(values) < area:
        alpha = reader.u8()
        count = 1
        if alpha == 254:
            count, alpha = reader.u8(), reader.u8()
        _append_run(values, bytes((alpha,)) * count, area)
    return bytes(values)


def _read_module_pixels(data, area, format_code, palette_size):
    alpha = None
    reader = SpriteReader(data)
    bits = _BIT_DEPTH.get(format_code)
    if bits:
        packed = reader.take((area * bits + 7) // 8)
        mask = (1 << bits) - 1
        indices = bytes((byte >> shift) & mask for byte in packed
                        for shift in range(8 - bits, -1, -bits))[:area]
    elif format_code == _RAW_INDEX:
        # One palette-index byte per pixel; Wrath of the Titans uses this
        # encoding inside its newer (content-type 3) sprite blocks.
        indices = bytes(reader.take(area))
    elif format_code == _ALPHA_PAIRS:
        values = reader.take(2 * area)
        alpha, indices = bytes(values[::2]), bytes(values[1::2])
    else:
        index_format = _ALPHA_RUN_FORMATS.get(format_code, format_code)
        if index_format not in (_PACKED_RUN, _INDEX_OR_REPEAT, _LITERAL_OR_REPEAT):
            raise SpriteFormatError('Unsupported sprite pixel encoding')
        indices = _read_index_runs(reader, area, index_format, palette_size)
        if format_code in _ALPHA_RUN_FORMATS:
            alpha = _read_alpha_runs(reader, area)

    if reader.remaining or max(indices) >= palette_size:
        raise SpriteFormatError('Invalid sprite palette index or payload length')
    return indices, alpha


def decode_sprite_images(layout):
    palettes = tuple(tuple(_palette_rgba(layout.palette_format, palette) for palette in bank)
                     for bank in layout.palettes)
    # Validate the whole pixel stream before the caller can publish any module.
    modules = []
    for module in layout.modules:
        palette_size = len(layout.palettes[module.palette_bank][0]) // PALETTE_WIDTH[layout.palette_format]
        indices, alpha = _read_module_pixels(module.pixels, module.width * module.height,
                                             layout.pixel_format, palette_size)
        modules.append((module, indices, alpha))
    return _render_modules(modules, palettes)


def _render_modules(modules, palettes):
    for module, indices, alpha in modules:
        size = (module.width, module.height)
        indexed = Image.frombytes('P', size, indices)
        alpha_image = Image.frombytes('L', size, alpha) if alpha is not None else None
        for variant, palette in enumerate(palettes[module.palette_bank]):
            indexed.putpalette(palette, rawmode='RGBA')
            image = indexed.convert('RGBA')
            if alpha_image is not None:
                image.putalpha(alpha_image)
            if image.getchannel('A').getbbox():
                yield module.index, module.palette_bank, variant, image
