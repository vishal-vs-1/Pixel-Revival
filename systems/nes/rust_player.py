"""Run NES games in-tool on the vendored Rust core, no external player involved.

Original and Enhanced both render inside our Tk window. Enhanced composites
the workspace's staged high-detail (HD) pack; with no pack it falls back to
scaled native output instead of failing. Standalone playback records unseen tiles
for the next pack build when the loaded library exposes capture.
"""

from __future__ import annotations

import ctypes
import os
import threading
import time
from pathlib import Path

from .metadata import DEFAULT_RENDER_SCALE
from .rust_core import RustCore, RustCoreError
from .rust_audio import RustAudioOut
from .frame_presenter import FramePresenter
from .thread_scheduling import playback_thread_priority

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

        with self._lock:
            if self._active is not None and self._active.poll() is None:
                raise RuntimeError("Close the current NES game before comparing another game")
            pair, log = launch_comparison(self, game, source, workspace,
                                         host_parent=host_parent, controls=controls,
                                         **kwargs)
            self._active = pair
            return pair, log

    def launch(self, game, source, workspace, *, original=False, host_parent=None,
               controls=None, capture=True, record_poses=False, pose_overlay=None,
               **kwargs):
        if host_parent is None:
            raise RuntimeError("The Rust NES player needs the application window")
        workspace = Path(workspace)
        player = RustNesPlayer(self._core, game, Path(source), workspace,
                               original=original, controls=controls,
                               host_parent=host_parent, record_poses=record_poses,
                               pose_overlay=pose_overlay, capture=capture)
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
                 controls=None, host_parent=None, embedded_parent=None,
                 record_poses=False, pose_overlay=None, audio_enabled=True,
                 capture=False, capture_workspace=None, pose_workspace=None):
        import tkinter as tk
        from tkinter import ttk
        from PIL import Image, ImageTk

        self._core = core
        self._game = dict(game)
        self._source = Path(source)
        self._workspace = Path(workspace)
        self._capture_workspace = Path(capture_workspace) if capture_workspace is not None else self._workspace
        self._controls = dict(controls) if controls else {}
        self._key_to_action = _controls_to_keys(self._controls) or dict(_TK_TO_ACTION)
        self._record_poses = bool(record_poses)
        self._pose_overlay = pose_overlay
        self._pose_started = False
        self._pose_samples = 0
        self._pose_direction = 0
        self._pose_actions = 0
        self._chr_ram = str((game.get("nes") or {}).get("chr_kind", "")).lower() == "ram"
        # Comparison saves stay private; its Enhanced recorder feeds the library.
        recording_folder = Path(pose_workspace) if pose_workspace is not None else self._workspace
        self._pose_journal = recording_folder / "playable-assets" / "live-recording.tsv"
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
                           "present": 0.0, "observe": 0.0, "overlay": 0.0}
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
        self._present_fps = 0.0
        self._present_base = 0
        self._tick_job = None
        self._tk_thread = threading.get_ident()
        self._log_path = (workspace / "logs" / f"rust-{time.time_ns()}.log")
        self._log_path.parent.mkdir(parents=True, exist_ok=True)
        self._held = set()
        self._mask = 0
        self._applied_mask = None
        self._comparison_clock = None
        self._pending_frame = None
        self._frame_lock = threading.Lock()
        self._battery_lock = threading.Lock()
        self._battery_generation = 0
        self._audio = RustAudioOut(enabled=audio_enabled)
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
        self._presenter = None
        self._presented_frames = 0
        self._skipped_presents = 0
        self._capture_armed = False
        self._capture_hd = None
        self._capture_hd_buf = None

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
        elif capture:
            self._setup_original_capture()

        try:
            self._build_ui(host_parent, embedded_parent)
        except BaseException:
            # Tk or native-window setup failed before the normal close path
            # exists; release the core handles here instead of leaking them.
            self._close_capture_table()
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
            from storage.atomic import atomic_json
            # Atomic replace: readers (launcher telemetry, probes) must never
            # see a half-written file.
            atomic_json(stats, {"updated": time.time(), "fps": self._fps,
                                          "present_fps": self._present_fps,
                                          "presented_frames": self._presenter.presented if self._presenter else self._presented_frames,
                                          "skipped_presents": self._presenter.skipped if self._presenter else self._skipped_presents,
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
                                          "gpu_scale_passes": 0})
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

    def _write_battery_file(self, data: bytes, generation=None):
        """Write a battery snapshot from a background thread (no pacing)."""
        path = self._battery_path()
        try:
            with self._battery_lock:
                if generation is not None and generation != self._battery_generation:
                    return
                path.parent.mkdir(parents=True, exist_ok=True)
                if path.is_file() and path.read_bytes() == data:
                    return
                from uuid import uuid4
                temporary = path.with_name(path.name + '.' + uuid4().hex + '.tmp')
                try:
                    temporary.write_bytes(data)
                    os.replace(temporary, path)
                finally:
                    temporary.unlink(missing_ok=True)
            self._write_log(f"battery saved ({len(data)} bytes)")
        except OSError:
            pass

    def _setup_original_capture(self):
        if not self._core.capture_supported:
            return
        table = None
        try:
            # The existing capture export is populated by the HD frame API.
            # An empty table observes tiles while the native framebuffer is displayed.
            table = self._core.hd_create(2)
            width, height = self._core.hd_output_size(table)
            buffer = bytearray(width * height * 4)
            self._core.hd_arm(self._handle, True)
            self._core.capture_arm(self._handle, True)
            self._capture_hd = table
            self._capture_hd_buf = buffer
            self._capture_armed = True
            self._write_log('Original tile capture armed')
        except (RustCoreError, ValueError, MemoryError) as error:
            if table is not None:
                try:
                    self._core.hd_destroy(table)
                except RustCoreError:
                    pass
            try:
                self._core.hd_arm(self._handle, False)
            except RustCoreError:
                pass
            self._write_log(f'Original tile capture unavailable: {error}')

    def _close_capture_table(self):
        table = getattr(self, '_capture_hd', None)
        self._capture_hd = None
        self._capture_hd_buf = None
        if table is not None:
            try:
                self._core.hd_destroy(table)
            except RustCoreError:
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
        hd = None
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
            if hd is not None:
                try:
                    self._core.hd_destroy(hd)
                except RustCoreError:
                    pass
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
        folder = getattr(self, '_capture_workspace', self._workspace) / ".runtime" / "nes-capture"
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

    def _record_observation(self, frame_number, observation):
        """Append a bounded native sample and return it for the image overlay."""
        if not self._record_poses or self._pose_samples >= 2400:
            return
        buttons = self._frame_mask & 0xFF
        if not self._pose_started:
            if not (buttons & 0xF3):
                return
            self._pose_started = True
        direction = buttons & 0xF0
        if direction:
            self._pose_direction = direction
        self._pose_actions |= buttons & 0x03
        if frame_number % 3:
            return
        mask = (buttons & 0x0C) | self._pose_actions | (
            direction or self._pose_direction)
        self._pose_direction = 0
        self._pose_actions = 0
        from .pose_observation import capture_frame
        frame = capture_frame(self._core, self._handle, frame_number, mask,
                              observation, ram=self._chr_ram)
        if not frame.sprites:
            return frame
        sprites = [f'{s.x},{s.y},{s.flags},{s.indices[0]},{s.indices[1]},{s.pattern}'
                   + (f',{s.colors}' if s.colors is not None else '')
                   for s in frame.sprites]
        if frame.camera is not None:
            sprites.insert(0, f'@camera={frame.camera[0]},{frame.camera[1]}')
        line = "\t".join((str(frame.number), str(frame.buttons), str(frame.height),
                          frame.palette, *sprites)) + "\n"
        try:
            with self._pose_journal.open("a", encoding="ascii") as stream:
                stream.write(line)
        except OSError:
            self._record_poses = False
            return frame
        self._pose_samples += 1
        return frame

    # -- lifecycle -----------------------------------------------------------
    def start(self):
        if ((self._record_poses or self._pose_overlay is not None)
                and getattr(self._core, 'pose_observation_supported', False) is True):
            try:
                self._core.pose_arm(self._handle)
            except RustCoreError as error:
                self._write_log(f'latched pose observation unavailable: {error}')
        if self._record_poses:
            try:
                from .live_player_capture import RECORDING_VERSION
                self._pose_journal.parent.mkdir(parents=True, exist_ok=True)
                self._pose_journal.write_bytes(RECORDING_VERSION)
                if not self._core.observation_supported:
                    self._write_log("pose recording unavailable: rebuild the Rust player")
                    self._record_poses = False
            except OSError as error:
                self._write_log(f"pose recording disabled: {error}")
                self._record_poses = False
        if self._pose_overlay is not None:
            if not self._core.observation_supported:
                self._write_log("pose replacement unavailable: rebuild the Rust player")
                self._pose_overlay = None
            else:
                try:
                    self._pose_overlay.bind(self._core, self._handle, self._chr_ram)
                except (AttributeError, OSError):
                    self._pose_overlay = None
        if not self._timer_active:
            _timer_begin()
            self._timer_active = True
        if self._native_view is not None:
            self._presenter = FramePresenter(self._present_native_frame, frame_interval=self._dt)
        # Start the frame pump first so audio samples queue while the device
        # opens; the feeder then begins with real data instead of silence.
        self._pump = threading.Thread(target=self._pump_loop, name="rust-nes-pump",
                                      daemon=True)
        self._pump.start()
        # Give the pump a couple of frames so the device opens with real audio
        # queued; otherwise a cold first launch can start with silence.
        deadline = time.monotonic() + 0.2
        while self._audio.enabled and time.monotonic() < deadline and self._audio._queue.qsize() < 2:
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

    @playback_thread_priority()
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
                # Only the pump may mutate the emulator after startup. ctypes
                # releases the GIL, so a Tk controller call here would race Rust.
                self._frame_mask = (self._comparison_clock.next_input()
                                    if self._comparison_clock is not None else self._mask)
                if self._frame_mask != self._applied_mask:
                    self._core.set_controller(self._handle, 0, self._frame_mask)
                    self._applied_mask = self._frame_mask
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
                    if self._capture_hd is not None:
                        self._core.hd_run_frame(self._handle, self._capture_hd, self._capture_hd_buf)
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
                observation = None
                pose_frame = None
                mark = perf()
                if self._record_poses or self._pose_overlay is not None:
                    try:
                        observation = self._core.observation(self._handle)
                    except RustCoreError:
                        observation = None
                if observation is not None and self._record_poses:
                    try:
                        pose_frame = self._record_observation(frames, observation)
                    except (OSError, ValueError) as error:
                        self._write_log(f"pose recording disabled: {error}")
                        self._record_poses = False
                stage['observe'] = max(stage['observe'], (perf() - mark) * 1000.0)
                mark = perf()
                if observation is not None and self._pose_overlay is not None:
                    try:
                        pending = (self._pose_overlay.apply(
                            pending[0], pending[1], pending[2], observation,
                            frame=pose_frame, frame_number=frames, buttons=self._frame_mask),
                            pending[1], pending[2])
                    except (OSError, ValueError) as error:
                        self._write_log(f"pose replacement disabled: {error}")
                        self._pose_overlay = None
                stage['overlay'] = max(stage['overlay'], (perf() - mark) * 1000.0)
                view = self._native_view
                # Tk can retire a failed presenter between these checks and
                # submission; keep this frame's worker reference stable.
                presenter = self._presenter
                if (view is not None and self._native_ready.is_set()
                        and presenter is not None):
                    if presenter.submit(pending):
                        pending = None
                    else:
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
                        self._battery_generation += 1
                        threading.Thread(
                            target=self._write_battery_file,
                            args=(snapshot, self._battery_generation), name="rust-nes-battery",
                            daemon=True).start()
                deadline += self._dt
                wait = deadline - time.monotonic()
                self._last_sleep_lag_ms = 0.0
                if wait > 0:
                    # Release the GIL for the whole wait so comparison and the
                    # audio feeder can run. Keep the absolute sample deadline.
                    self._stop.wait(wait)
                    lag = (time.monotonic() - deadline) * 1000.0
                    self._last_sleep_lag_ms = lag
                    if lag > self._deadline_lag_max:
                        self._deadline_lag_max = lag
                elif wait < -0.1:
                    # Recover a short scheduling delay without losing audio
                    # samples; discard debt only after a genuine long stall.
                    lag = -wait * 1000.0
                    if lag > self._deadline_lag_max:
                        self._deadline_lag_max = lag
                    deadline = time.monotonic()
        except threading.BrokenBarrierError:
            expected = (self._stop.is_set() or
                        (self._comparison_clock is not None and self._comparison_clock.closed))
            self._finish(0 if expected else 71,
                         "Comparison frame synchronization stopped")
        except RustCoreError as error:
            self._error = str(error)
            self._finish(71, f"Rust player failed: {error}")
        except Exception as error:  # never let the pump vanish silently
            self._error = str(error)
            self._finish(71, f"Rust player failed: {error}")

    def _present_native_frame(self, data, width, height):
        started = time.perf_counter()
        self._native_view.present(data, width, height)
        self._present_ms = (time.perf_counter() - started) * 1000.0
        self._stage_max['present'] = max(self._stage_max['present'], self._present_ms)
        self._direct_present = True
        self._present_size = (width, height)
        now = time.monotonic()
        gap = (now - self._present_gap_last) * 1000.0
        self._present_gap_max = max(self._present_gap_max, gap)
        if gap > 30.0:
            self._present_stalls += 1
        self._present_gap_last = now

    def _tick(self):
        self._tick_job = None
        if self._exit_code is not None:
            self.close()
            return
        if self._error is not None:
            self.status.set(f"Could not continue: {self._error}")
            self.close()
            return
        if self._presenter is not None and self._presenter.error is not None:
            self._write_log(f'Native presentation failed: {self._presenter.error}; using Tk')
            if not self._close_presenter():
                self._tick_job = self.window.after(10, self._tick)
                return
            self._direct_present = False
            self._native_ready.clear()
            if self._native_view is not None:
                self._native_view.close()
                self._native_view = None
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
            elapsed = max(0.001, now - self._fps_at)
            self._fps = (self._frames - self._fps_base) / elapsed
            displayed = self._presenter.presented if self._presenter else self._presented_frames
            self._present_fps = (displayed - self._present_base) / elapsed
            self._present_base = displayed
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
        self._audio.set_paused(self._paused)
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

    def _close_presenter(self):
        presenter = self._presenter
        if presenter is None:
            return True
        if not presenter.close():
            return False
        # Final telemetry must retain counts after the worker is detached.
        self._presented_frames = presenter.presented
        self._skipped_presents = presenter.skipped
        self._presenter = None
        return True

    def close(self):
        if getattr(self, "_closed", False):
            return
        self._stop.set()
        if self._comparison_clock is not None:
            self._comparison_clock.close()
        pump = self._pump
        if pump is not None and pump is not threading.current_thread():
            pump.join(timeout=2.0)
            if pump.is_alive():
                # A slow native call still owns the pointers. Retry on Tk
                # instead of freeing memory beneath a running emulator.
                self.window.after(20, self.close)
                return
        self._pump = None
        if not self._close_presenter():
            # Screen resources remain owned until the last native blit returns.
            self.window.after(20, self.close)
            return
        self._closed = True
        self._flush_capture()
        # Pump is joined above, so a synchronous save here cannot hitch play.
        try:
            data = self._core.get_battery(self._handle) if self._handle else None
        except RustCoreError:
            data = None
        if data:
            self._battery_generation += 1
            self._write_battery_file(bytes(data), self._battery_generation)
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
        self._close_capture_table()
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
