"""Publish ready textures and legacy compatibility views for the players."""
from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import uuid

from PIL import Image

from .work_state import _Game


def _names(folder: Path) -> set[str]:
    """One directory scan replaces thousands of individual existence checks."""
    try:
        return {entry.name for entry in os.scandir(folder)}
    except OSError:
        return set()


def _present(store, relative: str, prefix: str, names: set[str]) -> bool:
    """Match the usual cache layout without a filesystem resolve per image."""
    if relative.startswith(prefix):
        name = relative[len(prefix):]
        if name and "/" not in name:
            return name in names
    return store.resolve_path(relative).is_file()


def publish_compatibility(game: _Game, *, limit: int, commit_lock):
    """Maintain the current player's legacy texture folder outside active play."""
    rows = game.store.list_ready_details(game.recipe.key)
    directory = {"scale2x-rgba": "textures", "pixel-redraw": "textures-redraw"}.get(
        game.recipe.algorithm, "textures-ai")
    target_dir = game.store.resolve_path(directory)
    target_names = _names(target_dir)
    ui_names = _names(game.workspace / "textures-ui")
    marker = game.workspace / ".runtime" / "compatibility.json"
    if game.compat_recipe_key != game.recipe.key or game.compat_sources is None:
        game.compat_recipe_key, game.compat_sources = read_compatibility_state(
            game.workspace, game.recipe.key)
    already = game.compat_sources
    pending = []
    # One joined query replaces a source lookup per ready image; on large
    # catalogues that lookup alone stalled the coordinator for half a minute.
    for row in rows:
        if not row.relative_output or row.width is None:
            continue
        name = f"{row.source_id}.png"
        # Presence is enough to skip; requiring the compatibility marker too
        # restarted the whole copy from row zero whenever the app closed early.
        if name in target_names or name in ui_names:
            already.add(row.source_id)
            continue
        pending.append((row, target_dir / name))
    for row, target in pending[:limit]:
        target.parent.mkdir(parents=True, exist_ok=True)
        temporary = target.with_name(target.name + "." + uuid.uuid4().hex + ".tmp")
        try:
            shutil.copyfile(game.store.resolve_path(row.relative_output), temporary)
            with commit_lock:
                if game.paused:
                    game.compat_pending = True
                    return
                os.replace(temporary, target)
                already.add(row.source_id)
        finally:
            temporary.unlink(missing_ok=True)
    remaining = pending[limit:]
    if remaining:
        game.compat_pending = True
        return
    ready_ids = {row.source_id for row in rows if row.relative_output}
    game.compat_pending = not ready_ids.issubset(already)
    if not game.compat_pending:
        marker.parent.mkdir(parents=True, exist_ok=True)
        temporary = marker.with_name(marker.name + "." + uuid.uuid4().hex + ".tmp")
        payload = {"schema": 1, "recipe_key": game.recipe.key,
                   "sources": sorted(ready_ids)}
        try:
            temporary.write_text(json.dumps(payload, sort_keys=True, separators=(",", ":")),
                                 encoding="utf-8")
            os.replace(temporary, marker)
            game.compat_recipe_key = game.recipe.key
            game.compat_sources = set(ready_ids)
        finally:
            temporary.unlink(missing_ok=True)


def read_compatibility_state(workspace: Path, recipe_key: str) -> tuple[str, set[str]]:
    marker = workspace / ".runtime" / "compatibility.json"
    try:
        saved = json.loads(marker.read_text(encoding="utf-8"))
        if saved.get("recipe_key") == recipe_key and isinstance(saved.get("sources"), list):
            return recipe_key, {value for value in saved["sources"] if isinstance(value, str)}
    except (OSError, ValueError, TypeError):
        pass
    return "", set()


_VERIFY_SAMPLE = 64


def _verify_output(game: _Game, row) -> bool:
    """Fully decode one cached output; requeue it when the file is unusable."""
    try:
        with Image.open(game.store.resolve_path(row.relative_output)) as image:
            image.load()
            is_valid = image.size == (row.width * game.recipe.scale,
                                      row.height * game.recipe.scale)
    except (OSError, ValueError):
        is_valid = False
    if not is_valid:
        game.store.invalidate_ready(row.source_id, game.recipe.key)
    return is_valid


def publish_ready(game: _Game):
    """Record the verified ready set without re-checking every file each start."""
    rows = game.store.list_ready_details(game.recipe.key)
    current = {}
    for row in rows:
        if not row.relative_output or row.width is None or row.height is None:
            game.store.invalidate_ready(row.source_id, game.recipe.key)
            continue
        current[row.source_id] = row
    ready_dir = game.workspace / ".runtime" / "ready"
    manifest = ready_dir / (game.recipe.key + ".json")
    try:
        previous = json.loads(manifest.read_text(encoding="utf-8"))
        raw_sources = previous.get("sources", [])
        previous_sources = {v for v in raw_sources if isinstance(v, str)} if isinstance(raw_sources, list) else set()
        game.generation = max(game.generation, int(previous.get("generation", 0)))
    except (OSError, ValueError, TypeError, AttributeError):
        previous_sources = set()
    current_ids = set()
    prefix = f"textures-cache/{game.recipe.key}/"
    cache_names = _names(game.store.resolve_path(prefix))
    for source_id, row in current.items():
        if not _present(game.store, row.relative_output, prefix, cache_names):
            game.store.invalidate_ready(source_id, game.recipe.key)
            continue
        current_ids.add(source_id)
    # Outputs are verified when written, so a publish decodes only a bounded
    # sample. A first publish on a large catalogue must never stall startup.
    fresh = sorted(current_ids - previous_sources)
    trusted = sorted(current_ids & previous_sources)
    sample = trusted[game.ready_sample_cursor:game.ready_sample_cursor + _VERIFY_SAMPLE]
    game.ready_sample_cursor = (game.ready_sample_cursor + len(sample)) % max(1, len(trusted))
    valid_sources = set(current_ids)
    for source_id in fresh[:_VERIFY_SAMPLE] + sample:
        if not _verify_output(game, current[source_id]):
            valid_sources.discard(source_id)
    if valid_sources == previous_sources and manifest.is_file():
        return
    game.generation += 1
    payload = {"schema": 1, "generation": game.generation,
               "recipe_key": game.recipe.key, "scale": game.recipe.scale,
               "sources": sorted(valid_sources)}
    ready_dir.mkdir(parents=True, exist_ok=True)
    temporary = manifest.with_name(manifest.name + "." + uuid.uuid4().hex + ".tmp")
    try:
        temporary.write_text(json.dumps(payload, sort_keys=True, separators=(",", ":")),
                             encoding="utf-8")
        os.replace(temporary, manifest)
    finally:
        temporary.unlink(missing_ok=True)
