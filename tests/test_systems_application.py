"""Focused U12 checks without a window, ROM or native player."""

from collections import deque
from dataclasses import FrozenInstanceError
from queue import Queue
from types import SimpleNamespace
from unittest.mock import Mock

import pytest

from app import App
from engine import Library, atomic_json
from runtime_assets.contracts import pixel_recipe
from runtime_assets.store import AssetStore
from systems.application import ApplicationLibrary
from systems.contracts import GameRef, ImportRequest, JAVA_ME, NES
from systems.java_me import JavaMeSystem
from systems.presentation import SystemPresentation


JAVA_ID = "a" * 64
NES_ID = "b" * 64


def mixed():
    legacy = Mock()
    legacy.games.return_value = [{"id": JAVA_ID, "name": "Java"}]
    nes = Mock(system=NES, presentation=SystemPresentation("Choose a ROM", (("NES", "*.nes"),)))
    nes.games.return_value = [{"id": NES_ID, "name": "NES", "system": NES}]
    java = JavaMeSystem(legacy)
    service = Mock()
    return ApplicationLibrary((java, nes), service), legacy, nes, service


def test_hidden_catalog_rows_keep_their_owner_across_application_operations(tmp_path):
    library, legacy, nes, service = mixed()
    java, = library.games(JAVA_ME)
    rom, = library.games(NES)
    assert java["system"] == JAVA_ME
    assert library.control_profiles is legacy.control_profiles
    assert library.control_profiles_for(JAVA_ME) is legacy.control_profiles
    assert library.control_profiles_for(NES) is nes.control_profiles
    assert "system" not in legacy.games.return_value[0]
    assert library.workspace(JAVA_ID) is legacy.workspace.return_value
    assert library.texture_dir(GameRef(NES, NES_ID), "pixel") is nes.texture_dir.return_value
    nes.texture_dir.assert_called_once_with(NES_ID, "pixel")

    library.save(java)
    legacy.save.assert_called_once_with({"id": JAVA_ID, "name": "Java"})
    library.set_control_profile_id(NES_ID, "custom-1")
    nes.set_control_profile_id.assert_called_once_with(NES_ID, "custom-1")
    assert library.reference(NES_ID)==GameRef(NES,NES_ID)
    legacy.launch.side_effect = lambda game, *_args, **_kwargs: game.update(startup_hint="Updated")
    library.launch(java, original=True)
    assert java["startup_hint"] == "Updated"
    assert "system" not in legacy.launch.call_args.args[0]

    nes.prepare_style.return_value = rom
    assert library.prepare_style(rom, "ai") == rom
    nes.prepare_style.assert_called_once()
    log = tmp_path / "play.stats.json"
    library.set_player_state(NES_ID, True, log, allow_live=False)
    service.set_player_state.assert_called_once_with(NES_ID, True, log, allow_live=False)
    library.delete_game(NES_ID)
    nes.delete_game.assert_called_once_with(NES_ID)
    legacy.delete_game.assert_not_called()
    with pytest.raises(KeyError, match="Unknown game"):
        library.workspace(NES_ID)


def test_import_request_survives_another_system_view_and_is_immutable(tmp_path):
    library, legacy, nes, _ = mixed()
    request = ImportRequest(JAVA_ME, tmp_path / "saved.jar", "pixel")
    legacy.import_game.return_value = {"id": "c" * 64, "name": "Imported"}
    assert library.games(NES)[0]["id"] == NES_ID
    progress = Mock()
    result = library.import_game(request, progress)
    legacy.import_game.assert_called_once_with(request.path, progress, "pixel")
    nes.import_game.assert_not_called()
    assert library.workspace(result["id"]) is legacy.workspace.return_value
    rom_request = ImportRequest(NES, tmp_path / "saved.nes", "ai")
    nes.import_game.return_value = {"id": "d" * 64, "name": "New NES", "system": NES}
    library.games(JAVA_ME)
    rom = library.import_game(rom_request)
    assert nes.import_game.call_args.args[0] is rom_request
    assert library.workspace(rom["id"]) is nes.workspace.return_value
    with pytest.raises(FrozenInstanceError):
        request.system = NES
    with pytest.raises(FrozenInstanceError):
        GameRef(NES, NES_ID).id = JAVA_ID


def test_app_queue_snapshots_the_path_style_and_system(tmp_path, monkeypatch):
    app = App.__new__(App)
    app.busy = True
    app.pending_imports = deque()
    app.players = {}
    app.visible_system_id = JAVA_ME
    app.backend = SimpleNamespace(get=Mock(return_value="Fast pixel"))
    app.status = Mock()
    app.progress = Mock()
    app.messages = Queue()
    game = {"id": JAVA_ID, "name": "Fixture", "system": JAVA_ME}
    app.library = SimpleNamespace(import_game=Mock(return_value=game))
    monkeypatch.setattr("app.threading.Thread", lambda *, target, daemon: SimpleNamespace(start=target))

    source = tmp_path / "demo.jar"
    app.import_game(source)
    app.backend.get.return_value = "AI cartoon"
    app.busy = False
    app.start_pending()

    assert app.library.import_game.call_args.args[0] == ImportRequest(JAVA_ME, source, "pixel")
    imported_kind, imported=app.messages.get_nowait()
    idle_kind, idle=app.messages.get_nowait()
    assert imported_kind=="imported" and imported.ref==GameRef(JAVA_ME,game["id"])
    assert imported.detail==game["name"]
    assert idle_kind=="idle" and idle.system==JAVA_ME


def test_unknown_and_mismatched_references_never_reach_a_backend(tmp_path):
    library, legacy, nes, _ = mixed()
    with pytest.raises(ValueError, match="Unknown game system"):
        GameRef("other", JAVA_ID)
    with pytest.raises(ValueError, match="Invalid game identifier"):
        GameRef(NES, "../escape")
    with pytest.raises(ValueError, match="Unknown artwork backend"):
        ImportRequest(NES, tmp_path / "game.nes", "other")
    with pytest.raises(KeyError, match="Unknown game"):
        library.workspace(GameRef(NES, JAVA_ID))
    with pytest.raises(KeyError, match="Unknown game"):
        library.delete_game("c" * 64)
    legacy.workspace.assert_not_called()
    nes.workspace.assert_not_called()
    nes.delete_game.assert_not_called()
    java_only = ApplicationLibrary((JavaMeSystem(legacy),), Mock())
    with pytest.raises(LookupError, match="No backend registered"):
        java_only.import_game(ImportRequest(NES, tmp_path / "game.nes", "pixel"))


def test_catalog_and_new_import_reject_duplicate_ids(tmp_path):
    library, legacy, nes, _ = mixed()
    rom = {"id": JAVA_ID, "name": "Duplicate", "system": NES}

    def import_duplicate(*_args):
        nes.games.return_value.append(rom)
        return rom

    nes.import_game.side_effect = import_duplicate
    with pytest.raises(ValueError, match="Duplicate game identifier"):
        library.import_game(ImportRequest(NES, tmp_path / "game.nes", "pixel"))
    with pytest.raises(ValueError, match="Duplicate game identifier"):
        library.workspace(JAVA_ID)
    with pytest.raises(ValueError, match="Duplicate game identifier"):
        library.recover_assets()
    legacy.workspace.assert_not_called()
    nes.workspace.assert_not_called()
    legacy.recover_assets.assert_not_called()
    nes.recover_assets.assert_not_called()
    with pytest.raises(ValueError, match="Duplicate backend"):
        ApplicationLibrary((JavaMeSystem(legacy), JavaMeSystem(legacy)), Mock())


def test_startup_recovery_reuses_service_and_keeps_legacy_metadata(tmp_path, monkeypatch):
    service = Mock()
    create_service = Mock(return_value=service)
    monkeypatch.setattr("engine.AssetService", create_service)
    legacy = Library(tmp_path / "library", tmp_path / "rendered")
    library = ApplicationLibrary((JavaMeSystem(legacy),), legacy.asset_service)
    recipe = pixel_recipe()
    good = {"id": JAVA_ID, "name": "Java", "recipe_key": recipe.key}
    active = legacy.workspace(JAVA_ID)
    atomic_json(active / "game.json", good)
    AssetStore(active).register_recipe(recipe)
    atomic_json(legacy.workspace(NES_ID) / "game.json", {**good, "id": NES_ID})
    legacy._deleting.add(NES_ID)
    bad = legacy.workspace("c" * 64)
    atomic_json(bad / "game.json", {**good, "id": "d" * 64})
    original = (active / "game.json").read_bytes()
    errors = []

    library.recover_assets(errors.append)
    create_service.assert_called_once()
    service.register_game.assert_called_once_with(
        active, recipe, game=good, ai_recipe=None, queue_missing=False)
    assert len(errors) == 1 and "does not belong" in errors[0]
    assert library.games(JAVA_ME) == [{**good, "system": JAVA_ME}]
    assert (active / "game.json").read_bytes() == original
    library.close(timeout=0.2)
    library.close()
    library.recover_assets(errors.append)
    service.close.assert_called_once_with(timeout=0.2)
    assert service.register_game.call_count == 1
