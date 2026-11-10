"""Deterministic, immutable Mesen HD-pack generations."""

from __future__ import annotations

from dataclasses import dataclass
import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import uuid

import numpy as np
from PIL import Image

from runtime_assets.contracts import Recipe
from runtime_assets.store import AssetStore

from .bindings import NesBindingStore

MAX_PAGE_SIDE = 2048
DEFAULT_MAX_BYTES = 64 * 1024 * 1024


@dataclass(frozen=True)
class PackGeneration:
    generation: str
    directory: Path
    manifest: Path
    pack_scale: int
    source_count: int


def _premultiplied_resize(image: Image.Image, size: tuple[int, int]) -> Image.Image:
    pixels = np.asarray(image.convert("RGBA"), dtype=np.float32)
    alpha = pixels[:, :, 3:4] / 255.0
    premultiplied = pixels[:, :, :3] * alpha
    channels = []
    for index in range(3):
        channel = Image.fromarray(np.clip(premultiplied[:, :, index], 0, 255).astype(np.uint8), "L")
        channels.append(np.asarray(channel.resize(size, Image.Resampling.LANCZOS), dtype=np.float32))
    resized_alpha = np.asarray(
        Image.fromarray(pixels[:, :, 3].astype(np.uint8), "L").resize(size, Image.Resampling.LANCZOS),
        dtype=np.float32,
    )
    result = np.zeros((size[1], size[0], 4), dtype=np.uint8)
    divisor = np.maximum(resized_alpha / 255.0, 1e-6)
    for index, channel in enumerate(channels):
        result[:, :, index] = np.clip(channel / divisor, 0, 255).astype(np.uint8)
    result[:, :, 3] = np.clip(resized_alpha, 0, 255).astype(np.uint8)
    result[result[:, :, 3] == 0, :3] = 0
    return Image.fromarray(result, "RGBA")


def _pack_image(image: Image.Image, recipe: Recipe, pack_scale: int) -> Image.Image:
    expected = 8 * recipe.scale
    if image.size != (expected, expected):
        raise ValueError("Ready NES artwork has an unexpected tile size")
    target = 8 * pack_scale
    if recipe.scale == pack_scale:
        return image.convert("RGBA")
    if recipe.scale != 4 or pack_scale != 2:
        raise ValueError("NES pack conversion requires a cached 4x source")
    if recipe.algorithm == "scale2x-rgba":
        return image.convert("RGBA").resize((target, target), Image.Resampling.NEAREST)
    return _premultiplied_resize(image, (target, target))


def _selector_tag(selector: str) -> str:
    kind, values = selector.split(":", 1)
    tile, palette = values.split(",", 1)
    if kind == "rom":
        return f"{tile.upper()},{palette.upper()}"
    if kind == "ram" and len(tile) == 32:
        return f"{tile.upper()},{palette.upper()},0,-1"
    raise ValueError("Unsupported NES binding selector")


class NesPackGenerator:
    """Build a Mesen pack from ready shared-worker outputs."""

    def __init__(self, workspace: Path, *, max_bytes: int = DEFAULT_MAX_BYTES):
        self.workspace = Path(workspace).resolve()
        self.max_bytes = max(1024, int(max_bytes))
        self._packs = self.workspace / "nes" / "packs"
        self._pointer = self.workspace / ".runtime" / "nes-pack.json"

    def _current_generation(self, recipe: Recipe, pack_scale: int) -> PackGeneration | None:
        try:
            pointer = json.loads(self._pointer.read_text(encoding="utf-8"))
            generation = pointer["generation"]
            expected_relative = f"nes/packs/{generation}/manifest.json"
            if (pointer.get("schema") != 1 or pointer.get("recipe_key") != recipe.key or
                    pointer.get("pack_scale") != pack_scale or
                    pointer.get("manifest") != expected_relative or
                    not isinstance(generation, str) or len(generation) != 32 or
                    any(char not in "0123456789abcdef" for char in generation)):
                return None
            manifest_path = self.workspace / expected_relative
            if (manifest_path.is_symlink() or not manifest_path.is_file() or
                    not manifest_path.resolve().is_relative_to(self.workspace)):
                return None
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            if (manifest.get("generation") != generation or
                    manifest.get("recipe_key") != recipe.key or
                    manifest.get("pack_scale") != pack_scale):
                return None
            return PackGeneration(generation, manifest_path.parent, manifest_path,
                                  pack_scale, len(manifest.get("tiles", ())))
        except (OSError, KeyError, TypeError, ValueError, json.JSONDecodeError):
            return None

    def ensure_capture_pack(self, recipe: Recipe, *, pack_scale: int = 4) -> PackGeneration:
        """Keep an Enhanced launch possible before its first captured tiles are ready."""
        if pack_scale not in (2, 4):
            raise ValueError("NES pack scale must be 2 or 4")
        current = self._current_generation(recipe, pack_scale)
        if current is not None:
            return current

        # This selector is outside the NES tile/palette value range, so the
        # transparent bootstrap entry cannot replace visible game artwork.
        selector = "rom:7FFFFFFF,FFFFFFFF"
        binding_id = selector + "|bootstrap-noop-v1"
        source_id = hashlib.sha256((recipe.key + ":" + binding_id).encode("ascii")).hexdigest()
        seed = {
            "schema": 1,
            "recipe_key": recipe.key,
            "pack_scale": pack_scale,
            "bindings": [{"binding_id": binding_id, "selector": selector,
                          "source_id": source_id}],
        }
        generation = hashlib.sha256(
            json.dumps(seed, sort_keys=True, separators=(",", ":")).encode("utf-8")
        ).hexdigest()[:32]
        target = self._packs / generation
        manifest_path = target / "manifest.json"
        if target.exists():
            if not manifest_path.is_file() or manifest_path.is_symlink():
                raise ValueError("Incomplete NES bootstrap pack already occupies its generation")
            pack = PackGeneration(generation, target, manifest_path, pack_scale, 1)
            self._publish_pointer(pack, recipe)
            return pack

        self._packs.mkdir(parents=True, exist_ok=True)
        staging = self._packs / f".staging-{uuid.uuid4().hex}"
        staging.mkdir(parents=False, exist_ok=False)
        try:
            tile_size = 8 * pack_scale
            Image.new("RGBA", (tile_size, tile_size), (0, 0, 0, 0)).save(
                staging / "page-000.png", format="PNG")
            (staging / "hires.txt").write_text(
                f"<ver>109\n<scale>{pack_scale}\n<img>page-000.png\n"
                f"<tile>0,7FFFFFFF,FFFFFFFF,0,0,1,N\n", encoding="ascii")
            manifest = dict(seed)
            manifest.update({
                "generation": generation,
                "pages": ["page-000.png"],
                "tiles": [{"binding_id": binding_id, "selector": selector,
                           "source_id": source_id, "page": 0, "x": 0, "y": 0,
                           "size": tile_size}],
            })
            (staging / "manifest.json").write_text(
                json.dumps(manifest, sort_keys=True, separators=(",", ":")), encoding="utf-8")
            if sum(path.stat().st_size for path in staging.iterdir()) > self.max_bytes:
                raise ValueError("NES bootstrap pack exceeds the configured byte limit")
            os.replace(staging, target)
            pack = PackGeneration(generation, target, manifest_path, pack_scale, 1)
            self._publish_pointer(pack, recipe)
            return pack
        except BaseException:
            shutil.rmtree(staging, ignore_errors=True)
            raise

    def _publish_pointer(self, pack: PackGeneration, recipe: Recipe) -> None:
        self._pointer.parent.mkdir(parents=True, exist_ok=True)
        if self._pointer.is_symlink():
            raise ValueError("NES pack pointer cannot be a symbolic link")
        pointer_temp = self._pointer.with_name(self._pointer.name + f".{uuid.uuid4().hex}.tmp")
        try:
            pointer_temp.write_text(json.dumps({
                "schema": 1, "generation": pack.generation,
                "manifest": f"nes/packs/{pack.generation}/manifest.json",
                "recipe_key": recipe.key, "pack_scale": pack.pack_scale,
            }, sort_keys=True, separators=(",", ":")), encoding="utf-8")
            os.replace(pointer_temp, self._pointer)
        finally:
            pointer_temp.unlink(missing_ok=True)

    def _entries(self, store: AssetStore, recipe: Recipe, pack_scale: int):
        jobs = {job.source_id: job for job in store.list_ready(recipe.key)}
        bindings = NesBindingStore(self.workspace).list_bindings()
        entries = []
        for binding in bindings:
            job = jobs.get(binding.source_id)
            if job is None or not job.relative_output:
                continue
            path = store.resolve_path(job.relative_output)
            try:
                with Image.open(path) as opened:
                    opened.load()
                    image = _pack_image(opened, recipe, pack_scale)
            except (OSError, ValueError):
                continue
            entries.append((binding, image))
        return entries

    @staticmethod
    def _manifest_seed(recipe: Recipe, pack_scale: int, entries):
        return {
            "schema": 1,
            "recipe_key": recipe.key,
            "pack_scale": pack_scale,
            "bindings": [
                {"binding_id": binding.binding_id, "selector": binding.selector,
                 "source_id": binding.source_id}
                for binding, _image in entries
            ],
        }

    def publish(self, store: AssetStore, recipe: Recipe, *, pack_scale: int = 4) -> PackGeneration | None:
        if pack_scale not in (2, 4):
            raise ValueError("NES pack scale must be 2 or 4")
        entries = self._entries(store, recipe, pack_scale)
        if not entries:
            return None
        seed = self._manifest_seed(recipe, pack_scale, entries)
        generation = hashlib.sha256(
            json.dumps(seed, sort_keys=True, separators=(",", ":")).encode("utf-8")
        ).hexdigest()[:32]
        target = self._packs / generation
        manifest_path = target / "manifest.json"
        if manifest_path.is_file():
            return PackGeneration(generation, target, manifest_path, pack_scale, len(entries))
        staging = self._packs / f".staging-{uuid.uuid4().hex}"
        staging.mkdir(parents=True, exist_ok=False)
        try:
            pages = []
            page_limit = (MAX_PAGE_SIDE // (8 * pack_scale)) ** 2
            for page_number in range(math.ceil(len(entries) / page_limit)):
                subset = entries[page_number * page_limit:(page_number + 1) * page_limit]
                tile_size = 8 * pack_scale
                columns = max(1, min(MAX_PAGE_SIDE // tile_size, math.ceil(math.sqrt(len(subset)))))
                rows = math.ceil(len(subset) / columns)
                page = Image.new("RGBA", (columns * tile_size, rows * tile_size), (0, 0, 0, 0))
                for offset, (binding, image) in enumerate(subset):
                    x = (offset % columns) * tile_size
                    y = (offset // columns) * tile_size
                    page.paste(image, (x, y), image)
                    pages.append((binding, page_number, x, y))
                page.save(staging / f"page-{page_number:03d}.png", format="PNG")
            lines = ["<ver>109", f"<scale>{pack_scale}"]
            for page_number in range(math.ceil(len(entries) / page_limit)):
                lines.append(f"<img>page-{page_number:03d}.png")
            for binding, page_number, x, y in pages:
                lines.append(f"<tile>{page_number},{_selector_tag(binding.selector)},"
                             f"{x},{y},1,N")
            (staging / "hires.txt").write_text("\n".join(lines) + "\n", encoding="ascii")
            manifest = dict(seed)
            manifest.update({
                "generation": generation,
                "pages": [f"page-{index:03d}.png"
                          for index in range(math.ceil(len(entries) / page_limit))],
                "tiles": [{"binding_id": binding.binding_id, "selector": binding.selector,
                           "source_id": binding.source_id, "page": page, "x": x, "y": y,
                           "size": 8 * pack_scale}
                          for binding, page, x, y in pages],
            })
            (staging / "manifest.json").write_text(
                json.dumps(manifest, sort_keys=True, separators=(",", ":")), encoding="utf-8")
            total_bytes = sum(path.stat().st_size for path in staging.iterdir())
            if total_bytes > self.max_bytes:
                raise ValueError("NES pack exceeds the configured byte limit")
            self._packs.mkdir(parents=True, exist_ok=True)
            os.replace(staging, target)
            self._pointer.parent.mkdir(parents=True, exist_ok=True)
            pointer_temp = self._pointer.with_name(self._pointer.name + f".{uuid.uuid4().hex}.tmp")
            pointer_temp.write_text(json.dumps({
                "schema": 1, "generation": generation,
                "manifest": f"nes/packs/{generation}/manifest.json",
                "recipe_key": recipe.key, "pack_scale": pack_scale,
            }, sort_keys=True, separators=(",", ":")), encoding="utf-8")
            os.replace(pointer_temp, self._pointer)
            return PackGeneration(generation, target, target / "manifest.json",
                                  pack_scale, len(entries))
        except BaseException:
            shutil.rmtree(staging, ignore_errors=True)
            raise
