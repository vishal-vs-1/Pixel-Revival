"""Shared, non-blocking coordinator for imported and filesystem-discovered artwork."""
from __future__ import annotations

from pathlib import Path
import logging
import queue
import sqlite3
import threading
import time
import uuid
from typing import Callable, Iterable

from runtime_assets.contracts import Recipe, SourceRecord
from runtime_assets.ingest import LegacyCacheImporter, MigrationReport
from runtime_assets.integration import WorkspaceIntegration
from runtime_assets.store import AssetStore, JobLease, JobRecord
from runtime_assets.resources import ResourcePressure
from runtime_assets.admission import can_admit_live
from runtime_assets.discovery import ingest_events, read_source, reconcile_sources
from runtime_assets.execution import execute, execute_batch
from runtime_assets.publication import publish_compatibility, publish_ready, read_compatibility_state
from runtime_assets.work_state import _Batch, _Game, _Outcome, _Work


LOG = logging.getLogger(__name__)


class AssetService:
    """One coordinator and GPU worker; idle work can share one model run per batch."""

    def __init__(self, backend_factory: Callable[[Recipe], object], *,
                 listener: Callable[[str, dict], None] | None = None,
                 poll_seconds: float = 0.1, lease_seconds: float = 90.0,
                 batch_size: int = 512, batch_pixels: int = 1_048_576,
                 save_workers: int = 1, recover_previous_artwork: bool = False):
        self._backend_factory = backend_factory
        self._listener = listener
        self._poll_seconds = max(0.02, float(poll_seconds))
        self._lease_seconds = max(15.0, float(lease_seconds))
        self._batch_size = max(1, min(512, int(batch_size)))
        self._batch_pixels = max(1, min(4_194_304, int(batch_pixels)))
        self._save_workers = max(1, min(16, int(save_workers)))
        self._batch_disabled: set[str] = set()
        self._owner = "asset-service-" + uuid.uuid4().hex
        # Only the launcher holding its single-instance lock opts into takeover.
        self._recover_previous_artwork = recover_previous_artwork
        self._games: dict[str, _Game] = {}
        self._paused_all = False
        self._lock = threading.RLock()
        self._compat_commit_lock = threading.Lock()
        self._coordinator_gate = threading.RLock()
        self._stop = threading.Event()
        self._work: queue.Queue[_Batch | None] = queue.Queue(maxsize=1)
        self._outcomes: queue.Queue[_Outcome] = queue.Queue()
        self._coordinator: threading.Thread | None = None
        self._worker: threading.Thread | None = None
        self._active: _Batch | None = None
        self._completion_retry_at = 0.0
        self._lease_renew_at = 0.0
        self._cursor = 0
        self._closed = False

    def set_listener(self, listener: Callable[[str, dict], None] | None):
        self._listener = listener

    def _notify(self, game_id: str):
        listener = self._listener
        if listener is None:
            return
        try:
            listener(game_id, self.stats(game_id))
        except Exception:
            pass

    def _start(self):
        with self._lock:
            if self._closed:
                raise RuntimeError("AssetService is closed")
            if self._coordinator is not None:
                return
            self._stop.clear()
            self._worker = threading.Thread(target=self._worker_loop,
                                            name="asset-enhancer", daemon=True)
            self._coordinator = threading.Thread(target=self._coordinator_loop,
                                                 name="asset-coordinator", daemon=True)
            self._worker.start()
            self._coordinator.start()

    def register_game(self, workspace: Path, recipe: Recipe, *, game: dict,
                      ai_recipe: Recipe | None = None, queue_missing: bool = True,
                      paused: bool = False,
                      integration: WorkspaceIntegration | None = None,
                      defer_maintenance_during_play: bool = False,
                      progress=None, fresh_sources=None, recover: bool = False) -> MigrationReport:
        """Register a game and its static sources without waiting for enhancement."""
        with self._coordinator_gate:
            return self._register_game(workspace,recipe,game=game,ai_recipe=ai_recipe,
                                       queue_missing=queue_missing,paused=paused,integration=integration,
                                       defer_maintenance_during_play=defer_maintenance_during_play,
                                       progress=progress, fresh_sources=fresh_sources,
                                       recover=recover)

    def _register_game(self,workspace,recipe,*,game,ai_recipe,queue_missing,paused,integration,
                       defer_maintenance_during_play,progress,fresh_sources,recover=False):
        root = Path(workspace).resolve()
        if game.get("id") != root.name:
            raise ValueError("Game metadata ID does not match its workspace")
        with self._lock:
            if self._closed:
                raise RuntimeError("AssetService is closed")
            current = self._games.get(root.name)
            if current is not None and (current.workspace != root or current.removing):
                raise ValueError("Workspace does not match an active registration for this game")
        store = AssetStore(root)
        provisional = None
        if current is None:
            # Register the workspace before the catalogue scan so the library
            # can show the counts already stored in SQLite. Re-indexing a large
            # game takes minutes and used to leave a stale saved snapshot on
            # screen until it finished.
            with self._lock:
                if self._closed:
                    raise RuntimeError("AssetService is closed")
                key, sources = (read_compatibility_state(root, recipe.key)
                                if integration is None else ("", None))
                provisional = _Game(root, store, recipe, integration=integration,
                                    paused=True, compat_recipe_key=key,
                                    compat_sources=sources, removing=True)
                self._games[root.name] = provisional
            self._notify(root.name)
        try:
            if self._recover_previous_artwork and current is None:
                store.recover_previous_service_jobs(self._owner)
            counts = store.counts(recipe.key) if recover and integration is None else None
            indexed = counts and counts["sources"] > 0
            reuse = bool(indexed)
            if reuse:
                # Recovery only reattaches an established workspace. Re-reading
                # every source took minutes and delayed the first enhancement.
                store.register_recipe(recipe)
                queued = store.enqueue_unqueued_sources(recipe.key)
                report = MigrationReport(0, 0, 0, queued, 0, 0)
            elif integration is None:
                report = LegacyCacheImporter(root, store).import_workspace(
                    game, recipe, ai_recipe=ai_recipe, queue_missing=queue_missing,
                    progress=progress, fresh_sources=fresh_sources)
            else:
                store.register_recipe(recipe)
                report = integration.admit(root, store, game, recipe, queue_missing)
        except BaseException:
            if provisional is not None:
                with self._lock:
                    if self._games.get(root.name) is provisional:
                        self._games.pop(root.name, None)
            raise
        with self._lock:
            if self._closed:
                if provisional is not None:
                    self._games.pop(root.name, None)
                raise RuntimeError("AssetService is closed")
            if current is None:
                current = provisional
                current.paused = (paused or self._paused_all or
                                  (current.player_active and not current.live_enabled))
                current.removing = False
            else:
                if current.recipe.key != recipe.key or current.integration is not integration:
                    current.compat_recipe_key = ""
                    current.compat_sources = set()
                current.store = store
                current.recipe = recipe
                current.integration = integration
                current.paused = (paused or self._paused_all or
                                  (current.player_active and not current.live_enabled))
            current.defer_maintenance_during_play = defer_maintenance_during_play
            current.dirty_at = time.monotonic() + 0.5
            current.next_scan = time.monotonic() + 5.0
            current.compat_pending = True
        self._start()
        self._notify(root.name)
        return report

    def register_imported_sources(self, workspace: Path,
                                  sources: Iterable[SourceRecord]) -> int:
        """Validate and queue a batch of imported sources under the selected recipe."""
        with self._coordinator_gate:
            return self._register_imported_sources(workspace, sources)

    def _register_imported_sources(self, workspace, sources):
        game = self._game_for(workspace)
        if game.removing:
            raise RuntimeError("This artwork workspace is being removed")
        added = 0
        for source in sources:
            if not isinstance(source, SourceRecord):
                raise TypeError("Imported asset batches must contain SourceRecord values")
            path = game.store.resolve_path(source.relative_path)
            verified = read_source(path, source.source_id)
            if (verified.width, verified.height) != (source.width, source.height):
                raise ValueError("Imported source dimensions do not match its record")
            game.store.register_source(source)
            added += int(game.store.enqueue(source.source_id, game.recipe.key))
        if added:
            self._notify(game.workspace.name)
        return added

    def select_recipe(self, game_id: str, recipe: Recipe):
        # Recipe switches and removal serialize with the workspace publication hooks.
        with self._coordinator_gate:
            return self._select_recipe(game_id, recipe)

    def _select_recipe(self, game_id, recipe):
        game = self._game(game_id)
        if game.removing:
            raise RuntimeError("This artwork workspace is being removed")
        game.store.register_recipe(recipe)
        with self._lock:
            if game.recipe.key != recipe.key:
                game.compat_recipe_key = ""
                game.compat_sources = set()
            game.recipe = recipe
            game.dirty_at = time.monotonic() + 0.5
            game.compat_pending = True
        added = 0
        for source in game.store.list_sources():
            added += int(game.store.enqueue(source.source_id, recipe.key))
        self._start()
        self._notify(game_id)
        return added

    def pause(self, game_id: str):
        with self._compat_commit_lock:
            with self._lock:
                self._game(game_id).paused = True

    def resume(self, game_id: str):
        with self._lock:
            game = self._game(game_id)
            game.paused = False
            game.compat_pending = True

    def pause_all(self):
        with self._compat_commit_lock:
            with self._lock:
                self._paused_all = True
                for game in self._games.values():
                    game.paused = True

    def resume_all(self):
        with self._lock:
            self._paused_all = False
            for game in self._games.values():
                paused = game.player_active and not game.live_enabled
                if game.paused and not paused:
                    game.compat_pending = True
                game.paused = paused

    def set_player_state(self, game_id: str, running: bool, metrics_path: Path | None = None,
                         *, allow_live: bool = True):
        """Attach fresh local player metrics and enable conservative live admission."""
        game = self._game(game_id)
        metrics = Path(metrics_path).resolve() if metrics_path is not None else None
        changed = False
        with self._lock:
            changed = (game.player_active != bool(running) or
                       game.metrics_path != (metrics if running else None) or
                       game.live_enabled != bool(allow_live))
            started = running and (not game.player_active or game.metrics_path != metrics)
            game.player_active = bool(running)
            game.metrics_path = metrics if running else None
            game.live_enabled = bool(allow_live)
            game.paused = self._paused_all or (running and not allow_live)
            if started:
                game.metric_checked_at = 0.0
                game.metric_updated = 0.0
                game.metric_sampled = 0.0
                game.metric_values = None
                game.baseline_frames.clear()
                game.baseline_inputs.clear()
                game.baseline_fps.clear()
                game.last_live_job_at = 0.0
                game.live_reason = "Measuring game performance before live image work"
            elif not running:
                game.metric_values = None
                game.baseline_frames.clear()
                game.baseline_inputs.clear()
                game.baseline_fps.clear()
                game.live_reason = "Background preparation ready"
                game.paused = self._paused_all
            elif not allow_live:
                game.live_reason = "Live image work paused by the user"
            elif game.live_reason == "Live image work paused by the user":
                game.live_reason = "Measuring game performance before live image work"
                changed = True
        if changed:
            self._notify(game_id)

    def stats(self, game_id: str) -> dict:
        game = self._game(game_id)
        counts_unavailable = False
        try:
            counts = game.store.counts(game.recipe.key)
            game.counts_snapshot = dict(counts)
        except (OSError, RuntimeError, sqlite3.Error):
            counts_unavailable = True
            counts = {key: 0 for key in ('sources', 'QUEUED', 'RUNNING', 'DEFERRED', 'READY', 'FAILED')}
            counts.update(game.counts_snapshot)
        active = self._active
        active_images = sum(work.game is game for work in active.items) if active else 0
        counts.update(game_id=game_id, recipe_key=game.recipe.key,
                      paused=game.paused, total=sum(counts[state] for state in
                      ("QUEUED", "RUNNING", "DEFERRED", "READY", "FAILED")),
                      player_active=game.player_active,
                      live_enabled=game.live_enabled,
                      live_reason=game.live_reason,
                      live_jobs_started=game.live_jobs_started,
                      last_live_job_seconds=game.last_live_job_seconds,
                      active_batch_images=active_images,
                      reserved_images=max(0, counts['RUNNING'] - active_images),
                      background_batch_limit=game.batch_limit,
                      batch_fallbacks=game.batch_fallbacks,
                      resource_reason=game.resource_reason,
                      completion_error=game.completion_error,
                      counts_unavailable=counts_unavailable,
                      frame_p95_ms=(game.metric_values or {}).get("frame_p95_ms"),
                      input_queue_p95_ms=(game.metric_values or {}).get("input_queue_p95_ms"))
        return counts

    def reconcile(self, game_id: str) -> int:
        """Import newly published source PNGs and process a bounded event batch."""
        with self._coordinator_gate:
            game = self._game(game_id)
            if game.removing:
                return 0
            # Manual sync replays format-owned captures first, then recovers
            # saved originals that have no corresponding database entry.
            accepted = (self._discover_integrated(game) if game.integration is not None
                        else self._ingest_events(game, 32))
            added = self._reconcile_sources(game)
            queued = game.store.enqueue_unqueued_sources(game.recipe.key)
            if queued:
                self._notify(game_id)
            return accepted + added + queued

    def _discover_integrated(self, game: _Game) -> int:
        accepted = game.integration.discover(game.workspace, game.store, game.recipe)
        if accepted:
            game.dirty_at = time.monotonic() + 0.5
            self._notify(game.workspace.name)
        return accepted

    def unregister_game(self,game_id: str,*,timeout: float=60.0) -> bool:
        """Quiesce one game's writes before its files are removed; other games stay registered."""
        deadline=time.monotonic()+max(0.0,timeout)
        with self._coordinator_gate:
            with self._lock:
                game=self._games.get(game_id)
                if game is None:return False
                if game.player_active:
                    raise RuntimeError('Close the game before deleting it so its saves can finish writing.')
                was_paused=game.paused
                game.removing=True
                game.paused=True
        while True:
            with self._coordinator_gate:
                self._drain_outcomes()
                if self._active is None or all(work.game is not game for work in self._active.items):
                    with self._lock:self._games.pop(game_id,None)
                    return True
                if time.monotonic()>=deadline:
                    with self._lock:
                        game.removing=False
                        game.paused=was_paused or self._paused_all
                    raise TimeoutError('The current artwork batch is still finishing. Try deleting again when it finishes.')
            time.sleep(0.05)

    def _game(self, game_id: str) -> _Game:
        with self._lock:
            try:
                return self._games[game_id]
            except KeyError as error:
                raise KeyError(f"Game {game_id!r} is not registered with AssetService") from error

    def _game_for(self, workspace: Path) -> _Game:
        root = Path(workspace).resolve()
        game = self._game(root.name)
        if game.workspace != root:
            raise ValueError("Workspace does not match the registered game")
        return game


    def _reconcile_sources(self, game: _Game) -> int:
        registered = reconcile_sources(game)
        if registered:
            self._notify(game.workspace.name)
        return registered

    def _ingest_events(self, game: _Game, limit: int) -> int:
        event_dir = game.workspace / ".runtime" / "events"
        try:
            stamp = event_dir.stat().st_mtime_ns
        except OSError:
            stamp = -1
        if stamp == game.event_mtime_ns:
            return 0
        accepted = ingest_events(game, limit)
        game.event_mtime_ns = stamp
        if accepted:
            game.dirty_at = time.monotonic() + 0.5
            self._notify(game.workspace.name)
        return accepted

    def _coordinator_loop(self):
        while not self._stop.wait(self._poll_seconds):
            with self._coordinator_gate:self._coordinate_once()

    def _coordinate_once(self):
        now = time.monotonic()
        with self._lock:
            games = list(self._games.values())
            # Foreground opt-in defers idle scans and publication. Keep due
            # work pending and let any other active player keep its live path.
            if any(game.player_active and game.defer_maintenance_during_play for game in games):
                games = [game for game in games
                         if game.player_active and not game.defer_maintenance_during_play]
        for game in games:
            if game.removing:continue
            try:
                if now >= game.next_recover:
                    game.store.recover_expired()
                    game.next_recover = now + (2.0 if game.player_active else 5.0)
                if game.integration is not None:
                    if now >= game.next_scan:
                        game.next_scan = now + 0.5
                        self._discover_integrated(game)
                else:
                    # Runtime captures carry durable events; catalog recovery
                    # belongs to import and explicit reconciliation.
                    if self._ingest_events(game, 32):
                        game.dirty_at = now + 0.5
            except Exception:
                LOG.exception("asset coordinator failed for %s", game.workspace.name)
                game.next_scan = now + 5.0
                continue
        try:
            self._drain_outcomes()
            self._renew_active(now)
            if self._active is None:
                self._schedule_one()
        except Exception:
            LOG.exception("asset coordinator scheduling failed")
        for game in games:
            if game.removing:continue
            # Integrated formats may collect live input while running, but building
            # a replacement package is deferred until the player has closed.
            defer_integrated_publish = game.integration is not None and game.player_active
            if game.dirty_at is not None and now >= game.dirty_at and not defer_integrated_publish:
                try:
                    if game.integration is None:
                        self._publish_ready(game)
                    else:
                        game.integration.publish_ready(game.workspace, game.store, game.recipe)
                    game.dirty_at = None
                except Exception:
                    LOG.exception("asset publish failed for %s", game.workspace.name)
                    game.dirty_at = time.monotonic() + 1.0
            if game.integration is None and game.compat_pending and not game.paused:
                try:
                    # Small sprite copies are cheap; a larger slice lets a
                    # partially published game catch up within one session.
                    self._publish_compatibility(game, limit=64)
                except Exception:
                    LOG.exception("compatibility publish failed for %s", game.workspace.name)
                    game.compat_pending = True

    def _schedule_one(self):
        with self._lock:
            games = list(self._games.values())
            if not games:
                return
            # While any game is in the foreground, keep background games from
            # consuming GPU/CPU time. Only that player's own small live jobs may
            # run, and only after _can_admit_live approves fresh performance data.
            active_players = [game for game in games if game.player_active]
            if active_players:
                games = active_players
            start = self._cursor % len(games)
            ordered = games[start:] + games[:start]
            self._cursor = (start + 1) % len(games)
        for game in ordered:
            if game.paused or game.removing:
                continue
            if not game.store.has_waiting(game.recipe.key):
                if game.resource_reason:
                    game.resource_reason='';self._notify(game.workspace.name)
                continue
            if game.player_active and not self._can_admit_live(game):
                continue
            limit = 1
            pixel_budget = self._batch_pixels
            try:
                backend = self._backend_factory(game.recipe)
                if callable(getattr(backend, "enhance_many", None)):
                    budget=backend.batch_limits() if callable(getattr(backend,'batch_limits',None)) else None
                    if budget is not None and budget.reason:
                        if game.resource_reason!=budget.reason:
                            game.resource_reason=budget.reason;self._notify(game.workspace.name)
                        continue
                    if not game.player_active and game.recipe.key not in self._batch_disabled:
                        limit = self._batch_size
                        if budget is not None:
                            limit=min(limit,budget.max_images)
                            pixel_budget=min(pixel_budget,budget.max_pixels)
            except Exception:
                # Let the normal single-job path record a useful backend error.
                pass
            if not game.player_active:game.batch_limit=limit
            try:
                leases = game.store.claim_batch(self._owner, lease_seconds=self._lease_seconds,
                    recipe_key=game.recipe.key, max_count=limit, max_pixels=pixel_budget)
            except Exception:
                continue
            items = []
            recipe_json = game.store.recipe_json(game.recipe.key)
            recipe = Recipe.from_json(recipe_json) if recipe_json is not None else None
            for lease in leases:
                try:
                    source = game.store.get_source(lease.source_id)
                    if source is None or recipe is None:
                        raise ValueError("Job source or recipe is missing")
                    if game.player_active and source.width * source.height > 65_536:
                        game.store.defer(lease, retry_at=time.time() + 30.0,
                                         reason="Deferred during play: source is larger than the live-work limit")
                        self._set_live_reason(game, "Large images are queued until the player closes")
                        continue
                    items.append(_Work(game, lease, source, recipe))
                except (KeyError, ValueError, RuntimeError) as error:
                    game.store.mark_failed(lease, str(error), retry_at=None)
            if not items:
                continue
            batch = _Batch(tuple(items))
            try:
                self._work.put_nowait(batch)
            except queue.Full:
                for work in items:
                    game.store.mark_failed(work.lease, "Worker queue is busy", retry_at=time.time() + 1.0)
                continue
            self._active = batch
            game.resource_reason=''
            self._lease_renew_at = time.monotonic() + self._lease_seconds / 3
            if game.player_active:
                game.last_live_job_at = time.monotonic()
                game.live_jobs_started += 1
                self._set_live_reason(game, "Preparing one small image; further work waits for fresh performance data")
            self._notify(game.workspace.name)
            return

    def _can_admit_live(self, game: _Game) -> bool:
        return can_admit_live(game, busy=self._active is not None, set_reason=self._set_live_reason)


    def _set_live_reason(self, game: _Game, reason: str):
        if game.live_reason == reason:
            return
        game.live_reason = reason
        self._notify(game.workspace.name)

    def _renew_active(self, now: float):
        batch = self._active
        if batch is None or now < self._lease_renew_at:
            return
        items = []
        retry = False
        for work in batch.items:
            try:
                renewed = work.game.store.renew(work.lease, lease_seconds=self._lease_seconds)
            except Exception:
                renewed = None
                retry = True
            # Even a lost lease must occupy the GPU slot until its dispatched
            # work completes. Token checks prevent its stale outcome committing.
            items.append(_Work(work.game, renewed or work.lease, work.source, work.recipe))
        self._active = _Batch(tuple(items))
        self._lease_renew_at = now + (1.0 if retry else self._lease_seconds / 3)

    def _worker_loop(self):
        batch_backends = {}
        try:
            self._run_worker(batch_backends)
        finally:
            for backend in batch_backends.values():
                backend.close()

    def _run_worker(self, batch_backends):
        while not self._stop.is_set():
            try:
                batch = self._work.get(timeout=0.2)
            except queue.Empty:
                continue
            if batch is None:
                return
            if len(batch.items) > 1:
                started = time.monotonic()
                try:
                    backend = self._backend_factory(batch.items[0].recipe)
                    if callable(getattr(backend, "close", None)):
                        batch_backends[id(backend)] = backend
                    outputs = execute_batch(batch.items, backend, read_source,
                                            save_workers=self._save_workers)
                except Exception as error:
                    game = batch.items[0].game
                    with self._lock:
                        self._batch_disabled.add(batch.items[0].recipe.key)
                        game.batch_fallbacks += 1
                    try:
                        log = game.workspace / "logs" / "enhancement-batch.log"
                        log.parent.mkdir(parents=True, exist_ok=True)
                        with log.open("a", encoding="utf-8") as stream:
                            stream.write(f"{time.time():.3f}: Batch failed; retrying separately: {type(error).__name__}: {error}\n")
                    except OSError:
                        pass
                    elapsed = time.monotonic() - started
                    for work in batch.items:
                        self._outcomes.put(_Outcome(work, None,
                            f"Batch failed; retrying separately: {type(error).__name__}: {error}",
                            elapsed, retry_single=True))
                    continue
                else:
                    elapsed = time.monotonic() - started
                    for work, output in zip(batch.items, outputs):
                        self._outcomes.put(_Outcome(work, output, None, elapsed))
                    continue
            for work in batch.items:
                self._outcomes.put(self._run_single(work))

    def _run_single(self, work: _Work) -> _Outcome:
        started = time.monotonic()
        try:
            output = self._execute(work)
            return _Outcome(work, output, None, time.monotonic() - started)
        except Exception as error:
            return _Outcome(work, None, f"{type(error).__name__}: {error}",
                            time.monotonic() - started,resource_defer=isinstance(error,ResourcePressure))

    def _execute(self, work: _Work) -> str:
        return execute(work, self._backend_factory, read_source)


    def _drain_outcomes(self):
        if time.monotonic() < self._completion_retry_at:
            return
        changed = set()
        ready = {}
        retry = []
        completed = []
        # Failed commits are returned only after this snapshot has been drained.
        # Retaining the active slot bounds retries to one dispatched batch.
        for _ in range(self._outcomes.qsize()):
            try:
                outcome = self._outcomes.get_nowait()
            except queue.Empty:
                break
            work, game = outcome.work, outcome.work.game
            try:
                if outcome.resource_defer:
                    game.store.defer(work.lease,retry_at=time.time()+15,
                                     reason=outcome.error or 'Waiting for memory')
                    game.resource_reason=outcome.error or 'Waiting for memory'
                elif outcome.retry_single:
                    # Re-enter normal admission: a player may have opened or the
                    # user may have paused while the batch was still running.
                    game.store.reschedule(work.lease, outcome.error or "Retrying separately")
                elif outcome.error is not None:
                    retry_at = time.time() + min(300, 10 * (2 ** min(work.lease.attempts - 1, 4)))
                    if work.lease.attempts >= 3:
                        retry_at = None
                    game.store.mark_failed(work.lease, outcome.error, retry_at=retry_at)
                elif outcome.output is not None:
                    # Collect first: one transaction per game instead of one per image,
                    # which previously cost more than the artwork itself.
                    ready.setdefault(id(game), (game, []))[1].append(outcome)
                    continue
            except Exception as error:
                game.completion_error = f'Artwork result could not be saved: {error}'
                LOG.exception('artwork completion failed for %s', game.workspace.name)
                retry.append(outcome)
                changed.add(game.workspace.name)
                continue
            completed.append(outcome)
        for game, outcomes in ready.values():
            try:
                entries = [(item.work.lease, item.output) for item in outcomes]
                if game.store.mark_ready_batch(entries):
                    game.dirty_at = time.monotonic() + 0.5
                    game.compat_pending = True
                # A rejected stale token is terminal; it cannot overwrite its successor.
                completed.extend(outcomes)
            except Exception as error:
                game.completion_error = f'Artwork result could not be saved: {error}'
                LOG.exception('artwork completion failed for %s', game.workspace.name)
                retry.extend(outcomes)
                changed.add(game.workspace.name)
        failed_games = {id(item.work.game) for item in retry}
        for outcome in completed:
            work, game = outcome.work, outcome.work.game
            if id(game) not in failed_games:
                game.completion_error = ''
            if game.player_active:
                game.last_live_job_seconds = outcome.elapsed
            if self._active is not None:
                remaining = tuple(item for item in self._active.items if item.lease.token != work.lease.token)
                self._active = _Batch(remaining) if remaining else None
            changed.add(game.workspace.name)
        for outcome in retry:
            self._outcomes.put_nowait(outcome)
        self._completion_retry_at = time.monotonic() + 1.0 if retry else 0.0
        for game_id in changed:
            self._notify(game_id)

    def _publish_compatibility(self, game: _Game, *, limit: int):
        publish_compatibility(game, limit=limit, commit_lock=self._compat_commit_lock)


    def _publish_ready(self, game: _Game):
        publish_ready(game)
        self._notify(game.workspace.name)

    def close(self, timeout: float = 1.0):
        with self._lock:
            self._closed = True
        self._stop.set()
        try:
            self._work.put_nowait(None)
        except queue.Full:
            pass
        for thread in (self._coordinator, self._worker):
            if thread is not None and thread is not threading.current_thread():
                thread.join(max(0.0, timeout))
        if any(thread is not None and thread.is_alive()
               for thread in (self._coordinator, self._worker)):
            return False
        # The closing thread may drain only after the coordinator has released
        # ownership; otherwise both could mutate the active batch at once.
        self._drain_outcomes()
        return self._outcomes.empty()
