"""Embed the comparison players' Win32 surfaces in the launcher-owned panes."""

import ctypes as c
from ctypes import wintypes as w
import os


class _Placement(c.Structure):
    _fields_ = [("length", w.UINT), ("flags", w.UINT), ("show", w.UINT),
                ("min", w.POINT), ("max", w.POINT), ("normal", w.RECT)]


class ComparisonNativeWindows:
    def __init__(self):
        if os.name != "nt":
            raise RuntimeError("The shared comparison window requires Windows.")
        self._user = c.WinDLL("user32", use_last_error=True)
        self._enum_callback = c.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)
        functions = {
            "EnumWindows": ((self._enum_callback, w.LPARAM), w.BOOL),
            "EnumChildWindows": ((w.HWND, self._enum_callback, w.LPARAM), w.BOOL),
            "GetWindow": ((w.HWND, w.UINT), w.HWND),
            "GetParent": ((w.HWND,), w.HWND),
            "GetMenu": ((w.HWND,), w.HMENU),
            "SetMenu": ((w.HWND, w.HMENU), w.BOOL),
            "ShowWindow": ((w.HWND, c.c_int), w.BOOL),
            "GetWindowPlacement": ((w.HWND, c.POINTER(_Placement)), w.BOOL),
            "SystemParametersInfoW": ((w.UINT, w.UINT, c.c_void_p, w.UINT), w.BOOL),
            "GetWindowThreadProcessId": ((w.HWND, c.POINTER(w.DWORD)), w.DWORD),
            "GetWindowTextLengthW": ((w.HWND,), c.c_int),
            "GetWindowRect": ((w.HWND, c.POINTER(w.RECT)), w.BOOL),
            "GetWindowLongPtrW": ((w.HWND, c.c_int), c.c_ssize_t),
            "SetWindowLongPtrW": ((w.HWND, c.c_int, c.c_ssize_t), c.c_ssize_t),
            "SetParent": ((w.HWND, w.HWND), w.HWND),
            "SetWindowPos": ((w.HWND, w.HWND, c.c_int, c.c_int, c.c_int, c.c_int, w.UINT), w.BOOL),
            "GetAncestor": ((w.HWND, w.UINT), w.HWND),
            "GetForegroundWindow": ((), w.HWND),
            "GetFocus": ((), w.HWND),
            "PostMessageW": ((w.HWND, w.UINT, w.WPARAM, w.LPARAM), w.BOOL),
            "IsWindow": ((w.HWND,), w.BOOL),
            "IsWindowVisible": ((w.HWND,), w.BOOL),
        }
        for name, (args, result) in functions.items():
            function = getattr(self._user, name)
            function.argtypes = args
            function.restype = result

    def find_players(self, pids):
        windows = {}

        @self._enum_callback
        def collect(window, _):
            pid = w.DWORD()
            self._user.GetWindowThreadProcessId(window, c.byref(pid))
            if pid.value not in pids or self._user.GetWindow(window, 4):
                return True
            if not self._user.GetWindowTextLengthW(window):
                return True
            rect = w.RECT()
            if self._user.GetWindowRect(window, c.byref(rect)):
                area = (rect.right - rect.left) * (rect.bottom - rect.top)
                if area >= 32_000 and area > windows.get(pid.value, (0, None))[0]:
                    windows[pid.value] = area, window
            return True

        self._user.EnumWindows(collect, 0)
        return {pid: value[1] for pid, value in windows.items()}

    def style(self, window):
        return self._user.GetWindowLongPtrW(window, -16)

    def find_renderer_child(self, parent):
        """Return the largest child window, which is Mesen's NativeRenderer surface."""
        best = (0, None)

        @self._enum_callback
        def collect(window, _):
            if not self._user.IsWindowVisible(window):
                return True
            rect = w.RECT()
            if not self._user.GetWindowRect(window, c.byref(rect)):
                return True
            area = (rect.right - rect.left) * (rect.bottom - rect.top)
            if area > best[0]:
                best[:] = [area, window]
            return True

        self._user.EnumChildWindows(parent, collect, 0)
        return best[1]

    def parent_of(self, window):
        return self._user.GetParent(window)

    def embed(self, window, parent):
        style = self._user.GetWindowLongPtrW(window, -16)
        # SetParent keeps the original top-level style unless we replace it.
        child_style = (style & ~0xa1cf0000) | 0x40000000
        c.set_last_error(0)
        if not self._user.SetWindowLongPtrW(window, -16, child_style) and c.get_last_error():
            raise c.WinError(c.get_last_error())
        c.set_last_error(0)
        if not self._user.SetParent(window, parent) and c.get_last_error():
            raise c.WinError(c.get_last_error())

    def ensure(self, window, parent, width, height):
        """Re-apply the borderless child layout in case the player restyled itself."""
        actions = []
        style = self._user.GetWindowLongPtrW(window, -16)
        wanted = (style & ~0xa1cf0000) | 0x40000000
        if style != wanted:
            c.set_last_error(0)
            if not self._user.SetWindowLongPtrW(window, -16, wanted) and c.get_last_error():
                raise c.WinError(c.get_last_error())
            actions.append(f"style:{style:#x}->{wanted:#x}")
        if self._user.GetMenu(window):
            c.set_last_error(0)
            if not self._user.SetMenu(window, None) and c.get_last_error():
                raise c.WinError(c.get_last_error())
            actions.append("menu")
        if self._user.GetParent(window) != parent:
            c.set_last_error(0)
            if not self._user.SetParent(window, parent) and c.get_last_error():
                raise c.WinError(c.get_last_error())
            actions.append("parent")
        flags = 0x0034  # keep Z order and focus, apply the frame change
        if not self._user.SetWindowPos(window, None, 0, 0, max(1, width), max(1, height), flags):
            raise c.WinError(c.get_last_error())
        return actions

    def fit(self, window, width, height, *, show=False):
        flags = 0x0034 | (0x0060 if show else 0)  # keep Z order and focus, apply the frame change
        if not self._user.SetWindowPos(window, None, 0, 0, max(1, width), max(1, height), flags):
            raise c.WinError(c.get_last_error())

    def work_area(self):
        """Usable desktop rectangle so tiled players avoid the taskbar."""
        rect = w.RECT()
        if not self._user.SystemParametersInfoW(0x30, 0, c.byref(rect), 0):
            raise c.WinError(c.get_last_error())
        return rect.left, rect.top, rect.right, rect.bottom

    def halves(self):
        """Left/right rectangles splitting the usable desktop area."""
        left, top, right, bottom = self.work_area()
        middle = left + (right - left) // 2
        return (left, top, middle, bottom), (middle, top, right, bottom)

    def minimized(self, window):
        info = _Placement()
        info.length = c.sizeof(_Placement)
        if not self._user.GetWindowPlacement(window, c.byref(info)):
            raise c.WinError(c.get_last_error())
        return info.show == 2

    def hide_from_taskbar(self, window):
        """Keep a background player out of the taskbar and Alt+Tab."""
        style = self._user.GetWindowLongPtrW(window, -20)
        wanted = (style | 0x00000080) & ~0x00040000  # add WS_EX_TOOLWINDOW, drop APPWINDOW
        if style != wanted:
            c.set_last_error(0)
            if not self._user.SetWindowLongPtrW(window, -20, wanted) and c.get_last_error():
                raise c.WinError(c.get_last_error())

    def place_behind(self, window, rect):
        """Put a visible source window behind everything at the requested rect."""
        x, y, right, bottom = rect
        flags = 0x0010  # keep the current activation state
        if not self._user.SetWindowPos(window, 1, x, y, max(1, right - x),
                                       max(1, bottom - y), flags):  # HWND_BOTTOM
            raise c.WinError(c.get_last_error())

    def place(self, window, rect):
        """Tile an external player window without stealing keyboard focus."""
        x, y, right, bottom = rect[0], rect[1], rect[2], rect[3]
        if self.minimized(window):
            c.set_last_error(0)
            # Restoring a minimized window may briefly activate it; the tick
            # below returns keyboard focus to the shared control window.
            if not self._user.ShowWindow(window, 9) and c.get_last_error():
                raise c.WinError(c.get_last_error())
        flags = 0x0054  # keep Z order and focus, show
        if not self._user.SetWindowPos(window, None, x, y,
                                       max(1, right - x), max(1, bottom - y), flags):
            raise c.WinError(c.get_last_error())

    def tile_pair(self, first, second):
        """Put two external player windows side by side; returns their rects."""
        rects = self.halves()
        self.place(first, rects[0])
        self.place(second, rects[1])
        return rects

    def is_foreground(self, host):
        return self._user.GetForegroundWindow() == self._user.GetAncestor(host, 2)

    def has_focus(self, host):
        return self._user.GetFocus() == host

    def close(self, window):
        self.release(window)
        if self._user.IsWindow(window):
            self._user.PostMessageW(window, 0x0010, 0, 0)

    def post_close(self, window):
        """Ask a normal top-level player to close without changing its style."""
        if self._user.IsWindow(window):
            self._user.PostMessageW(window, 0x0010, 0, 0)

    def release(self, window):
        """Restore an embedded window to top-level so its own close path runs."""
        style = self._user.GetWindowLongPtrW(window, -16)
        top = (style & ~0x40000000) | 0x00CF0000
        if style != top:
            c.set_last_error(0)
            if not self._user.SetWindowLongPtrW(window, -16, top) and c.get_last_error():
                raise c.WinError(c.get_last_error())
        c.set_last_error(0)
        if not self._user.SetParent(window, None) and c.get_last_error():
            raise c.WinError(c.get_last_error())
        flags = 0x0027  # keep size, position and order, apply the frame change
        if not self._user.SetWindowPos(window, None, 0, 0, 0, 0, flags):
            raise c.WinError(c.get_last_error())
