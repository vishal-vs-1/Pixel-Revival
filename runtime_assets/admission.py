"""Use current game metrics to decide whether artwork may run during play."""
from __future__ import annotations

import json
import math
from statistics import median
import time

from .work_state import _Game


def can_admit_live(game: _Game, *, busy: bool, set_reason) -> bool:
    now = time.monotonic()
    if not game.live_enabled:
        set_reason(game, "Live image work paused by the user")
        return False
    if busy:
        set_reason(game, "Waiting for the current image job to finish")
        return False
    if game.metrics_path is None:
        set_reason(game, "Waiting for player performance data")
        return False
    if now - game.metric_checked_at >= 0.5:
        game.metric_checked_at = now
        try:
            report = json.loads(game.metrics_path.read_text(encoding="utf-8"))
            updated = float(report.get("updated", 0.0))
            values = {key: float(report.get(key, 0.0)) for key in
                      ("fps", "frame_p95_ms", "input_queue_p95_ms")}
            if any(not math.isfinite(value) or value < 0 for value in values.values()):
                raise ValueError("non-finite player metrics")
            game.metric_updated = updated
            game.metric_values = values
        except (OSError, ValueError, TypeError, json.JSONDecodeError):
            set_reason(game, "Waiting for fresh player performance data")
            return False
    report = game.metric_values or {}
    age = time.time() - game.metric_updated
    if age < -2.0 or age > 4.0 or report.get("fps", 0.0) <= 0 or report.get("frame_p95_ms", 0.0) <= 0:
        set_reason(game, "Performance data is stale; image work is deferred")
        return False
    if game.metric_updated != game.metric_sampled and not busy:
        game.metric_sampled = game.metric_updated
        if len(game.baseline_frames) < 4:
            game.baseline_frames.append(report["frame_p95_ms"])
            game.baseline_inputs.append(report["input_queue_p95_ms"])
            game.baseline_fps.append(report["fps"])
    if len(game.baseline_frames) < 4:
        set_reason(game, "Measuring game performance before live image work")
        return False
    baseline_frame = median(game.baseline_frames)
    baseline_input = median(game.baseline_inputs)
    baseline_fps = median(game.baseline_fps)
    if (report["frame_p95_ms"] > max(60.0, baseline_frame * 1.8 + 10.0) or
            report["input_queue_p95_ms"] > max(25.0, baseline_input * 2.0 + 10.0) or
            report["fps"] < max(10.0, baseline_fps * 0.60)):
        set_reason(game, "Frame or input latency is high; image work is deferred")
        return False
    if now - game.last_live_job_at < 8.0:
        cooldown = max(8.0, min(30.0, game.last_live_job_seconds * 2.0))
        if now - game.last_live_job_at < cooldown:
            set_reason(game, "Live image-work cooldown; saved artwork remains active")
            return False
    set_reason(game, "Performance headroom available for one small image")
    return True
