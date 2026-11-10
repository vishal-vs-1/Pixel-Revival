import threading
from dataclasses import replace
from unittest.mock import Mock

import pytest
from PIL import Image

from runtime_assets.contracts import SourceRecord, image_source_id, pixel_recipe
from runtime_assets.ingest import MigrationReport
from runtime_assets.service import AssetService


class FixtureIntegration:
    def __init__(self):
        self.admitted = []
        self.published = []
        self.ready = threading.Event()

    def admit(self, workspace, store, game, recipe, queue_missing):
        assert workspace == store.workspace
        assert store.recipe_json(recipe.key) == recipe.canonical_json()
        self.admitted.append((workspace, game["id"], queue_missing))
        return MigrationReport(0, 0, 0, 0, 0, 0)

    def discover(self, workspace, store, recipe):
        image = Image.new("RGBA", (2, 2), (40, 20, 60, 255))
        source_id = image_source_id(image)
        if store.get_source(source_id) is not None:
            return 0
        relative = f"fixture/{source_id}.png"
        path = store.resolve_path(relative)
        path.parent.mkdir(parents=True, exist_ok=True)
        image.save(path)
        store.register_source(SourceRecord(source_id, 2, 2, relative, "fixture"))
        return int(store.enqueue(source_id, recipe.key))

    def publish_ready(self, workspace, store, recipe):
        self.published.append((workspace, recipe.key))
        if store.list_ready(recipe.key):
            self.ready.set()


class Upscaler:
    def enhance(self, image, recipe, _workdir):
        return image.resize((image.width * recipe.scale, image.height * recipe.scale))


def test_integration_uses_shared_worker_and_only_its_own_workspace(tmp_path, monkeypatch):
    plugin = FixtureIntegration()
    legacy_import = Mock(side_effect=AssertionError("Integrated workspace used the legacy scanner"))
    monkeypatch.setattr("runtime_assets.service.LegacyCacheImporter", legacy_import)
    workspace = tmp_path / ("1" * 64)
    recipe = pixel_recipe()
    service = AssetService(lambda _recipe: Upscaler(), poll_seconds=0.03)
    try:
        service.register_game(workspace, recipe, game={"id": workspace.name}, integration=plugin)
        service.reconcile(workspace.name)
        assert plugin.ready.wait(5)
        assert service.stats(workspace.name)["READY"] == 1
        assert plugin.admitted == [(workspace, workspace.name, True)]
        assert all(folder == workspace for folder, _key in plugin.published)
        assert not (workspace / ".runtime" / "ready").exists()
        assert not (workspace / "textures").exists()
        legacy_import.assert_not_called()
    finally:
        service.close(2)


def test_recipe_switch_and_removal_guard_integration_publication(tmp_path, monkeypatch):
    workspace = tmp_path / ("2" * 64)
    plugin = FixtureIntegration()
    plugin.discover = Mock(return_value=0)
    service = AssetService(lambda _recipe: Upscaler())
    monkeypatch.setattr(service, "_start", lambda: None)
    recipe = pixel_recipe()
    new_recipe = replace(recipe, implementation_version="fixture-style")
    try:
        service.register_game(workspace, recipe, game={"id": workspace.name}, integration=plugin)
        service.select_recipe(workspace.name, new_recipe)
        service._game(workspace.name).dirty_at = 0
        with service._coordinator_gate:
            service._coordinate_once()
        assert plugin.published == [(workspace, new_recipe.key)]
        assert service.unregister_game(workspace.name)
        with service._coordinator_gate:
            service._coordinate_once()
        assert plugin.published == [(workspace, new_recipe.key)]
        assert not plugin.discover.called
    finally:
        service.close()


def test_integrated_package_publication_waits_until_player_closes(tmp_path, monkeypatch):
    workspace = tmp_path / ("4" * 64)
    plugin = FixtureIntegration()
    service = AssetService(lambda _recipe: Upscaler())
    monkeypatch.setattr(service, "_start", lambda: None)
    recipe = pixel_recipe()
    try:
        service.register_game(workspace, recipe, game={"id": workspace.name}, integration=plugin)
        service.set_player_state(workspace.name, True, allow_live=False)
        service.pause_all()
        service.resume_all()
        service.register_game(workspace, recipe, game={"id": workspace.name}, integration=plugin)
        assert service.stats(workspace.name)["paused"]
        service._game(workspace.name).dirty_at = 0
        with service._coordinator_gate:
            service._coordinate_once()
        assert plugin.published == []

        service.set_player_state(workspace.name, False, allow_live=False)
        service.resume_all()
        assert not service.stats(workspace.name)["paused"]
        service._game(workspace.name).dirty_at = 0
        with service._coordinator_gate:
            service._coordinate_once()
        assert plugin.published == [(workspace, recipe.key)]
    finally:
        service.close()


def test_duplicate_workspace_id_rejected_before_custom_admission(tmp_path):
    workspace = tmp_path / ("3" * 64)
    other = tmp_path / "elsewhere" / workspace.name
    plugin = FixtureIntegration()
    service = AssetService(lambda _recipe: Upscaler())
    try:
        service.register_game(workspace, pixel_recipe(), game={"id": workspace.name}, integration=plugin)
        with pytest.raises(ValueError, match="active registration"):
            service.register_game(other, pixel_recipe(), game={"id": workspace.name}, integration=plugin)
        assert not other.exists()
        assert len(plugin.admitted) == 1
    finally:
        service.close()
