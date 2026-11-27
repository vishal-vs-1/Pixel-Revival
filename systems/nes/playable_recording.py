"""Bounded, opt-in OAM journal for the portable NES player."""

import json
import re

from .capture import owned_path

MAX_RECORDS = 512
MAX_BYTES = MAX_RECORDS * 96

_RECORDER = r'''
local anchors = {ANCHORS}
local blank = {BLANK}
local journal = JOURNAL
local rom = ROMPATH
local seen, pending, last, count, frame = {}, nil, nil, 0, 0
local function byte(addr, memory) return emu.read(addr, memory) end
local function record()
    frame = frame + 1
    if frame % 4 ~= 0 or count >= 512 then return end
    if emu.getRomInfo().path:gsub('\\','/'):lower() ~= rom:lower() then return end
    local state = emu.getState()
    if state['ppu.control.largeSprites'] then return end
    local bank = state['ppu.control.spritePatternAddr'] or 0
    local cells, occupied = {}, {}
    for slot = 0,63 do
        local a = slot * 4
        local y = byte(a, emu.memType.nesSpriteRam)
        if y < 239 then
            local x = byte(a+3, emu.memType.nesSpriteRam)
            local k = y*256+x
            local tile = byte(a+1, emu.memType.nesSpriteRam)
            local flags = byte(a+2, emu.memType.nesSpriteRam)
            if not blank[bank//16+tile] then
                cells[slot] = {x=x, y=y, tile=tile, flags=flags}
                occupied[k] = occupied[k] == nil and cells[slot] or false
            end
        end
    end
    local x, y = 256, 240
    for _,slot in ipairs(anchors) do
        local cell = cells[slot]
        if not cell then pending, last = nil, nil; return end
        x, y = math.min(x,cell.x), math.min(y,cell.y)
    end
    local best, top, matches = nil, y, 0
    for offset = 0, 32-4*#anchors, 8 do
        local candidate, has = {}, {}
        local start = y-offset
        if x <= 240 and start >= 0 and start <= 207 then
            for row = 0,3 do
                for column = 0,1 do
                    local cell = occupied[(start+row*8)*256+x+column*8]
                    if cell then candidate[#candidate+1] = cell; has[cell] = true end
                end
            end
            local valid = #candidate == 8
            for _,slot in ipairs(anchors) do valid = valid and has[cells[slot]] end
            if valid then best, top, matches = candidate, start, matches+1 end
        end
    end
    if matches ~= 1 then
        best, top = {}, y
        if x > 240 or y >= 240-4*#anchors then pending,last=nil,nil; return end
        for i = 0,#anchors-1 do
            local cell = occupied[(top+(i//2)*8)*256+x+(i%2)*8]
            if not cell then pending, last = nil, nil; return end
            best[#best+1] = cell
        end
        local has = {}; for _,cell in ipairs(best) do has[cell] = true end
        for _,slot in ipairs(anchors) do if not has[cells[slot]] then pending, last = nil,nil; return end end
    end
    local parts = {}
    for _,cell in ipairs(best) do
        local p = 0x3f11 + (cell.flags&3)*4
        parts[#parts+1] = string.format('%03X%02X%02X%02X%d', bank//16+cell.tile,
            byte(p,emu.memType.nesPpuMemory),byte(p+1,emu.memType.nesPpuMemory),
            byte(p+2,emu.memType.nesPpuMemory),cell.flags>>6)
    end
    local key = table.concat(parts)
    local close = last and math.abs(x-last.x)<=16 and math.abs(top+4*#best-last.bottom)<=24
    if not close or pending ~= key then
        pending = key
    elseif not seen[key] then
        local f = io.open(journal, 'a')
        if f then
            local written = f:write(key,'\n')
            if f:close() and written then seen[key]=true; count=count+1 end
        end
    end
    last = {x=x,bottom=top+4*#best}
end
emu.addEventCallback(record, emu.eventType.endFrame)
'''


def write_recorder(home, slots, journal, chr_rom, source):
    script = owned_path(home, 'record-player.lua')
    values = {
        'ANCHORS': ','.join(map(str, slots)),
        'BLANK': ','.join(f'[{i}]=true' for i in range(512) if not any(chr_rom[i * 16:(i + 1) * 16])),
        'ROMPATH': json.dumps(source.as_posix(), ensure_ascii=False),
        'JOURNAL': json.dumps(journal.as_posix(), ensure_ascii=False),
    }
    lua = re.sub(r'\b(?:ANCHORS|BLANK|ROMPATH|JOURNAL)\b', lambda match: values[match[0]], _RECORDER)
    script.write_text(lua, encoding='utf-8')
    return script


def read_recording(path):
    with path.open('rb') as stream:
        payload = stream.read(MAX_BYTES + 1)
    if len(payload) > MAX_BYTES:
        raise ValueError('The player pose recording exceeds its size limit.')
    for line in payload.splitlines(keepends=True):
        if re.fullmatch(rb'(?:[0-1][0-9A-F]{2}(?:[0-3][0-9A-F]){3}[0-3]){4}(?:(?:[0-1][0-9A-F]{2}(?:[0-3][0-9A-F]){3}[0-3]){4})?\r?\n', line):
            yield tuple((int(line[i:i+3],16), 'FF'+line[i+3:i+9].decode('ascii'), int(line[i+9:i+10]))
                        for i in range(0, len(line.rstrip()), 10))
