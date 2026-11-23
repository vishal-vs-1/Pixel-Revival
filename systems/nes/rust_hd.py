"""Load a Mesen HD pack into the Rust compositor.

Reads `.runtime/nes-pack.json` plus
`nes/packs/<generation>/manifest.json` and page PNGs, and feeds
each tile's art to an `RsHdTable` through the `mesen-player` FFI. Selectors
use the same `rom:INDEX,PALETTE` / `ram:PATTERN,PALETTE` grammar as
`systems/nes/pack.py`; RAM tiles are currently skipped because the v1
recorder reports them as misses.
"""

from __future__ import annotations

import json
import os
from dataclasses import dataclass
from pathlib import Path

from PIL import Image

from .pack import PRIORITY_SAFE_PACK

MAX_POINTER_BYTES = 65536


@dataclass(frozen=True)
class HdTileEntry:
    sprite: bool
    chr_index: int
    palette: int
    image: Image.Image  # RGBA, size x size


def parse_selector(selector: str) -> tuple[str, int | bytes, int] | None:
    """Return (kind, key, palette) or None when the entry is unusable.

    kind is "rom" (key is the CHR tile number) or "ram" (key is 16 raw
    pattern bytes). Mirrors the validation in `pack._selector_tag`.
    """
    try:
        kind, values = selector.split(":", 1)
        tile, palette_text = values.split(",", 1)
        palette = int(palette_text, 16)
        if not 0 <= palette <= 0xFFFF_FFFF:
            return None
        if kind == "rom":
            return ("rom", int(tile, 16), palette)
        if kind == "ram" and len(tile) == 32:
            return ("ram", bytes.fromhex(tile), palette)
    except (ValueError, AttributeError):
        pass
    return None


def _read_pointer(workspace: Path):
    try:
        pointer = json.loads((workspace / ".runtime" / "nes-pack.json")
                             .read_text(encoding="utf-8"))
        generation = pointer.get("generation")
        manifest = pointer.get("manifest")
        scale = pointer.get("pack_scale")
        if (not isinstance(generation, str) or len(generation) != 32 or
                any(c not in "0123456789abcdef" for c in generation) or
                manifest != f"nes/packs/{generation}/manifest.json" or
                scale not in (2, 4)):
            return None
        manifest_path = workspace / manifest
        if manifest_path.is_symlink() or not manifest_path.is_file():
            return None
        document = json.loads(manifest_path.read_text(encoding="utf-8"))
        if (document.get("generation") != generation or
                document.get("pack_scale") != scale or
                not isinstance(document.get("tiles"), list) or
                not isinstance(document.get("pages"), list)):
            return None
        return document, manifest_path.parent, scale
    except (OSError, ValueError, TypeError, KeyError, json.JSONDecodeError):
        return None


def load_pack_tiles(workspace: Path) -> tuple[list[HdTileEntry], int] | None:
    """Read the current pack generation. Returns (entries, scale) or None."""
    resolved = _read_pointer(Path(workspace))
    if resolved is None:
        return None
    document, folder, scale = resolved
    tile_size = 8 * scale
    pages: dict[str, Image.Image] = {}
    entries: list[HdTileEntry] = []
    for tile in document["tiles"]:
        try:
            selector = tile["selector"]
            page, x, y, size = tile["page"], tile["x"], tile["y"], tile["size"]
            parsed = parse_selector(selector)
            if (parsed is None or parsed[0] != "rom" or size != tile_size or
                    not isinstance(tile.get("binding_id"), str)):
                continue
            _, chr_index, palette = parsed
            name = document["pages"][page] if isinstance(page, int) else None
            if (not isinstance(name, str) or Path(name).name != name
                    or Path(name).is_absolute() or any(ch in name for ch in "/\\:")):
                return None
            if name not in pages:
                path = folder / name
                try:
                    resolved = path.resolve()
                    if (path.is_symlink() or not resolved.is_relative_to(folder.resolve())
                            or not resolved.is_file()):
                        return None
                except (OSError, ValueError):
                    return None
                with Image.open(resolved) as opened:
                    opened.load()
                    pages[name] = opened.convert("RGBA")
            sheet = pages[name]
            art = sheet.crop((x, y, x + size, y + size))
            entries.append(HdTileEntry(
                sprite=tile["binding_id"].endswith("|sprite"),
                chr_index=chr_index, palette=palette, image=art))
        except (KeyError, TypeError, ValueError, OSError):
            continue
    return entries, scale


def owned_path(path: Path, root: Path, *, directory: bool = False) -> Path:
    """Resolve a path that must stay inside its private workspace root."""
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


def read_pack_snapshot(workspace: Path) -> tuple[Path, int]:
    """Validate the staged pack pointer and return its manifest and scale."""
    workspace = Path(workspace)
    pointer = workspace / ".runtime" / "nes-pack.json"
    try:
        raw = pointer.read_bytes()
        if len(raw) > MAX_POINTER_BYTES:
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
        if document.get("background_priority") != PRIORITY_SAFE_PACK:
            raise ValueError("NES artwork needs rebuilding to preserve sprite priority")
        return resolved, int(document["pack_scale"])
    except (OSError, KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
        raise ValueError("NES pack snapshot is invalid") from error
