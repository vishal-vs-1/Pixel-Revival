"""Fast Win32 frame presentation for the in-tool NES player.

Tk's `PhotoImage` repaint costs ~11-25 ms per frame at window sizes we use, so
60 fps is impossible through the canvas image. This view mirrors the working
minifb frontend: a native child window whose paint is a single GDI
`StretchDIBits` over a top-down 32-bpp DIB. Our framebuffer is already
`0x00RRGGBB` in u32, which is `B,G,R,X` in memory and matches the DIB layout,
so no per-frame conversion is needed.
"""

from __future__ import annotations

import ctypes
import threading
import time
from ctypes import wintypes as w

WS_CHILD = 0x40000000
WS_VISIBLE = 0x10000000
WS_CLIPSIBLINGS = 0x04000000
WS_EX_NOACTIVATE = 0x08000000
WS_EX_TRANSPARENT = 0x00000020
CS_HREDRAW = 0x0002
CS_VREDRAW = 0x0001
WM_DESTROY = 0x0002
WM_PAINT = 0x000F
WM_ERASEBKGND = 0x0014
WM_NCHITTEST = 0x0084
WM_MOUSEACTIVATE = 0x0021
MA_NOACTIVATE = 3
GWL_STYLE = -16
WS_CLIPCHILDREN = 0x02000000
HTTRANSPARENT = -1
BI_RGB = 0
DIB_RGB_COLORS = 0
SRCCOPY = 0x00CC0020
COLORONCOLOR = 3
HALFTONE = 4


class BITMAPINFOHEADER(ctypes.Structure):
    _fields_ = [
        ("biSize", w.DWORD), ("biWidth", w.LONG), ("biHeight", w.LONG),
        ("biPlanes", w.WORD), ("biBitCount", w.WORD),
        ("biCompression", w.DWORD), ("biSizeImage", w.DWORD),
        ("biXPelsPerMeter", w.LONG), ("biYPelsPerMeter", w.LONG),
        ("biClrUsed", w.DWORD), ("biClrImportant", w.DWORD),
    ]


class RGBQUAD(ctypes.Structure):
    _fields_ = [("rgbBlue", ctypes.c_ubyte), ("rgbGreen", ctypes.c_ubyte),
                ("rgbRed", ctypes.c_ubyte), ("rgbReserved", ctypes.c_ubyte)]


class BITMAPINFO(ctypes.Structure):
    _fields_ = [("bmiHeader", BITMAPINFOHEADER), ("bmiColors", RGBQUAD * 1)]


class RECT(ctypes.Structure):
    _fields_ = [("left", w.LONG), ("top", w.LONG),
                ("right", w.LONG), ("bottom", w.LONG)]


class POINT(ctypes.Structure):
    _fields_ = [("x", w.LONG), ("y", w.LONG)]


class WNDCLASS(ctypes.Structure):
    _fields_ = [
        ("style", w.UINT), ("lpfnWndProc", ctypes.c_void_p),
        ("cbClsExtra", ctypes.c_int), ("cbWndExtra", ctypes.c_int),
        ("hInstance", w.HANDLE), ("hIcon", w.HANDLE), ("hCursor", w.HANDLE),
        ("hbrBackground", w.HANDLE), ("lpszMenuName", w.LPCWSTR),
        ("lpszClassName", w.LPCWSTR),
    ]


class Win32FrameView:
    """Own one child HWND and present RGB frames with GDI."""

    _registered = False
    _wndproc = None
    _class_name = "JARRevivalNESFrame"

    def __init__(self, parent_hwnd: int):
        self._user = ctypes.WinDLL("user32", use_last_error=True)
        self._gdi = ctypes.WinDLL("gdi32", use_last_error=True)
        self._lock = threading.RLock()
        self._parent = int(parent_hwnd)
        self._data = None
        self._src = (0, 0)
        self._closed = False
        self._brush = None
        # Double-buffer: compose into an offscreen bitmap, then BitBlt once.
        # Painting FillRect + StretchDIBits straight to the window shows an
        # intermediate black frame (flicker).
        self._memdc = None
        self._membmp = None
        self._mem_old = None
        self._mem_w = 0
        self._mem_h = 0
        # Last GDI stage costs (ms) for stall diagnosis; read after present().
        self._last_fill_ms = 0.0
        self._last_blit_ms = 0.0
        self._last_bitblt_ms = 0.0
        self._configure()

        if not Win32FrameView._registered:
            self._register()

        self._hinstance = self._kernel.GetModuleHandleW(None)
        handle = self._user.CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
            self._class_name, "", WS_CHILD | WS_CLIPSIBLINGS,
            0, 0, 16, 16, self._parent, None, self._hinstance, None)
        if not handle:
            raise ctypes.WinError(ctypes.get_last_error())
        self.handle = int(handle)
        Win32FrameView._instances[self.handle] = self
        self._brush = self._gdi.CreateSolidBrush(0)
        # Tk may repaint the parent canvas over its children on resize/erase;
        # WS_CLIPCHILDREN keeps the native frame visible.
        try:
            style = self._user.GetWindowLongPtrW(w.HWND(self._parent), GWL_STYLE)
            self._user.SetWindowLongPtrW(w.HWND(self._parent), GWL_STYLE,
                                         style | WS_CLIPCHILDREN)
            self._user.SetWindowPos(w.HWND(self._parent), None, 0, 0, 0, 0,
                                    0x0002 | 0x0001 | 0x0004 | 0x0010 | 0x0020)
        except (OSError, AttributeError):
            pass
        self._visible = False
        self._show(False)

    # -- setup ---------------------------------------------------------------
    def _register(self):
        user = self._user
        WNDPROC = ctypes.WINFUNCTYPE(w.LPARAM, w.HWND, w.UINT, w.WPARAM, w.LPARAM)

        def _proc(hwnd, msg, wparam, lparam):
            view = Win32FrameView._instance_for(hwnd)
            if msg == WM_ERASEBKGND:
                return 1
            if msg == WM_NCHITTEST:
                return HTTRANSPARENT
            if msg == WM_MOUSEACTIVATE:
                return MA_NOACTIVATE
            if msg == WM_PAINT:
                if view is not None:
                    view._paint()
                # Without validating, Windows keeps re-posting WM_PAINT.
                user.ValidateRect(hwnd, None)
                return 0
            if msg == WM_DESTROY:
                return 0
            return user.DefWindowProcW(hwnd, msg, wparam, lparam)

        Win32FrameView._wndproc = WNDPROC(_proc)
        Win32FrameView._instances = {}
        Win32FrameView._instance_for = staticmethod(
            lambda hwnd: Win32FrameView._instances.get(int(hwnd)))

        user.DefWindowProcW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]
        user.DefWindowProcW.restype = w.LPARAM
        user.RegisterClassW.argtypes = [ctypes.POINTER(WNDCLASS)]
        user.RegisterClassW.restype = w.ATOM
        user.CreateWindowExW.argtypes = [
            w.DWORD, w.LPCWSTR, w.LPCWSTR, w.DWORD,
            ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int,
            w.HWND, w.HMENU, w.HANDLE, w.LPVOID]
        user.CreateWindowExW.restype = w.HWND
        user.MoveWindow.argtypes = [w.HWND, ctypes.c_int, ctypes.c_int,
                                    ctypes.c_int, ctypes.c_int, w.BOOL]
        user.MoveWindow.restype = w.BOOL
        user.DestroyWindow.argtypes = [w.HWND]
        user.DestroyWindow.restype = w.BOOL
        user.GetClientRect.argtypes = [w.HWND, ctypes.POINTER(RECT)]
        user.GetClientRect.restype = w.BOOL
        user.GetDC.argtypes = [w.HWND]
        user.GetDC.restype = w.HDC
        user.ReleaseDC.argtypes = [w.HWND, w.HDC]
        user.ReleaseDC.restype = ctypes.c_int
        user.ScreenToClient.argtypes = [w.HWND, ctypes.POINTER(POINT)]
        user.ScreenToClient.restype = w.BOOL
        user.ShowWindow.argtypes = [w.HWND, ctypes.c_int]
        user.ShowWindow.restype = w.BOOL
        user.ValidateRect.argtypes = [w.HWND, ctypes.c_void_p]
        user.ValidateRect.restype = w.BOOL

        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.GetModuleHandleW.argtypes = [w.LPCWSTR]
        kernel.GetModuleHandleW.restype = w.HANDLE
        wc = WNDCLASS()
        wc.style = CS_HREDRAW | CS_VREDRAW
        wc.lpfnWndProc = ctypes.cast(Win32FrameView._wndproc, ctypes.c_void_p)
        wc.hInstance = kernel.GetModuleHandleW(None)
        wc.lpszClassName = self._class_name
        if not user.RegisterClassW(ctypes.byref(wc)):
            error = ctypes.get_last_error()
            if error != 1410:  # already registered
                raise ctypes.WinError(error)
        Win32FrameView._registered = True

    def _configure(self):
        user = self._user
        user.DefWindowProcW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]
        user.DefWindowProcW.restype = w.LPARAM
        user.RegisterClassW.argtypes = [ctypes.POINTER(WNDCLASS)]
        user.RegisterClassW.restype = w.ATOM
        user.CreateWindowExW.argtypes = [
            w.DWORD, w.LPCWSTR, w.LPCWSTR, w.DWORD,
            ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int,
            w.HWND, w.HMENU, w.HANDLE, w.LPVOID]
        user.CreateWindowExW.restype = w.HWND
        user.MoveWindow.argtypes = [w.HWND, ctypes.c_int, ctypes.c_int,
                                    ctypes.c_int, ctypes.c_int, w.BOOL]
        user.MoveWindow.restype = w.BOOL
        user.DestroyWindow.argtypes = [w.HWND]
        user.DestroyWindow.restype = w.BOOL
        user.GetClientRect.argtypes = [w.HWND, ctypes.POINTER(RECT)]
        user.GetClientRect.restype = w.BOOL
        user.GetDC.argtypes = [w.HWND]
        user.GetDC.restype = w.HDC
        user.ReleaseDC.argtypes = [w.HWND, w.HDC]
        user.ReleaseDC.restype = ctypes.c_int
        user.ScreenToClient.argtypes = [w.HWND, ctypes.POINTER(POINT)]
        user.ScreenToClient.restype = w.BOOL
        user.ShowWindow.argtypes = [w.HWND, ctypes.c_int]
        user.ShowWindow.restype = w.BOOL
        user.ValidateRect.argtypes = [w.HWND, ctypes.c_void_p]
        user.ValidateRect.restype = w.BOOL
        user.GetWindowLongPtrW.argtypes = [w.HWND, ctypes.c_int]
        user.GetWindowLongPtrW.restype = ctypes.c_ssize_t
        user.SetWindowLongPtrW.argtypes = [w.HWND, ctypes.c_int, ctypes.c_ssize_t]
        user.SetWindowLongPtrW.restype = ctypes.c_ssize_t
        user.SetWindowPos.argtypes = [w.HWND, w.HWND, ctypes.c_int, ctypes.c_int,
                                      ctypes.c_int, ctypes.c_int, w.UINT]
        user.SetWindowPos.restype = w.BOOL
        user.FillRect.argtypes = [w.HDC, ctypes.POINTER(RECT), w.HBRUSH]
        user.FillRect.restype = ctypes.c_int
        gdi = self._gdi
        gdi.StretchDIBits.argtypes = [
            w.HDC, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int,
            ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int,
            ctypes.c_void_p, ctypes.POINTER(BITMAPINFO), w.UINT, w.DWORD]
        gdi.StretchDIBits.restype = ctypes.c_int
        gdi.SetStretchBltMode.argtypes = [w.HDC, ctypes.c_int]
        gdi.SetStretchBltMode.restype = ctypes.c_int
        gdi.CreateSolidBrush.argtypes = [w.DWORD]
        gdi.CreateSolidBrush.restype = w.HBRUSH
        gdi.DeleteObject.argtypes = [w.HANDLE]
        gdi.DeleteObject.restype = w.BOOL
        gdi.CreateCompatibleDC.argtypes = [w.HDC]
        gdi.CreateCompatibleDC.restype = w.HDC
        gdi.CreateCompatibleBitmap.argtypes = [w.HDC, ctypes.c_int, ctypes.c_int]
        gdi.CreateCompatibleBitmap.restype = w.HBITMAP
        gdi.CreateDIBSection.argtypes = [w.HDC, ctypes.POINTER(BITMAPINFO), w.UINT,
                                       ctypes.POINTER(ctypes.c_void_p), w.HANDLE, w.DWORD]
        gdi.CreateDIBSection.restype = w.HBITMAP
        gdi.SelectObject.argtypes = [w.HDC, w.HGDIOBJ]
        gdi.SelectObject.restype = w.HGDIOBJ
        gdi.BitBlt.argtypes = [w.HDC, ctypes.c_int, ctypes.c_int, ctypes.c_int,
                               ctypes.c_int, w.HDC, ctypes.c_int, ctypes.c_int,
                               w.DWORD]
        gdi.BitBlt.restype = w.BOOL
        gdi.DeleteDC.argtypes = [w.HDC]
        gdi.DeleteDC.restype = w.BOOL
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.GetModuleHandleW.argtypes = [w.LPCWSTR]
        kernel.GetModuleHandleW.restype = w.HANDLE
        self._kernel = kernel

    # -- placement -----------------------------------------------------------
    def place_widget(self, widget):
        """Move the child over a Tk widget (canvas) inside the same toplevel."""
        with self._lock:
            if self._closed:
                return
            point = POINT(int(widget.winfo_rootx()), int(widget.winfo_rooty()))
            self._user.ScreenToClient(self._parent, ctypes.byref(point))
            width = max(1, int(widget.winfo_width()))
            height = max(1, int(widget.winfo_height()))
            self._user.MoveWindow(self.handle, int(point.x), int(point.y),
                                  width, height, True)

    def show(self):
        with self._lock:
            self._show(True)

    def _show(self, visible: bool):
        if self._closed or visible == self._visible:
            return
        self._visible = visible
        self._user.ShowWindow(self.handle, 5 if visible else 0)

    # -- presentation --------------------------------------------------------
    def present(self, data: bytes, src_w: int, src_h: int):
        with self._lock:
            if self._closed or src_w <= 0 or src_h <= 0:
                return
            self._data = data
            self._src = (int(src_w), int(src_h))
            self._show(True)
            self._paint()

    def _ensure_memdc(self, width: int, height: int):
        """Return an offscreen DC with a bitmap matching the client size."""
        if (self._memdc is not None and self._membmp is not None
                and width == self._mem_w and height == self._mem_h):
            return self._memdc
        self._release_memdc()
        hdc_screen = self._user.GetDC(self.handle)
        if not hdc_screen:
            return None
        try:
            memdc = self._gdi.CreateCompatibleDC(hdc_screen)
            if not memdc:
                return None
            # A device-dependent bitmap can force GPU readback when software
            # scaling writes it. Keep the back buffer in CPU-owned 32-bit memory.
            info = BITMAPINFO()
            info.bmiHeader.biSize = ctypes.sizeof(BITMAPINFOHEADER)
            info.bmiHeader.biWidth = width
            info.bmiHeader.biHeight = -height
            info.bmiHeader.biPlanes = 1
            info.bmiHeader.biBitCount = 32
            info.bmiHeader.biCompression = BI_RGB
            bits = ctypes.c_void_p()
            membmp = self._gdi.CreateDIBSection(memdc, ctypes.byref(info), DIB_RGB_COLORS,
                                               ctypes.byref(bits), None, 0)
            if not membmp:
                self._gdi.DeleteDC(memdc)
                return None
            self._mem_old = self._gdi.SelectObject(memdc, membmp)
            self._memdc = memdc
            self._membmp = membmp
            self._mem_w, self._mem_h = width, height
            return memdc
        finally:
            self._user.ReleaseDC(self.handle, hdc_screen)

    def _release_memdc(self):
        if self._memdc is not None:
            try:
                if self._mem_old is not None:
                    self._gdi.SelectObject(self._memdc, self._mem_old)
            except OSError:
                pass
            self._mem_old = None
            if self._membmp is not None:
                try:
                    self._gdi.DeleteObject(self._membmp)
                except OSError:
                    pass
                self._membmp = None
            try:
                self._gdi.DeleteDC(self._memdc)
            except OSError:
                pass
            self._memdc = None
        self._mem_w = self._mem_h = 0

    def _paint(self):
        with self._lock:
            self._paint_locked()

    def _paint_locked(self):
        if self._closed:
            return
        perf = time.perf_counter
        mark = perf()
        rect = RECT()
        self._user.GetClientRect(self.handle, ctypes.byref(rect))
        width = max(1, rect.right - rect.left)
        height = max(1, rect.bottom - rect.top)
        # Compose offscreen, then present with a single BitBlt so the screen
        # never shows the intermediate cleared frame (flicker).
        memdc = self._ensure_memdc(width, height)
        if memdc is None or not self._data:
            # No back buffer (or nothing to draw yet): clear once, directly.
            hdc = self._user.GetDC(self.handle)
            if not hdc:
                return
            try:
                self._user.FillRect(hdc, ctypes.byref(rect), self._brush)
                if not self._data:
                    return
                self._blit(hdc, width, height)
            finally:
                self._user.ReleaseDC(self.handle, hdc)
            return
        mem_rect = RECT(0, 0, width, height)
        self._user.FillRect(memdc, ctypes.byref(mem_rect), self._brush)
        fill_ms = (perf() - mark) * 1000.0
        mark = perf()
        self._blit(memdc, width, height)
        blit_ms = (perf() - mark) * 1000.0
        hdc = self._user.GetDC(self.handle)
        if not hdc:
            return
        try:
            mark = perf()
            self._gdi.BitBlt(hdc, 0, 0, width, height, memdc, 0, 0, SRCCOPY)
            bitblt_ms = (perf() - mark) * 1000.0
        finally:
            self._user.ReleaseDC(self.handle, hdc)
        self._last_fill_ms = fill_ms
        self._last_blit_ms = blit_ms
        self._last_bitblt_ms = bitblt_ms

    def _blit(self, hdc, width: int, height: int):
        """Stretch the current frame into `hdc`, letterboxed, nearest-neighbor."""
        src_w, src_h = self._src
        scale = min(width / src_w, height / src_h)
        dst_w = max(1, int(src_w * scale))
        dst_h = max(1, int(src_h * scale))
        dst_x = (width - dst_w) // 2
        dst_y = (height - dst_h) // 2
        info = BITMAPINFO()
        info.bmiHeader.biSize = ctypes.sizeof(BITMAPINFOHEADER)
        info.bmiHeader.biWidth = src_w
        info.bmiHeader.biHeight = -src_h  # top-down
        info.bmiHeader.biPlanes = 1
        info.bmiHeader.biBitCount = 32
        info.bmiHeader.biCompression = BI_RGB
        self._gdi.SetStretchBltMode(hdc, COLORONCOLOR)
        source = ctypes.cast(ctypes.c_char_p(self._data), ctypes.c_void_p)
        self._gdi.StretchDIBits(
            hdc, dst_x, dst_y, dst_w, dst_h,
            0, 0, src_w, src_h,
            source,
            ctypes.byref(info), DIB_RGB_COLORS, SRCCOPY)

    # -- lifecycle -----------------------------------------------------------
    def close(self):
        with self._lock:
            if self._closed:
                return
            self._closed = True
            try:
                self._release_memdc()
            except (OSError, AttributeError):
                pass
            try:
                Win32FrameView._instances.pop(self.handle, None)
                self._user.DestroyWindow(self.handle)
            except (OSError, AttributeError):
                pass
            if self._brush:
                try:
                    self._gdi.DeleteObject(self._brush)
                except OSError:
                    pass
                self._brush = None


def available() -> bool:
    return hasattr(ctypes, "WinDLL")
