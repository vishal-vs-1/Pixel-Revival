"""Publish ready textures and legacy compatibility views for the players."""
from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import uuid

from PIL import Image

from .work_state import _Game


def publish_compatibility(game: _Game, *, limit: int, commit_lock):
    """Maintain the current player's legacy texture folder outside active play."""
    jobs = game.store.list_ready(game.recipe.key)
    marker = game.workspace / ".runtime" / "compatibility.json"
    if game.compat_recipe_key != game.recipe.key or game.compat_sources is None:
        game.compat_recipe_key, game.compat_sources = read_compatibility_state(
            game.workspace, game.recipe.key)
    already = game.compat_sources
    pending = []
    for job in jobs:
        if not job.relative_output:
            continue
        source = game.store.get_source(job.source_id)
        if source is None:
            continue
        directory = "textures" if game.recipe.algorithm == "scale2x-rgba" else "textures-ai"
        target = game.store.resolve_path(f"{directory}/{job.source_id}.png")
        if (job.source_id in already and target.is_file()) or (
                game.workspace / "textures-ui" / f"{job.source_id}.png").is_file():
            already.add(job.source_id)
            continue
        pending.append((job, target))
    for job, target in pending[:limit]:
        target.parent.mkdir(parents=True, exist_ok=True)
        temporary = target.with_name(target.name + "." + uuid.uuid4().hex + ".tmp")
        try:
            shutil.copyfile(game.store.resolve_path(job.relative_output), temporary)
            with commit_lock:
                if game.paused:
                    game.compat_pending = True
                    return
                os.replace(temporary, target)
                already.add(job.source_id)
        finally:
            temporary.unlink(missing_ok=True)
    remaining = [job for job, _ in pending[limit:]]
    if remaining:
        game.compat_pending = True
        return
    ready_ids = {job.source_id for job in jobs if job.relative_output}
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


def publish_ready(game: _Game):
    valid_sources = []
    for job in game.store.list_ready(game.recipe.key):
        if not job.relative_output:
            game.store.invalidate_ready(job.source_id, job.recipe_key)
            continue
        source = game.store.get_source(job.source_id)
        if source is None:
            game.store.invalidate_ready(job.source_id, job.recipe_key)
            continue
        path = game.store.resolve_path(job.relative_output)
        try:
            with Image.open(path) as image:
                image.verify()
            with Image.open(path) as image:
                is_valid = image.size == (source.width * game.recipe.scale,
                                          source.height * game.recipe.scale)
        except (OSError, ValueError):
            is_valid = False
        if is_valid:
            valid_sources.append(job.source_id)
        else:
            game.store.invalidate_ready(job.source_id, job.recipe_key)
    ready_dir = game.workspace / ".runtime" / "ready"
    ready_dir.mkdir(parents=True, exist_ok=True)
    manifest = ready_dir / (game.recipe.key + ".json")
    try:
        previous = json.loads(manifest.read_text(encoding="utf-8"))
        game.generation = max(game.generation, int(previous.get("generation", 0)))
    except (OSError, ValueError, TypeError):
        pass
    game.generation += 1
    payload = {"schema": 1, "generation": game.generation,
               "recipe_key": game.recipe.key, "scale": game.recipe.scale,
               "sources": sorted(valid_sources)}
    temporary = manifest.with_name(manifest.name + "." + uuid.uuid4().hex + ".tmp")
    try:
        temporary.write_text(json.dumps(payload, sort_keys=True, separators=(",", ":")),
                             encoding="utf-8")
        os.replace(temporary, manifest)
    finally:
        temporary.unlink(missing_ok=True)
