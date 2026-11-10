"""Verified common graphics runtime manifests and native handshakes."""

from __future__ import annotations

from dataclasses import dataclass
import hashlib
import json
from pathlib import Path
import re

MAX_MANIFEST_BYTES = 65536
_BUILD_ID = re.compile(r"[0-9a-f]{64}\Z")


def is_redirected_path(path: Path) -> bool:
    """Detect symlinks and junctions across the supported Python versions."""
    junction_check = getattr(path, "is_junction", None)
    return path.is_symlink() or bool(junction_check and junction_check())


def _owned_file(path: Path, root: Path, suffix: str | None = None) -> Path:
    if not isinstance(path, Path) or not path.is_absolute():
        raise ValueError("Runtime paths must be absolute")
    if is_redirected_path(path):
        raise ValueError("Runtime paths cannot be redirected")
    resolved = path.resolve()
    if not resolved.is_relative_to(root.resolve()) or not resolved.is_file():
        raise ValueError("Runtime file is outside the installed runtime")
    if suffix is not None and resolved.suffix.casefold() != suffix:
        raise ValueError("Runtime file has an unexpected type")
    return resolved


@dataclass(frozen=True)
class GraphicsRuntimeManifest:
    schema: int
    protocol: str
    abi_version: int
    build_id: str
    dll: Path

    def __post_init__(self):
        if self.schema != 1 or self.protocol != "common-v1" or self.abi_version != 1:
            raise ValueError("Unsupported common graphics runtime manifest")
        if _BUILD_ID.fullmatch(self.build_id) is None:
            raise ValueError("Graphics build ID must be a lowercase SHA-256 digest")
        if not self.dll.is_absolute():
            raise ValueError("Graphics DLL path must be absolute")


@dataclass(frozen=True)
class GraphicsHandshake:
    abi_version: int
    build_id: str
    actual_device: str

    def __post_init__(self):
        if self.abi_version != 1 or _BUILD_ID.fullmatch(self.build_id) is None:
            raise ValueError("Native graphics handshake is incompatible")
        if not isinstance(self.actual_device, str) or not self.actual_device.strip() or len(self.actual_device) > 256:
            raise ValueError("Native graphics handshake must report a device")


class GraphicsRuntimeResolver:
    """Read one installed manifest and verify the exact DLL bytes it names."""

    def __init__(self, install_root: Path, *, digest_file=None):
        self.install_root = Path(install_root).resolve()
        self.digest_file = digest_file or self._digest_file

    @staticmethod
    def _digest_file(path: Path) -> str:
        digest = hashlib.sha256()
        with path.open("rb") as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(chunk)
        return digest.hexdigest()

    def resolve(self, manifest_path: Path) -> GraphicsRuntimeManifest:
        manifest_path = _owned_file(Path(manifest_path), self.install_root, ".json")
        raw = manifest_path.read_bytes()
        if len(raw) > MAX_MANIFEST_BYTES:
            raise ValueError("Graphics runtime manifest exceeds 64 KiB")
        try:
            document = json.loads(raw)
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise ValueError("Graphics runtime manifest is not valid JSON") from error
        if not isinstance(document, dict) or set(document) != {
                "schema", "protocol", "abi_version", "build_id", "dll"}:
            raise ValueError("Graphics runtime manifest fields are invalid")
        dll_value = document["dll"]
        if not isinstance(dll_value, str) or not dll_value or len(dll_value) > 512:
            raise ValueError("Graphics DLL path is invalid")
        dll = Path(dll_value)
        if not dll.is_absolute():
            # Portable packages keep consumer manifests beside the one shared DLL tree.
            dll = manifest_path.parent / dll
        checked = _owned_file(dll, self.install_root, ".dll")
        digest = self.digest_file(checked)
        if digest != document["build_id"]:
            raise ValueError("Graphics DLL does not match its manifest")
        return GraphicsRuntimeManifest(document["schema"], document["protocol"],
                                       document["abi_version"], document["build_id"], checked)

    @staticmethod
    def verify_handshake(manifest: GraphicsRuntimeManifest, handshake: GraphicsHandshake):
        if (manifest.abi_version != handshake.abi_version or
                manifest.build_id != handshake.build_id):
            raise ValueError("Native graphics handshake does not match the installed runtime")
        return handshake
