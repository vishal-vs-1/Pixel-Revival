"""Persist the Java ME GPU implementation choice independently of GPU selection."""

from __future__ import annotations

from dataclasses import dataclass
import json
from pathlib import Path
from typing import Callable

from persistence import atomic_json
from .runtime import is_redirected_path

COMMON_BACKEND = "common-v1"
LEGACY_BACKEND = "legacy"
BACKENDS = frozenset((COMMON_BACKEND, LEGACY_BACKEND))
MAX_PREFERENCE_BYTES = 4096


@dataclass(frozen=True, slots=True)
class BackendPreference:
    backend: str
    warning: str | None = None


class GraphicsBackendPreferenceStore:
    """Own one global JME GPU backend preference, defaulting new installs to common-v1."""

    def __init__(self, path: Path, writer: Callable = atomic_json):
        self.path = Path(path)
        self.writer = writer

    def load(self) -> BackendPreference:
        if is_redirected_path(self.path):
            return BackendPreference(LEGACY_BACKEND,
                                    "Renderer preference file is redirected")
        if not self.path.exists():
            return BackendPreference(COMMON_BACKEND)
        try:
            if self.path.stat().st_size > MAX_PREFERENCE_BYTES:
                raise ValueError("The renderer preference file is too large")
            value = json.loads(self.path.read_text(encoding="utf-8"))
            if (not isinstance(value, dict) or value.get("schema") != 1 or
                    value.get("backend") not in BACKENDS):
                raise ValueError("The renderer preference file is invalid")
            return BackendPreference(value["backend"])
        except (OSError, UnicodeDecodeError, json.JSONDecodeError, ValueError) as error:
            return BackendPreference(LEGACY_BACKEND,
                                     f"Renderer preference could not be read: {error}")

    def save(self, backend: str) -> BackendPreference:
        if backend not in BACKENDS:
            raise ValueError("Choose the shared or legacy GPU renderer")
        self.writer(self.path, {"schema": 1, "backend": backend})
        return BackendPreference(backend)
