"""A bounded display worker keeps blocking screen operations off emulation."""

import threading
import time
from collections import deque

from .thread_scheduling import playback_thread_priority


class FramePresenter:
    def __init__(self, present, *, frame_interval=0.0):
        self._present = present
        self._interval = max(0.0, frame_interval)
        self._condition = threading.Condition()
        # One frame of lead absorbs short producer delays; a blocked screen
        # still retains at most two frames and never backs up emulation.
        self._capacity = 2 if self._interval else 1
        self._pending = deque()
        self._stopping = False
        self.error = None
        self.presented = 0
        self.skipped = 0
        self._thread = threading.Thread(target=self._run, name='nes-present', daemon=True)
        self._thread.start()

    def submit(self, frame):
        with self._condition:
            if self._stopping or self.error is not None:
                return False
            if len(self._pending) == self._capacity:
                self._pending.popleft()
                self.skipped += 1
            self._pending.append(frame)
            self._condition.notify()
        return True

    @playback_thread_priority()
    def _run(self):
        deadline = None
        while True:
            with self._condition:
                while not self._pending and not self._stopping:
                    self._condition.wait()
                if self._stopping:
                    return
                if deadline is None:
                    deadline = time.monotonic() + self._interval
                delay = deadline - time.monotonic()
                if delay > 0:
                    self._condition.wait(delay)
                    continue
                frame = self._pending.popleft()
            try:
                self._present(*frame)
            except Exception as error:
                self.error = error
                return
            self.presented += 1
            deadline += self._interval
            now = time.monotonic()
            if self._interval and deadline < now:
                # A genuine display stall discards old work instead of
                # bursting frames to repay time already lost on screen.
                with self._condition:
                    while len(self._pending) > 1:
                        self._pending.popleft()
                        self.skipped += 1
                deadline = now + self._interval

    def close(self, timeout=2.0):
        with self._condition:
            self._stopping = True
            self._pending.clear()
            self._condition.notify()
        if threading.current_thread() is not self._thread:
            self._thread.join(timeout)
        stopped = not self._thread.is_alive()
        if stopped:
            # Release bound player/window references on the closing thread;
            # leaving a cycle lets later worker-thread garbage collection
            # finalize Tk objects outside their owning thread.
            self._present = None
        return stopped
