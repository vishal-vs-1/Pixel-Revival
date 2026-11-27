"""ctypes binding for the vendored Rust NES core (`rust-nes/mesen-player`).

The DLL is built from this repo (`rust-nes/README.md`) and lives under
`build/rust-nes-target/release/mesen_player.dll` (overridable with
`RUST_NES_DLL`). Every entry point is panic-safe and null-safe on the Rust
side; this module turns negative returns into `RuntimeError` with the
native message attached.
"""

from __future__ import annotations

import ctypes
import hashlib
import os
from pathlib import Path

ABI_VERSION = 1

_ENV_DLL = "RUST_NES_DLL"

_BUTTON_BITS = {
    "A": 0x01, "B": 0x02, "SELECT": 0x04, "START": 0x08,
    "UP": 0x10, "DOWN": 0x20, "LEFT": 0x40, "RIGHT": 0x80,
}


def find_dll(app_root: Path | None = None) -> Path | None:
    """Locate the built Rust player library without loading it.

    An explicit `app_root` is authoritative (tests and portable builds must not
    pick up an unrelated checkout), while a bare call falls back to this repo.
    """
    override = os.environ.get(_ENV_DLL)
    if override:
        candidate = Path(override)
        if candidate.is_file():
            return candidate
        return None
    root = Path(app_root) if app_root is not None else Path(__file__).resolve().parents[2]
    release = root / "build" / "rust-nes-target" / "release"
    # Experiment builds require RUST_NES_DLL, regardless of file timestamps.
    standard = release / 'mesen_player.dll'
    return standard if standard.is_file() else None


class RustCoreError(RuntimeError):
    """The Rust core refused an operation; the message came from the core."""


class RustCore:
    """Own one loaded `mesen_player.dll`; create players from ROM bytes."""

    def __init__(self, dll_path: Path | None = None, *, app_root: Path | None = None):
        path = Path(dll_path) if dll_path is not None else find_dll(app_root)
        if path is None or not path.is_file():
            raise RustCoreError("Rust NES player is not built; see rust-nes/README.md")
        try:
            self._lib = ctypes.WinDLL(str(path))
        except OSError as error:
            raise RustCoreError(f"Could not load Rust NES player: {error}") from error
        lib = self._lib
        lib.rs_abi_version.argtypes = []
        lib.rs_abi_version.restype = ctypes.c_uint
        if int(lib.rs_abi_version()) != ABI_VERSION:
            raise RustCoreError("Rust NES player ABI is not version 1")
        self.identity = {'path': str(path.resolve()), 'abi': ABI_VERSION,
                         'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}
        lib.rs_is_alive.argtypes = [ctypes.c_void_p]
        lib.rs_is_alive.restype = ctypes.c_int
        lib.rs_create.argtypes = [ctypes.c_char_p, ctypes.c_size_t, ctypes.c_uint,
                                  ctypes.c_char_p, ctypes.c_size_t]
        lib.rs_create.restype = ctypes.c_void_p
        lib.rs_destroy.argtypes = [ctypes.c_void_p]
        lib.rs_destroy.restype = None
        lib.rs_run_frame.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t]
        lib.rs_run_frame.restype = ctypes.c_int
        lib.rs_framebuffer.argtypes = [ctypes.c_void_p,
                                       ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_int),
                                       ctypes.c_char_p, ctypes.c_size_t]
        lib.rs_framebuffer.restype = ctypes.POINTER(ctypes.c_uint32)
        lib.rs_drain_audio.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_float),
                                       ctypes.c_size_t, ctypes.c_char_p, ctypes.c_size_t]
        lib.rs_drain_audio.restype = ctypes.c_int
        lib.rs_set_controller.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_ubyte,
                                          ctypes.c_char_p, ctypes.c_size_t]
        lib.rs_set_controller.restype = ctypes.c_int
        lib.rs_reset.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t]
        lib.rs_reset.restype = ctypes.c_int
        lib.rs_battery_size.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t]
        lib.rs_battery_size.restype = ctypes.c_int
        lib.rs_get_battery.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t,
                                        ctypes.c_char_p, ctypes.c_size_t]
        lib.rs_get_battery.restype = ctypes.c_int
        lib.rs_set_battery.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t,
                                        ctypes.c_char_p, ctypes.c_size_t]
        lib.rs_set_battery.restype = ctypes.c_int
        lib.rs_save_state.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t,
                                      ctypes.c_char_p, ctypes.c_size_t]
        lib.rs_save_state.restype = ctypes.c_ssize_t
        lib.rs_load_state.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t,
                                      ctypes.c_char_p, ctypes.c_size_t]
        lib.rs_load_state.restype = ctypes.c_int
        lib.rs_frame_count.argtypes = [ctypes.c_void_p]
        lib.rs_frame_count.restype = ctypes.c_ulonglong
        lib.rs_region.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t]
        lib.rs_region.restype = ctypes.c_int
        lib.rs_mapper_id.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t]
        lib.rs_mapper_id.restype = ctypes.c_int
        lib.rs_hd_create.argtypes = [ctypes.c_uint]
        lib.rs_hd_create.restype = ctypes.c_void_p
        lib.rs_hd_destroy.argtypes = [ctypes.c_void_p]
        lib.rs_hd_destroy.restype = None
        lib.rs_hd_clear.argtypes = [ctypes.c_void_p]
        lib.rs_hd_clear.restype = None
        lib.rs_hd_add_tile.argtypes = [ctypes.c_void_p, ctypes.c_ubyte, ctypes.c_uint32,
                                       ctypes.c_uint32, ctypes.c_char_p, ctypes.c_size_t,
                                       ctypes.c_uint, ctypes.c_char_p, ctypes.c_size_t]
        lib.rs_hd_add_tile.restype = ctypes.c_int
        lib.rs_hd_output_size.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_int),
                                          ctypes.POINTER(ctypes.c_int)]
        lib.rs_hd_output_size.restype = ctypes.c_int
        lib.rs_hd_stats.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_ulonglong),
                                    ctypes.POINTER(ctypes.c_ulonglong)]
        lib.rs_hd_stats.restype = None
        lib.rs_hd_run_frame.argtypes = [ctypes.c_void_p, ctypes.c_void_p,
                                        ctypes.POINTER(ctypes.c_uint32), ctypes.c_size_t,
                                        ctypes.c_char_p, ctypes.c_size_t]
        lib.rs_hd_run_frame.restype = ctypes.c_int
        lib.rs_hd_arm.argtypes = [ctypes.c_void_p, ctypes.c_ubyte,
                                  ctypes.c_char_p, ctypes.c_size_t]
        lib.rs_hd_arm.restype = ctypes.c_int
        lib.rs_hd_debug.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_ubyte),
                                    ctypes.POINTER(ctypes.c_ubyte),
                                    ctypes.POINTER(ctypes.c_ulonglong)]
        lib.rs_hd_debug.restype = ctypes.c_int
        try:
            lib.rs_capture_arm.argtypes = [ctypes.c_void_p, ctypes.c_ubyte,
                                           ctypes.c_char_p, ctypes.c_size_t]
            lib.rs_capture_arm.restype = ctypes.c_int
            lib.rs_capture_export.argtypes = [ctypes.c_void_p,
                                              ctypes.POINTER(ctypes.c_uint64),
                                              ctypes.c_size_t]
            lib.rs_capture_export.restype = ctypes.c_int
            lib.rs_tile_pattern.argtypes = [ctypes.c_void_p, ctypes.c_uint32,
                                            ctypes.POINTER(ctypes.c_ubyte)]
            lib.rs_tile_pattern.restype = ctypes.c_int
            lib.rs_standard_palette.argtypes = [ctypes.POINTER(ctypes.c_ubyte)]
            lib.rs_standard_palette.restype = ctypes.c_int
            self._capture_supported = True
        except AttributeError:
            # Older build without capture exports; playback still works.
            self._capture_supported = False
        try:
            lib.rs_observation.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_ubyte),
                                           ctypes.c_size_t, ctypes.c_char_p,
                                           ctypes.c_size_t]
            lib.rs_observation.restype = ctypes.c_int
            self._observation_supported = True
        except AttributeError:
            self._observation_supported = False
        self._pose_handles = set()
        self._pose_cameras = {}
        try:
            lib.rs_pose_version.argtypes = []
            lib.rs_pose_version.restype = ctypes.c_uint
            lib.rs_pose_arm.argtypes = [ctypes.c_void_p, ctypes.c_ubyte, ctypes.c_char_p, ctypes.c_size_t]
            lib.rs_pose_arm.restype = ctypes.c_int
            lib.rs_pose_frame.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_ubyte),
                                          ctypes.c_size_t, ctypes.c_char_p, ctypes.c_size_t]
            lib.rs_pose_frame.restype = ctypes.c_int
            self._pose_observation_supported = lib.rs_pose_version() == 2
        except AttributeError:
            self._pose_observation_supported = False
        self.path = path

    @staticmethod
    def _err():
        return ctypes.create_string_buffer(256)

    @staticmethod
    def _check(code: int, err, what: str):
        if int(code) < 0:
            raise RustCoreError(f"{what} failed: {err.value.decode('utf-8', 'replace')}")
        return code

    def button_mask(self, actions) -> int:
        mask = 0
        for action in actions:
            mask |= _BUTTON_BITS.get(action, 0)
        return mask

    def create(self, rom: bytes, *, sample_rate: int = 44100) -> int:
        err = self._err()
        handle = self._lib.rs_create(bytes(rom), len(rom), int(sample_rate), err, 256)
        if not handle:
            raise RustCoreError(f"ROM rejected: {err.value.decode('utf-8', 'replace')}")
        return handle

    def destroy(self, handle: int):
        self._pose_handles.discard(handle)
        self._pose_cameras.pop(handle, None)
        self._lib.rs_destroy(handle)

    def run_frame(self, handle: int):
        err = self._err()
        self._check(self._lib.rs_run_frame(handle, err, 256), err, "run_frame")

    def framebuffer(self, handle: int) -> tuple[bytes, int, int]:
        err = self._err()
        width, height = ctypes.c_int(0), ctypes.c_int(0)
        ptr = self._lib.rs_framebuffer(handle, ctypes.byref(width), ctypes.byref(height),
                                       err, 256)
        if not ptr or width.value <= 0 or height.value <= 0:
            raise RustCoreError(f"framebuffer failed: {err.value.decode('utf-8', 'replace')}")
        count = width.value * height.value
        return ctypes.string_at(ptr, count * 4), width.value, height.value

    def drain_audio(self, handle: int, capacity: int = 4096) -> bytes:
        err = self._err()
        floats = (ctypes.c_float * capacity)()
        got = self._lib.rs_drain_audio(handle, floats, capacity, err, 256)
        self._check(got, err, "drain_audio")
        return bytes(floats)[:got * 4] if got else b""

    def set_controller(self, handle: int, port: int, mask: int):
        err = self._err()
        self._check(self._lib.rs_set_controller(handle, port, mask & 0xFF, err, 256),
                    err, "set_controller")

    def reset(self, handle: int):
        err = self._err()
        self._check(self._lib.rs_reset(handle, err, 256), err, "reset")

    def battery_size(self, handle: int) -> int:
        # -1 means "no battery" and is a normal answer, not an error.
        err = self._err()
        return int(self._lib.rs_battery_size(handle, err, 256))

    def get_battery(self, handle: int) -> bytes | None:
        err = self._err()
        size = self._lib.rs_battery_size(handle, err, 256)
        if size < 0:
            return None
        out = ctypes.create_string_buffer(size)
        self._check(self._lib.rs_get_battery(handle, out, size, err, 256), err, "get_battery")
        return out.raw

    def set_battery(self, handle: int, data: bytes):
        err = self._err()
        self._check(self._lib.rs_set_battery(handle, bytes(data), len(data), err, 256),
                    err, "set_battery")

    def save_state(self, handle: int) -> bytes:
        err = self._err()
        need = self._lib.rs_save_state(handle, None, 0, err, 256)
        if need <= 0:
            raise RustCoreError(f"save_state failed: {err.value.decode('utf-8', 'replace')}")
        out = ctypes.create_string_buffer(need)
        got = self._lib.rs_save_state(handle, out, need, err, 256)
        if got != need:
            raise RustCoreError("save_state size changed between calls")
        return out.raw

    def load_state(self, handle: int, blob: bytes):
        err = self._err()
        self._check(self._lib.rs_load_state(handle, bytes(blob), len(blob), err, 256),
                    err, "load_state")

    def frame_count(self, handle: int) -> int:
        return int(self._lib.rs_frame_count(handle))

    def region(self, handle: int) -> int:
        err = self._err()
        return self._check(self._lib.rs_region(handle, err, 256), err, "region")

    def mapper_id(self, handle: int) -> int:
        err = self._err()
        return self._check(self._lib.rs_mapper_id(handle, err, 256), err, "mapper_id")

    def hd_create(self, scale: int) -> int:
        handle = self._lib.rs_hd_create(int(scale))
        if not handle:
            raise RustCoreError("HD table creation failed (scale must be 2 or 4)")
        return handle

    def hd_destroy(self, handle: int):
        if handle:
            self._lib.rs_hd_destroy(handle)

    def hd_clear(self, handle: int):
        if handle:
            self._lib.rs_hd_clear(handle)

    def hd_output_size(self, handle: int) -> tuple[int, int]:
        width, height = ctypes.c_int(0), ctypes.c_int(0)
        if self._lib.rs_hd_output_size(handle, ctypes.byref(width),
                                      ctypes.byref(height)) != 0:
            raise RustCoreError("HD output size failed")
        return width.value, height.value

    def hd_add_tile(self, handle: int, *, sprite: bool, chr_index: int,
                    palette: int, rgba: bytes, size: int) -> int:
        err = self._err()
        count = self._lib.rs_hd_add_tile(handle, 1 if sprite else 0,
                                        chr_index & 0xFFFFFFFF,
                                        palette & 0xFFFFFFFF,
                                        bytes(rgba), len(rgba), size, err, 256)
        return self._check(count, err, "hd_add_tile")

    def hd_stats(self, handle: int) -> tuple[int, int]:
        lookups = ctypes.c_ulonglong(0)
        hits = ctypes.c_ulonglong(0)
        self._lib.rs_hd_stats(handle, ctypes.byref(lookups), ctypes.byref(hits))
        return int(lookups.value), int(hits.value)

    def hd_run_frame(self, handle: int, hd: int, out: bytearray) -> int:
        err = self._err()
        array_type = ctypes.c_uint32 * (len(out) // 4)
        buffer = array_type.from_buffer(out)
        got = self._lib.rs_hd_run_frame(handle, hd, buffer, len(out), err, 256)
        return self._check(got, err, "hd_run_frame")

    def hd_arm(self, handle: int, active: bool):
        err = self._err()
        self._check(self._lib.rs_hd_arm(handle, 1 if active else 0, err, 256),
                    err, "hd_arm")

    def capture_arm(self, handle: int, active: bool):
        if not self.capture_supported:
            raise RustCoreError("this Rust build has no tile capture")
        err = self._err()
        self._check(self._lib.rs_capture_arm(handle, 1 if active else 0, err, 256),
                    err, "capture_arm")

    @property
    def capture_supported(self) -> bool:
        return bool(getattr(self, "_capture_supported", False))

    def capture_export(self, handle: int) -> list[int]:
        if not self.capture_supported:
            return []
        total = int(self._lib.rs_capture_export(handle, None, 0))
        if total <= 0:
            return []
        buffer = (ctypes.c_uint64 * total)()
        got = int(self._lib.rs_capture_export(handle, buffer, total))
        if got < 0:
            raise RustCoreError("capture_export failed")
        return [int(buffer[index]) for index in range(min(got, total))]

    def tile_pattern(self, handle: int, index: int) -> bytes:
        if not self.capture_supported:
            raise RustCoreError("this Rust build has no tile capture")
        out = (ctypes.c_ubyte * 16)()
        if self._lib.rs_tile_pattern(handle, int(index), out) != 0:
            raise RustCoreError(f"tile_pattern failed for tile {index}")
        return bytes(out)

    @property
    def observation_supported(self) -> bool:
        return bool(getattr(self, "_observation_supported", False))

    def observation(self, handle: int):
        """Return (large_sprites, sprite_base, oam, palette) from the core."""
        if not self.observation_supported:
            raise RustCoreError("this Rust build has no pose observation")
        out = (ctypes.c_ubyte * 291)()
        err = self._err()
        got = int(self._lib.rs_observation(handle, out, 291, err, 256))
        if got != 291:
            raise RustCoreError(
                f"observation failed: {err.value.decode('utf-8', 'replace')}")
        large = bool(out[0])
        bank = int(out[1]) | (int(out[2]) << 8)
        oam = bytes(out[3:3 + 0x100])
        palette = bytes(out[3 + 0x100:3 + 0x100 + 0x20])
        return large, bank, oam, palette

    @property
    def pose_observation_supported(self):
        return bool(getattr(self, '_pose_observation_supported', False))

    def pose_arm(self, handle, active=True):
        if not self.pose_observation_supported:
            return
        err = self._err()
        self._check(self._lib.rs_pose_arm(handle, bool(active), err, 256), err, 'pose_arm')
        if active:
            self._pose_handles.add(handle)
        else:
            self._pose_handles.discard(handle)
            self._pose_cameras.pop(handle, None)

    def pose_frame(self, handle):
        if handle not in self._pose_handles:
            return None
        out = (ctypes.c_ubyte * (4 + 128 * 25))()
        err = self._err()
        count = self._check(self._lib.rs_pose_frame(handle, out, len(out), err, 256), err, 'pose_frame')
        if count > 128:
            raise RustCoreError('pose_frame exceeds its record limit')
        data = bytes(out)
        camera = int.from_bytes(data[:2], 'little'), int.from_bytes(data[2:4], 'little')
        self._pose_cameras[handle] = camera if all(value < 512 for value in camera) else None
        return [data[4 + i * 25:4 + (i + 1) * 25] for i in range(count)]

    def pose_camera(self, handle):
        return self._pose_cameras.get(handle)

    def standard_palette(self) -> bytes:
        if not self.capture_supported:
            raise RustCoreError("this Rust build has no tile capture")
        out = (ctypes.c_ubyte * 192)()
        if self._lib.rs_standard_palette(out) != 0:
            raise RustCoreError("standard palette unavailable")
        return bytes(out)

    def hd_debug(self, handle: int) -> tuple[bool, bool, int]:
        # Debug probe: (armed, chr_ram, pending_records) without draining.
        active, is_ram, pending = ctypes.c_ubyte(0), ctypes.c_ubyte(0), ctypes.c_ulonglong(0)
        code = self._lib.rs_hd_debug(handle, ctypes.byref(active),
                                     ctypes.byref(is_ram), ctypes.byref(pending))
        if int(code) < 0:
            raise RustCoreError("hd_debug failed")
        return bool(active.value), bool(is_ram.value), int(pending.value)

    def is_alive(self, handle: int) -> bool:
        return bool(self._lib.rs_is_alive(handle))
