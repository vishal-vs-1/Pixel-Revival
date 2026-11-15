"""Validate and stage an immutable HD pack for an embedded session."""
from __future__ import annotations

import json
import os
from pathlib import Path
import re
import shutil
import uuid
from ..pack import _selector_tag


MAX_PACK_BYTES = 64 * 1024 * 1024


_HEX32 = re.compile(r"[0-9a-f]{32}\Z")


_HEX64 = re.compile(r"[0-9a-f]{64}\Z")


_PAGE = re.compile(r"page-(\d{3,4})\.png\Z")


def _reject_duplicate_keys(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("NES pack contains duplicate JSON fields")
        result[key] = value
    return result


def _regular_owned_file(path: Path, root: Path) -> Path:
    path = Path(path)
    root = Path(root).resolve()
    if path.is_symlink() or path.is_junction():
        raise ValueError("NES pack files cannot be links")
    resolved = path.resolve(strict=True)
    if not resolved.is_relative_to(root) or not resolved.is_file():
        raise ValueError("NES pack file escapes its generation")
    current = root
    for part in resolved.relative_to(root).parts:
        current = current / part
        if current.is_symlink() or current.is_junction():
            raise ValueError("NES pack paths cannot traverse links")
    return resolved


def _pack_files(manifest_path: Path, workspace: Path, expected_scale: int):
    workspace = Path(workspace).resolve()
    manifest_path = Path(manifest_path)
    generation_root = manifest_path.parent
    if generation_root.is_symlink() or generation_root.is_junction():
        raise ValueError("NES pack generation cannot be a link")
    generation_root = generation_root.resolve(strict=True)
    if manifest_path.name != "manifest.json" or not generation_root.is_relative_to(workspace / "nes" / "packs"):
        raise ValueError("NES pack manifest is outside its immutable generation")
    current = workspace
    for part in generation_root.relative_to(workspace).parts:
        current = current / part
        if current.is_symlink() or current.is_junction():
            raise ValueError("NES pack paths cannot traverse links")
    if not _HEX32.fullmatch(generation_root.name):
        raise ValueError("NES pack generation identity is invalid")
    manifest_path = _regular_owned_file(manifest_path, generation_root)
    with manifest_path.open("rb") as stream:
        manifest_bytes = stream.read(MAX_PACK_BYTES + 1)
    if len(manifest_bytes) > MAX_PACK_BYTES:
        raise ValueError("NES pack manifest exceeds 64 MiB")
    data = json.loads(manifest_bytes.decode("utf-8"), object_pairs_hook=_reject_duplicate_keys)
    if (not isinstance(data, dict) or set(data) != {
            "schema", "recipe_key", "pack_scale", "bindings", "generation", "pages", "tiles"} or
            data["schema"] != 1 or data["generation"] != generation_root.name or
            data["pack_scale"] != expected_scale or expected_scale not in (2, 4) or
            not isinstance(data["recipe_key"], str) or not 1 <= len(data["recipe_key"]) <= 256):
        raise ValueError("NES pack manifest does not match the selected session")
    pages = data["pages"]
    bindings = data["bindings"]
    tiles = data["tiles"]
    if (not isinstance(pages, list) or not 1 <= len(pages) <= 4096 or
            not isinstance(bindings, list) or not 1 <= len(bindings) <= 65536 or
            not isinstance(tiles, list) or len(tiles) != len(bindings)):
        raise ValueError("NES pack page or tile counts exceed supported limits")
    expected_pages = [f"page-{index:03d}.png" for index in range(len(pages))]
    if pages != expected_pages:
        raise ValueError("NES pack page names are invalid or out of order")
    by_id = {}
    for binding in bindings:
        if (not isinstance(binding, dict) or set(binding) != {"binding_id", "selector", "source_id"} or
                not all(isinstance(binding[key], str) for key in ("binding_id", "selector", "source_id")) or
                not 1 <= len(binding["binding_id"]) <= 256 or
                not _HEX64.fullmatch(binding["source_id"]) or binding["binding_id"] in by_id):
            raise ValueError("NES pack binding is invalid or duplicated")
        _selector_tag(binding["selector"])
        by_id[binding["binding_id"]] = binding

    scale_pixels = 8 * expected_scale
    tile_rows = []
    occupied = set()
    for tile in tiles:
        if (not isinstance(tile, dict) or set(tile) != {
                "binding_id", "selector", "source_id", "page", "x", "y", "size"}):
            raise ValueError("NES pack tile definition has an invalid shape")
        if not all(isinstance(tile[key], str) for key in ("binding_id", "selector", "source_id")):
            raise ValueError("NES pack tile identity fields must be text")
        binding = by_id.get(tile["binding_id"])
        if (binding is None or binding["selector"] != tile["selector"] or
                binding["source_id"] != tile["source_id"] or tile["size"] != scale_pixels or
                not all(type(tile[key]) is int for key in ("page", "x", "y")) or
                tile["page"] < 0 or tile["page"] >= len(pages) or tile["x"] < 0 or tile["y"] < 0 or
                tile["x"] % scale_pixels or tile["y"] % scale_pixels or
                (tile["page"], tile["x"], tile["y"]) in occupied):
            raise ValueError("NES pack tile does not match its binding or atlas position")
        occupied.add((tile["page"], tile["x"], tile["y"]))
        tile_rows.append(tile)
    if set(by_id) != {tile["binding_id"] for tile in tile_rows}:
        raise ValueError("NES pack tile definitions do not cover every binding")

    files = [("hires.txt", _regular_owned_file(generation_root / "hires.txt", generation_root))]
    page_sizes = []
    total = len(manifest_bytes)
    total += files[0][1].stat().st_size
    if total > MAX_PACK_BYTES:
        raise ValueError("NES pack exceeds the 64 MiB size limit")
    for name in pages:
        match = _PAGE.fullmatch(name)
        if match is None:
            raise ValueError("NES pack page filename is invalid")
        page_path = _regular_owned_file(generation_root / name, generation_root)
        size = page_path.stat().st_size
        total += size
        if size < 33:
            raise ValueError("NES pack image page is too small")
        with page_path.open("rb") as stream:
            header = stream.read(33)
        if (header[:8] != bytes((137, 80, 78, 71, 13, 10, 26, 10)) or
                header[8:16] != b"\x00\x00\x00\rIHDR" or
                header[24:29] != bytes((8, 6, 0, 0, 0))):
            raise ValueError("NES pack page is not a generated RGBA PNG")
        width = int.from_bytes(header[16:20], "big")
        height = int.from_bytes(header[20:24], "big")
        if (not 0 < width <= 2048 or not 0 < height <= 2048 or
                width % scale_pixels or height % scale_pixels):
            raise ValueError("NES pack page dimensions are invalid")
        page_sizes.append((width, height))
        files.append((name, page_path))
    if total > MAX_PACK_BYTES:
        raise ValueError("NES pack exceeds the 64 MiB size limit")
    for tile in tile_rows:
        width, height = page_sizes[tile["page"]]
        if tile["x"] > width - scale_pixels or tile["y"] > height - scale_pixels:
            raise ValueError("NES pack tile lies outside its atlas page")

    definition_path = files[0][1]
    definition = definition_path.read_bytes()
    if any(byte > 0x7f for byte in definition):
        raise ValueError("NES pack definition must be ASCII")
    lines = definition.decode("ascii").replace("\r\n", "\n").split("\n")
    if lines and lines[-1] == "":
        lines.pop()
    expected_lines = ["<ver>109", f"<scale>{expected_scale}", *(f"<img>{name}" for name in pages)]
    for tile in tile_rows:
        expected_lines.append(
            f"<tile>{tile['page']},{_selector_tag(tile['selector'])},{tile['x']},{tile['y']},1,N")
    if lines != expected_lines:
        raise ValueError("NES pack text definitions do not match the checked manifest")
    files[0] = (files[0][0], _regular_owned_file(definition_path, generation_root))
    return files


def _stage_pack(manifest_path: Path, workspace: Path, home: Path, rom_path: Path, scale: int) -> Path:
    files = _pack_files(manifest_path, workspace, scale)
    root = Path(home).resolve() / "HdPacks"
    root.mkdir(parents=True, exist_ok=True)
    if root.is_symlink() or root.is_junction():
        raise ValueError("Mesen HD pack folder cannot be a link")
    rom_name = Path(rom_path).stem
    if not rom_name or rom_name in (".", "..") or Path(rom_name).name != rom_name:
        raise ValueError("NES ROM name cannot be used as an HD pack folder")
    target = root / rom_name
    staging = root / (".staging-" + uuid.uuid4().hex)
    if target.exists() or target.is_symlink() or target.is_junction():
        raise ValueError("Mesen HD pack destination already exists")
    staging.mkdir(parents=False, exist_ok=False)
    try:
        for name, source in files:
            output = staging / name
            with source.open("rb") as input_stream, output.open("xb") as output_stream:
                shutil.copyfileobj(input_stream, output_stream, 64 * 1024)
                output_stream.flush()
                os.fsync(output_stream.fileno())
        os.replace(staging, target)
        return target
    except BaseException:
        if staging.exists() and staging.parent.resolve() == root.resolve():
            shutil.rmtree(staging)
        raise
