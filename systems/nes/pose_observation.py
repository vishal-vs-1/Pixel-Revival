"""Decode original PPU data once for recording and pose replacement."""

from .live_player_capture import Frame, Sprite
from .live_pose import LiveCell


def capture_frame(core, handle, number, buttons, observation, *, ram=False):
    large, bank, oam, palette = observation
    height = 16 if large else 8
    colors = ''.join(f'{value & 0x3f:02x}' for value in palette[16:32])
    if getattr(core, 'pose_observation_supported', False) is True:
        cells = core.pose_frame(handle)
        if cells is not None:
            sprites = tuple(Sprite(cell[0], cell[1], cell[2],
                (-1 if ram else int.from_bytes(cell[4:6], 'little'), -2),
                cell[9:25].hex(), 'FF' + cell[6:9].hex().upper())
                for cell in cells if any(cell[9:25]))
            return Frame(number, buttons & 255, 8, colors, sprites, core.pose_camera(handle))
    patterns = {}
    sprites = []
    for slot in range(64):
        y, tile, flags, x = oam[slot * 4:slot * 4 + 4]
        if y >= 239:
            continue
        address = ((tile & 1) * 4096 + (tile & 0xfe) * 16
                   if large else bank + tile * 16)
        first = address // 16
        indices = (first, first + 1) if large else (first,)
        data = []
        for index in indices:
            if index not in patterns:
                try:
                    patterns[index] = core.tile_pattern(handle, index) or b''
                except Exception:
                    patterns[index] = b''
            data.append(patterns[index])
        if any(len(pattern) != 16 for pattern in data) or not any(any(p) for p in data):
            continue
        sprites.append(Sprite(x, y, flags,
                              (-1, -1) if ram else (first, first + 1 if large else -2),
                              b''.join(data).hex()))
    return Frame(number, buttons & 255, height, colors, tuple(sprites))


def frame_cells(frame):
    cells = []
    for sprite in frame.sprites:
        offset = (sprite.flags & 3) * 8 + 2
        colors = sprite.colors or 'FF' + frame.palette[offset:offset + 6].upper()
        mirrors = sprite.flags >> 6
        for part in range(frame.height // 8):
            row = 1 - part if frame.height == 16 and mirrors & 2 else part
            index = sprite.indices[part]
            cells.append(LiveCell(sprite.x, sprite.y + 1 + row * 8,
                                  None if index < 0 else index, colors, mirrors,
                                  sprite.pattern[part * 32:(part + 1) * 32]))
    return tuple(cells)
