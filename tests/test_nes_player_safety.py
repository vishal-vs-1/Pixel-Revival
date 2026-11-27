"""Regression checks for native ownership, paired input and audio starvation."""

import threading
from types import SimpleNamespace
from unittest.mock import Mock
import pytest

from systems.nes.comparison_clock import ComparisonClock
from systems.nes.rust_audio import RustAudioOut
from systems.nes.rust_player import RustNesPlayer
from systems.nes.library import NesLibrary


def test_original_can_capture_tiles_without_loading_an_enhanced_pack():
    player = RustNesPlayer.__new__(RustNesPlayer)
    player._core = Mock(capture_supported=True)
    player._core.hd_create.return_value = 10
    player._core.hd_output_size.return_value = (512, 480)
    player._handle = 20
    player._hd = None
    player._capture_armed = False
    player._write_log = Mock()
    player._setup_original_capture()
    assert player._hd is None
    assert player._capture_hd == 10
    assert len(player._capture_hd_buf) == 512 * 480 * 4
    player._core.hd_arm.assert_called_once_with(20, True)
    player._core.capture_arm.assert_called_once_with(20, True)
    assert player._capture_armed
    player._close_capture_table()
    player._core.hd_destroy.assert_called_once_with(10)
    assert player._capture_hd is None


def test_ui_controller_events_do_not_call_into_a_running_native_core():
    player = RustNesPlayer.__new__(RustNesPlayer)
    player._held = set()
    player._mask = 0
    player._core = Mock()
    player.press('RIGHT')
    player.press('A')
    assert player._mask == 129
    player.release_all()
    assert player._mask == 0
    player._core.set_controller.assert_not_called()


def test_audio_never_reuses_a_header_until_windows_returns_it():
    audio = RustAudioOut()
    audio._winmm = Mock()
    event = Mock()

    def not_returned(**_):
        audio._stop.set()
        return False

    event.wait.side_effect = not_returned
    audio._events = [event]
    audio._feed()
    audio._winmm.waveOutWrite.assert_not_called()
    event.clear.assert_not_called()


def test_comparison_muted_side_does_not_open_device_or_queue_audio():
    audio = RustAudioOut(enabled=False)
    audio._open = Mock()
    audio.start()
    for _ in range(100):
        audio.feed(bytes(2048))
    audio._open.assert_not_called()
    assert audio._queue.empty()
    assert audio.muted and not audio.playing


def test_paired_clock_waits_for_both_panes_and_shares_one_input_sample():
    clock = ComparisonClock()
    entered = threading.Event()
    samples = []

    def pane():
        entered.set()
        samples.append(clock.next_input())

    worker = threading.Thread(target=pane)
    worker.start()
    assert entered.wait(1)
    assert not samples
    clock.mask = 129
    assert clock.next_input() == 129
    worker.join(1)
    assert samples == [129]


def test_closing_comparison_unblocks_a_waiting_pane():
    clock = ComparisonClock()
    entered = threading.Event()
    stopped = threading.Event()

    def pane():
        entered.set()
        try:
            clock.next_input()
        except threading.BrokenBarrierError:
            stopped.set()

    worker = threading.Thread(target=pane)
    worker.start()
    assert entered.wait(1)
    clock.close()
    worker.join(1)
    assert stopped.is_set()


def test_close_does_not_free_native_pointers_while_a_frame_is_still_running():
    player = RustNesPlayer.__new__(RustNesPlayer)
    player._stop = threading.Event()
    player._comparison_clock = None
    player._pump = Mock()
    player._pump.is_alive.return_value = True
    player._core = Mock()
    player._flush_capture = Mock()
    player.window = Mock()
    player.close()
    player._core.destroy.assert_not_called()
    player._flush_capture.assert_not_called()
    assert player._stop.is_set()
    player.window.after.assert_called_once_with(20, player.close)


def test_close_keeps_screen_resources_while_a_present_is_still_running():
    player = RustNesPlayer.__new__(RustNesPlayer)
    player._stop = threading.Event()
    player._comparison_clock = None
    player._pump = None
    player._presenter = Mock()
    player._presenter.close.return_value = False
    player._core = Mock()
    player._native_view = Mock()
    player._flush_capture = Mock()
    player.window = Mock()
    player.close()
    player._native_view.close.assert_not_called()
    player._core.destroy.assert_not_called()
    player._flush_capture.assert_not_called()
    assert player._stop.is_set()
    player.window.after.assert_called_once_with(20, player.close)


def test_display_counts_survive_worker_shutdown():
    player = RustNesPlayer.__new__(RustNesPlayer)
    player._presenter = SimpleNamespace(close=lambda: True, presented=123, skipped=4)
    assert player._close_presenter()
    assert player._presenter is None
    assert (player._presented_frames, player._skipped_presents) == (123, 4)


def test_comparison_keeps_parent_until_a_slow_pane_finishes_closing():
    from systems.nes.rust_comparison import RustComparisonPlayers

    pair = RustComparisonPlayers.__new__(RustComparisonPlayers)
    pair.expected_exit = False
    pair.clock = ComparisonClock()
    pair._watch_job = None
    pair.release_all = Mock()
    pair.window = Mock()
    slow = SimpleNamespace(_handle=object(), close=Mock())
    finished = SimpleNamespace(_handle=None, close=Mock())
    pair.players = [slow, finished]
    pair.close()
    pair.window.destroy.assert_not_called()
    pair.window.after.assert_called_once_with(20, pair._finish_close)
    assert pair.expected_exit and pair.clock.closed
    slow._handle = None
    pair._finish_close()
    pair._finish_close()
    pair.window.destroy.assert_called_once()


def test_an_old_background_save_cannot_overwrite_the_final_battery_snapshot(tmp_path):
    player = RustNesPlayer.__new__(RustNesPlayer)
    player._battery_lock = threading.Lock()
    player._battery_generation = 2
    path = tmp_path / 'game.sav'
    player._battery_path = lambda: path
    player._write_log = Mock()
    player._write_battery_file(b'new final save', 2)
    player._write_battery_file(b'old pending save', 1)
    assert path.read_bytes() == b'new final save'
    assert list(tmp_path.iterdir()) == [path]


@pytest.mark.parametrize('fail', [False, True])
def test_pose_import_protects_its_game_without_holding_the_library_lock(tmp_path, fail):
    entered, finish = threading.Event(), threading.Event()
    results = []

    def import_poses(*_args, **_kwargs):
        entered.set()
        assert finish.wait(2)
        if fail:
            raise ValueError('invalid recording')
        return 6

    library = NesLibrary(tmp_path, asset_service=Mock(),
                         playable_assets=SimpleNamespace(import_recording=import_poses))
    identity = 'a' * 64

    def worker():
        try:
            results.append(library.import_playable_recording(identity))
        except ValueError as error:
            results.append(str(error))

    thread = threading.Thread(target=worker)
    thread.start()
    try:
        assert entered.wait(1)
        assert library._lock.acquire(blocking=False)
        library._lock.release()
        with pytest.raises(RuntimeError, match='poses to finish'):
            library._require_idle(identity)
        library._require_idle('b' * 64)
    finally:
        finish.set()
        thread.join(2)
    assert not library._pose_imports
    assert results == ['invalid recording' if fail else 6]
