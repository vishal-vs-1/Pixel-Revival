"""Injected control-schema descriptors shared by the settings editor."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Callable

from control_profiles import (
    ACTION_ORDER,
    CUSTOM_SLOT_IDS,
    REQUIRED_ACTIONS,
    SUPPORTED_KEYS,
    ProfileValidationError,
    create_profile,
    default_seed,
)
from systems.nes.controls import (
    NES_ACTION_ORDER,
    NES_CUSTOM_SLOT_IDS,
    NES_DEFAULT_PROFILE,
    NES_SUPPORTED_KEYS,
    NesProfileValidationError,
    create_nes_profile,
    nes_default_seed,
)


@dataclass(frozen=True)
class ControlSchema:
    """Immutable editor metadata with injected validation and seed factories."""

    system_id: str
    title: str
    instructions: str
    actions: tuple[str, ...]
    custom_slot_ids: tuple[str, ...]
    required_actions: frozenset[str]
    supported_keys: frozenset[str]
    default_seed: Callable[[], dict[str, str]]
    create_profile: Callable[[str, str, object], object]
    validation_error: type[ValueError]
    immutable_default: object


JME_CONTROL_SCHEMA = ControlSchema(
    system_id="j2me",
    title="Keyboard profiles",
    instructions="Create up to three shared profiles. Choose a saved profile for each game in its Play tab.",
    actions=ACTION_ORDER,
    custom_slot_ids=CUSTOM_SLOT_IDS,
    required_actions=REQUIRED_ACTIONS,
    supported_keys=SUPPORTED_KEYS,
    default_seed=default_seed,
    create_profile=create_profile,
    validation_error=ProfileValidationError,
    immutable_default=None,
)


NES_CONTROL_SCHEMA = ControlSchema(
    system_id="nes",
    title="NES controller profiles",
    instructions="Create up to three shared NES controller profiles. Choose one when launching a NES game.",
    actions=NES_ACTION_ORDER,
    custom_slot_ids=NES_CUSTOM_SLOT_IDS,
    required_actions=frozenset(NES_ACTION_ORDER),
    supported_keys=NES_SUPPORTED_KEYS,
    default_seed=nes_default_seed,
    create_profile=create_nes_profile,
    validation_error=NesProfileValidationError,
    immutable_default=NES_DEFAULT_PROFILE,
)
