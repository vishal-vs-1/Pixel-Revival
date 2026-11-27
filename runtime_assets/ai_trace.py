"""Bounded, opt-in diagnostics for native AI device selection."""

from datetime import datetime, timezone
import json
import os
from pathlib import Path
import threading
import time
import uuid

from storage.paths import DATA_ROOT

_LOCK = threading.Lock()
_MAX_BYTES = 2 * 1024 * 1024


class AiDeviceTrace:
    def __init__(self):
        self.enabled = False
        self._root = DATA_ROOT / 'local-work' / 'diagnostics' / 'ai-device-trace'
        self._id = uuid.uuid4().hex
        self._started = time.monotonic()
        try:
            config_path = self._root / 'enabled.json'
            if config_path.stat().st_size > 4096:
                return
            config = json.loads(config_path.read_text(encoding='utf-8'))
            expires = float(config['expires_at'])
            self.enabled = time.time() < expires <= time.time() + 86400
        except (OSError, ValueError, TypeError, KeyError):
            pass

    def record(self, event, **details):
        if not self.enabled:
            return
        try:
            row = {'utc': datetime.now(timezone.utc).isoformat(), 'event': event,
                   'run_id': self._id, 'parent_pid': os.getpid(),
                   'elapsed_seconds': round(time.monotonic() - self._started, 4), **details}
            data = (json.dumps(row, ensure_ascii=True) + '\n').encode('utf-8')
            path = self._root / 'events.jsonl'
            # Log once per batch boundary. Reaching the cap disables logging,
            # without delaying or failing the enhancement worker.
            with _LOCK:
                size = path.stat().st_size if path.exists() else 0
                if size + len(data) > _MAX_BYTES:
                    self.enabled = False
                    return
                with path.open('ab') as stream:
                    stream.write(data)
        except (OSError, ValueError, TypeError):
            self.enabled = False
