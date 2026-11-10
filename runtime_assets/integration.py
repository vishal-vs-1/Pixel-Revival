"""Optional workspace-specific boundaries around the shared artwork scheduler."""

from pathlib import Path
from typing import Protocol

from runtime_assets.contracts import Recipe
from runtime_assets.ingest import MigrationReport
from runtime_assets.store import AssetStore


class WorkspaceIntegration(Protocol):
    """Hooks run on the coordinator gate for one owned workspace."""

    def admit(self, workspace: Path, store: AssetStore, game: dict, recipe: Recipe,
              queue_missing: bool) -> MigrationReport: ...

    def discover(self, workspace: Path, store: AssetStore, recipe: Recipe) -> int: ...

    def publish_ready(self, workspace: Path, store: AssetStore, recipe: Recipe) -> None: ...
