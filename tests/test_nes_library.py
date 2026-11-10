import threading
import json
from concurrent.futures import ThreadPoolExecutor
from unittest.mock import Mock

import pytest

from game_storage import GameStorage
from systems.contracts import ImportRequest, NES
from systems.nes.library import NesLibrary
from systems.nes.controls import NesControlProfileStore, nes_default_seed
from systems.nes.rom import NesRomInspector


def source(tmp_path, name="合成.nes", value=1):
    path = tmp_path / name
    path.write_bytes(b"NES\x1a\x01" + bytes(11) + bytes((value,)) * 16384 + b"extra")
    return path


def add(library, file, style="pixel"):
    return library.import_game(ImportRequest(NES, file, style))


def write_json(path, document):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(document), encoding="utf-8")


def test_import_reimport_and_delete_keep_rom_identity_and_saves(tmp_path):
    original = source(tmp_path)
    register = Mock()
    service = Mock()
    root = tmp_path / "nes"
    library = NesLibrary(root, asset_service=service, register_artwork=register)
    game = add(library, original)
    identity = game["id"]
    stored = root / "library" / identity / "game.nes"
    assert stored.read_bytes() == original.read_bytes()
    assert library.games() == [game] and game["runtime_status"] == "setup-needed"
    with pytest.raises(RuntimeError, match="Set up"):
        library.launch(game)
    workspace = library.workspace(identity)
    save = workspace / "saves" / "mesen" / "battery"
    save.parent.mkdir(parents=True)
    save.write_bytes(b"progress")
    (workspace / "texture.png").write_bytes(b"temporary")
    renamed = source(tmp_path, "Renamed.nes")
    assert add(library, renamed, "ai")["id"] == identity
    assert len(library.games()) == 1 and register.call_count == 2
    assert library.games()[0]["original_filename"] == original.name
    assert save.read_bytes() == b"progress"

    assert library.delete_game(identity) == workspace / "saves"
    assert library.games() == [] and not stored.exists()
    assert not (workspace / "texture.png").exists() and save.read_bytes() == b"progress"
    assert add(library, renamed)["id"] == identity and save.read_bytes() == b"progress"
    assert stored.read_bytes() == original.read_bytes()
    service.close.assert_not_called()


def test_import_failure_preserves_saves_and_does_not_publish(tmp_path):
    original = source(tmp_path)
    info = NesRomInspector().inspect(original)
    writer = Mock(side_effect=OSError("metadata locked"))
    register = Mock()
    root = tmp_path / "nes"
    library = NesLibrary(root, asset_service=Mock(), write_metadata=writer, register_artwork=register)
    saves = library.workspace(info.id) / "saves"
    saves.mkdir(parents=True)
    (saves / "state").write_bytes(b"keep")
    with pytest.raises(OSError, match="metadata locked"):
        add(library, original)
    assert not (root / "library" / info.id / "game.nes").exists()
    assert (saves / "state").read_bytes() == b"keep" and not library.games()
    assert original.is_file()
    register.assert_not_called()


def test_detail_scale_change_refreshes_the_nes_pack_recipe(tmp_path):
    register = Mock()
    library = NesLibrary(tmp_path / "nes", asset_service=Mock(), register_artwork=register)
    game = add(library, source(tmp_path))
    register.reset_mock()

    library.save({**game, "render_scale": 4})

    assert register.call_count == 1
    folder, updated = register.call_args.args
    assert folder == library.workspace(game["id"])
    assert updated["render_scale"] == 4
    assert library.games()[0]["render_scale"] == 4


def test_failed_source_purge_restores_the_nes_source_not_a_jar(tmp_path, monkeypatch):
    register = Mock()
    service = Mock()
    root = tmp_path / "nes"
    library = NesLibrary(root, asset_service=service, register_artwork=register)
    original = source(tmp_path)
    game = add(library, original)
    identity = game["id"]
    folder = library.workspace(identity)
    (folder / "saves").mkdir()
    (folder / "saves" / "slot").write_bytes(b"slot")
    real = GameStorage._remove

    def fail_after_source_removed(path, parent):
        real(path, parent)
        if parent == root / "library":
            raise OSError("simulated purge failure")

    monkeypatch.setattr(GameStorage, "_remove", staticmethod(fail_after_source_removed))
    with pytest.raises(OSError, match="simulated purge"):
        library.delete_game(identity)
    stored = root / "library" / identity
    assert (stored / "game.nes").read_bytes() == original.read_bytes()
    assert not (stored / "game.jar").exists()
    assert (folder / "saves" / "slot").read_bytes() == b"slot"
    assert library.games() == [game]
    assert register.call_args.kwargs == {"recovering": True}


def test_unrelated_delete_can_finish_while_first_waits(tmp_path):
    service = Mock()
    library = NesLibrary(tmp_path / "nes", asset_service=service)
    first = source(tmp_path)
    first_id = add(library, first)["id"]
    second_id = add(library, source(tmp_path, "Other.nes", value=2))["id"]
    entered, release = threading.Event(), threading.Event()

    def unregister(identity, **_kwargs):
        if identity == first_id:
            entered.set()
            assert release.wait(5)
        return True

    service.unregister_game.side_effect = unregister
    with ThreadPoolExecutor(2) as pool:
        job = pool.submit(library.delete_game, first_id)
        try:
            assert entered.wait(3)
            with pytest.raises(RuntimeError, match="being deleted"):
                add(library, first)
            pool.submit(library.delete_game, second_id).result(3)
        finally:
            release.set()
        job.result(3)
    assert library.games() == []


def test_injected_runtime_keeps_running_game_protected(tmp_path):
    runtime = Mock()
    runtime.available.return_value = True
    process = Mock()
    process.poll.return_value = None
    runtime.launch.return_value = (process, tmp_path / "log")
    library = NesLibrary(tmp_path / "nes", asset_service=Mock(), runtime=runtime)
    game = add(library, source(tmp_path))
    assert game["runtime_status"] == "not-probed"
    library.launch(game, original=True)
    assert runtime.launch.call_args.args[1].name == "game.nes"
    with pytest.raises(RuntimeError, match="Close the NES player"):
        library.delete_game(game["id"])


def test_control_selection_accepts_saved_slots_and_default_alias_without_blocking_player(tmp_path):
    runtime = Mock()
    runtime.available.return_value = True
    process = Mock()
    process.poll.return_value = None
    runtime.launch.return_value = (process, tmp_path / "runtime.log")
    profiles = NesControlProfileStore(tmp_path / "config" / "nes.json", write_json)
    bindings = nes_default_seed()
    bindings["A"] = "KEY_Q"
    profiles.save_slot("slot-1", "Racing", bindings)
    library = NesLibrary(tmp_path / "nes", asset_service=Mock(), runtime=runtime,
                         control_profiles=profiles)
    game = add(library, source(tmp_path))
    original = library._read(game["id"])

    library.set_control_profile_id(game["id"], "slot-1")
    selected = library._read(game["id"])
    assert selected["control_profile_id"] == "slot-1"
    assert {key: value for key, value in selected.items() if key != "control_profile_id"} == original
    library.launch(game, controls={"A": "KEY_W"}, controls_fallback_reason="caller")
    first = runtime.launch.call_args.kwargs
    assert first["controls"] == {**nes_default_seed(), "A": "KEY_Q"}
    assert first["controls_fallback_reason"] is None

    # Selection affects the next launch only and remains editable while this player is open.
    bindings["A"] = "KEY_W"
    profiles.save_slot("slot-1", "Racing", bindings)
    process.poll.return_value = 0
    library.launch(game, original=True)
    second = runtime.launch.call_args.kwargs
    assert second["controls"] == {**nes_default_seed(), "A": "KEY_W"}
    assert second["original"] is True
    assert first["controls"]["A"] == "KEY_Q"

    library.set_control_profile_id(game["id"], None)
    assert "control_profile_id" not in library._read(game["id"])


def test_control_selection_rejects_unsaved_or_unavailable_profiles(tmp_path):
    profiles = NesControlProfileStore(tmp_path / "config" / "nes.json", write_json)
    library = NesLibrary(tmp_path / "nes", asset_service=Mock(), control_profiles=profiles)
    game = add(library, source(tmp_path))
    with pytest.raises(ValueError, match="existing saved"):
        library.set_control_profile_id(game["id"], "slot-2")
    without_store = NesLibrary(tmp_path / "other", asset_service=Mock())
    other = add(without_store, source(tmp_path, "Other.nes", value=2))
    with pytest.raises(ValueError, match="not available"):
        without_store.set_control_profile_id(other["id"], "slot-1")

    runtime = Mock()
    runtime.available.return_value = True
    runtime.launch.return_value = (Mock(), tmp_path / "runtime.log")
    without_store = NesLibrary(tmp_path / "other", asset_service=Mock(), runtime=runtime)
    row = without_store._read(other["id"])
    row["control_profile_id"] = "slot-1"
    without_store._write_metadata(without_store._metadata_path(other["id"]), row)
    without_store.launch(other)
    assert runtime.launch.call_args.kwargs["controls"] == nes_default_seed()
    assert runtime.launch.call_args.kwargs["controls_fallback_reason"] == (
        "NES control profile store is unavailable"
    )


def test_stale_profile_selection_loads_and_falls_back_at_launch(tmp_path):
    runtime = Mock()
    runtime.available.return_value = True
    process = Mock()
    process.poll.return_value = None
    runtime.launch.return_value = (process, tmp_path / "runtime.log")
    profiles = NesControlProfileStore(tmp_path / "config" / "nes.json", write_json)
    library = NesLibrary(tmp_path / "nes", asset_service=Mock(), runtime=runtime,
                         control_profiles=profiles)
    game = add(library, source(tmp_path))
    row = library._read(game["id"])
    row["control_profile_id"] = "slot-3"
    library._write_metadata(library._metadata_path(game["id"]), row)

    assert library.games()[0]["control_profile_id"] == "slot-3"
    library.launch(game, original=True)
    assert runtime.launch.call_args.kwargs["controls"] == nes_default_seed()
    assert "no longer saved" in runtime.launch.call_args.kwargs["controls_fallback_reason"]


@pytest.mark.parametrize("name", ["../game.nes", "C:game.nes", "NUL.nes", "game.nes.", "game/one.nes"])
def test_source_basename_validation(tmp_path, name):
    with pytest.raises(ValueError, match="basename"):
        GameStorage(tmp_path / "library", tmp_path / "rendered", source_filename=name)
