import pytest

from control_schema import JME_CONTROL_SCHEMA, NES_CONTROL_SCHEMA
from control_profiles import ControlProfileStore, default_seed
from persistence import atomic_json
from systems.nes.controls import NES_ACTION_ORDER, NesControlProfileStore, nes_default_seed


def test_schema_descriptors_keep_jme_and_nes_actions_separate():
    assert JME_CONTROL_SCHEMA.system_id == "j2me"
    assert NES_CONTROL_SCHEMA.system_id == "nes"
    assert NES_CONTROL_SCHEMA.actions == NES_ACTION_ORDER
    assert set(NES_CONTROL_SCHEMA.required_actions) == set(NES_ACTION_ORDER)
    assert JME_CONTROL_SCHEMA.default_seed() == default_seed()
    assert NES_CONTROL_SCHEMA.default_seed() == nes_default_seed()


def test_injected_create_factories_reject_the_other_system_shape():
    with pytest.raises(ValueError):
        NES_CONTROL_SCHEMA.create_profile("slot-1", "JME", default_seed())
    with pytest.raises(ValueError):
        JME_CONTROL_SCHEMA.create_profile("slot-1", "NES", nes_default_seed())


def test_schema_slots_are_fixed_and_default_is_not_an_editable_slot():
    assert JME_CONTROL_SCHEMA.custom_slot_ids == ("slot-1", "slot-2", "slot-3")
    assert NES_CONTROL_SCHEMA.custom_slot_ids == ("slot-1", "slot-2", "slot-3")
    assert NES_CONTROL_SCHEMA.immutable_default.slot_id == "default"


def test_same_slot_number_is_independent_in_both_profile_files(tmp_path):
    java = ControlProfileStore(tmp_path / "control-profiles.json", atomic_json)
    nes = NesControlProfileStore(tmp_path / "nes-control-profiles.json", atomic_json)
    java.save_slot("slot-1", "Java layout", default_seed())
    java_contents = java.path.read_bytes()
    assert nes.load().get("slot-1") is None

    nes.save_slot("slot-1", "NES layout", nes_default_seed())

    assert java.path.read_bytes() == java_contents
    assert java.load().get("slot-1").name == "Java layout"
    assert nes.load().get("slot-1").name == "NES layout"
