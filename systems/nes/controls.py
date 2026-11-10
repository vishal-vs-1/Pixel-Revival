"""Pure NES control profiles and their isolated persistence boundary."""

from __future__ import annotations

from dataclasses import dataclass
import json
from pathlib import Path
import threading
from typing import Callable, Mapping


NES_DEFAULT_SLOT_ID = "default"
NES_CUSTOM_SLOT_IDS = ("slot-1", "slot-2", "slot-3")
NES_PROFILE_SCHEMA = "nes-control-profiles-v1"
NES_WIRE_SCHEMA = "nes-controls-v1"
MAX_DOCUMENT_BYTES = 16 * 1024
MAX_BINDINGS_ARGUMENT = 1024

NES_ACTION_ORDER = (
    "UP", "DOWN", "LEFT", "RIGHT", "A", "B", "START", "SELECT",
)

NES_DEFAULT_SEED = (
    ("UP", "ARROW_UP"),
    ("DOWN", "ARROW_DOWN"),
    ("LEFT", "ARROW_LEFT"),
    ("RIGHT", "ARROW_RIGHT"),
    ("A", "KEY_X"),
    ("B", "KEY_Z"),
    ("START", "ENTER"),
    ("SELECT", "SPACE"),
)

_LETTERS = frozenset(f"KEY_{letter}" for letter in "ABCDEFGHIJKLMNOPQRSTUVWXYZ")
_DIGITS = frozenset(f"DIGIT_{digit}" for digit in "0123456789")
_NUMPAD = frozenset(f"NUMPAD_{digit}" for digit in "0123456789")
_ARROWS = frozenset(("ARROW_UP", "ARROW_DOWN", "ARROW_LEFT", "ARROW_RIGHT"))
NES_SUPPORTED_KEYS = _LETTERS | _DIGITS | _NUMPAD | _ARROWS | frozenset(
    ("ENTER", "NUMPAD_ENTER", "SPACE")
)
_ACTION_SET = frozenset(NES_ACTION_ORDER)


class NesProfileValidationError(ValueError):
    """A NES profile value or persisted document violates its contract."""


@dataclass(frozen=True)
class NesControlProfile:
    """An immutable eight-action pad mapping."""

    slot_id: str
    name: str
    bindings: tuple[tuple[str, str], ...]

    def __post_init__(self) -> None:
        object.__setattr__(self, "bindings", tuple(tuple(pair) for pair in self.bindings))


@dataclass(frozen=True)
class NesProfileLoadResult:
    """Saved custom slots and a file-level problem, if one was found."""

    slots: tuple[NesControlProfile, ...]
    problem: str | None = None

    def get(self, slot_id: str) -> NesControlProfile | None:
        if slot_id == NES_DEFAULT_SLOT_ID:
            return NES_DEFAULT_PROFILE
        return next((profile for profile in self.slots if profile.slot_id == slot_id), None)


@dataclass(frozen=True)
class NesProfileResolution:
    """Resolved controls and the reason a request fell back to Default."""

    profile: NesControlProfile
    reason: str | None = None


NES_DEFAULT_PROFILE = NesControlProfile(
    NES_DEFAULT_SLOT_ID, "Default", NES_DEFAULT_SEED
)
EMPTY_NES_PROFILE_RESULT = NesProfileLoadResult(())


def _validate_name(name: object) -> str:
    if not isinstance(name, str):
        raise NesProfileValidationError("Profile name must be text.")
    normalized = name.strip()
    if not 1 <= len(normalized) <= 32:
        raise NesProfileValidationError("Profile name must contain 1 to 32 characters.")
    if any(ord(character) < 32 or 127 <= ord(character) <= 159 for character in normalized):
        raise NesProfileValidationError("Profile name cannot contain control characters.")
    return normalized


def _validate_bindings(bindings: object) -> tuple[tuple[str, str], ...]:
    if not isinstance(bindings, dict):
        raise NesProfileValidationError("Bindings must be an action-to-key object.")
    unknown_actions = set(bindings) - _ACTION_SET
    missing_actions = _ACTION_SET - set(bindings)
    if unknown_actions:
        raise NesProfileValidationError(
            f"Unknown actions: {', '.join(sorted(map(str, unknown_actions)))}."
        )
    if missing_actions:
        raise NesProfileValidationError(
            f"Missing actions: {', '.join(sorted(missing_actions))}."
        )

    used_keys: dict[str, str] = {}
    ordered: list[tuple[str, str]] = []
    for action in NES_ACTION_ORDER:
        key = bindings[action]
        if not isinstance(key, str) or key not in NES_SUPPORTED_KEYS:
            raise NesProfileValidationError(f"{action} has an unsupported physical key.")
        previous_action = used_keys.get(key)
        if previous_action is not None:
            raise NesProfileValidationError(f"{key} is already assigned to {previous_action}.")
        used_keys[key] = action
        ordered.append((action, key))
    return tuple(ordered)


def nes_default_seed() -> dict[str, str]:
    """Return a fresh editable copy of the standard NES pad mapping."""
    return dict(NES_DEFAULT_SEED)


def normalize_nes_bindings(bindings: object) -> dict[str, str]:
    """Return a fresh, complete launch map; empty legacy maps mean Default."""
    if bindings is None or (isinstance(bindings, Mapping) and not bindings):
        return nes_default_seed()
    if not isinstance(bindings, Mapping):
        raise NesProfileValidationError("Bindings must be an action-to-key object.")
    return dict(_validate_bindings(dict(bindings)))


def create_nes_profile(slot_id: str, name: str, bindings: object) -> NesControlProfile:
    if slot_id not in NES_CUSTOM_SLOT_IDS:
        raise NesProfileValidationError("Choose one of the three NES custom slots.")
    return NesControlProfile(slot_id, _validate_name(name), _validate_bindings(bindings))


def _reject_duplicate_keys(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise NesProfileValidationError(f"Duplicate JSON field {key!r}.")
        result[key] = value
    return result


def _decode_profile(slot_id: str, data: object) -> NesControlProfile:
    if not isinstance(data, dict) or set(data) != {"name", "bindings"}:
        raise NesProfileValidationError(f"{slot_id} must contain only name and bindings.")
    return create_nes_profile(slot_id, data["name"], data["bindings"])


class NesControlProfileStore:
    """Own the NES-only profile document and its injected atomic writer."""

    def __init__(self, path: str | Path,
                 write_json: Callable[[Path, object], None]) -> None:
        self.path = Path(path)
        self._write_json = write_json
        self._lock = threading.RLock()

    def _read(self) -> NesProfileLoadResult:
        try:
            with self.path.open("rb") as document:
                raw = document.read(MAX_DOCUMENT_BYTES + 1)
            if len(raw) > MAX_DOCUMENT_BYTES:
                raise NesProfileValidationError("NES control profile file exceeds 16 KiB.")
            data = json.loads(raw.decode("utf-8"), object_pairs_hook=_reject_duplicate_keys)
            if not isinstance(data, dict) or set(data) != {"schema", "version", "slots"}:
                raise NesProfileValidationError(
                    "NES control profile file must contain only schema, version and slots."
                )
            if data["schema"] != NES_PROFILE_SCHEMA:
                raise NesProfileValidationError("Unsupported NES control profile schema.")
            if type(data["version"]) is not int or data["version"] != 1:
                raise NesProfileValidationError("Unsupported NES control profile version.")
            slots_data = data["slots"]
            if not isinstance(slots_data, dict) or set(slots_data) - set(NES_CUSTOM_SLOT_IDS):
                raise NesProfileValidationError(
                    "NES control profile slots must use the three fixed slot IDs."
                )
            slots = tuple(
                _decode_profile(slot_id, slots_data[slot_id])
                for slot_id in NES_CUSTOM_SLOT_IDS if slot_id in slots_data
            )
            return NesProfileLoadResult(slots)
        except FileNotFoundError:
            return EMPTY_NES_PROFILE_RESULT
        except (OSError, UnicodeError, json.JSONDecodeError, NesProfileValidationError) as error:
            return NesProfileLoadResult((), str(error))

    def load(self) -> NesProfileLoadResult:
        with self._lock:
            return self._read()

    def save_slot(self, slot_id: str, name: str,
                  bindings: object) -> NesControlProfile:
        profile = create_nes_profile(slot_id, name, bindings)
        with self._lock:
            current = self._read()
            if current.problem:
                raise NesProfileValidationError(
                    f"Cannot replace an unreadable NES profile file: {current.problem}"
                )
            slots = {item.slot_id: item for item in current.slots}
            slots[slot_id] = profile
            payload = {
                "schema": NES_PROFILE_SCHEMA,
                "version": 1,
                "slots": {
                    item.slot_id: {
                        "name": item.name,
                        "bindings": dict(item.bindings),
                    }
                    for item in (slots[key] for key in NES_CUSTOM_SLOT_IDS if key in slots)
                },
            }
            self._write_json(self.path, payload)
        return profile


def resolve_nes_selection(selection: object,
                          result: NesProfileLoadResult) -> NesProfileResolution:
    """Resolve a saved selection without letting stale data block a Default launch."""
    if selection is None or selection == NES_DEFAULT_SLOT_ID:
        return NesProfileResolution(NES_DEFAULT_PROFILE)
    if not isinstance(selection, str) or selection not in NES_CUSTOM_SLOT_IDS:
        return NesProfileResolution(NES_DEFAULT_PROFILE, "unknown NES control profile selection")
    if result.problem:
        return NesProfileResolution(
            NES_DEFAULT_PROFILE,
            f"NES control profile file could not be read: {result.problem}",
        )
    profile = result.get(selection)
    if profile is None:
        return NesProfileResolution(
            NES_DEFAULT_PROFILE,
            f"selected NES control profile {selection} is no longer saved",
        )
    return NesProfileResolution(profile)


def serialize_nes_bindings(profile: NesControlProfile) -> str:
    """Build the bounded ASCII-only NES launch value."""
    if not isinstance(profile, NesControlProfile):
        raise NesProfileValidationError("Only a NES control profile can use the NES serializer.")
    validated = _validate_bindings(dict(profile.bindings))
    value = NES_WIRE_SCHEMA + "|" + "|".join(
        f"{action}={key}" for action, key in validated
    )
    encoded = value.encode("ascii")
    if len(encoded) > MAX_BINDINGS_ARGUMENT:
        raise NesProfileValidationError(
            "NES control profile launch value exceeds 1024 ASCII characters."
        )
    return value


def deserialize_nes_bindings(value: object) -> dict[str, str]:
    """Decode only the NES wire schema and reject Java ME or unknown schemas."""
    if not isinstance(value, str) or not value.isascii():
        raise NesProfileValidationError("NES control value must be ASCII text.")
    if len(value.encode("ascii")) > MAX_BINDINGS_ARGUMENT:
        raise NesProfileValidationError("NES control value exceeds 1024 ASCII characters.")
    parts = value.split("|")
    if not parts or parts[0] != NES_WIRE_SCHEMA or len(parts) != len(NES_ACTION_ORDER) + 1:
        raise NesProfileValidationError("Unsupported NES control wire schema.")
    pairs: dict[str, str] = {}
    for part in parts[1:]:
        action, separator, key = part.partition("=")
        if not separator or action in pairs:
            raise NesProfileValidationError("Malformed NES control wire value.")
        pairs[action] = key
    return dict(_validate_bindings(pairs))
