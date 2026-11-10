"""Private NES session manifests and argument-list launch boundaries."""

from __future__ import annotations

from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import uuid
from collections.abc import Mapping
from types import MappingProxyType

from graphics.runtime import GraphicsRuntimeManifest
from .controls import normalize_nes_bindings

MAX_SESSION_MANIFEST_BYTES = 65536
MAX_BUILD_INFO_BYTES = 65536
TILE_CAPTURE_FEATURE = "nes-tile-capture-rnt1"
_ID = re.compile(r"[0-9a-f]{64}\Z")
_SESSION = re.compile(r"[0-9a-f]{32}\Z")


def _owned_path(path: Path, root: Path, *, directory=False) -> Path:
    path = Path(path)
    root = Path(os.path.abspath(root))
    candidate = Path(os.path.abspath(path))
    if not path.is_absolute():
        raise ValueError("Session paths must be absolute and owned")
    try:
        relative = candidate.relative_to(root)
    except ValueError as error:
        raise ValueError("Session path escapes its private root") from error
    current = root
    for part in (".", *relative.parts):
        if part != ".":
            current = current / part
        if current.is_symlink() or current.is_junction():
            raise ValueError("Session paths cannot use symbolic links or junctions")
    resolved = candidate.resolve()
    if not resolved.is_relative_to(root.resolve()):
        raise ValueError("Session path escapes its private root")
    if directory and resolved.exists() and not resolved.is_dir():
        raise ValueError("Session directory is not a directory")
    return resolved


@dataclass(frozen=True)
class NesSessionManifest:
    schema: int
    game_id: str
    session_id: str
    rom_path: Path
    rom_sha256: str
    home: Path
    saves: Path
    events: Path
    status: Path
    mode: str
    pack_manifest: Path | None
    pack_scale: int
    controls: Mapping[str, str]
    graphics_manifest: Path

    def __post_init__(self):
        if self.schema != 1 or _ID.fullmatch(self.game_id) is None or _SESSION.fullmatch(self.session_id) is None:
            raise ValueError("Invalid NES session identity")
        if not self.rom_path.is_absolute() or _ID.fullmatch(self.rom_sha256) is None:
            raise ValueError("NES session ROM identity is invalid")
        if self.mode not in ("original", "enhanced") or self.pack_scale not in (2, 4):
            raise ValueError("NES session mode or scale is invalid")
        if self.mode == "enhanced" and self.pack_manifest is None:
            raise ValueError("Enhanced NES sessions require an immutable pack snapshot")
        if not isinstance(self.controls, Mapping) or not self.graphics_manifest.is_absolute():
            raise ValueError("NES session settings are invalid")
        # Each session owns its frozen controls; later profile edits affect only later launches.
        object.__setattr__(self, "controls", MappingProxyType(normalize_nes_bindings(self.controls)))

    def document(self):
        return {
            "schema": self.schema, "game_id": self.game_id, "session_id": self.session_id,
            "rom_path": str(self.rom_path), "rom_sha256": self.rom_sha256,
            "home": str(self.home), "saves": str(self.saves), "events": str(self.events),
            "status": str(self.status), "mode": self.mode,
            "pack_manifest": str(self.pack_manifest) if self.pack_manifest else None,
            "pack_scale": self.pack_scale, "controls": dict(self.controls),
            "graphics_manifest": str(self.graphics_manifest),
        }


class NesSessionFactory:
    def __init__(self, *, writer=None, session_id_factory=None):
        self.writer = writer or self._write
        self.session_id_factory = session_id_factory or (lambda: uuid.uuid4().hex)

    @staticmethod
    def _write(path: Path, document: dict):
        temporary = path.with_name(path.name + f".{uuid.uuid4().hex}.tmp")
        path.parent.mkdir(parents=True, exist_ok=True)
        try:
            temporary.write_text(json.dumps(document, sort_keys=True, separators=(",", ":")),
                                 encoding="utf-8")
            os.replace(temporary, path)
        finally:
            temporary.unlink(missing_ok=True)

    @staticmethod
    def _sha256(path: Path) -> str:
        digest = hashlib.sha256()
        with path.open("rb") as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(chunk)
        return digest.hexdigest()

    def create(self, workspace: Path, game: dict, rom_path: Path,
               graphics_manifest: GraphicsRuntimeManifest, *, original: bool,
               pack_manifest: Path | None = None, pack_scale: int = 4,
               controls: dict | None = None, graphics_manifest_path: Path | None = None) -> NesSessionManifest:
        controls_snapshot = normalize_nes_bindings(controls)
        workspace = Path(workspace).resolve()
        rom_path = _owned_path(Path(rom_path), workspace.parent.parent, directory=False)
        if not rom_path.is_file():
            raise ValueError("NES ROM is missing")
        if _ID.fullmatch(game.get("id", "")) is None:
            raise ValueError("NES game identity is invalid")
        if not original and pack_manifest is None:
            raise ValueError("Enhanced NES sessions require an immutable pack snapshot")
        pack = None if original else _owned_path(Path(pack_manifest), workspace, directory=False)
        if not original and not pack.is_file():
            raise ValueError("NES pack snapshot is missing")
        session_id = self.session_id_factory()
        if _SESSION.fullmatch(session_id) is None:
            raise ValueError("Session ID factory returned an invalid value")
        session_root = _owned_path(workspace / ".runtime" / "sessions" / session_id,
                                   workspace, directory=True)
        home = session_root / "home"
        saves = _owned_path(workspace / "saves" / "mesen", workspace, directory=True)
        events = session_root / "events"
        status = session_root / "status.json"
        session_root.mkdir(parents=True, exist_ok=False)
        for folder in (home, events):
            folder.mkdir(parents=True, exist_ok=False)
        saves.mkdir(parents=True, exist_ok=True)
        if graphics_manifest_path is None:
            graphics_manifest_path = graphics_manifest.dll.parent / "graphics-runtime.json"
        graphics_manifest_path = Path(graphics_manifest_path)
        if not graphics_manifest_path.is_absolute():
            raise ValueError("Graphics manifest path must be absolute")
        graphics_manifest_path = graphics_manifest_path.resolve()
        manifest = NesSessionManifest(
            1, game["id"], session_id, rom_path, self._sha256(rom_path), home, saves,
            events, status, "original" if original else "enhanced", pack, pack_scale,
            controls_snapshot, graphics_manifest_path)
        payload = manifest.document()
        encoded = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
        if len(encoded) > MAX_SESSION_MANIFEST_BYTES:
            raise ValueError("NES session manifest exceeds 64 KiB")
        self.writer(session_root / "launch.json", payload)
        return manifest


class NesLaunchArguments:
    def __init__(self, executable: Path):
        executable = Path(executable).resolve()
        if not executable.is_file():
            raise ValueError("NES runtime executable is missing")
        self.executable = executable

    def build(self, manifest: NesSessionManifest) -> list[str]:
        return [str(self.executable), f"--revival-launch={manifest.home.parent / 'launch.json'}"]


class NesProcessRuntime:
    """Process boundary with shell disabled and no forced kill during saves."""

    def __init__(self, executable: Path, *, process_factory=subprocess.Popen):
        self.arguments = NesLaunchArguments(executable)
        self.process_factory = process_factory

    def launch(self, manifest: NesSessionManifest, *, controls_fallback_reason: str | None = None,
               environment: dict[str, str] | None = None):
        args = self.arguments.build(manifest)
        log = manifest.home.parent / "runtime.log"
        with log.open("w", encoding="utf-8") as stream:
            if controls_fallback_reason:
                reason = " ".join(str(controls_fallback_reason).split())[:300]
                stream.write(f"NES controls fell back to Default: {reason}\n")
                stream.flush()
            options = {
                "cwd": manifest.home, "shell": False,
                "stdout": stream, "stderr": subprocess.STDOUT,
            }
            if environment is not None:
                options["env"] = dict(environment)
            process = self.process_factory(args, **options)
        return process, log

    @staticmethod
    def close(process, *, timeout: float = 5.0):
        if timeout <= 0:
            raise ValueError("Close timeout must be positive")
        try:
            return process.wait(timeout=timeout)
        except subprocess.TimeoutExpired as error:
            raise TimeoutError("NES runtime did not report a clean stop") from error


class NesManagedRuntime:
    """Adapt the session boundary to the library's injected runtime contract."""

    def __init__(self, executable: Path, graphics_manifest: Path, *,
                 resolver=None, session_factory=None, process_factory=subprocess.Popen):
        self.executable = Path(executable).resolve()
        self.graphics_manifest = Path(graphics_manifest)
        if not self.graphics_manifest.is_absolute():
            raise ValueError("Graphics manifest path must be absolute")
        self.resolver = resolver
        self.session_factory = session_factory or NesSessionFactory()
        self.process_factory = process_factory

    def _graphics(self):
        from graphics.runtime import GraphicsRuntimeResolver
        manifest_root = self.graphics_manifest.parent
        if manifest_root.name.casefold() in {"jme", "nes"}:
            manifest_root = manifest_root.parent
        resolver = self.resolver or GraphicsRuntimeResolver(manifest_root)
        return resolver.resolve(self.graphics_manifest)

    def available(self):
        try:
            self._graphics()
            if not self.executable.is_file() or self.executable.is_symlink():
                return False
            player_root = self.executable.parent
            required = ("MesenCore.dll", "libSkiaSharp.dll", "libHarfBuzzSharp.dll")
            for name in required:
                path = player_root / name
                if not path.is_file() or path.is_symlink() or path.is_junction():
                    return False
            return True
        except (OSError, ValueError):
            return False

    def capture_available(self):
        """Report capture support separately from ordinary Original playback."""
        path = self.graphics_manifest.parent / "build-info.json"
        try:
            if path.is_symlink() or not path.is_file() or path.stat().st_size > MAX_BUILD_INFO_BYTES:
                return False
            document = json.loads(path.read_text(encoding="utf-8"))
            features = document.get("features")
            return (document.get("schema") == 1 and isinstance(features, list) and
                    TILE_CAPTURE_FEATURE in features)
        except (OSError, ValueError, TypeError, json.JSONDecodeError):
            return False

    @staticmethod
    def _pack_snapshot(workspace: Path):
        pointer = workspace / ".runtime" / "nes-pack.json"
        try:
            raw = pointer.read_bytes()
            if len(raw) > MAX_SESSION_MANIFEST_BYTES:
                raise ValueError("NES pack pointer exceeds 64 KiB")
            document = json.loads(raw)
            relative = document["manifest"]
            path = Path(relative)
            if path.is_absolute() or path.as_posix() != relative or not path.parts:
                raise ValueError("NES pack pointer path is invalid")
            resolved = (workspace / path).resolve()
            if not resolved.is_relative_to(workspace.resolve()) or not resolved.is_file():
                raise ValueError("NES pack snapshot is missing")
            if document.get("schema") != 1 or document.get("generation") not in resolved.parts:
                raise ValueError("NES pack pointer is invalid")
            return resolved, int(document["pack_scale"])
        except (OSError, KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
            raise ValueError("NES pack snapshot is invalid") from error

    def launch(self, game, source, workspace, *, original=False, **kwargs):
        capture_requested = kwargs.get("capture") is True
        if capture_requested and (original or not self.capture_available()):
            raise RuntimeError("This NES player build does not support tile capture")
        graphics = self._graphics()
        pack = None
        requested_scale = int(game.get("render_scale", 4))
        if requested_scale not in (2, 4):
            raise ValueError("NES display detail must be 2x or 4x")
        pack_scale = requested_scale
        if not original:
            pack, pack_scale = self._pack_snapshot(Path(workspace))
            if pack_scale != requested_scale:
                raise ValueError("The NES artwork pack is being prepared for the selected detail; try again shortly")
        session = self.session_factory.create(
            workspace, game, source, graphics, original=original,
            pack_manifest=pack, pack_scale=pack_scale,
            controls=kwargs.get("controls"), graphics_manifest_path=self.graphics_manifest)
        child_environment = os.environ.copy()
        # Clear inherited switches first so each player launch has an explicit policy.
        child_environment.pop("J2ME_REVIVAL_ENABLE_NES_ENHANCED", None)
        child_environment.pop("J2ME_REVIVAL_NES_CAPTURE", None)
        if not original:
            child_environment["J2ME_REVIVAL_ENABLE_NES_ENHANCED"] = "1"
            if kwargs.get("capture") is True:
                child_environment["J2ME_REVIVAL_NES_CAPTURE"] = "1"
        graphics_directory = str(graphics.dll.parent)
        search_paths = [graphics_directory, *child_environment.get("PATH", "").split(os.pathsep)]
        child_environment["PATH"] = os.pathsep.join(
            dict.fromkeys(path for path in search_paths if path))
        return NesProcessRuntime(self.executable, process_factory=self.process_factory).launch(
            session,
            controls_fallback_reason=kwargs.get("controls_fallback_reason"),
            environment=child_environment,
        )
