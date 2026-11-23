from io import BytesIO
from threading import RLock
from types import SimpleNamespace
from unittest.mock import Mock

from launcher.comparison import ComparisonWindow, comparison_key
from systems.comparison import ComparisonPlayers
from systems.jme.playback import JavaMePlayback


def test_java_comparison_copies_saves_and_tracks_partial_launch(tmp_path):
    playback = JavaMePlayback()
    playback.lock = RLock()
    playback._players = {}
    playback._check_not_deleting = Mock()
    playback.workspace = lambda _identity: tmp_path
    original_save = tmp_path / "saves" / "rms" / "game.rms"
    original_save.parent.mkdir(parents=True)
    original_save.write_bytes(b"initial save")
    original_player = Mock()
    original_player.poll.return_value = None
    save_folders = []

    def launch(_game, original, *, comparison_saves):
        save_folders.append(comparison_saves)
        current_save = comparison_saves / "rms" / "game.rms"
        assert current_save.read_bytes() == b"initial save"
        if not original:
            raise OSError("could not start Enhanced")
        current_save.write_bytes(b"test progress")
        return original_player, tmp_path / "original.log"

    playback._launch = launch
    pair, log = playback.launch({"id": "test"}, comparison=True)
    assert playback._players["test"] is pair
    assert pair.startup_error == "could not start Enhanced"
    assert log.name == "original.log"
    assert len(set(save_folders)) == 2
    assert original_save.read_bytes() == b"initial save"
    assert pair.poll() is None
    original_player.poll.return_value = 0
    assert pair.poll() == 0


def test_comparison_close_is_not_reported_as_a_player_crash():
    players = [Mock(), Mock()]
    for player in players:
        player.poll.return_value = None
    pair = ComparisonPlayers(lambda original: (players[0 if original else 1], None))
    assert pair.poll() is None
    for player in players:
        player.poll.return_value = 1
    pair.expected_exit = True
    assert pair.poll() == 0
    pair.expected_exit = False
    assert pair.poll() == 1


def test_comparison_window_close_marks_the_pair_as_expected_to_exit():
    def view():
        window = ComparisonWindow.__new__(ComparisonWindow)
        window.closing = False
        window.pair = SimpleNamespace(players=[], expected_exit=False)
        window._log = Mock()
        window._release_all = Mock()
        window.hint = Mock()
        window.native = Mock()
        window.native.find_players.return_value = {}
        window.windows = {}
        window.tick_job = Mock()
        window.window = Mock()
        return window

    closed = view()
    closed.close()
    assert closed.pair.expected_exit is True

    crashed = view()
    crashed.close(expected=False)
    assert crashed.pair.expected_exit is False


def test_comparison_sends_identical_key_transitions_to_both_players():
    players = [SimpleNamespace(stdin=BytesIO()), SimpleNamespace(stdin=BytesIO())]
    pair = ComparisonPlayers(lambda original: (players[0 if original else 1], None))
    for key, pressed in (("Left", True), ("KP_End", True), ("Left", False), ("KP_1", False)):
        pair.send_key(comparison_key(key), pressed)
    expected = b"+ARROW_LEFT\n+NUMPAD_1\n-ARROW_LEFT\n-NUMPAD_1\n"
    assert [player.stdin.getvalue() for player in players] == [expected, expected]


def test_comparison_holds_key_until_release_and_clears_on_focus_loss():
    view = ComparisonWindow.__new__(ComparisonWindow)
    view.input_ready = True
    view.closing = False
    view.held = set()
    view._enqueue = Mock()
    for key in ("Up", "Up", "q"):
        view._press(SimpleNamespace(keysym=key))
    view._release(SimpleNamespace(keysym="q"))
    view._release_all()
    assert [call.args for call in view._enqueue.call_args_list] == [
        ("ARROW_UP", True), ("KEY_Q", True), ("KEY_Q", False), ("ARROW_UP", False),
    ]
    assert not view.held


def test_comparison_reapplies_borderless_layout_and_tracks_new_handles():
    from unittest.mock import call
    view = ComparisonWindow.__new__(ComparisonWindow)
    original = SimpleNamespace(pid=11, poll=Mock(return_value=None))
    enhanced = SimpleNamespace(pid=22, poll=Mock(return_value=None))
    view.pair = SimpleNamespace(players=[original, enhanced])
    view.windows = {11: 101, 22: 202}
    panes = [SimpleNamespace(winfo_id=Mock(return_value=1001 + index),
                             winfo_width=Mock(return_value=640),
                             winfo_height=Mock(return_value=480)) for index in range(2)]
    view.panes = panes
    native = Mock()
    native.find_players.return_value = {11: 101, 22: 303}
    native.ensure.return_value = []
    view.native = native

    view._enforce([original, enhanced])

    native.embed.assert_called_once_with(303, panes[1].winfo_id())
    assert view.windows == {11: 101, 22: 303}
    assert native.ensure.call_args_list == [
        call(101, panes[0].winfo_id(), 640, 480),
        call(303, panes[1].winfo_id(), 640, 480),
    ]


def test_comparison_skips_enforcement_while_a_player_window_is_missing():
    view = ComparisonWindow.__new__(ComparisonWindow)
    original = SimpleNamespace(pid=11, poll=Mock(return_value=None))
    enhanced = SimpleNamespace(pid=22, poll=Mock(return_value=None))
    view.pair = SimpleNamespace(players=[original, enhanced])
    view.windows = {11: 101, 22: 202}
    view.panes = [Mock(), Mock()]
    native = Mock()
    native.find_players.return_value = {11: 101}
    view.native = native

    view._enforce([original, enhanced])

    native.embed.assert_not_called()
    native.ensure.assert_not_called()
    assert view.windows == {11: 101, 22: 202}


def test_comparison_logs_attach_and_corrections(tmp_path):
    view = ComparisonWindow.__new__(ComparisonWindow)
    view._log_path = str(tmp_path / "comparison-window.log")
    view._log("comparison-window v3 attach watching original=11 enhanced=22")
    native = Mock()
    native.style.return_value = 0x14CF0000
    native.parent_of.return_value = 777
    native.find_players.return_value = {11: 101, 22: 202}
    view.native = native
    view.windows = {}
    panes = [SimpleNamespace(winfo_id=Mock(return_value=1001 + index),
                             winfo_width=Mock(return_value=640),
                             winfo_height=Mock(return_value=480)) for index in range(2)]
    view.panes = panes
    view.pair = SimpleNamespace(players=[SimpleNamespace(pid=11, poll=Mock(return_value=None)),
                                        SimpleNamespace(pid=22, poll=Mock(return_value=None))])

    view._attach(view.pair.players)

    log = (tmp_path / "comparison-window.log").read_text(encoding="utf-8")
    assert "attach pid=11 window=101" in log
    assert "attach pid=22 window=202" in log
    assert "style=0x" in log


def _tiled_view(players, windows):
    view = ComparisonWindow.__new__(ComparisonWindow)
    view.embed_players = False
    view.pair = SimpleNamespace(players=players)
    view.windows = dict(windows)
    view.native = Mock()
    view._log_path = None
    return view


def test_tiled_attach_places_original_left_and_enhanced_right():
    original = SimpleNamespace(pid=11, poll=Mock(return_value=None))
    enhanced = SimpleNamespace(pid=22, poll=Mock(return_value=None))
    view = _tiled_view([original, enhanced], {})
    view.native.find_players.return_value = {11: 101, 22: 202}
    view.native.tile_pair.return_value = ((0, 0, 640, 480), (640, 0, 1280, 480))

    view._attach([original, enhanced])

    assert view.attached is True
    view.native.tile_pair.assert_called_once_with(101, 202)
    assert view.windows == {11: 101, 22: 202}


def test_tiled_enforce_replaces_a_changed_handle_and_restores_minimized():
    original = SimpleNamespace(pid=11, poll=Mock(return_value=None))
    enhanced = SimpleNamespace(pid=22, poll=Mock(return_value=None))
    view = _tiled_view([original, enhanced], {11: 101, 22: 202})
    view.native.find_players.return_value = {11: 101, 22: 303}
    view.native.halves.return_value = ((0, 0, 640, 480), (640, 0, 1280, 480))
    view.native.minimized.return_value = False

    view._enforce([original, enhanced])

    view.native.place.assert_called_once_with(303, (640, 0, 1280, 480))
    assert view.windows == {11: 101, 22: 303}

    view.native.reset_mock()
    view.native.find_players.return_value = {11: 101, 22: 303}
    view.native.minimized.side_effect = [True, False]
    logged = []
    view._log = logged.append

    view._enforce([original, enhanced])

    view.native.place.assert_called_once_with(101, (0, 0, 640, 480))
    assert any("restore pid=11" in message for message in logged)


def test_tiled_enforce_leaves_placed_windows_alone():
    original = SimpleNamespace(pid=11, poll=Mock(return_value=None))
    enhanced = SimpleNamespace(pid=22, poll=Mock(return_value=None))
    view = _tiled_view([original, enhanced], {11: 101, 22: 202})
    view.native.find_players.return_value = {11: 101, 22: 202}
    view.native.halves.return_value = ((0, 0, 640, 480), (640, 0, 1280, 480))
    view.native.minimized.return_value = False

    view._enforce([original, enhanced])

    view.native.place.assert_not_called()


def test_close_restores_top_level_before_posting_close():
    from launcher.comparison_native import ComparisonNativeWindows
    native = ComparisonNativeWindows.__new__(ComparisonNativeWindows)
    user = Mock()
    user.GetWindowLongPtrW.return_value = 0x56000000
    user.IsWindow.return_value = True
    native._user = user

    native.close(101)

    user.SetWindowLongPtrW.assert_called_once_with(101, -16, 0x16CF0000)
    user.SetParent.assert_called_once_with(101, None)
    args = user.SetWindowPos.call_args.args
    assert args[:6] == (101, None, 0, 0, 0, 0)
    assert args[6] & 0x0020  # frame change applied
    user.PostMessageW.assert_called_once_with(101, 0x0010, 0, 0)
