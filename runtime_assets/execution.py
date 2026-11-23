"""Run artwork backends and validate and save their results."""
from __future__ import annotations

import json
import os
from pathlib import Path
import tempfile
import uuid

from PIL import Image

from .work_state import _Work


def execute(work: _Work, backend_factory, read_source) -> str:
    store, source, recipe = work.game.store, work.source, work.recipe
    source_path = store.resolve_path(source.relative_path)
    image = read_source(source_path, source.source_id)
    if image.size != (source.width, source.height):
        raise ValueError("Source dimensions changed after registration")
    backend = backend_factory(recipe)
    temporary_root = work.game.workspace / ".runtime" / "tmp"
    temporary_root.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="job-", dir=temporary_root) as workdir:
        raw = backend.enhance(image, recipe, Path(workdir))
        result = raw if raw.mode == "RGBA" else raw.convert("RGBA")
        _publish_execution_report(work.game.workspace,Path(workdir))
        return _save_result(work, result)


def execute_batch(items: tuple[_Work, ...], backend, read_source,
                  save_workers: int = 1) -> list[str]:
    first = items[0]
    images = []
    for work in items:
        if work.game is not first.game or work.recipe != first.recipe:
            raise ValueError("A model batch must share one workspace and recipe")
        image = read_source(work.game.store.resolve_path(work.source.relative_path), work.source.source_id)
        if image.size != (work.source.width, work.source.height):
            raise ValueError("Source dimensions changed after registration")
        images.append(image)
    temporary_root = first.game.workspace / ".runtime" / "tmp"
    temporary_root.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="batch-", dir=temporary_root) as workdir:
        results = list(backend.enhance_many(images, first.recipe, Path(workdir)))
        if len(results) != len(items):
            raise ValueError("Backend returned an unexpected batch size")
        _publish_execution_report(first.game.workspace,Path(workdir))
        workers = max(1, min(int(save_workers), len(items)))
        # Resolve targets before threading: concurrent Path.resolve() calls on
        # Windows can return an extended \\?\ form that fails the workspace
        # containment check. Resolution is also the expensive part, so one
        # serial pass is both safer and faster than one call per save thread.
        pairs = []
        for work, result in zip(items, results):
            relative = f"textures-cache/{first.recipe.key}/{work.source.source_id}.png"
            target = first.game.store.resolve_path(relative)
            target.parent.mkdir(parents=True, exist_ok=True)
            pairs.append((work, result, target))
        if workers == 1:
            return [_save_result(
                        work, result if result.mode == "RGBA" else result.convert("RGBA"),
                        target)
                    for work, result, target in pairs]
        # Saves spend most of their time in per-file filesystem/AV latency and
        # zlib, so a small thread pool overlaps them without extra processes.
        from concurrent.futures import ThreadPoolExecutor
        with ThreadPoolExecutor(max_workers=workers,
                                thread_name_prefix="asset-save") as saves:
            return list(saves.map(_save_pair, pairs))


def _save_pair(pair) -> str:
    work, result, target = pair
    return _save_result(
        work, result if result.mode == "RGBA" else result.convert("RGBA"), target)


def _publish_execution_report(workspace: Path,workdir: Path):
    source=workdir/'logs/last-conversion-gpu.json'
    try:
        if not source.is_file() or source.stat().st_size>32_768:return
        report=json.loads(source.read_text(encoding='utf-8'))
        if not isinstance(report,dict):return
        destination=workspace/'logs/last-conversion-gpu.json'
        destination.parent.mkdir(parents=True,exist_ok=True)
        temporary=destination.with_name('conversion-'+uuid.uuid4().hex+'.tmp')
        try:
            temporary.write_text(json.dumps(report,indent=2),encoding='utf-8')
            os.replace(temporary,destination)
        finally:temporary.unlink(missing_ok=True)
    except (OSError,ValueError):pass


def _save_result(work: _Work, result: Image.Image, target: Path | None = None) -> str:
    store, source, recipe = work.game.store, work.source, work.recipe
    expected = (source.width * recipe.scale, source.height * recipe.scale)
    if result.size != expected:
        raise ValueError(f"Backend returned {result.size}; expected {expected}")
    relative = f"textures-cache/{recipe.key}/{source.source_id}.png"
    target = target or store.resolve_path(relative)
    target.parent.mkdir(parents=True, exist_ok=True)
    temporary = target.with_name(target.name + "." + work.lease.token + ".tmp")
    try:
        result.save(temporary, format="PNG")
        # One full decode replaces verify() plus a second open: same check, half the reads.
        with Image.open(temporary) as checked:
            checked.load()
            if checked.size != expected:
                raise ValueError("Encoded enhancement has unexpected dimensions")
        os.replace(temporary, target)
    finally:
        temporary.unlink(missing_ok=True)
    return relative
