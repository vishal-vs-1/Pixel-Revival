"""Typed bindings for the pinned Mesen core used by the in-process NES host."""

from __future__ import annotations

import ctypes
import os
from pathlib import Path


class RevivalHdPlaybackStatus(ctypes.Structure):
    _fields_ = [
        ("struct_bytes", ctypes.c_uint32),
        ("abi_version", ctypes.c_uint32),
        ("pack_loaded", ctypes.c_uint32),
        ("pack_scale", ctypes.c_uint32),
        ("common_renderer_active", ctypes.c_uint32),
        ("frame_width", ctypes.c_uint32),
        ("frame_height", ctypes.c_uint32),
        ("reserved", ctypes.c_uint32),
        ("identity_available", ctypes.c_uint32),
        ("graphics_abi_version", ctypes.c_uint32),
        ("submitted_frames", ctypes.c_uint64),
        ("presented_frames", ctypes.c_uint64),
        ("graphics_build_id", ctypes.c_char * 65),
        ("adapter_vendor", ctypes.c_char * 129),
        ("adapter_renderer", ctypes.c_char * 257),
        ("adapter_version", ctypes.c_char * 129),
    ]


if ctypes.sizeof(RevivalHdPlaybackStatus) != 640:
    raise RuntimeError("Mesen HD playback status ABI layout changed")


class MesenNativeApi:
    """Bind and validate the C ABI without initializing the emulator instance."""

    def __init__(self, library):
        self.library = library
        signatures = {
            "InitDll": (None, ()),
            "SetRevivalRenderer": (None, (ctypes.c_bool,)),
            "SetRevivalNesObserver": (None, (ctypes.c_bool,)),
            "InitializeEmu": (None, (ctypes.c_char_p, ctypes.c_void_p, ctypes.c_void_p,
                                     ctypes.c_bool, ctypes.c_bool, ctypes.c_bool, ctypes.c_bool)),
            "LoadRom": (ctypes.c_bool, (ctypes.c_char_p, ctypes.c_char_p)),
            "IsRunning": (ctypes.c_bool, ()),
            "IsPaused": (ctypes.c_bool, ()),
            "GetStopCode": (ctypes.c_int32, ()),
            "Pause": (None, ()),
            "Resume": (None, ()),
            "Stop": (None, ()),
            "Release": (None, ()),
            "SetKeyState": (None, (ctypes.c_uint16, ctypes.c_bool)),
            "ResetKeyState": (None, ()),
            "GetKeyCode": (ctypes.c_uint16, (ctypes.c_char_p,)),
            "SetRendererSize": (None, (ctypes.c_uint32, ctypes.c_uint32)),
            "SetRevivalSaveFolder": (ctypes.c_bool, (ctypes.c_char_p,)),
            "SetRevivalNesControls": (ctypes.c_bool,
                                      (ctypes.POINTER(ctypes.c_uint16), ctypes.c_uint32)),
            "StartRevivalNesCapture": (ctypes.c_bool,
                                       (ctypes.c_wchar_p, ctypes.c_char_p, ctypes.c_char_p)),
            "StopRevivalNesCapture": (None, ()),
            "GetRevivalNesHdPlaybackStatus": (None, (ctypes.POINTER(RevivalHdPlaybackStatus),)),
            "AddKnownGameFolder": (None, (ctypes.c_char_p,)),
        }
        for name, (result, arguments) in signatures.items():
            function = getattr(library, name)
            function.restype = result
            function.argtypes = list(arguments)
            setattr(self, name, function)

    def hd_playback_status(self) -> RevivalHdPlaybackStatus:
        status = RevivalHdPlaybackStatus()
        self.GetRevivalNesHdPlaybackStatus(ctypes.byref(status))
        if status.struct_bytes != ctypes.sizeof(status) or status.abi_version != 2:
            raise RuntimeError("Mesen returned an incompatible HD playback status")
        return status

    @classmethod
    def load(cls, path: Path):
        """Load an absolute DLL path; its dependencies must already be resolvable."""
        path = Path(path)
        if os.name != "nt" or ctypes.sizeof(ctypes.c_void_p) != 8:
            raise RuntimeError("The Mesen native host requires Windows x64")
        if not path.is_absolute() or not path.is_file():
            raise ValueError("The Mesen core DLL must be an existing absolute path")
        return cls(ctypes.WinDLL(str(path)))
