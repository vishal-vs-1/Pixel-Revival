"""Build and validate the shared native runtime package layout."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import zipfile

from .runtime import GraphicsRuntimeResolver

ABI_BUILD_ID = re.compile(r"[0-9a-f]{64}\Z")
MAX_ARCHIVE_ENTRIES = 10000
MAX_ARCHIVE_BYTES = 2 * 1024 * 1024 * 1024
PACKAGE_ROOT = "JAR-Revival-CommonGraphics"


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_consumer_manifests(runtime_root: Path, abi_build_id: str) -> None:
    """Write JME and NES manifests that resolve the same installed core DLL."""
    runtime_root = Path(runtime_root).resolve()
    if ABI_BUILD_ID.fullmatch(abi_build_id) is None:
        raise ValueError("Native ABI build ID is invalid")
    core = runtime_root / "common" / "revival_graphics.dll"
    shim = runtime_root / "common" / "revival_graphics_jni.dll"
    if not core.is_file() or not shim.is_file():
        raise FileNotFoundError("Both shared graphics DLLs must be staged before writing manifests")
    digest = sha256_file(core)
    document = {
        "schema": 1,
        "protocol": "common-v1",
        "abi_version": 1,
        "build_id": digest,
        "dll": "../common/revival_graphics.dll",
    }
    for consumer in ("jme", "nes"):
        folder = runtime_root / consumer
        folder.mkdir(parents=True, exist_ok=True)
        target = folder / "graphics-runtime.json"
        target.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")

    hashes = {
        "schema": 1,
        "protocol": "common-v1",
        "abi_build_id": abi_build_id,
        "files": {
            "common/revival_graphics.dll": digest,
            "common/revival_graphics_jni.dll": sha256_file(shim),
        },
    }
    (runtime_root / "common" / "runtime-hashes.json").write_text(
        json.dumps(hashes, indent=2) + "\n", encoding="utf-8")


def validate_runtime_tree(runtime_root: Path) -> dict[str, str]:
    """Check both system manifests resolve one bounded, hash-matched DLL."""
    runtime_root = Path(runtime_root).resolve()
    common = runtime_root / "common"
    abi_file = common / "abi-build-id.txt"
    abi_build_id = abi_file.read_text(encoding="ascii").strip()
    if ABI_BUILD_ID.fullmatch(abi_build_id) is None:
        raise ValueError("Staged ABI build ID is malformed")
    core = common / "revival_graphics.dll"
    shim = common / "revival_graphics_jni.dll"
    hashes = json.loads((common / "runtime-hashes.json").read_text(encoding="utf-8"))
    if hashes.get("schema") != 1 or hashes.get("protocol") != "common-v1":
        raise ValueError("Runtime hashes manifest is incompatible")
    if hashes.get("abi_build_id") != abi_build_id:
        raise ValueError("Runtime hashes manifest has a different ABI build ID")
    expected = hashes.get("files")
    actual = {
        "common/revival_graphics.dll": sha256_file(core),
        "common/revival_graphics_jni.dll": sha256_file(shim),
    }
    if expected != actual:
        raise ValueError("Staged native DLL hashes do not match runtime-hashes.json")
    resolved = []
    for consumer in ("jme", "nes"):
        path = runtime_root / consumer / "graphics-runtime.json"
        manifest = GraphicsRuntimeResolver(runtime_root).resolve(path)
        if manifest.dll != core.resolve():
            raise ValueError(f"{consumer} manifest does not resolve to the one shared DLL")
        resolved.append(manifest.build_id)
    if resolved[0] != resolved[1]:
        raise ValueError("JME and NES manifests identify different graphics DLL bytes")
    return {"abi_build_id": abi_build_id, "dll_sha256": resolved[0], "jni_sha256": actual["common/revival_graphics_jni.dll"]}


def _archive_member(name: str) -> str:
    raw_parts = name.split("/")
    if not name or "\\" in name or name.startswith("/") or ":" in raw_parts[0]:
        raise ValueError("Runtime archive contains a non-portable path")
    if any(part in ("", ".", "..") for part in raw_parts):
        raise ValueError("Runtime archive contains a path traversal")
    parts = PurePosixPath(name).parts
    return "/".join(parts)


def validate_runtime_archive(archive: Path) -> dict[str, str]:
    """Validate a built common-runtime ZIP without extracting archive paths."""
    archive = Path(archive)
    with zipfile.ZipFile(archive) as package:
        infos = package.infolist()
        if len(infos) > MAX_ARCHIVE_ENTRIES:
            raise ValueError("Runtime archive has too many entries")
        normalized = [_archive_member(info.filename.rstrip("/") if info.is_dir() else info.filename)
                      for info in infos]
        names = [name for info, name in zip(infos, normalized) if not info.is_dir()]
        if len(names) != len({name.casefold() for name in names}):
            raise ValueError("Runtime archive contains duplicate paths")
        if sum(info.file_size for info in infos) > MAX_ARCHIVE_BYTES:
            raise ValueError("Runtime archive exceeds the uncompressed size limit")
        for info, name in zip(infos, normalized):
            mode = (info.external_attr >> 16) & 0xFFFF
            if mode & 0o170000 == 0o120000:
                raise ValueError("Runtime archive cannot contain symbolic links")
            lower = name.casefold()
            if lower.endswith((".nes", ".sav", ".srm", ".state")) or any(
                    part in {"library", "rendered-games", "saves"} for part in PurePosixPath(lower).parts):
                raise ValueError("Runtime archive cannot contain game or save data")

        prefix = PACKAGE_ROOT + "/"
        if any(not name.startswith(prefix) for name in names):
            raise ValueError("Runtime archive has files outside its named package folder")
        members = {name[len(prefix):]: prefix + name[len(prefix):] for name in names if name.startswith(prefix)}
        required = {
            "common/revival_graphics.dll", "common/revival_graphics_jni.dll",
            "common/abi-build-id.txt", "common/dependencies.json", "common/runtime-hashes.json",
            "jme/graphics-runtime.json", "nes/graphics-runtime.json", "LICENSE",
            "README.txt", "source/native-graphics-source.zip",
        }
        missing = required - members.keys()
        if missing:
            raise ValueError("Runtime archive is missing: " + ", ".join(sorted(missing)))

        dependencies = json.loads(package.read(members["common/dependencies.json"]))
        nes_runtime = dependencies.get("nes_runtime", {})
        if (dependencies.get("schema_version") != 1 or dependencies.get("target") != "windows-x64" or
                not isinstance(nes_runtime.get("commit"), str) or len(nes_runtime["commit"]) != 40 or
                any(char not in "0123456789abcdef" for char in nes_runtime["commit"])):
            raise ValueError("Runtime archive dependency pins are invalid")

        core_name = "common/revival_graphics.dll"
        jni_name = "common/revival_graphics_jni.dll"
        core_hash = hashlib.sha256(package.read(members[core_name])).hexdigest()
        jni_hash = hashlib.sha256(package.read(members[jni_name])).hexdigest()
        abi_id = package.read(members["common/abi-build-id.txt"]).decode("ascii").strip()
        if ABI_BUILD_ID.fullmatch(abi_id) is None:
            raise ValueError("Runtime archive ABI build ID is malformed")
        for consumer in ("jme", "nes"):
            doc = json.loads(package.read(members[f"{consumer}/graphics-runtime.json"]))
            if set(doc) != {"schema", "protocol", "abi_version", "build_id", "dll"}:
                raise ValueError(f"{consumer} graphics manifest fields are invalid")
            if doc["schema"] != 1 or doc["protocol"] != "common-v1" or doc["abi_version"] != 1:
                raise ValueError(f"{consumer} graphics manifest protocol is incompatible")
            if doc["dll"] != "../common/revival_graphics.dll" or doc["build_id"] != core_hash:
                raise ValueError(f"{consumer} graphics manifest does not identify the shared DLL")
        hash_doc = json.loads(package.read(members["common/runtime-hashes.json"]))
        if hash_doc.get("abi_build_id") != abi_id or hash_doc.get("files") != {
                "common/revival_graphics.dll": core_hash,
                "common/revival_graphics_jni.dll": jni_hash}:
            raise ValueError("Runtime archive DLL hashes do not match its hash manifest")

        with zipfile.ZipFile(package.open(members["source/native-graphics-source.zip"])) as sources:
            source_names = set(sources.namelist())
            if not {"native-graphics/CMakeLists.txt", "native-graphics/include/revival_graphics.h"} <= source_names:
                raise ValueError("Corresponding graphics source archive is incomplete")
    return {"abi_build_id": abi_id, "dll_sha256": core_hash, "jni_sha256": jni_hash}
