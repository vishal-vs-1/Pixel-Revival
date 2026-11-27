"""Keep paired NES frames and controller samples aligned."""

import threading


class ComparisonClock:
    def __init__(self):
        self.mask = 0
        self.frame_mask = 0
        self.closed = False
        self._barrier = threading.Barrier(2, action=self._sample)

    def _sample(self):
        self.frame_mask = self.mask

    def next_input(self):
        self._barrier.wait(timeout=5.0)
        return self.frame_mask

    def close(self):
        self.closed = True
        self._barrier.abort()
