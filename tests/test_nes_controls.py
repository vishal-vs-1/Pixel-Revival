import json

import pytest

from systems.nes.controls import (
    NES_ACTION_ORDER,
    NES_CUSTOM_SLOT_IDS,
    NES_DEFAULT_PROFILE,
    NesControlProfile,
    NesControlProfileStore,
    NesProfileValidationError,
    create_nes_profile,
    deserialize_nes_bindings,
    nes_default_seed,
    resolve_nes_selection,
    serialize_nes_bindings,
)


def text_json_writer(path, document):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(document, ensure_ascii=False), encoding="utf-8")


def new_store(tmp_path, writer=text_json_writer):
    return NesControlProfileStore(tmp_path / "config" / "nes-control-profiles.json", writer)


def profile(slot_id="slot-1", name="NES pad"):
    return create_nes_profile(slot_id, name, nes_default_seed())


def test_default_is_immutable_and_missing_file_resolves_default(tmp_path):
    result = new_store(tmp_path).load()

    assert result.slots == ()
    assert result.problem is None
    assert result.get("default") == NES_DEFAULT_PROFILE
    assert tuple(dict(NES_DEFAULT_PROFILE.bindings)) == NES_ACTION_ORDER
    with pytest.raises(AttributeError):
        NES_DEFAULT_PROFILE.name = "Changed"
    resolution = resolve_nes_selection("slot-2", result)
    assert resolution.profile == NES_DEFAULT_PROFILE
    assert "no longer saved" in resolution.reason


def test_three_slots_persist_in_fixed_order_and_replacement_is_scoped(tmp_path):
    store = new_store(tmp_path)
    store.save_slot("slot-3", "Three", nes_default_seed())
    store.save_slot("slot-1", "One", nes_default_seed())
    store.save_slot("slot-2", "Two", nes_default_seed())
    store.save_slot("slot-1", "Renamed", nes_default_seed())

    result = store.load()
    assert [item.slot_id for item in result.slots] == list(NES_CUSTOM_SLOT_IDS)
    assert [(item.slot_id, item.name) for item in result.slots] == [
        ("slot-1", "Renamed"), ("slot-2", "Two"), ("slot-3", "Three")
    ]
    assert result.get("default") == NES_DEFAULT_PROFILE


def test_invalid_or_wrong_schema_file_defaults_without_overwrite(tmp_path):
    path = tmp_path / "nes-control-profiles.json"
    path.write_text('{"schema":"control-profiles-v1","version":1,"slots":{}}', encoding="utf-8")
    before = path.read_bytes()

    store = NesControlProfileStore(path, text_json_writer)
    result = store.load()

    assert result.slots == ()
    assert "schema" in result.problem
    fallback = resolve_nes_selection("slot-1", result)
    assert fallback.profile == NES_DEFAULT_PROFILE
    assert fallback.reason
    with pytest.raises(NesProfileValidationError, match="Cannot replace"):
        store.save_slot("slot-1", "Replacement", nes_default_seed())
    assert path.read_bytes() == before


def test_validation_rejects_wrong_action_shape_and_duplicate_keys():
    missing = nes_default_seed()
    del missing["SELECT"]
    with pytest.raises(NesProfileValidationError, match="Missing actions"):
        create_nes_profile("slot-1", "Bad", missing)

    duplicate = nes_default_seed()
    duplicate["B"] = duplicate["A"]
    with pytest.raises(NesProfileValidationError, match="already assigned"):
        create_nes_profile("slot-1", "Bad", duplicate)


def test_nes_wire_round_trip_is_ascii_and_rejects_java_schema():
    encoded = serialize_nes_bindings(profile())
    assert encoded.isascii()
    assert encoded.startswith("nes-controls-v1|")
    assert deserialize_nes_bindings(encoded) == nes_default_seed()

    with pytest.raises(NesProfileValidationError, match="Only a NES"):
        serialize_nes_bindings(object())
    with pytest.raises(NesProfileValidationError, match="wire schema"):
        deserialize_nes_bindings("v1|UP=ARROW_UP")


def test_profile_values_are_snapshot_copies():
    bindings = nes_default_seed()
    snapshot = create_nes_profile("slot-1", "Snapshot", bindings)
    bindings["A"] = "KEY_Q"
    assert dict(snapshot.bindings)["A"] == "KEY_X"
    assert isinstance(snapshot, NesControlProfile)
