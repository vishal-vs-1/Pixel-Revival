"""Run NES games in-tool on the vendored Rust core, no external player involved.

Original and Enhanced both render inside our Tk window. Enhanced composites
the workspace's staged high-detail (HD) pack; with no pack it falls back to
scaled native output instead of failing. Enhanced runs record unseen tiles for
the next pack build when the loaded library exposes capture.
"""

from __future__ import annotations

import ctypes
from ctypes import wintypes as w
import os
import queue
import threading
import time
from pathlib import Path

from .metadata import DEFAULT_RENDER_SCALE
from .rust_core import RustCore, RustCoreError

_SAMPLE_RATE = 44100
_FRAME_DT_NTSC = 1.0 / 60.0988
_FRAME_DT_PAL = 1.0 / 50.007

# Windows defaults to a ~15.6 ms timer, which makes 60 fps sleeps wake late.
# Raise it only while a player is alive; comparison panes share the request.
_TIMER_LOCK = threading.Lock()
_TIMER_USERS = 0


def _timer_begin():
    global _TIMER_USERS
    with _TIMER_LOCK:
        if _TIMER_USERS == 0:
            try:
                ctypes.windll.winmm.timeBeginPeriod(1)
            except (AttributeError, OSError):
                pass
        _TIMER_USERS += 1


def _timer_end():
    global _TIMER_USERS
    with _TIMER_LOCK:
        if _TIMER_USERS <= 0:
            return
        _TIMER_USERS -= 1
        if _TIMER_USERS == 0:
            try:
                ctypes.windll.winmm.timeEndPeriod(1)
            except (AttributeError, OSError):
                pass

_TK_TO_ACTION = {
    "up": "UP", "down": "DOWN", "left": "LEFT", "right": "RIGHT",
    "x": "A", "z": "B", "return": "START", "space": "SELECT",
}


def _tk_keysym(token: str) -> str | None:
    """Translate a stored control key token to a Tk keysym."""
    if token.startswith("KEY_"):
        return token[4:].lower()
    if token.startswith("DIGIT_"):
        return token[6:]
    if token.startswith("NUMPAD_"):
        return "kp_" + token[7:].lower()
    return {
        "ARROW_UP": "up", "ARROW_DOWN": "down",
        "ARROW_LEFT": "left", "ARROW_RIGHT": "right",
        "ENTER": "return", "NUMPAD_ENTER": "kp_enter", "SPACE": "space",
    }.get(token)


def _controls_to_keys(controls) -> dict[str, str]:
    mapped = {}
    for action, token in (controls or {}).items():
        key = _tk_keysym(str(token))
        if key is not None:
            mapped[key] = action
    return mapped


class _WaveFormat(ctypes.Structure):
    _fields_ = [("tag", w.WORD), ("channels", w.WORD), ("samples_per_sec", w.DWORD),
                ("avg_bytes_per_sec", w.DWORD), ("block_align", w.WORD),
                ("bits_per_sample", w.WORD), ("extra", w.WORD)]


class _WaveHeader(ctypes.Structure):
    _fields_ = [("data", ctypes.c_void_p), ("buffer_length", w.DWORD),
                ("bytes_recorded", w.DWORD), ("user", ctypes.c_size_t),
                ("flags", w.DWORD), ("loops", w.DWORD),
                ("next", ctypes.c_void_p), ("reserved", ctypes.c_size_t)]


class RustAudioOut:
    """Best-effort streaming audio through winmm waveOut. Never raises after init.

    If the device cannot open, the player runs muted (`playing` stays False)
    instead of failing the game launch.
    """

    def __init__(self, sample_rate=_SAMPLE_RATE, buffer_samples=512, buffers=12):
        self.sample_rate = sample_rate
        self.buffer_samples = buffer_samples
        self.buffers = buffers
        self.playing = False
        self.muted = True
        self._failed = False
        self._winmm = None
        self._handle = w.HANDLE()
        self._headers = []
        self._events = []
        self._queue: queue.Queue[bytes] = queue.Queue(maxsize=16)
        self._stop = threading.Event()
        self._thread = None
        self._pending = bytearray()
        self.fed_bytes = 0
        self.written = 0
        self.underruns = 0
        self.dropped = 0
        self.late_blocks = 0

    def start(self, log=None):
        try:
            self._open()
        except (OSError, RuntimeError) as error:
            if log is not None:
                try:
                    log(f"Rust audio unavailable, running muted: {error}\n")
                except OSError:
                    pass
            self._failed = True
            self.muted = True
            return
        self.muted = False
        self._stop.clear()
        self._thread = threading.Thread(target=self._feed, name="rust-nes-audio",
                                        daemon=True)
        self._thread.start()
        self.playing = True

    def _open(self):
        winmm = ctypes.WinDLL("winmm", use_last_error=True)
        winmm.waveOutOpen.argtypes = [ctypes.POINTER(w.HANDLE), w.UINT,
                                      ctypes.c_void_p, ctypes.c_void_p,
                                      ctypes.c_void_p, w.DWORD]
        winmm.waveOutOpen.restype = w.UINT
        winmm.waveOutPrepareHeader.argtypes = [w.HANDLE, ctypes.c_void_p, w.UINT]
        winmm.waveOutPrepareHeader.restype = w.UINT
        winmm.waveOutWrite.argtypes = [w.HANDLE, ctypes.c_void_p, w.UINT]
        winmm.waveOutWrite.restype = w.UINT
        winmm.waveOutUnprepareHeader.argtypes = [w.HANDLE, ctypes.c_void_p, w.UINT]
        winmm.waveOutUnprepareHeader.restype = w.UINT
        winmm.waveOutReset.argtypes = [w.HANDLE]
        winmm.waveOutReset.restype = w.UINT
        winmm.waveOutClose.argtypes = [w.HANDLE]
        winmm.waveOutClose.restype = w.UINT
        wave_out_proc = ctypes.WINFUNCTYPE(None, w.HANDLE, w.UINT, ctypes.c_void_p,
                                           ctypes.c_void_p, ctypes.c_void_p)

        def _callback(_handle, message, _instance, param1, _param2):
            if message == 0x3BD:  # WOM_DONE; param1 is the WAVEHDR pointer
                try:
                    hdr = ctypes.cast(param1, ctypes.POINTER(_WaveHeader)).contents
                    index = int(hdr.user)
                except (ValueError, OSError):
                    return
                if 0 <= index < len(self._events):
                    self._events[index].set()

        self._callback = wave_out_proc(_callback)
        fmt = _WaveFormat(1, 1, self.sample_rate, self.sample_rate * 2, 2, 16, 0)
        handle = w.HANDLE()
        result = winmm.waveOutOpen(ctypes.byref(handle), 0xFFFFFFFF, ctypes.byref(fmt),
                                   ctypes.cast(self._callback, ctypes.c_void_p),
                                   None, 0x00030000)
        if result != 0:
            raise RuntimeError(f"waveOutOpen failed ({result:#x})")
        self._winmm = winmm
        self._handle = handle
        self._blocks = []
        for index in range(self.buffers):
            raw = (ctypes.c_byte * (self.buffer_samples * 2))()
            header = _WaveHeader(ctypes.cast(raw, w.LPVOID), self.buffer_samples * 2,
                                 0, index, 0, 0, None, 0)
            if winmm.waveOutPrepareHeader(handle, ctypes.byref(header),
                                          ctypes.sizeof(header)) != 0:
                raise RuntimeError("waveOutPrepareHeader failed")
            self._blocks.append((raw, header))
            self._events.append(threading.Event())
            self._events[-1].set()

        # Prefill most buffers with silence so a short UI hitch cannot starve
        # the device before the first game samples arrive. With 512-sample
        # blocks, ten prefilled buffers give a ~116 ms cushion.
        for index in range(self.buffers):
            raw, header = self._blocks[index]
            header.bytes_recorded = self.buffer_samples * 2
            if winmm.waveOutWrite(handle, ctypes.byref(header),
                                  ctypes.sizeof(header)) == 0:
                self._events[index].clear()
                self.written += 1

    def feed(self, pcm16: bytes):
        if self._failed or not pcm16:
            return
        self.fed_bytes += len(pcm16)
        try:
            self._queue.put_nowait(bytes(pcm16))
        except queue.Full:
            self.dropped += 1
            try:
                self._queue.get_nowait()
            except queue.Empty:
                pass
            try:
                self._queue.put_nowait(bytes(pcm16))
            except queue.Full:
                self.dropped += 1

    def _feed(self):
        """Fill whole device buffers from the queue.

        Each frame produces about 735 samples, but a device buffer holds 1024.
        Writing one frame per buffer padded every block with silence, so the
        stream fell behind and music stuttered. Accumulate across chunks and
        always hand the device exactly `buffer_samples` samples.
        """
        need = self.buffer_samples * 2
        block_seconds = self.buffer_samples / self.sample_rate
        index = 0
        last_write = None
        while not self._stop.is_set():
            event = self._events[index]
            if not event.wait(timeout=1.0) and not self._stop.is_set():
                self.underruns += 1
            if self._stop.is_set():
                return
            event.clear()
            # Pull chunks until this buffer is full. One frame arrives every
            # ~16.7 ms, so wait a little longer than a frame; the prefilled
            # cushion (10 blocks ≈ 116 ms) covers this without starving.
            while len(self._pending) < need and not self._stop.is_set():
                try:
                    # Waiting up to 50 ms is safe: ten prefilled blocks give a
                    # ~116 ms cushion, so the device never runs dry meanwhile.
                    self._pending.extend(self._queue.get(timeout=0.05))
                except queue.Empty:
                    break
            raw, header = self._blocks[index]
            take = min(len(self._pending), need)
            if take:
                ctypes.memmove(raw, bytes(self._pending[:take]), take)
                del self._pending[:take]
            if take < need:
                ctypes.memset(ctypes.byref(raw, take), 0, need - take)
                if not self._stop.is_set():
                    self.underruns += 1
            header.bytes_recorded = need
            header.flags &= ~0x00000001
            started = time.monotonic()
            if last_write is not None and started - last_write > block_seconds * 2:
                # A late full block is inaudible to the partial-fill counter.
                self.late_blocks += 1
            if self._winmm.waveOutWrite(self._handle, ctypes.byref(header),
                                        ctypes.sizeof(header)) != 0:
                event.set()
                return
            last_write = time.monotonic()
            self.written += 1
            index = (index + 1) % self.buffers

    def stop(self):
        self._stop.set()
        thread, self._thread = self._thread, None
        if thread is not None and thread is not threading.current_thread():
            thread.join(timeout=2.0)
        try:
            if self._winmm is not None and self._handle:
                try:
                    self._winmm.waveOutReset(self._handle)
                except OSError:
                    pass
                for _raw, header in self._blocks:
                    try:
                        self._winmm.waveOutUnprepareHeader(self._handle,
                                                          ctypes.byref(header),
                                                          ctypes.sizeof(header))
                    except OSError:
                        pass
                try:
                    self._winmm.waveOutClose(self._handle)
                except OSError:
                    pass
        finally:
            self._winmm = None
            self._handle = w.HANDLE()
            self._blocks = []
            self.playing = False


def _float_to_int16(raw_floats: bytes) -> bytes:
    try:
        import numpy as _np
    except ImportError:
        _np = None
    if _np is not None:
        values = _np.frombuffer(raw_floats, dtype=_np.float32)
        clipped = _np.clip(values, -1.0, 1.0)
        return (clipped * 32767.0).astype(_np.int16).tobytes()
    import struct as _struct
    count = len(raw_floats) // 4
    floats = _struct.unpack(f"<{count}f", raw_floats[:count * 4])
    return _struct.pack(f"<{count}h",
                        *(max(-32768, min(32767, int(round(v * 32767.0)))) for v in floats))


class RustNesRuntime:
    """NES playback inside our own window via the Rust core.

    Original and Enhanced both render in-tool. Enhanced composites the
    workspace's staged HD pack; with no pack it falls back to scaled native
    output instead of failing. Enhanced runs also record unseen tiles for the
    next pack build when the loaded library exposes capture.
    """

    supports_enhanced = True
    supports_in_tool_comparison = True

    def __init__(self, dll_path: Path | None = None, *, app_root: Path | None = None):
        self._core = RustCore(dll_path, app_root=app_root)
        self._lock = threading.RLock()
        self._active = None

    @property
    def core(self):
        return self._core

    def available(self):
        return True

    def capture_available(self):
        return bool(self._core.capture_supported)

    def _pack_snapshot(self, workspace: Path):
        from .rust_hd import read_pack_snapshot

        return read_pack_snapshot(workspace)

    def launch_comparison(self, game, source, workspace, *, host_parent=None,
                          controls=None, **kwargs):
        """Open Original and Enhanced side by side in our own window."""
        from .rust_comparison import launch_comparison

        return launch_comparison(self, game, source, workspace,
                                 host_parent=host_parent, controls=controls,
                                 **kwargs)

    def launch(self, game, source, workspace, *, original=False, host_parent=None,
               controls=None, capture=False, **kwargs):
        if host_parent is None:
            raise RuntimeError("The Rust NES player needs the application window")
        if capture and original:
            raise RuntimeError("Tile capture is available in Enhanced play")
        workspace = Path(workspace)
        player = RustNesPlayer(self._core, game, Path(source), workspace,
                               original=original, controls=controls,
                               host_parent=host_parent)
        with self._lock:
            if self._active is not None and self._active.poll() is None:
                player.close()
                raise RuntimeError("Only one Rust NES game can run at a time; "
                                     "close the current game first")
            self._active = player
        player.start()
        return player, player.log_path

    def shutdown(self, timeout: float = 60.0):
        with self._lock:
            player = self._active
        if player is not None and player.poll() is None:
            try:
                player.shutdown(timeout=timeout)
            except (OSError, RuntimeError):
                pass


class RustNesPlayer:
    """Tk window + worker threads driving one Rust core instance."""

    def __init__(self, core, game, source: Path, workspace: Path, *, original=True,
                 controls=None, host_parent=None, embedded_parent=None):
        import tkinter as tk
        from tkinter import ttk
        from PIL import Image, ImageTk

        self._core = core
        self._game = dict(game)
        self._source = Path(source)
        self._workspace = Path(workspace)
        self._controls = dict(controls) if controls else {}
        self._key_to_action = _controls_to_keys(self._controls) or dict(_TK_TO_ACTION)
        self._timer_active = False
        self._present_gap_last = time.monotonic()
        self._present_gap_max = 0.0
        self._present_stalls = 0
        self._pump_gap_last = time.monotonic()
        self._pump_gap_max = 0.0
        self._pump_stalls = 0
        # Per-stage worst case within the current stats window (ms). Finds
        # which pump stage spikes on a stalled frame: emulation, frame copy,
        # audio convert/feed, GDI present, or the pacing sleep itself.
        self._stage_max = {"emulate": 0.0, "copy": 0.0, "audio": 0.0,
                           "present": 0.0}
        self._deadline_lag_max = 0.0
        self._last_sleep_lag_ms = 0.0
        self._direct_present = False
        self._native_ready = threading.Event()
        self._present_size = (0, 0)
        self._tk = tk
        self._Image = Image
        self._ImageTk = ImageTk
        self._ttk = ttk
        self._handle = None
        self._stop = threading.Event()
        self._paused = False
        self._exit_code = None
        self._error = None
        self._frames = 0
        self._fps_at = time.monotonic()
        self._fps_base = 0
        self._fps = 0.0
        self._tick_job = None
        self._tk_thread = threading.get_ident()
        self._log_path = (workspace / "logs" / f"rust-{time.time_ns()}.log")
        self._log_path.parent.mkdir(parents=True, exist_ok=True)
        self._held = set()
        self._mask = 0
        self._pending_frame = None
        self._frame_lock = threading.Lock()
        self._audio = RustAudioOut()
        self._pump = None
        self._enhanced = not original
        self._embedded = embedded_parent is not None
        self._hd = None
        self._hd_buf = None
        self._hd_size = (0, 0)
        self._last_frame = None
        self._painted_size = None
        self._fullscreen = False
        self._display_scale = 1.0
        self._paint_ms = 0.0
        self._paint_decode_ms = 0.0
        self._paint_resize_ms = 0.0
        self._paint_photo_ms = 0.0
        self._paint_blit_ms = 0.0
        self._native_view = None
        self._present_ms = 0.0
        self._capture_armed = False

        rom = self._source.read_bytes()
        self._write_log(f"ROM: {self._source} ({len(rom)} bytes)")
        self._handle = core.create(rom)
        region = core.region(self._handle)
        self._dt = _FRAME_DT_PAL if region != 0 else _FRAME_DT_NTSC
        self._write_log(f"created; region={region} mapper={core.mapper_id(self._handle)}")
        battery_path = self._battery_path()
        if battery_path.is_file():
            try:
                core.set_battery(self._handle, battery_path.read_bytes())
                self._write_log(f"battery restored ({battery_path.stat().st_size} bytes)")
            except (OSError, RustCoreError, ValueError) as error:
                self._write_log(f"battery ignored: {error}")
        if self._enhanced:
            self._setup_enhanced()
            mode = "Enhanced HD" if self._hd is not None else "Enhanced (pack missing)"
            self._write_log(f"mode: {mode}")

        try:
            self._build_ui(host_parent, embedded_parent)
        except BaseException:
            # Tk or native-window setup failed before the normal close path
            # exists; release the core handles here instead of leaking them.
            hd, self._hd = self._hd, None
            if hd is not None:
                try:
                    core.hd_destroy(hd)
                except RustCoreError:
                    pass
            handle, self._handle = self._handle, None
            if handle is not None:
                try:
                    core.destroy(handle)
                except RustCoreError:
                    pass
            raise

    def _build_ui(self, host_parent, embedded_parent):
        """Build a standalone window, or a pane inside a comparison frame.

        Embedded players live in a `ttk.Frame` and leave title, focus and
        protocol handling to the owning comparison window.
        """
        tk, ttk = self._tk, self._ttk
        mode = "Enhanced HD" if self._hd is not None else "Original"
        self.status = tk.StringVar(value="Starting Rust NES player…")
        if self._embedded:
            self.view = tk.Frame(embedded_parent, bg="#0c121c")
            self.view.pack(fill="both", expand=True)
            self.window = self.view.winfo_toplevel()
            self.pause_button = self.close_button = self.fullscreen_button = None
            canvas_parent = self.view
        else:
            self.view = self.window = tk.Toplevel(host_parent)
            self.window.title(f"{self._game.get('name', 'NES')} · NES · {mode}")
            self.window.geometry("1024x820")
            self.window.minsize(400, 360)
            self.window.configure(bg="#0c121c")
            # No toolbar: the game fills the window like the Java ME player.
            # F11 toggles fullscreen and clicking returns keyboard focus.
            self.pause_button = self.close_button = self.fullscreen_button = None
            canvas_parent = self.window
            try:
                # Start large so a 4x NES frame fits; F11 is true fullscreen.
                self.window.state("zoomed")
            except tk.TclError:
                pass
        self._photo = self._ImageTk.PhotoImage(
            self._Image.new("RGB", (512, 480), "black"))
        self._canvas = tk.Canvas(canvas_parent, width=512, height=480, bg="black",
                                 highlightthickness=0)
        self._canvas.pack(fill="both", expand=True,
                          padx=(0 if self._embedded else 10),
                          pady=(0 if self._embedded else (0, 10)))
        self._canvas.create_image(0, 0, anchor="center", image=self._photo)
        self._canvas.bind("<Configure>", self._canvas_resized)
        self._canvas.bind("<Button-1>", self._click_focus)
        if os.name == "nt":
            try:
                from .win32_view import Win32FrameView
                # Parent to the canvas HWND, not the toplevel: Tk owns a real
                # window per widget and the canvas would otherwise cover the
                # native surface when Tk redraws it.
                self._native_view = Win32FrameView(self._canvas.winfo_id())
            except (OSError, ImportError, AttributeError):
                self._native_view = None
        if self._native_view is not None:
            self.window.after(0, self._place_native)
        if not self._embedded:
            self.window.bind("<KeyPress>", self._key_down)
            self.window.bind("<KeyRelease>", self._key_up)
            self.window.bind("<FocusOut>", self._focus_lost)
            self.window.bind("<F11>", lambda _event: self.toggle_fullscreen())
            self.window.bind("<Escape>", self._escape)
            self.window.protocol("WM_DELETE_WINDOW", self.close)

    def _canvas_resized(self, _event=None):
        self._painted_size = None
        self._place_native()

    def _place_native(self):
        self._native_ready.clear()
        if self._native_view is None:
            return
        try:
            self._native_view.place_widget(self._canvas)
            self._native_view.show()
            self._native_ready.set()
        except (OSError, AttributeError, self._tk.TclError):
            self._native_view = None

    def _click_focus(self, _event=None):
        # The native child is hit-test transparent, so clicks land here and
        # should return keyboard focus to the player (single or comparison).
        try:
            self.window.focus_force()
        except self._tk.TclError:
            pass

    def _escape(self, _event=None):
        if self._fullscreen:
            self.toggle_fullscreen()
            return "break"
        return None

    def toggle_fullscreen(self):
        self._fullscreen = not self._fullscreen
        try:
            self.window.attributes("-fullscreen", self._fullscreen)
        except self._tk.TclError:
            self._fullscreen = False
        if getattr(self, "fullscreen_button", None) is not None:
            try:
                self.fullscreen_button.configure(
                    text="Windowed (F11)" if self._fullscreen else "Fullscreen (F11)")
            except self._tk.TclError:
                pass

    # -- logging -------------------------------------------------------------
    def _write_log(self, message):
        try:
            with self._log_path.open("a", encoding="utf-8") as stream:
                stream.write(" ".join(str(message).split())[:1000] + "\n")
        except OSError:
            pass
        try:
            stats = self._log_path.with_suffix(".stats.json")
            import json as _json
            import os as _os
            # Atomic replace: readers (launcher telemetry, probes) must never
            # see a half-written file.
            temporary = stats.with_name(stats.name + ".part")
            temporary.write_text(_json.dumps({"updated": time.time(), "fps": self._fps,
                                          "backend": "rust-nes",
                                          "scale": round(self._display_scale, 2),
                                          "audio_written": self._audio.written,
                                          "audio_underruns": self._audio.underruns,
                                          "audio_dropped": self._audio.dropped,
                                          "audio_late_blocks": self._audio.late_blocks,
                                          "paint_ms": round(self._paint_ms, 2),
                                          "present_ms": round(self._present_ms, 2),
                                          "present_gap_max_ms": round(self._present_gap_max, 1),
                                          "present_stalls": self._present_stalls,
                                          "pump_gap_max_ms": round(self._pump_gap_max, 1),
                                          "pump_stalls": self._pump_stalls,
                                          "stage_max_ms": {key: round(value, 2)
                                                           for key, value in self._stage_max.items()},
                                          "deadline_lag_max_ms": round(self._deadline_lag_max, 1),
                                          "direct_present": self._direct_present,
                                          "native_view": self._native_view is not None,
                                          "fast_scaler_submit_ms": 0.0,
                                          "input_queue_p95_ms": 0.0,
                                          "gpu_scale_fallbacks": 0,
                                          "gpu_scale_passes": 0}), encoding="utf-8")
            _os.replace(temporary, stats)
        except OSError:
            pass

    def _reset_gap_stats(self):
        self._present_gap_max = 0.0
        self._present_stalls = 0
        self._pump_gap_max = 0.0
        self._pump_stalls = 0
        for key in self._stage_max:
            self._stage_max[key] = 0.0
        self._deadline_lag_max = 0.0

    @property
    def log_path(self):
        return self._log_path

    def _battery_path(self):
        return (self._workspace / "saves" / "mesen" / "battery" /
                (self._source.stem + ".sav"))

    def _write_battery_file(self, data: bytes):
        """Write a battery snapshot from a background thread (no pacing)."""
        path = self._battery_path()
        try:
            path.parent.mkdir(parents=True, exist_ok=True)
            if path.is_file() and path.read_bytes() == data:
                return
            path.write_bytes(data)
            self._write_log(f"battery saved ({len(data)} bytes)")
        except OSError:
            pass

    def _setup_enhanced(self):
        """Create the compositor table (pack optional) and arm tile capture.

        A missing or unreadable pack still gets an empty 4x table so Enhanced
        play can record tiles for the next preparation; output then falls back
        to scaled original frames.
        """
        from .rust_hd import load_pack_tiles

        scale = int(self._game.get("render_scale", DEFAULT_RENDER_SCALE) or DEFAULT_RENDER_SCALE)
        if scale not in (2, 4):
            scale = DEFAULT_RENDER_SCALE
        entries = []
        try:
            loaded = load_pack_tiles(self._workspace)
        except Exception as error:
            self._write_log(f"HD pack unreadable, using scaled native: {error}")
            loaded = None
        if loaded is None:
            self._write_log("no HD pack staged; Enhanced renders scaled native "
                            "and records tiles")
        else:
            entries, scale = loaded
        try:
            hd = self._core.hd_create(scale)
            added = 0
            for entry in entries:
                try:
                    self._core.hd_add_tile(
                        hd, sprite=entry.sprite, chr_index=entry.chr_index,
                        palette=entry.palette,
                        rgba=entry.image.tobytes(), size=entry.image.width)
                    added += 1
                except RustCoreError:
                    continue
            width, height = self._core.hd_output_size(hd)
            self._hd = hd
            self._hd_size = (width, height)
            self._hd_buf = bytearray(width * height * 4)
            self._core.hd_arm(self._handle, True)
            if self._core.capture_supported:
                try:
                    self._core.capture_arm(self._handle, True)
                    self._capture_armed = True
                except RustCoreError:
                    self._capture_armed = False
            self._write_log(f"HD ready: {added}/{len(entries)} tiles, "
                            f"{width}x{height} output; capture "
                            f"{'armed' if self._capture_armed else 'unavailable'}")
        except (RustCoreError, ValueError, MemoryError) as error:
            self._write_log(f"HD setup failed, using scaled native: {error}")
            self._hd = None
            self._hd_buf = None
            self._capture_armed = False

    def _flush_capture(self):
        """Write observed tiles as a capture chunk for the artwork worker."""
        if not self._capture_armed or self._handle is None:
            return
        self._capture_armed = False
        try:
            keys = self._core.capture_export(self._handle)
            self._core.capture_arm(self._handle, False)
        except RustCoreError:
            return
        if not keys:
            self._write_log("capture: no new tiles observed")
            return
        from struct import Struct
        from uuid import uuid4

        from .tiles import Tile

        identity = str(self._game.get("id", ""))
        if len(identity) != 64:
            return
        try:
            palette = self._core.standard_palette()
        except RustCoreError:
            return
        header = Struct("<4s64s32sIHH192s")
        tile_struct = Struct("<QiI16sBBBB")
        folder = self._workspace / ".runtime" / "nes-capture"
        folder.mkdir(parents=True, exist_ok=True)
        session = uuid4().hex
        written = 0
        batch = []
        sequence = 0
        for key in keys[:8192]:
            sprite = bool(key >> 63)
            index = (key >> 32) & 0x7FFFFFFF
            colors = key & 0xFFFFFFFF
            try:
                pattern = self._core.tile_pattern(self._handle, index)
                tile = Tile(written, index, colors, pattern, False, sprite, False)
            except (RustCoreError, ValueError):
                continue
            batch.append(tile)
            written += 1
            if len(batch) == 128:
                self._write_capture_chunk(folder, session, sequence, identity,
                                          palette, batch)
                sequence += 1
                batch = []
                if sequence >= 64:
                    break
        if batch and sequence < 64:
            self._write_capture_chunk(folder, session, sequence, identity,
                                      palette, batch)
            sequence += 1
        self._write_log(f"capture: {written} tiles in {sequence} chunk(s)")

    def _write_capture_chunk(self, folder, session, sequence, identity,
                             palette, tiles):
        from struct import pack

        payload = [pack("<4s64s32sIHH192s", b"RNT1", identity.encode("ascii"),
                        session.encode("ascii"), sequence, len(tiles), 0, palette)]
        for tile in tiles:
            payload.append(pack("<QiI16sBBBB", tile.frame, tile.index,
                                tile.palette, tile.pattern, int(tile.ram),
                                int(tile.sprite), int(tile.transparent), 0))
        path = folder / f"{session}-{sequence:08x}.rnt"
        temporary = path.with_name(path.name + ".part")
        with temporary.open("wb") as stream:
            stream.write(b"".join(payload))
        temporary.replace(path)

    # -- lifecycle -----------------------------------------------------------
    def start(self):
        if not self._timer_active:
            _timer_begin()
            self._timer_active = True
        # Start the frame pump first so audio samples queue while the device
        # opens; the feeder then begins with real data instead of silence.
        self._pump = threading.Thread(target=self._pump_loop, name="rust-nes-pump",
                                      daemon=True)
        self._pump.start()
        # Give the pump a couple of frames so the device opens with real audio
        # queued; otherwise a cold first launch can start with silence.
        deadline = time.monotonic() + 0.2
        while time.monotonic() < deadline and self._audio._queue.qsize() < 2:
            time.sleep(0.005)
        self._audio.start(log=self._write_log)
        self.window.after(50, self._tick)
        if not self._embedded:
            self.window.focus_set()
        self.status.set("Game running in JAR Revival (Rust core)" +
                        ("" if not self._audio.muted else " · muted"))

    def poll(self):
        return self._exit_code

    def _finish(self, code, reason=""):
        if self._exit_code is not None:
            return
        self._exit_code = code
        if reason:
            self._write_log(reason)

    def _pump_loop(self):
        try:
            deadline = time.monotonic()
            frames = 0
            while not self._stop.is_set():
                if self._paused:
                    time.sleep(0.05)
                    deadline = time.monotonic() + self._dt
                    self._pump_gap_last = time.monotonic()
                    continue
                loop_top = time.monotonic()
                gap = (loop_top - self._pump_gap_last) * 1000.0
                if gap > self._pump_gap_max:
                    self._pump_gap_max = gap
                if gap > 30.0:
                    self._pump_stalls += 1
                self._pump_gap_last = loop_top
                body_start = loop_top
                perf = time.perf_counter
                stage = self._stage_max
                mark = perf()
                if self._hd is not None:
                    self._core.hd_run_frame(self._handle, self._hd, self._hd_buf)
                    emulate_ms = (perf() - mark) * 1000.0
                    mark = perf()
                    pending = (bytes(self._hd_buf),) + self._hd_size
                    copy_ms = (perf() - mark) * 1000.0
                else:
                    self._core.run_frame(self._handle)
                    emulate_ms = (perf() - mark) * 1000.0
                    mark = perf()
                    raw, width, height = self._core.framebuffer(self._handle)
                    pending = (bytes(raw), width, height)
                    copy_ms = (perf() - mark) * 1000.0
                if emulate_ms > stage["emulate"]:
                    stage["emulate"] = emulate_ms
                if copy_ms > stage["copy"]:
                    stage["copy"] = copy_ms
                frames += 1
                self._frames = frames
                mark = perf()
                raw_audio = self._core.drain_audio(self._handle)
                if raw_audio:
                    self._audio.feed(_float_to_int16(raw_audio))
                audio_ms = (perf() - mark) * 1000.0
                if audio_ms > stage["audio"]:
                    stage["audio"] = audio_ms
                view = self._native_view
                if view is not None and self._native_ready.is_set():
                    # Present from the emulation thread so Tk's event loop
                    # cannot hold a ready frame back.
                    try:
                        started = time.perf_counter()
                        view.present(pending[0], pending[1], pending[2])
                        present_ms = (time.perf_counter() - started) * 1000.0
                        self._present_ms = present_ms
                        if present_ms > stage["present"]:
                            stage["present"] = present_ms
                        self._direct_present = True
                        self._present_size = (pending[1], pending[2])
                        now = time.monotonic()
                        gap = (now - self._present_gap_last) * 1000.0
                        if gap > self._present_gap_max:
                            self._present_gap_max = gap
                        if gap > 30.0:
                            self._present_stalls += 1
                            # No file write here: logging from the pacing
                            # thread on every stall stalls the next frame too.
                            # Counters plus per-second stage maxima in the
                            # stats carry the same signal without disk IO.
                        self._present_gap_last = now
                        pending = None
                    except (OSError, AttributeError):
                        self._direct_present = False
                else:
                    self._direct_present = False
                if pending is not None:
                    with self._frame_lock:
                        self._pending_frame = pending
                if frames % 1800 == 0:
                    # File IO off the pacing thread: a save write stalling
                    # here would show up as a frozen frame every ~30 s.
                    data = None
                    try:
                        data = self._core.get_battery(self._handle)
                    except RustCoreError:
                        data = None
                    if data:
                        snapshot = bytes(data)
                        threading.Thread(
                            target=self._write_battery_file,
                            args=(snapshot,), name="rust-nes-battery",
                            daemon=True).start()
                deadline += self._dt
                wait = deadline - time.monotonic()
                self._last_sleep_lag_ms = 0.0
                if wait > 0.002:
                    # Sleep the bulk, then spin the last ~2 ms: plain
                    # time.sleep overshoots by ~1 ms per frame, and that
                    # drift accumulates into a catch-up burst every second
                    # or two. The spin costs a little CPU on this thread.
                    time.sleep(max(0.0, wait - 0.002))
                    spin_until = deadline
                    while time.monotonic() < spin_until:
                        pass
                    # How late the wakeup was past the deadline (negative
                    # means early).
                    lag = (time.monotonic() - deadline) * 1000.0
                    self._last_sleep_lag_ms = lag
                    if lag > self._deadline_lag_max:
                        self._deadline_lag_max = lag
                elif wait < -self._dt * 0.75:
                    # More than ~3/4 frame late (e.g. a blocking GDI BitBlt
                    # under DWM): resync instead of bursting. Bursting shows
                    # the next frame almost immediately, which reads as a
                    # freeze-then-jump; one long frame is far less visible.
                    # The audio feeder's ~116 ms cushion covers the skipped
                    # catch-up frame.
                    lag = -wait * 1000.0
                    if lag > self._deadline_lag_max:
                        self._deadline_lag_max = lag
                    deadline = time.monotonic() + self._dt
                elif time.monotonic() - deadline > 0.1:
                    # Resync only after a real stall; otherwise keep catching
                    # up so audio production does not permanently run slow.
                    deadline = time.monotonic()
        except RustCoreError as error:
            self._error = str(error)
            self._finish(71, f"Rust player failed: {error}")
        except Exception as error:  # never let the pump vanish silently
            self._error = str(error)
            self._finish(71, f"Rust player failed: {error}")

    def _tick(self):
        self._tick_job = None
        if self._exit_code is not None:
            self._destroy()
            return
        if self._error is not None:
            self.status.set(f"Could not continue: {self._error}")
            self._destroy()
            return
        with self._frame_lock:
            pending = self._pending_frame
            self._pending_frame = None
        if self._direct_present and self._native_view is not None:
            pending = None
            width, height = self._present_size
            canvas_w = self._canvas.winfo_width()
            canvas_h = self._canvas.winfo_height()
            if width and height and canvas_w > 8 and canvas_h > 8:
                self._display_scale = min(canvas_w / width, canvas_h / height)
                self._painted_size = (canvas_w, canvas_h)
        if pending is not None:
            self._last_frame = pending
        did_present = False
        if not self._direct_present and self._last_frame is not None and (
                pending is not None or self._painted_size is None):
            data, width, height = self._last_frame
            if self._native_view is not None:
                try:
                    started = time.perf_counter()
                    self._native_view.present(bytes(data), width, height)
                    self._present_ms = (time.perf_counter() - started) * 1000.0
                    canvas_w = self._canvas.winfo_width()
                    canvas_h = self._canvas.winfo_height()
                    if canvas_w > 8 and canvas_h > 8:
                        self._display_scale = min(canvas_w / width, canvas_h / height)
                        self._painted_size = (canvas_w, canvas_h)
                    else:
                        self._painted_size = (width, height)
                    did_present = True
                except OSError:
                    self._native_view = None
            if self._native_view is None:
                try:
                    self._paint(self._last_frame)
                    did_present = True
                except (ValueError, self._tk.TclError):
                    pass
        now = time.monotonic()
        if did_present:
            gap = (now - self._present_gap_last) * 1000.0
            if gap > self._present_gap_max:
                self._present_gap_max = gap
            if gap > 30.0:
                self._present_stalls += 1
                self._write_log(f"present stall {gap:.1f} ms")
            self._present_gap_last = now
        if now - self._fps_at >= 1.0:
            self._fps = (self._frames - self._fps_base) / max(0.001, now - self._fps_at)
            self._fps_base = self._frames
            self._fps_at = now
            self._write_log("heartbeat")
            self._reset_gap_stats()
        self._tick_job = self.window.after(10, self._tick)

    def _paint(self, frame):
        """Fit the frame to the canvas at an integer scale, keeping 4:3.

        The core exposes XRGB u32 values (0x00RRGGBB). In little-endian memory
        that is B,G,R,X, so Pillow's raw `BGRX` decoder restores the colors
        without touching the emulator.
        """
        started = time.perf_counter()
        data, width, height = frame
        canvas_w = self._canvas.winfo_width()
        canvas_h = self._canvas.winfo_height()
        if canvas_w < 8 or canvas_h < 8:
            canvas_w, canvas_h = width, height
        if getattr(self, "_fit_exact", False):
            # Comparison panes must show both games at the same physical size;
            # integer-only scaling made Original smaller than Enhanced.
            ratio = min(canvas_w / width, canvas_h / height)
            target = (max(1, int(width * ratio)), max(1, int(height * ratio)))
            self._display_scale = ratio
        else:
            # Prefer whole-number scaling (crisp 4x); fall back to fitting small windows.
            factor = min(canvas_w // width, canvas_h // height)
            if factor >= 1:
                target = (width * factor, height * factor)
                self._display_scale = float(factor)
            else:
                ratio = min(canvas_w / width, canvas_h / height)
                target = (max(1, int(width * ratio)), max(1, int(height * ratio)))
                self._display_scale = ratio
        image = self._Image.frombytes("RGB", (width, height), bytes(data),
                                      "raw", "BGRX")
        self._paint_decode_ms = (time.perf_counter() - started) * 1000.0
        resized_at = time.perf_counter()
        if target != (width, height):
            # High-detail art is mostly smooth; NEAREST downscaling avoids the
            # 10-20 ms/frame Pillow filters that starved the window, while
            # upscales stay crisp.
            resample = self._Image.NEAREST
            image = image.resize(target, resample)
        self._paint_resize_ms = (time.perf_counter() - resized_at) * 1000.0
        photo_at = time.perf_counter()
        self._photo = self._ImageTk.PhotoImage(image)
        self._paint_photo_ms = (time.perf_counter() - photo_at) * 1000.0
        blit_at = time.perf_counter()
        self._canvas.itemconfig(1, image=self._photo, anchor="center")
        self._canvas.coords(1, canvas_w // 2, canvas_h // 2)
        self._paint_blit_ms = (time.perf_counter() - blit_at) * 1000.0
        self._painted_size = (canvas_w, canvas_h)
        self._paint_ms = (time.perf_counter() - started) * 1000.0

    # -- input ---------------------------------------------------------------
    def _apply_keys(self):
        mask = 0
        for action in self._held:
            mask |= {"UP": 0x10, "DOWN": 0x20, "LEFT": 0x40, "RIGHT": 0x80,
                     "A": 0x01, "B": 0x02, "START": 0x08, "SELECT": 0x04}.get(action, 0)
        if mask != self._mask:
            self._mask = mask
            try:
                self._core.set_controller(self._handle, 0, mask)
            except RustCoreError:
                pass

    def _key_down(self, event):
        action = self._key_to_action.get(event.keysym.lower())
        if action is None or action in self._held:
            return None
        self._held.add(action)
        self._apply_keys()
        return "break"

    def _key_up(self, event):
        action = self._key_to_action.get(event.keysym.lower())
        if action is None or action not in self._held:
            return None
        self._held.discard(action)
        self._apply_keys()
        return "break"

    def _focus_lost(self, _event):
        self.release_all()

    def press(self, action):
        """Press one NES action (shared comparison input)."""
        if action is None or action in self._held:
            return
        self._held.add(action)
        self._apply_keys()

    def release(self, action):
        if action is None or action not in self._held:
            return
        self._held.discard(action)
        self._apply_keys()

    def release_all(self):
        if self._held:
            self._held.clear()
            self._apply_keys()

    def toggle_pause(self):
        self._paused = not self._paused
        if self.pause_button is not None:
            try:
                self.pause_button.configure(text="Resume" if self._paused else "Pause")
            except self._tk.TclError:
                pass
        self.status.set("Game paused" if self._paused else
                        "Game running in JAR Revival (Rust core)")

    # -- close ---------------------------------------------------------------
    def terminate(self):
        self.close()

    def close(self):
        if self._exit_code is not None:
            return
        self._stop.set()
        pump, self._pump = self._pump, None
        if pump is not None and pump is not threading.current_thread():
            pump.join(timeout=2.0)
        self._flush_capture()
        # Pump is joined above, so a synchronous save here cannot hitch play.
        try:
            data = self._core.get_battery(self._handle) if self._handle else None
        except RustCoreError:
            data = None
        if data:
            self._write_battery_file(bytes(data))
        self._audio.stop()
        self._finish(0)
        if self._tick_job is not None:
            try:
                self.window.after_cancel(self._tick_job)
            except self._tk.TclError:
                pass
            self._tick_job = None
        if threading.get_ident() == self._tk_thread:
            self._destroy()
        else:
            try:
                self.window.after(0, self._destroy)
            except self._tk.TclError:
                pass

    def shutdown(self, timeout=8.0):
        self.close()
        started = time.monotonic()
        while self._handle is not None and time.monotonic() - started < timeout:
            time.sleep(0.05)

    def _destroy(self):
        if self._timer_active:
            self._timer_active = False
            _timer_end()
        if self._tick_job is not None:
            try:
                self.window.after_cancel(self._tick_job)
            except self._tk.TclError:
                pass
            self._tick_job = None
        if self._native_view is not None:
            self._native_ready.clear()
            view, self._native_view = self._native_view, None
            try:
                view.close()
            except OSError:
                pass
        # Embedded players own only their pane; the comparison window closes
        # itself once both panes are gone.
        try:
            (self.view if self._embedded else self.window).destroy()
        except self._tk.TclError:
            pass
        finally:
            if self._handle is not None:
                handle, self._handle = self._handle, None
                if self._hd is not None:
                    hd, self._hd = self._hd, None
                    try:
                        self._core.hd_destroy(hd)
                    except RustCoreError:
                        pass
                try:
                    self._core.destroy(handle)
                except RustCoreError:
                    pass
