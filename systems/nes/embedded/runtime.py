"""Connect the optional embedded host to the NES library runtime contract."""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import threading
from .api import MesenNativeApi
from ..runtime import IN_PROCESS_FEATURE, MAX_BUILD_INFO_BYTES, NesManagedRuntime, NesSessionFactory
from .core import MesenCoreHost
from .window import NesInProcessPlayer
from .pack import _stage_pack


class NesInProcessRuntime:
    """Adapt the app-owned view and native core to the NES library runtime contract."""

    def __init__(self, core_dll: Path, executable: Path, graphics_manifest: Path, *,
                 resolver=None, session_factory=None, api_factory=MesenNativeApi.load):
        self.core_dll = Path(core_dll).resolve()
        self.executable = Path(executable).resolve()
        self.graphics_manifest = Path(graphics_manifest).resolve()
        self.resolver = resolver
        self.session_factory = session_factory or NesSessionFactory()
        self.api_factory = api_factory
        self.host = None
        self._active = None
        self._launching = False
        self._lock = threading.RLock()
        self._managed_fallback = NesManagedRuntime(
            self.executable, self.graphics_manifest, resolver=self.resolver,
            session_factory=self.session_factory,
        )

    def _graphics(self):
        from graphics.runtime import GraphicsRuntimeResolver
        manifest_root = self.graphics_manifest.parent
        if manifest_root.name.casefold() in {"jme", "nes"}:
            manifest_root = manifest_root.parent
        resolver = self.resolver or GraphicsRuntimeResolver(manifest_root)
        return resolver.resolve(self.graphics_manifest)

    def _build_info(self):
        path = self.graphics_manifest.parent / "build-info.json"
        if path.is_symlink() or path.is_junction() or not path.is_file() or path.stat().st_size > MAX_BUILD_INFO_BYTES:
            raise ValueError("Mesen runtime build information is missing or invalid")
        document = json.loads(path.read_text(encoding="utf-8"))
        return path, document

    def available(self):
        try:
            self._graphics()
            path, info = self._build_info()
            del path
            if (info.get("schema") != 1 or not isinstance(info.get("features"), list) or
                    IN_PROCESS_FEATURE not in info["features"]):
                return False
            if (not self.core_dll.is_file() or self.core_dll.is_symlink() or self.core_dll.is_junction() or
                    not self.executable.is_file() or self.executable.is_symlink() or
                    self.executable.is_junction()):
                return False
            digest_state = hashlib.sha256()
            with self.core_dll.open("rb") as stream:
                for block in iter(lambda: stream.read(1024 * 1024), b""):
                    digest_state.update(block)
            digest = digest_state.hexdigest()
            return digest == info.get("native_core_sha256")
        except (OSError, ValueError, TypeError, json.JSONDecodeError):
            return False

    def capture_available(self):
        try:
            _path, info = self._build_info()
            return info.get("schema") == 1 and "nes-tile-capture-rnt1" in info.get("features", ())
        except (OSError, ValueError, TypeError, json.JSONDecodeError):
            return False

    @staticmethod
    def _pack_snapshot(workspace: Path):
        return NesManagedRuntime._pack_snapshot(workspace)

    def _finished(self, player):
        with self._lock:
            if self._active is player:
                self._active = None

    def launch(self, game, source, workspace, *, original=False, host_parent=None, **kwargs):
        if host_parent is None:
            raise RuntimeError("The in-process NES player needs the application window")
        with self._lock:
            if self._launching:
                raise RuntimeError("An in-process NES game is already starting")
            if self._active is not None and self._active.poll() is None:
                raise RuntimeError("Only one in-process NES game can run at a time; close the current game first")
            self._launching = True
        try:
            capture = kwargs.get("capture") is True
            if capture and (original or not self.capture_available()):
                raise RuntimeError("This Mesen build does not support NES tile capture")
            graphics = self._graphics()
            scale = int(game.get("render_scale", 4))
            if scale not in (2, 4):
                raise ValueError("NES display detail must be 2x or 4x")
            pack = None
            if not original:
                pack, pack_scale = self._pack_snapshot(Path(workspace))
                if pack_scale != scale:
                    raise ValueError("The NES artwork pack is being prepared for the selected detail; try again shortly")
            session = self.session_factory.create(
                workspace, game, source, graphics, original=original,
                pack_manifest=pack, pack_scale=scale,
                controls=kwargs.get("controls"), graphics_manifest_path=self.graphics_manifest)
            session_root = session.home.parent
            log = session_root / "runtime.log"
            log.write_text("", encoding="utf-8")
            if kwargs.get("controls_fallback_reason"):
                with log.open("a", encoding="utf-8") as stream:
                    stream.write("NES controls fell back to Default: " +
                                 " ".join(str(kwargs["controls_fallback_reason"]).split())[:300] + "\n")
            if not original:
                _stage_pack(pack, Path(workspace), session.home, source, scale)
            with self._lock:
                if self.host is None:
                    self.host = MesenCoreHost(self.core_dll, graphics.dll.parent, self.api_factory)
                player = NesInProcessPlayer(self, session, parent=host_parent,
                                            original=original, capture=capture,
                                            on_finished=self._finished)
                player._launch_spec = (game, source, Path(workspace), original, dict(kwargs))
                self._active = player
            player._write_status("starting")
            return player, log
        finally:
            with self._lock:
                self._launching = False

    def _launch_managed_fallback(self, player):
        if player._launch_spec is None:
            raise RuntimeError("The managed fallback has no saved launch settings")
        game, source, workspace, original, kwargs = player._launch_spec
        if not self._managed_fallback.available():
            raise RuntimeError("The managed Mesen fallback runtime is incomplete")
        return self._managed_fallback.launch(
            game, source, workspace, original=original,
            capture=kwargs.get("capture") is True,
            controls=kwargs.get("controls"),
            controls_fallback_reason=kwargs.get("controls_fallback_reason"),
        )

    def shutdown(self, timeout: float = 60.0):
        with self._lock:
            player = self._active
            host = self.host
        if player is not None:
            player.close_blocking(timeout=timeout)
        if host is not None:
            host.shutdown(timeout=timeout)
        with self._lock:
            self.host = None
            self._active = None
