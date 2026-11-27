"""Internal state shared by the artwork scheduler and workers."""
from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path

from .contracts import Recipe, SourceRecord
from .integration import WorkspaceIntegration
from .store import AssetStore, JobLease


@dataclass
class _Game:
    workspace: Path
    store: AssetStore
    recipe: Recipe
    integration: WorkspaceIntegration | None = None
    paused: bool = False
    generation: int = 0
    dirty_at: float | None = None
    next_scan: float = 0.0
    next_recover: float = 0.0
    event_mtime_ns: int = -1
    compat_pending: bool = True
    compat_recipe_key: str = ""
    compat_sources: set[str] | None = None
    player_active: bool = False
    metrics_path: Path | None = None
    live_enabled: bool = True
    live_reason: str = "Background preparation ready"
    metric_checked_at: float = 0.0
    metric_updated: float = 0.0
    metric_sampled: float = 0.0
    metric_values: dict | None = None
    baseline_frames: list[float] = field(default_factory=list)
    baseline_inputs: list[float] = field(default_factory=list)
    baseline_fps: list[float] = field(default_factory=list)
    last_live_job_at: float = 0.0
    last_live_job_seconds: float = 0.0
    live_jobs_started: int = 0
    batch_fallbacks: int = 0
    batch_limit: int = 16
    resource_reason: str = ''
    removing: bool = False
    ready_sample_cursor: int = 0
    defer_maintenance_during_play: bool = False


@dataclass(frozen=True)
class _Work:
    game: _Game
    lease: JobLease
    source: SourceRecord
    recipe: Recipe


@dataclass(frozen=True)
class _Batch:
    items: tuple[_Work, ...]


@dataclass(frozen=True)
class _Outcome:
    work: _Work
    output: str | None
    error: str | None
    elapsed: float
    retry_single: bool = False
    resource_defer: bool = False
