"""Bounded Windows PCM output for the in-tool NES player."""

import ctypes
from ctypes import wintypes as w
import queue
import threading
import time

_SAMPLE_RATE = 44100


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

    def __init__(self, sample_rate=_SAMPLE_RATE, buffer_samples=512, buffers=12, *, enabled=True):
        self.enabled = bool(enabled)
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
        self._paused = False
        self.fed_bytes = 0
        self.written = 0
        self.underruns = 0
        self.dropped = 0
        self.late_blocks = 0

    def start(self, log=None):
        if not self.enabled:
            return
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
            self.stop()
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
        for name in ("waveOutPause", "waveOutRestart"):
            getattr(winmm, name).argtypes = [w.HANDLE]
            getattr(winmm, name).restype = w.UINT
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
            self._events[index].clear()
            if winmm.waveOutWrite(handle, ctypes.byref(header),
                                  ctypes.sizeof(header)) == 0:
                self.written += 1
            else:
                self._events[index].set()

    def feed(self, pcm16: bytes):
        if not self.enabled or self._failed or not pcm16:
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

    def set_paused(self, paused):
        self._paused = bool(paused)
        if self._winmm is not None and self._handle:
            operation = self._winmm.waveOutPause if paused else self._winmm.waveOutRestart
            operation(self._handle)

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
            if not event.wait(timeout=0.05):
                # A timeout does not return ownership of a device buffer.
                # Rewriting an in-flight WAVEHDR can corrupt audio playback.
                continue
            if self._stop.is_set():
                return
            if self._paused:
                self._stop.wait(0.01)
                continue
            event.clear()
            # Pull chunks until this buffer is full. One frame arrives every
            # ~16.7 ms, so wait a little longer than a frame; the prefilled
            # cushion (10 blocks ≈ 116 ms) covers this without starving.
            while len(self._pending) < need and not self._stop.is_set():
                try:
                    # Waiting up to 50 ms is safe: ten prefilled blocks give a
                    # ~116 ms cushion, so the device never runs dry meanwhile.
                    self._pending.extend(self._queue.get(timeout=0.016))
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
                self._failed = True
                self.muted = True
                self.playing = False
                event.set()
                return
            last_write = time.monotonic()
            self.written += 1
            index = (index + 1) % self.buffers

    def stop(self):
        self._stop.set()
        for event in self._events:
            event.set()
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
