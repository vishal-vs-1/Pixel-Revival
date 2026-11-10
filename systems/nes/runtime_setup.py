"""Locate an installed or developer-built managed NES runtime."""

from __future__ import annotations

import os
from pathlib import Path
from typing import Callable, Mapping

from .runtime import NesManagedRuntime


RUNTIME_ROOT_ENV = "J2ME_REVIVAL_NES_RUNTIME_ROOT"


class NesRuntimeLocator:
    """Return a usable NES runtime without changing Java ME or user data."""

    def __init__(self, app_root: Path, *, packaged: bool,
                 environment: Mapping[str, str] | None = None,
                 runtime_factory: Callable[[Path, Path], object] | None = None):
        self.app_root = Path(app_root).resolve()
        self.packaged = bool(packaged)
        self.environment = os.environ if environment is None else environment
        self.runtime_factory = runtime_factory or NesManagedRuntime

    def _roots(self) -> tuple[Path, ...]:
        configured = self.environment.get(RUNTIME_ROOT_ENV)
        if configured is not None:
            root = Path(configured)
            # An explicit but invalid override must not silently select another runtime.
            return (root.resolve(),) if root.is_absolute() else ()

        roots = [self.app_root / "tools" / "nes-runtime"]
        if not self.packaged:
            roots.extend((
                self.app_root / "build" / "common-runtime-u18c-capture",
                self.app_root / "build" / "common-runtime-u22",
            ))
        return tuple(root.resolve() for root in roots)

    def locate(self):
        for root in self._roots():
            executable = root / "nes" / "player" / "Mesen.exe"
            manifest = root / "nes" / "graphics-runtime.json"
            if not executable.is_file() or not manifest.is_file():
                continue
            try:
                runtime = self.runtime_factory(executable, manifest)
                if runtime.available():
                    return runtime
            except (OSError, ValueError):
                continue
        return None
