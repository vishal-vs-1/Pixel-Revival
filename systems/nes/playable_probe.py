"""Run bounded controller probes in an isolated Mesen process."""

import ctypes as c
import os
from pathlib import Path
import re
import subprocess
from struct import unpack_from
import sys
from time import monotonic, sleep
from zlib import decompressobj, error as ZlibError

from .playable_checkpoints import MAX_STATE_BYTES, read_checkpoint

SCRIPT = """
local frame, start, first, last = 0, %d, %d, %d
%s
emu.addEventCallback(function()
    local input = { start = frame >= start and frame < start + 4 }
    %s
    %s
    emu.setInput(input, 0)
end, emu.eventType.inputPolled)
emu.addEventCallback(function()
    frame = frame + 1
    if frame < first or frame > last then return end
    local state = emu.getState()
    local height = state['ppu.control.largeSprites'] and 16 or 8
    local bank = state['ppu.control.spritePatternAddr'] or 0
    local palette = {}
    for i = 0,15 do palette[#palette+1] = string.format('%%02x', emu.read(0x3f10+i, emu.memType.nesPpuMemory)) end
    emu.log(string.format('P %%d %%d %%d %%s', frame, height, bank, table.concat(palette)))
    for block = 0,1 do
        local bytes = {}
        for i = block*128,block*128+127 do
            bytes[#bytes+1] = string.format('%%02x', emu.read(i, emu.memType.nesSpriteRam))
        end
        emu.log(string.format('O %%d %%d %%s', frame, block, table.concat(bytes)))
    end
    if frame == last then emu.log('DONE'); emu.breakExecution() end
end, emu.eventType.endFrame)
"""

INPUTS = {
    'neutral': '',
    'left': 'input.left = frame >= first + 10 and frame < first + 43',
    'right': 'input.right = frame >= first + 10 and frame < first + 43',
    'turn': 'input.right = frame >= first + 10 and frame < first + 45; input.left = frame >= first + 45',
    'turn_right': 'input.left = frame >= first + 10 and frame < first + 32; input.right = frame >= first + 32',
    'jump': 'input.right = frame >= first + 10; input.a = frame >= first + 25 and frame < first + 51',
    'jump_left': 'input.left = frame >= first + 10; input.a = frame >= first + 25 and frame < first + 51',
    'climb_up': 'input.up = frame >= first + 10 and frame < first + 55',
    'climb_down': 'input.down = frame >= first + 10 and frame < first + 55',
    'checkpoint_left': 'input.left = frame >= first + 10',
    'checkpoint_right': 'input.right = frame >= first + 10',
}


def _checkpoint_bytes(path):
    raw = read_checkpoint(path)
    if len(raw) > MAX_STATE_BYTES or len(raw) < 39 or not raw.startswith(b'MSS'):
        raise ValueError('Choose a Mesen save state for this game (up to 512 KiB).')
    _, version, console, size, width, height, _, packed = unpack_from('<8I', raw, 3)
    if (version != 4 or console != 2 or not 0 < size <= 256 * 240 * 4 or (width, height) != (256, 240)
            or not 0 < packed <= len(raw) - 39):
        raise ValueError('The Mesen checkpoint has an unsupported video header.')
    start = 35 + packed
    name_size, = unpack_from('<I', raw, start)
    if not 0 < name_size <= 512 or start + 4 + name_size >= len(raw):
        raise ValueError('The Mesen checkpoint is incomplete.')
    try:
        decoder = decompressobj()
        frame = decoder.decompress(raw[35:start], size + 1)
    except ZlibError as error:
        raise ValueError('The Mesen checkpoint video could not be decoded.') from error
    if len(frame) != size or not decoder.eof or decoder.unused_data:
        raise ValueError('The Mesen checkpoint video is incomplete.')
    return raw


def trace_player(runtime, rom, home, profile, track, *, state=None):
    home.mkdir(parents=True)
    args = [str(runtime.executable.parent / 'MesenCore.dll'), str(runtime._graphics().dll.parent), str(rom),
            str(home), str(profile.first), str(profile.start), profile.throttle, track, str(state or '')]
    command = ([sys.executable, '--nes-player-probe'] if getattr(sys, 'frozen', False)
               else [sys.executable, '-m', 'systems.nes.playable_probe'])
    result = subprocess.run([*command, *args], stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            timeout=25, creationflags=subprocess.CREATE_NO_WINDOW)
    if result.returncode:
        raise RuntimeError('The NES controller replay failed. ' + result.stdout[-800:].decode('utf-8', 'replace'))
    log = (home / 'probe.log').read_text(encoding='ascii')
    entries = {}
    for frame, height, bank, palette in re.findall(r'^P (\d+) (8|16) (\d+) ([0-9a-f]{32})$', log, re.M):
        entries[int(frame)] = (int(height), int(bank), bytes.fromhex(palette), [])
    for frame, block, payload in re.findall(r'^O (\d+) ([01]) ([0-9a-f]{256})$', log, re.M):
        sprites = entries[int(frame)][3]
        raw = bytes.fromhex(payload)
        if len(sprites) != int(block) * 32:
            raise ValueError('The NES controller replay returned out-of-order sprite data.')
        sprites.extend(tuple(raw[offset:offset + 4]) for offset in range(0, 128, 4))
    if (sorted(entries) != list(range(profile.first, profile.first + 96))
            or any(len(entry[3]) != 64 for entry in entries.values())):
        raise ValueError('The NES controller replay returned incomplete sprite data.')
    return entries


def run(arguments):
    dll, graphics, rom, home, first, start, throttle, track, state = arguments
    first, start = int(first), int(start)
    with os.add_dll_directory(str(Path(dll).parent)), os.add_dll_directory(graphics):
        core = c.WinDLL(dll)

    def call(name, output=None, types=(), *args):
        function = getattr(core, name)
        function.restype = output
        function.argtypes = types
        return function(*args)

    throttle_code = ('input.a = frame >= first - 200; input.b = input.a' if throttle == 'both'
                     else 'input.b = frame >= first - 40')
    if state:
        # Check the decoder's result before collecting any sprite data.
        data = _checkpoint_bytes(Path(state))
        packed = ''.join(f'\\{byte:03d}' for byte in data)
        restore = f'''local callback
callback = emu.addMemoryCallback(function()
    emu.removeMemoryCallback(callback, emu.callbackType.exec, 0x8000, 0xffff)
    assert(emu.loadSavestate("{packed}"), "Mesen could not load the selected checkpoint")
    emu.log('CHECKPOINT')
end, emu.callbackType.exec, 0x8000, 0xffff)'''
        throttle_code = ''
    else:
        restore = ''
    script = (SCRIPT % (start, first, first + 95, restore, throttle_code, INPUTS[track])).encode('ascii')
    call('InitDll')
    try:
        signature = (c.c_char_p, c.c_void_p, c.c_void_p) + (c.c_bool,) * 4
        call('InitializeEmu', None, signature, home.encode(), None, None, True, True, True, True)
        call('Pause')
        if not call('LoadRom', c.c_bool, (c.c_char_p, c.c_char_p), rom.encode(), None):
            raise RuntimeError('Could not load the NES cartridge for detection.')
        call('InitializeDebugger')
        script_id = call('LoadScript', c.c_int32, (c.c_char_p,) * 3 + (c.c_int32,),
                         b'PlayerSpriteProbe', home.encode(), script, -1)
        if script_id <= 0:
            raise RuntimeError('Mesen could not start the controller replay.')
        call('SetEmulationFlag', None, (c.c_uint32, c.c_bool), 4, True)
        call('ResumeExecution')
        call('Resume')
        buffer = c.create_string_buffer(131072)
        deadline = monotonic() + 17
        while monotonic() < deadline:
            call('GetScriptLog', None, (c.c_int32, c.c_char_p, c.c_uint32), script_id, buffer, len(buffer))
            if b'\nDONE' in buffer.value and (not state or b'\nCHECKPOINT' in buffer.value):
                (Path(home) / 'probe.log').write_bytes(buffer.value)
                break
            sleep(0.1)
        else:
            raise RuntimeError('Replay did not finish. ' + buffer.value[-400:].decode('ascii', 'replace'))
        call('ResumeExecution')
        call('RemoveScript', None, (c.c_int32,), script_id)
        call('ReleaseDebugger')
    finally:
        call('Release')


if __name__ == '__main__':
    run(sys.argv[1:])
