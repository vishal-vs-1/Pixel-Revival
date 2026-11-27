import threading
import time

from systems.nes.frame_presenter import FramePresenter


def test_slow_display_does_not_block_producer_and_discards_obsolete_pending_frames():
    entered, release, latest = threading.Event(), threading.Event(), threading.Event()
    painted = []

    def present(data, _width, _height):
        painted.append(data)
        if data == b'first':
            entered.set()
            assert release.wait(2)
        if data == b'latest':
            latest.set()

    worker = FramePresenter(present)
    try:
        assert worker.submit((b'first', 1, 1))
        assert entered.wait(1)
        # These calls must return while the native blit is still blocked.
        assert worker.submit((b'obsolete', 1, 1))
        assert worker.submit((b'latest', 1, 1))
        assert painted == [b'first']
        assert worker.skipped == 1
        release.set()
        assert latest.wait(1)
    finally:
        release.set()
        assert worker.close()
    assert painted == [b'first', b'latest']


def test_display_shutdown_does_not_claim_in_flight_resources_are_released():
    entered, release = threading.Event(), threading.Event()

    def present(*_):
        entered.set()
        assert release.wait(2)

    worker = FramePresenter(present)
    try:
        worker.submit((bytes(4), 1, 1))
        assert entered.wait(1)
        assert not worker.close(timeout=0)
        assert not worker.submit((bytes(4), 1, 1))
    finally:
        release.set()
        assert worker.close()


def test_native_present_failure_is_reported_and_stops_frame_admission():
    failed = threading.Event()

    def present(*_):
        failed.set()
        raise OSError('native window unavailable')

    worker = FramePresenter(present)
    worker.submit((bytes(4), 1, 1))
    assert failed.wait(1)
    assert worker.close()
    assert isinstance(worker.error, OSError)
    assert not worker.submit((bytes(4), 1, 1))


def test_paced_display_spaces_buffered_frames_instead_of_showing_a_burst():
    first, second = threading.Event(), threading.Event()
    painted = []

    def present(data, *_):
        painted.append((data, time.monotonic()))
        (first if data == b'first' else second).set()

    worker = FramePresenter(present, frame_interval=0.1)
    try:
        worker.submit((b'first', 1, 1))
        worker.submit((b'second', 1, 1))
        assert not first.wait(.02)
        assert first.wait(1)
        assert second.wait(1)
    finally:
        assert worker.close()
    assert [data for data, _ in painted] == [b'first', b'second']
    assert painted[1][1] - painted[0][1] >= .08
