"""Failure-path checks for review fixes, without windows or game execution."""

from io import BytesIO
import os
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock
import zipfile

import pytest
from PIL import Image

from app import App
from launcher.playback import LauncherPlayback
from launcher.readiness import describe_readiness
from runtime_assets.contracts import SourceRecord, image_source_id, pixel_recipe
from runtime_assets.service import AssetService
from runtime_assets.store import AssetStore
from runtime_assets.work_state import _Batch, _Game, _Outcome, _Work
from systems.jme.library import Library
from systems.nes.rust_core import find_dll
from systems.playback_session import JavaProcess, PlaybackSession


def test_launcher_retains_ownership_until_standalone_java_exits():
    raw = SimpleNamespace(stdin=BytesIO(), poll=Mock(return_value=None))
    process = JavaProcess(raw)
    app = App.__new__(App)
    app.root = Mock()
    app.players = {'fixture': PlaybackSession(None, process, Path('fixture.log'))}
    app.library = Mock()
    app.nes_runtime = None
    app._delete_workers = Mock()
    app._preview_worker = Mock()
    app.close()
    assert raw.stdin.getvalue() == b'!CLOSE\n'
    app.root.destroy.assert_not_called()
    app.library.close.assert_not_called()
    app.close()
    assert raw.stdin.getvalue() == b'!CLOSE\n'
    raw.poll.return_value = 0
    app.close()
    app.root.destroy.assert_called_once()


def test_unknown_live_player_never_gets_killed_to_exit(monkeypatch):
    app = App.__new__(App)
    app.root = Mock()
    app.library = Mock()
    app.players = {'fixture': PlaybackSession(None, SimpleNamespace(poll=lambda: None), Path('log'))}
    notice = Mock()
    monkeypatch.setattr('app.messagebox.showerror', notice)
    app.close()
    notice.assert_called_once()
    app.root.destroy.assert_not_called()


def test_shutdown_waits_for_close_import_event():
    app = App.__new__(App)
    app.root = Mock()
    app.library = Mock()
    app.players = {}
    app._recording_players = {'fixture': True}
    app.close()
    app.root.after.assert_called_once()
    app.root.destroy.assert_not_called()


@pytest.mark.parametrize('failure_method', ['mark_ready_batch', 'mark_failed', 'defer', 'reschedule'])
def test_completion_write_retries_without_running_conversion_again(tmp_path, monkeypatch, failure_method):
    recipe = pixel_recipe()
    store = AssetStore(tmp_path)
    store.register_recipe(recipe)
    image = Image.new('RGBA', (2, 2), (40, 60, 80, 255))
    source = SourceRecord(image_source_id(image), 2, 2, 'original-assets/fixture.png', 'imported')
    (tmp_path / 'original-assets').mkdir()
    image.save(tmp_path / source.relative_path)
    store.register_source(source)
    store.enqueue(source.source_id, recipe.key)
    lease = store.claim('fixture', lease_seconds=90)
    game = _Game(tmp_path, store, recipe)
    work = _Work(game, lease, source, recipe)
    image.resize((8, 8)).save(tmp_path / 'result.png')
    options = dict(output='result.png', error=None)
    if failure_method != 'mark_ready_batch':
        options.update(output=None, error='fixture conversion error')
    options.update(resource_defer=failure_method == 'defer', retry_single=failure_method == 'reschedule')
    service = AssetService(lambda _: pytest.fail('Conversion must not run for a completion retry'))
    service._games[tmp_path.name] = game
    service._active = _Batch((work,))
    service._outcomes.put(_Outcome(work, elapsed=0.1, **options))
    original = getattr(store, failure_method)
    monkeypatch.setattr(store, failure_method, Mock(side_effect=OSError('database unavailable')))
    service._drain_outcomes()
    assert service._active is not None
    assert service._outcomes.qsize() == 1
    assert 'database unavailable' in service.stats(tmp_path.name)['completion_error']
    monkeypatch.setattr(store, failure_method, original)
    service._completion_retry_at = 0
    service._drain_outcomes()
    assert service._active is None
    assert service._outcomes.empty()
    assert not game.completion_error
    assert store.counts(recipe.key)['RUNNING'] == 0
    assert service.close()


def test_next_dll_requires_explicit_selection(tmp_path, monkeypatch):
    monkeypatch.delenv('RUST_NES_DLL', raising=False)
    release = tmp_path / 'build/rust-nes-target/release'
    release.mkdir(parents=True)
    standard, experiment = release / 'mesen_player.dll', release / 'mesen_player_next.dll'
    experiment.write_bytes(b'fixture')
    assert find_dll(tmp_path) is None
    standard.write_bytes(b'fixture')
    os.utime(experiment, (2_000_000_000, 2_000_000_000))
    assert find_dll(tmp_path) == standard
    monkeypatch.setenv('RUST_NES_DLL', str(experiment))
    assert find_dll(tmp_path) == experiment


def test_catalog_reads_never_run_migration_and_service_is_injected(tmp_path, monkeypatch):
    service = Mock()
    monkeypatch.setattr('systems.jme.library.AssetService', Mock(side_effect=AssertionError('Unexpected service owner')))
    library = Library(tmp_path / 'library', tmp_path / 'rendered', asset_service=service)
    folder = library.workspace('a' * 64)
    folder.mkdir()
    (folder / 'game.json').write_text('{"id":"' + 'a' * 64 + '","name":"Fixture"}')
    library._migrate_catalog_entry = Mock(side_effect=AssertionError('Catalog read performed migration'))
    library.prepare_ui_assets = Mock(side_effect=AssertionError('Catalog read prepared assets'))
    before = (folder / 'game.json').read_bytes()
    assert library.games()[0]['name'] == 'Fixture'
    assert library.asset_service is service
    assert (folder / 'game.json').read_bytes() == before


def test_recording_off_keeps_image_replacement_independent():
    launcher = LauncherPlayback()
    launcher.image_play = SimpleNamespace(get=lambda: True)
    launcher.poses_play = SimpleNamespace(get=lambda: False)
    assert launcher._sprite_playback_options({'id': 'a' * 64, 'record_artwork': False}) == {'pose_mode': 'replace'}
    assert launcher._sprite_playback_options({'id': 'a' * 64}) == {'pose_mode': 'record-replace'}


def test_readiness_distinguishes_setup_artwork_and_recording():
    text = describe_readiness({'system': 'nes', 'runtime_status': 'setup-needed', 'record_artwork': False,
        'asset_work': {'READY': 12, 'completion_error': 'Disk unavailable'}}, image=True)
    assert 'setup or preparation needed' in text
    assert 'Base artwork: prepared' in text
    assert 'Character art: image' in text
    assert 'experimental' in text
    assert 'Recording on next launch: off' in text
    assert 'Disk unavailable' in text


def test_catalog_migration_keeps_concurrent_preferences(tmp_path):
    library = Library(tmp_path / 'library', tmp_path / 'rendered', asset_service=Mock())
    folder = library.workspace('a' * 64)
    folder.mkdir()
    path = folder / 'game.json'
    path.write_text('{"id":"' + 'a' * 64 + '","name":"New name","record_artwork":false}')
    migrated = library._update_metadata_fields(path, {'ui_recipe': 'smooth-text-v1'})
    assert migrated['name'] == 'New name'
    assert migrated['record_artwork'] is False
    assert migrated['ui_recipe'] == 'smooth-text-v1'
    library._update_metadata_fields(path, {'width': 200, 'height': 300, 'screen_source': 'manual'})
    migrated = library._update_metadata_fields(path,
        {'width': 176, 'height': 220, 'screen_source': 'profile'}, default_screen_only=True)
    assert (migrated['width'], migrated['height'], migrated['screen_source']) == (200, 300, 'manual')


def test_database_unavailability_does_not_hide_completion_error(tmp_path, monkeypatch):
    recipe = pixel_recipe()
    store = AssetStore(tmp_path)
    game = _Game(tmp_path, store, recipe, completion_error='Result could not be saved')
    game.counts_snapshot = {'READY': 7}
    service = AssetService(Mock())
    service._games[tmp_path.name] = game
    monkeypatch.setattr(store, 'counts', Mock(side_effect=OSError('offline')))
    stats = service.stats(tmp_path.name)
    assert stats['counts_unavailable']
    assert stats['READY'] == 7
    assert stats['completion_error'] == 'Result could not be saved'


@pytest.mark.parametrize('control', [b'old class', b'jme.launcherControl'])
def test_package_dependency_check_requires_current_java_and_native_identity(tmp_path, monkeypatch, control):
    from tools.runtime_check import check_runtime
    (tmp_path / 'build').mkdir()
    java = tmp_path / 'tools/runtime/bin/java.exe'
    java.parent.mkdir(parents=True)
    java.touch()
    with zipfile.ZipFile(tmp_path / 'build/jme-player.jar', 'w') as jar:
        jar.writestr('org/recompile/mobile/ComparisonInput.class', control)
        jar.writestr('org/recompile/mobile/GpuPlayer.class', b'fixture')
    core = Mock(return_value=SimpleNamespace(identity={'sha256': 'fixture', 'abi': 1}))
    monkeypatch.setattr('systems.nes.rust_core.RustCore', core)
    if control == b'old class':
        with pytest.raises(RuntimeError, match='rebuilding'):
            check_runtime(tmp_path)
        core.assert_not_called()
    else:
        assert check_runtime(tmp_path)['abi'] == 1
        core.assert_called_once_with(app_root=tmp_path)


def test_session_timeout_keeps_ownership_without_forced_termination(monkeypatch):
    process = SimpleNamespace(poll=lambda: None, request_close=Mock(), terminate=Mock())
    session = PlaybackSession(None, process, Path('log'))
    clock = Mock(return_value=1.0)
    monkeypatch.setattr('systems.playback_session.time.monotonic', clock)
    assert session.request_close() is False
    clock.return_value = 12.0
    with pytest.raises(RuntimeError, match='still saving'):
        session.request_close()
    process.request_close.assert_called_once()
    process.terminate.assert_not_called()


def test_shutdown_does_not_drain_while_coordinator_still_owns_outcomes():
    service = AssetService(Mock())
    service._coordinator = Mock()
    service._coordinator.is_alive.return_value = True
    service._drain_outcomes = Mock()
    assert service.close(timeout=0) is False
    service._drain_outcomes.assert_not_called()
    service._coordinator.is_alive.return_value = False
    assert service.close(timeout=0) is True
    service._drain_outcomes.assert_called_once()
