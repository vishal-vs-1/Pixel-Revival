"""Scoped Windows scheduling for the bounded NES playback workers."""

import ctypes
import os
from contextlib import contextmanager


def _priority_api():
    if os.name != 'nt':
        return None
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    kernel.GetCurrentThread.argtypes = []
    kernel.GetCurrentThread.restype = ctypes.c_void_p
    kernel.GetThreadPriority.argtypes = [ctypes.c_void_p]
    kernel.GetThreadPriority.restype = ctypes.c_int
    kernel.SetThreadPriority.argtypes = [ctypes.c_void_p, ctypes.c_int]
    kernel.SetThreadPriority.restype = ctypes.c_int
    return kernel


@contextmanager
def playback_thread_priority():
    restore = None
    try:
        api = _priority_api()
        if api is not None:
            thread = api.GetCurrentThread()
            previous = api.GetThreadPriority(thread)
            # These workers sleep between frames. A modest priority increase
            # prevents ordinary background work from stretching a frame; never
            # alter process priority or lower an existing higher thread level.
            if previous < 1 and api.SetThreadPriority(thread, 1):
                restore = (api, thread, previous)
    except (AttributeError, OSError):
        pass
    try:
        yield
    finally:
        if restore is not None:
            api, thread, previous = restore
            try:
                api.SetThreadPriority(thread, previous)
            except (AttributeError, OSError):
                pass
