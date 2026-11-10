"""Cross-system worker results stay attached to their original game reference."""

from concurrent.futures import Future
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock

import pytest

from app import App
from systems.contracts import GameRef, JAVA_ME, NES
from systems.ui_events import ApplicationEvent


def test_events_are_frozen_and_validate_scoped_game_identity():
    ref=GameRef(NES,"b"*64)
    event=ApplicationEvent("imported",ref.system,ref.id,"New cartridge")
    assert event.ref==ref
    with pytest.raises(AttributeError):
        event.system=JAVA_ME
    with pytest.raises(ValueError):
        ApplicationEvent("deleted",NES,"not-an-id")


def test_hidden_mode_import_completion_does_not_change_visible_selection():
    app=App.__new__(App)
    app.visible_system_id=JAVA_ME
    app.search=SimpleNamespace(set=Mock())
    app.tree=SimpleNamespace(selection_set=Mock(),see=Mock(),exists=Mock(return_value=True))
    app.refresh=Mock()
    app.status=SimpleNamespace(set=Mock())

    App._handle_imported_event(app,ApplicationEvent("imported",NES,"b"*64,"Test cartridge"))

    app.refresh.assert_not_called()
    app.search.set.assert_not_called()
    app.tree.selection_set.assert_not_called()
    assert "NES library" in app.status.set.call_args.args[0]


def test_visible_mode_import_completion_selects_only_its_reference():
    ref=GameRef(JAVA_ME,"a"*64)
    app=App.__new__(App)
    app.visible_system_id=JAVA_ME
    app.search=SimpleNamespace(set=Mock())
    app.tree=SimpleNamespace(selection_set=Mock(),see=Mock(),exists=Mock(return_value=True))
    app.refresh=Mock()
    app.status=SimpleNamespace(set=Mock())

    App._handle_imported_event(app,ApplicationEvent("imported",ref.system,ref.id,"Test JAR"))

    app.refresh.assert_called_once_with()
    app.search.set.assert_called_once_with("")
    app.tree.selection_set.assert_called_once_with(ref.id)
    app.tree.see.assert_called_once_with(ref.id)


def test_hidden_mode_delete_completion_cleans_its_own_refs_only():
    ref=GameRef(NES,"b"*64)
    app=App.__new__(App)
    app.visible_system_id=JAVA_ME
    app._catalog=[{"id":ref.id,"system":NES}]
    app.deleting_games={ref.id:{"id":ref.id}}
    app._asset_stats={ref:{"READY":1}}
    app.players={ref.id:(object(),None)}
    app.player_refs={ref.id:ref}
    app._preview_key="java-preview"
    app.refresh=Mock()
    app.status=SimpleNamespace(set=Mock())

    App._handle_deleted_event(app,ApplicationEvent("deleted",NES,ref.id,"Test cartridge"))

    assert not app.deleting_games and not app._asset_stats
    assert not app._catalog
    assert not app.players and not app.player_refs
    assert app._preview_key=="java-preview"
    app.refresh.assert_not_called()


def test_delete_error_in_hidden_mode_does_not_refresh_visible_library(monkeypatch):
    ref=GameRef(NES,"b"*64)
    app=App.__new__(App)
    app.visible_system_id=JAVA_ME
    app.deleting_games={ref.id:{"id":ref.id}}
    app.refresh=Mock()
    app.status=SimpleNamespace(set=Mock())
    app.root=object()
    error=Mock()
    monkeypatch.setattr("app.messagebox.showerror",error)

    App._handle_delete_error_event(app,ApplicationEvent("delete_error",NES,ref.id,"Test cartridge\nlocked"))

    assert ref.id not in app.deleting_games
    app.refresh.assert_not_called()
    error.assert_called_once()


def test_system_specific_profile_store_is_used_for_the_selected_game():
    nes_store=SimpleNamespace(load=Mock(return_value=SimpleNamespace(slots=(),problem=None)))
    app=App.__new__(App)
    app.library=SimpleNamespace(control_profiles=SimpleNamespace(load=Mock()))
    app.nes_control_profiles=nes_store
    app.control_profile_combo=SimpleNamespace(configure=Mock())
    app.control_profile=SimpleNamespace(set=Mock())
    app.control_profile_info=SimpleNamespace(set=Mock())
    app.control_profile_choice_ids={}
    app.busy=False
    app.deleting_games={}

    App._refresh_control_profile_controls(app,{"id":"b"*64,"system":NES})

    nes_store.load.assert_called_once_with()
    app.library.control_profiles.load.assert_not_called()
    assert app.control_profile_choice_ids=={"Default":"default"}


def test_capture_tick_only_retries_active_players_awaiting_registration():
    app=App.__new__(App)
    running=GameRef(NES,"b"*64)
    closed=GameRef(JAVA_ME,"a"*64)
    ready=GameRef(JAVA_ME,"c"*64)
    app.root=Mock()
    app.library=Mock()
    app._pending_player_states={running,closed}
    active=Mock()
    active.poll.return_value=None
    app.players={running.id:(active,Path("nes.log")),ready.id:(active,Path("java.log"))}

    app.capture_tick()

    app.library.set_player_state.assert_called_once_with(
        running,True,Path("nes.stats.json"),allow_live=False)
    app.library.games.assert_not_called()
    app.library.resume_all.assert_not_called()
    assert not app._pending_player_states


def test_cover_completion_skips_stale_result_and_only_starts_latest_request():
    app=App.__new__(App)
    old=GameRef(JAVA_ME,"a"*64)
    current=GameRef(JAVA_ME,"b"*64)
    stale=Future()
    stale.set_result((None,10))
    app._preview_future=(((old,"pixel","old.png"),True),stale)
    app._preview_request=((current,"pixel","new.png"),True)
    app._preview_worker=Mock()
    app.library=Mock()
    app.card="#000000"
    app.game_if_selected=Mock(return_value={"id":current.id,"system":JAVA_ME})
    app._show_java_artwork=Mock()

    app._poll_java_preview()

    app._show_java_artwork.assert_not_called()
    app._preview_worker.submit.assert_called_once()
    assert app._preview_worker.submit.call_args.args[2:5]==(current,"pixel","new.png")


def test_search_changes_schedule_one_filter_without_reading_library():
    app=App.__new__(App)
    app.root=Mock()
    app.library=Mock()
    app._show_catalog=Mock()
    app._catalog_filter_pending=None

    app._schedule_catalog_filter()
    app._schedule_catalog_filter()
    app.root.after_idle.assert_called_once()
    app.root.after_idle.call_args.args[0]()

    app._show_catalog.assert_called_once_with()
    app.library.games.assert_not_called()
    assert app._catalog_filter_pending is None


@pytest.mark.parametrize("save_error", [None, OSError("read only")])
def test_system_switch_uses_saved_selection_and_loaded_catalog(save_error):
    app=App.__new__(App)
    app.visible_system_id=JAVA_ME
    app.system_choice=Mock()
    app.system_choice.get.return_value=NES
    app.app_settings=Mock()
    app.app_settings.save_visible_system.side_effect=save_error
    app._apply_system_palette=Mock()
    app._show_system_details=Mock()
    app._refresh_system_presentation=Mock()
    app._show_catalog=Mock()
    app.refresh=Mock()
    app.status=Mock()

    app.set_visible_system()

    app.app_settings.save_visible_system.assert_called_once_with(NES)
    app.refresh.assert_not_called()
    if save_error:
        assert app.visible_system_id==JAVA_ME
        app.system_choice.set.assert_called_once_with(JAVA_ME)
        app._apply_system_palette.assert_not_called()
    else:
        assert app.visible_system_id==NES
        app._apply_system_palette.assert_called_once_with()
        app._show_catalog.assert_called_once_with()
