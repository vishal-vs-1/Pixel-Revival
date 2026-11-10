"""Locate and verify the shared native runtime used by Java ME GPU playback."""

from __future__ import annotations

import json
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping

from .runtime import GraphicsRuntimeResolver, is_redirected_path
from .runtime_package import validate_runtime_tree

RUNTIME_ROOT_ENV = "J2ME_REVIVAL_GRAPHICS_RUNTIME_ROOT"


class JmeGraphicsRuntimeLocator:
    """Resolve one validated renderer bundle; explicit invalid roots fail closed."""

    def __init__(self, app_root: Path, *, packaged: bool,
                 environment: Mapping[str, str] | None = None,
                 preferred_manifest: Path | None = None):
        self.app_root = Path(app_root).resolve()
        self.packaged = bool(packaged)
        self.environment = os.environ if environment is None else environment
        self.preferred_manifest = Path(preferred_manifest) if preferred_manifest else None
        self.last_error: str | None = None

    def _roots(self) -> tuple[Path, ...]:
        configured = self.environment.get(RUNTIME_ROOT_ENV)
        if configured is not None:
            root = Path(configured)
            return (root,) if root.is_absolute() else ()

        roots = []
        if self.preferred_manifest is not None:
            parent = self.preferred_manifest.parent
            roots.append(parent.parent if parent.name.casefold() in {"jme", "nes"} else parent)
        roots.append(self.app_root / "tools" / "common-runtime")
        if not self.packaged:
            roots.extend((
                self.app_root / "build" / "common-runtime-u18c-capture",
                self.app_root / "build" / "common-runtime-u23-resize",
                self.app_root / "build" / "common-runtime-u22",
            ))
        return tuple(roots)

    @staticmethod
    def _resolve(root: Path):
        if is_redirected_path(root):
            raise ValueError("Shared graphics runtime root is redirected")
        root = root.resolve()
        manifest_path = root / "jme" / "graphics-runtime.json"
        identity = validate_runtime_tree(root)
        manifest = GraphicsRuntimeResolver(root).resolve(manifest_path)
        core = manifest.dll
        shim = root / "common" / "revival_graphics_jni.dll"
        if is_redirected_path(shim):
            raise ValueError("Shared graphics JNI shim is redirected")
        shim = shim.resolve()
        if not shim.is_relative_to(root.resolve()) or not shim.is_file():
            raise ValueError("Shared graphics JNI shim is missing or outside its runtime")

        # The package hash manifest also authenticates the JNI shim loaded by Java.
        hashes_path = root / "common" / "runtime-hashes.json"
        if hashes_path.stat().st_size > 65536:
            raise ValueError("Shared graphics hash manifest is too large")
        hashes = json.loads(hashes_path.read_text(encoding="utf-8"))
        expected = hashes.get("files", {}).get("common/revival_graphics_jni.dll")
        if expected != identity["jni_sha256"]:
            raise ValueError("Shared graphics JNI shim hash is inconsistent")
        return JmeGraphicsRuntime(root, manifest_path.resolve(), core, shim,
                                  manifest.build_id)

    def locate(self):
        self.last_error = None
        roots = self._roots()
        if not roots:
            self.last_error = f"{RUNTIME_ROOT_ENV} must be an absolute path"
            return None
        failures = []
        for root in dict.fromkeys(roots):
            try:
                return self._resolve(root)
            except (OSError, ValueError, KeyError, TypeError, json.JSONDecodeError) as error:
                failures.append(f"{root}: {error}")
                if self.environment.get(RUNTIME_ROOT_ENV) is not None:
                    break
        self.last_error = "; ".join(failures) if failures else "No shared graphics runtime was found"
        return None


@dataclass(frozen=True, slots=True)
class JmeGraphicsRuntime:
    root: Path
    manifest: Path
    core: Path
    shim: Path
    build_id: str
