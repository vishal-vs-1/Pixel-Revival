"""Capture replay adapter for the shared artwork scheduler."""

from pathlib import Path

from runtime_assets.ingest import MigrationReport
from runtime_assets.store import AssetStore

from .bindings import NesBindingStore
from .capture import committed_chunks, quarantine, read_chunk
from .pack import NesPackGenerator


class NesCaptureIntegration:
    """Admit and replay native tiles without adding NES branches to AssetService."""

    def __init__(self, identity: str, recipe, *, expected_palette_profile=None,
                 replay_limit: int = 4, pack_scale: int = 4, pack_generator=None):
        self.identity = identity
        self.recipe = recipe
        self.expected_palette_profile = expected_palette_profile
        self.replay_limit = max(1, min(int(replay_limit), 32))
        if pack_scale not in (2, 4):
            raise ValueError("NES pack scale must be 2 or 4")
        self.pack_scale = pack_scale
        self.pack_generator = pack_generator

    def _replay(self, workspace: Path, store: AssetStore, recipe) -> int:
        bindings = NesBindingStore(workspace)
        added = 0
        for path in committed_chunks(workspace, self.replay_limit):
            try:
                chunk = read_chunk(path, self.identity)
                if (self.expected_palette_profile is not None and
                        chunk.palette_profile != self.expected_palette_profile):
                    raise ValueError("NES capture uses an unexpected palette profile")
                added += bindings.ingest(chunk, store, recipe)
                path.unlink()
            except (OSError, ValueError):
                # Bad input is isolated from the player and retained in a small quarantine.
                try:
                    quarantine(path)
                except OSError:
                    pass
        return added

    def admit(self, workspace, store, game, recipe, queue_missing):
        added = self._replay(workspace, store, recipe)
        # Enhanced capture needs a valid staged pack before Mesen can expose tiles.
        generator = self.pack_generator or NesPackGenerator(workspace)
        generator.ensure_capture_pack(recipe, pack_scale=self.pack_scale)
        return MigrationReport(added, 0, 0, added, 0, 0)

    def discover(self, workspace, store, recipe):
        return self._replay(workspace, store, recipe)

    def publish_ready(self, workspace, store, recipe):
        generator = self.pack_generator or NesPackGenerator(workspace)
        generation = generator.publish(store, recipe, pack_scale=self.pack_scale)
        if generation is None:
            return generator.ensure_capture_pack(recipe, pack_scale=self.pack_scale)
        return generation
