from types import SimpleNamespace
from unittest.mock import Mock, call

import pytest

from systems.nes import thread_scheduling


@pytest.mark.parametrize('fail', [False, True])
def test_priority_is_scoped_to_the_current_thread_and_restored_on_exit(monkeypatch, fail):
    api = SimpleNamespace(GetCurrentThread=Mock(return_value=123),
                          GetThreadPriority=Mock(return_value=0),
                          SetThreadPriority=Mock(return_value=True))
    monkeypatch.setattr(thread_scheduling, '_priority_api', lambda: api)
    try:
        with thread_scheduling.playback_thread_priority():
            api.SetThreadPriority.assert_called_once_with(123, 1)
            if fail:
                raise ValueError('worker failed')
    except ValueError:
        assert fail
    assert api.SetThreadPriority.call_args_list == [call(123, 1), call(123, 0)]


@pytest.mark.parametrize('priority', [1, 2, 0x7FFFFFFF])
def test_existing_higher_priority_and_failed_queries_are_untouched(monkeypatch, priority):
    api = SimpleNamespace(GetCurrentThread=lambda: 123,
                          GetThreadPriority=lambda _thread: priority,
                          SetThreadPriority=Mock())
    monkeypatch.setattr(thread_scheduling, '_priority_api', lambda: api)
    with thread_scheduling.playback_thread_priority():
        pass
    api.SetThreadPriority.assert_not_called()


@pytest.mark.parametrize('unavailable', [False, True])
def test_missing_platform_support_cannot_interrupt_playback(monkeypatch, unavailable):
    def platform():
        if unavailable:
            raise OSError('unavailable')
        return None

    monkeypatch.setattr(thread_scheduling, '_priority_api', platform)
    with thread_scheduling.playback_thread_priority():
        reached = True
    assert reached
