"""One launcher session's process, identity and cooperative close state."""

from dataclasses import dataclass
from pathlib import Path
import time

from systems.contracts import GameRef


@dataclass
class PlaybackSession:
    ref: GameRef | None
    process: object
    log: Path
    recording: bool = False
    closing_at: float | None = None

    def __iter__(self):
        # Existing event/telemetry consumers still unpack process and log.
        return iter((self.process, self.log))

    def __getitem__(self, index):
        return (self.process, self.log)[index]

    def request_close(self):
        if self.process.poll() is not None:
            return True
        if self.closing_at is None:
            view = getattr(self.process, 'view', None)
            target = view if view is not None else self.process
            close = (getattr(target, 'request_close', None) or
                     getattr(target, 'shutdown', None) or getattr(target, 'close', None))
            if not callable(close):
                raise RuntimeError('Close this player window to finish saving before exiting.')
            close()
            self.closing_at = time.monotonic()
        if self.process.poll() is None and time.monotonic() - self.closing_at > 10:
            raise RuntimeError('The player is still saving or closing; close its window and retry.')
        return self.process.poll() is not None


class JavaProcess:
    """Keep Popen compatibility while adding an application-owned close request."""

    def __init__(self, process):
        self._process = process

    def __getattr__(self, name):
        return getattr(self._process, name)

    def request_close(self):
        if self.poll() is not None:
            return
        if self.stdin is None:
            raise RuntimeError('Player control pipe is unavailable; close its window first.')
        self.stdin.write(b'!CLOSE\n')
        self.stdin.flush()
