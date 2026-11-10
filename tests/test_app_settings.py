"""Application mode persistence does not disturb either game library."""

import json

import pytest

from systems.app_settings import ApplicationSettingsStore
from systems.contracts import JAVA_ME, NES


def test_visible_system_defaults_then_round_trips_with_injected_writer(tmp_path):
    writes=[]

    def writer(path, value):
        writes.append((path, value))
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(value), encoding="utf-8")

    path=tmp_path/'config'/'application.json'
    settings=ApplicationSettingsStore(path,writer)
    assert settings.load_visible_system()==JAVA_ME
    settings.save_visible_system(NES)
    assert writes==[(path,{"schema":1,"visible_system":NES})]
    assert ApplicationSettingsStore(path,writer).load_visible_system()==NES


@pytest.mark.parametrize("contents", [
    b"not json", b'{"schema":2,"visible_system":"nes"}',
    b'{"schema":1,"visible_system":"other"}', b" " * 4097,
    b'{"schema":1,"visible_system":"j2me","visible_system":"nes"}',
])
def test_invalid_application_settings_fall_back_without_overwriting_file(tmp_path,contents):
    path=tmp_path/'config'/'application.json'
    path.parent.mkdir()
    path.write_bytes(contents)
    settings=ApplicationSettingsStore(path,lambda *_:pytest.fail("read must not write"))
    assert settings.load_visible_system()==JAVA_ME
    assert path.read_bytes()==contents


def test_unknown_system_cannot_be_saved(tmp_path):
    settings=ApplicationSettingsStore(tmp_path/'application.json',lambda *_:None)
    with pytest.raises(ValueError,match="Unknown game system"):
        settings.save_visible_system("other")
