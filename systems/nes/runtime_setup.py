"""Locate an installed or developer-built managed NES runtime."""

from __future__ import annotations

import os
import json
from pathlib import Path
from typing import Callable, Mapping

from .runtime import IN_PROCESS_FEATURE, MAX_BUILD_INFO_BYTES, NesManagedRuntime


RUNTIME_ROOT_ENV = "J2ME_REVIVAL_NES_RUNTIME_ROOT"
IN_PROCESS_ENV = "J2ME_REVIVAL_NES_INPROCESS"


class NesRuntimeLocator:
    """Return a usable NES runtime without changing Java ME or user data."""

    def __init__(self, app_root: Path, *, packaged: bool,
                 environment: Mapping[str, str] | None = None,
                 runtime_factory: Callable[[Path, Path], object] | None = None,
                 inprocess_factory: Callable[[Path, Path, Path], object] | None = None):
        self.app_root = Path(app_root).resolve()
        self.packaged = bool(packaged)
        self.environment = os.environ if environment is None else environment
        self.runtime_factory = runtime_factory or NesManagedRuntime
        self.inprocess_factory = inprocess_factory

    def _experimental_enabled(self):
        return self.environment.get(IN_PROCESS_ENV) == "1"

    def _roots(self) -> tuple[Path, ...]:
        configured = self.environment.get(RUNTIME_ROOT_ENV)
        if configured is not None:
            root = Path(configured)
            # An explicit but invalid override must not silently select another runtime.
            return (root.resolve(),) if root.is_absolute() else ()

        roots = [self.app_root / "tools" / "nes-runtime"]
        if not self.packaged and self._experimental_enabled():
            roots.insert(0, self.app_root / "build" / "common-runtime-inprocess-v1")
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
                build_info = manifest.parent / "build-info.json"
                features = []
                if build_info.is_file() and not build_info.is_symlink() and not build_info.is_junction():
                    if build_info.stat().st_size <= MAX_BUILD_INFO_BYTES:
                        document = json.loads(build_info.read_text(encoding="utf-8"))
                        if isinstance(document, dict):
                            features = document.get("features", [])
                # A native access violation can end the app before Python can fall back.
                if isinstance(features, list) and IN_PROCESS_FEATURE in features:
                    if not self._experimental_enabled():
                        continue
                    factory = self.inprocess_factory
                    if factory is None:
                        from .embedded.runtime import NesInProcessRuntime
                        factory = NesInProcessRuntime
                    runtime = factory(executable.parent / "MesenCore.dll", executable, manifest)
                else:
                    runtime = self.runtime_factory(executable, manifest)
                if runtime.available():
                    return runtime
            except (OSError, ValueError):
                continue
        return None
