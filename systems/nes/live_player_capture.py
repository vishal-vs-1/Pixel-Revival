"""Bounded gameplay observations from the Mesen sprite table."""

from dataclasses import dataclass
import json
import re

from .capture import owned_path


MAX_SAMPLES = 2400
MAX_LINE = 12288
RECORDING_VERSION = b'nes-live-player\t2\n'

_HOOK = r"""
local path, journal, limit = ROMPATH, JOURNAL, LIMIT
local buttons = {'a','b','select','start','up','down','left','right'}
local frame, samples, started = 0, 0, false
local recentDirection, recentActions = 0, 0
local ppu, oam = emu.memType.nesPpuMemory, emu.memType.nesSpriteRam
local hex = {}
for value = 0, 255 do hex[value] = string.format('%02x', value) end

local function tileAt(address, cache)
    if cache[address] ~= nil then return cache[address] end
    local bytes, visible = {}, false
    for i = 0, 15 do
        local value = emu.read(address + i, ppu)
        bytes[i+1], visible = hex[value], visible or value ~= 0
    end
    local mapped = emu.convertAddress(address, ppu)
    local index = -2
    if mapped and mapped.memType == emu.memType.nesChrRom and mapped.address % 16 == 0 then
        index = mapped.address // 16
    elseif mapped and mapped.memType == emu.memType.nesChrRam then
        index = -1
    end
    local tile = {index, table.concat(bytes), visible}
    cache[address] = tile
    return tile
end

local early

local function snapshot()
    local state = emu.getState()
    local height = state['ppu.control.largeSprites'] and 16 or 8
    local bank = state['ppu.control.spritePatternAddr']
    if bank ~= 0 and bank ~= 4096 then return end
    local palette = {}
    for i = 0, 15 do palette[i+1] = hex[emu.read(0x3f10+i, ppu) & 63] end
    local row, cache = {height, table.concat(palette)}, {}
    for slot = 0, 63 do
        local entry = slot * 4
        local y = emu.read(entry, oam)
        if y < 239 then
            local tile = emu.read(entry+1, oam)
            local x = emu.read(entry+3, oam)
            local flags = emu.read(entry+2, oam)
            local address = height == 16 and ((tile & 1)*4096 + (tile & 0xfe)*16) or (bank + tile*16)
            local a = tileAt(address, cache)
            local b = height == 16 and tileAt(address+16, cache) or {-2,'',false}
            if a[3] or b[3] then
                row[#row+1] = table.concat({x,y,flags,a[1],b[1],a[2]..b[2]}, ',')
            end
        end
    end
    return row
end

local function sample()
    if samples >= limit then return end
    if emu.getRomInfo().path:gsub('\\','/'):lower() ~= path:lower() then return end
    frame = frame + 1
    local input, mask = emu.getInput(0), 0
    for i, button in ipairs(buttons) do
        if input[button] == true then mask = mask | (1 << (i-1)) end
    end
    local direction = mask & 240
    if direction ~= 0 then recentDirection = direction end
    recentActions = recentActions | (mask & 3)
    started = started or (mask & 243) ~= 0
    if not started or frame % 3 ~= 0 then return end
    mask = (mask & 12) | recentActions | (direction ~= 0 and direction or recentDirection)
    recentDirection, recentActions = 0, 0
    local late = snapshot()
    -- Some games switch graphics banks after the visible sprites are fetched.
    -- Keep the earlier snapshot when the end-of-frame bank exposes no tiles.
    if (not late or #late == 2) and early and #early > 2 then late = early end
    early = nil
    if not late then return end
    local row = {frame,mask}
    for _, part in ipairs(late) do row[#row+1] = part end
    local output = io.open(journal, 'ab')
    if output then
        local written = output:write(table.concat(row, '\t'), '\n')
        if output:close() and written then samples = samples + 1 end
    end
end

emu.addEventCallback(function()
    if started and samples < limit and (frame+1) % 3 == 0
        and emu.getRomInfo().path:gsub('\\','/'):lower() == path:lower() then
        early = snapshot()
    end
end, emu.eventType.startFrame)
emu.addEventCallback(sample, emu.eventType.endFrame)
"""


@dataclass(frozen=True)
class Sprite:
    x: int
    y: int
    flags: int
    indices: tuple[int, int]
    pattern: str
    colors: str | None = None


@dataclass(frozen=True)
class Frame:
    number: int
    buttons: int
    height: int
    palette: str
    sprites: tuple[Sprite, ...]
    camera: tuple[int, int] | None = None


def write_hook(home, journal, source):
    path = owned_path(home, 'observe-player.lua')
    values = {'ROMPATH': json.dumps(source.as_posix(), ensure_ascii=False),
              'JOURNAL': json.dumps(journal.as_posix(), ensure_ascii=False), 'LIMIT': str(MAX_SAMPLES)}
    path.write_text(re.sub(r'\b(?:ROMPATH|JOURNAL|LIMIT)\b', lambda m: values[m[0]], _HOOK), encoding='utf-8')
    return path


def read_frames(path):
    with path.open('rb') as stream:
        if stream.readline(MAX_LINE) not in (RECORDING_VERSION, b'nes-live-player\t1\n'):
            raise ValueError('Invalid NES player recording header.')
        last = 0
        for count in range(MAX_SAMPLES + 1):
            row = stream.readline(MAX_LINE + 1)
            if not row:
                break
            if count == MAX_SAMPLES or len(row) > MAX_LINE:
                raise ValueError('The NES player recording exceeds its size limit.')
            if not row.endswith(b'\n'):
                break
            number, buttons, height, palette, *records = row.decode('ascii').rstrip('\r\n').split('\t')
            camera = None
            if records and records[0].startswith('@camera='):
                camera = tuple(map(int, records.pop(0)[8:].split(',')))
                if len(camera) != 2 or any(not 0 <= value < 512 for value in camera):
                    raise ValueError('Invalid camera observation.')
            number, buttons, height = int(number), int(buttons), int(height)
            if (not last < number <= 2**31 or not 0 <= buttons <= 255 or height not in (8, 16)
                    or not re.fullmatch('(?:[0-3][0-9a-f]){16}', palette) or len(records) > 128):
                raise ValueError('Invalid NES player recording frame.')
            sprites = []
            for record in records:
                fields = record.split(',')
                if len(fields) not in (6, 7):
                    raise ValueError('Invalid NES player recording sprite.')
                x, y, flags, first, second, pattern = fields[:6]
                colors = fields[6] if len(fields) == 7 else None
                if colors is not None and not re.fullmatch('FF(?:[0-3][0-9A-F]){3}', colors):
                    raise ValueError('Invalid sprite palette.')
                x, y, flags, first, second = map(int, (x, y, flags, first, second))
                if (not 0 <= x <= 255 or not 0 <= y < 239 or not 0 <= flags <= 255
                        or any(not -2 <= index < 0x400000 for index in (first, second))
                        or len(pattern) != height * 4 or not re.fullmatch('[0-9a-f]+', pattern)):
                    raise ValueError('Invalid NES player recording sprite.')
                sprites.append(Sprite(x, y, flags, (first, second), pattern, colors))
            last = number
            yield Frame(number, buttons, height, palette, tuple(sprites), camera)
