"""Validate and register source images and runtime capture events."""
from __future__ import annotations

import json
import os
from pathlib import Path, PurePosixPath
import uuid

from PIL import Image

from .contracts import SourceRecord, image_source_id
from .work_state import _Game


def read_source(path: Path, expected_id: str | None = None) -> Image.Image:
    with Image.open(path) as decoded:
        width, height = decoded.size
        if width <= 0 or height <= 0 or width * height > 4_194_304:
            raise ValueError("Source image dimensions are outside the supported limit")
        decoded.load()
        image = decoded.convert("RGBA")
    source_id = image_source_id(image)
    if expected_id is not None and source_id != expected_id:
        raise ValueError("Source pixels do not match their source ID")
    return image


def reconcile_sources(game: _Game) -> int:
    known = {source.source_id for source in game.store.list_sources()}
    registered = 0
    directory = game.workspace / "original-assets"
    if directory.is_dir():
        for path in sorted(directory.glob("*.png")):
            if len(path.stem) != 64 or path.stem in known:
                continue
            try:
                resolved = path.resolve(strict=True)
                resolved.relative_to(game.workspace)
                image = read_source(resolved, path.stem)
                source = SourceRecord(path.stem, image.width, image.height,
                                      resolved.relative_to(game.workspace).as_posix(),
                                      "legacy-original")
                game.store.register_source(source)
                game.store.enqueue(source.source_id, game.recipe.key)
                known.add(source.source_id)
                registered += 1
            except (OSError, ValueError):
                continue
    return registered


def ingest_events(game: _Game, limit: int) -> int:
    event_dir = game.workspace / ".runtime" / "events"
    if not event_dir.is_dir():
        return 0
    accepted = 0
    for event_path in sorted(event_dir.glob("*.json"))[:limit]:
        try:
            event = json.loads(event_path.read_text(encoding="utf-8"))
            if (not isinstance(event, dict) or event.get("schema") != 1 or
                    event.get("type") != "capture-ready"):
                raise ValueError("Unsupported capture event")
            event_id = event.get("event_id")
            source_id = event.get("source_id")
            relative = event.get("source_path")
            source_parts = PurePosixPath(relative).parts if isinstance(relative, str) else ()
            if (not isinstance(source_id, str) or len(source_id) != 64 or
                    not source_parts or source_parts[0] not in ("original-assets", "captures") or
                    PurePosixPath(relative).stem != source_id):
                raise ValueError("Capture event source path is invalid")
            source_path = game.store.resolve_path(relative)
            source_path.resolve(strict=True).relative_to(game.workspace)
            image = read_source(source_path, source_id)
            if (event.get("width"), event.get("height")) != image.size:
                raise ValueError("Capture event dimensions do not match its image")
            origin = event.get("origin")
            if not isinstance(origin, str) or not origin or len(origin) > 64:
                raise ValueError("Capture event origin is invalid")
            source = SourceRecord(source_id, image.width, image.height, relative, origin)
            game.store.register_capture_event(event_id, source, game.recipe)
            event_path.unlink(missing_ok=True)
            accepted += 1
        except (OSError, ValueError, TypeError, KeyError):
            # A malformed service-owned event is isolated so it cannot block later events.
            rejected = game.workspace / ".runtime" / "rejected-events"
            rejected.mkdir(parents=True, exist_ok=True)
            try:
                os.replace(event_path, rejected / (event_path.name + "." + uuid.uuid4().hex))
            except OSError:
                pass
    return accepted
