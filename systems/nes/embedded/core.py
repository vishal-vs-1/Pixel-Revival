"""Serialize calls and manage lifetime for the single native Mesen core."""
from __future__ import annotations

import ctypes
import os
from pathlib import Path
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from ..capture import CAPTURE_DIRECTORY, owned_path
from .api import MesenNativeApi
from .keys import _key_name


class MesenCoreHost:
    """Serialize native core calls and keep its single emulator alive until app shutdown."""

    def __init__(self, core_dll: Path, graphics_directory: Path, api_factory=MesenNativeApi.load):
        self.core_dll = Path(core_dll).resolve()
        self.graphics_directory = Path(graphics_directory).resolve()
        self.api_factory = api_factory
        self._executor = ThreadPoolExecutor(max_workers=1, thread_name_prefix="mesen-core")
        self._api = None
        self._dll_directories = []
        self._initialized = False
        self._released = False
        self._session_active = False
        self._lock = threading.RLock()

    def submit(self, callback, *args):
        with self._lock:
            if self._released:
                raise RuntimeError("Mesen core has already been released")
            return self._executor.submit(callback, *args)

    def start(self, session, window_handle: int, viewer_handle: int, *, original: bool, capture: bool):
        return self.submit(self._start, session, window_handle, viewer_handle, original, capture)

    def _start(self, session, window_handle: int, viewer_handle: int, original: bool, capture: bool):
        if self._session_active:
            raise RuntimeError("An in-process NES game is already active")
        if self._api is None:
            if os.name != "nt" or ctypes.sizeof(ctypes.c_void_p) != 8:
                raise RuntimeError("In-process Mesen requires Windows x64")
            self._dll_directories = [
                os.add_dll_directory(str(self.core_dll.parent)),
                os.add_dll_directory(str(self.graphics_directory)),
            ]
            self._api = self.api_factory(self.core_dll)
        api = self._api
        try:
            if not self._initialized:
                api.InitDll()
                self._initialized = True
            api.SetRevivalRenderer(not original)
            api.SetRevivalNesObserver(capture)
            api.InitializeEmu(os.fsencode(session.home), ctypes.c_void_p(window_handle),
                              ctypes.c_void_p(viewer_handle), False, False, False, False)
            self._session_active = True
            if not api.SetRevivalSaveFolder(os.fsencode(session.saves)):
                raise RuntimeError("Mesen could not set the game-owned save folder")
            actions = ("UP", "DOWN", "LEFT", "RIGHT", "A", "B", "START", "SELECT")
            keys = {action: api.GetKeyCode(_key_name(session.controls[action]).encode("utf-8"))
                    for action in actions}
            key_values = [keys[action] for action in actions]
            if not all(key_values):
                raise RuntimeError("Mesen does not recognize one of the selected NES keys")
            if not api.SetRevivalNesControls((ctypes.c_uint16 * len(key_values))(*key_values), len(key_values)):
                raise RuntimeError("Mesen could not apply the selected NES controls")
            api.AddKnownGameFolder(os.fsencode(session.rom_path.parent))
            if not api.LoadRom(os.fsencode(session.rom_path), None):
                raise RuntimeError("Mesen did not load the selected NES cartridge")
            if capture:
                # The artwork worker reads committed chunks from the game's shared capture inbox.
                inbox = owned_path(session.home.parents[3], CAPTURE_DIRECTORY)
                if not api.StartRevivalNesCapture(str(inbox), session.game_id.encode("ascii"),
                                                  session.session_id.encode("ascii")):
                    raise RuntimeError("Mesen could not start NES tile capture")
            if not original:
                deadline = time.monotonic() + 8.0
                while time.monotonic() < deadline:
                    status = api.hd_playback_status()
                    if (status.pack_loaded and status.pack_scale == session.pack_scale and
                            status.common_renderer_active and status.presented_frames > 0):
                        return keys, status
                    if not api.IsRunning():
                        break
                    time.sleep(0.04)
                raise RuntimeError("Enhanced NES rendering did not become ready in the app-owned view")
            return keys, None
        except BaseException:
            self._stop_native_session()
            raise

    def _stop_native_session(self):
        if not self._session_active or self._api is None:
            return
        api = self._api
        try:
            api.ResetKeyState()
            api.Stop()
        finally:
            try:
                api.StopRevivalNesCapture()
            finally:
                api.SetRevivalNesObserver(False)
                self._session_active = False

    def stop(self):
        return self.submit(self._stop_native_session)

    def key_state(self, key_code: int, pressed: bool):
        return self.submit(self._api.SetKeyState, key_code, bool(pressed)) if self._api is not None else None

    def reset_keys(self):
        if self._api is not None:
            return self.submit(self._api.ResetKeyState)
        return None

    def resize(self, width: int, height: int):
        if width <= 0 or height <= 0:
            return None
        return self.submit(self._api.SetRendererSize, min(width, 8192), min(height, 8192)) \
            if self._api is not None and self._session_active else None

    def pause(self, paused: bool):
        if self._api is not None and self._session_active:
            return self.submit(self._api.Pause if paused else self._api.Resume)
        return None

    def shutdown(self, timeout: float = 60.0):
        with self._lock:
            if self._released:
                return
        future = self.submit(self._shutdown_native)
        future.result(timeout=timeout)
        with self._lock:
            self._released = True
        self._executor.shutdown(wait=True, cancel_futures=False)
        for directory in self._dll_directories:
            directory.close()
        self._dll_directories.clear()

    def _shutdown_native(self):
        if self._api is None:
            return
        try:
            self._stop_native_session()
        finally:
            if self._initialized:
                self._api.Release()
                self._initialized = False
