"""Persist the small set of application-wide display choices."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Callable

from .contracts import JAVA_ME, NES, SystemId, validate_system

MAX_SETTINGS_BYTES = 4096
SCHEMA = 1


def _reject_duplicate_fields(pairs):
    document={}
    for key,value in pairs:
        if key in document:
            raise ValueError("Application settings contain a duplicate field.")
        document[key]=value
    return document


class ApplicationSettingsStore:
    """Own only the selected system; malformed settings are left untouched."""

    def __init__(self, path: str | Path,
                 write_json: Callable[[Path, object], None]) -> None:
        self.path = Path(path)
        self._write_json = write_json

    def load_visible_system(self) -> SystemId:
        try:
            with self.path.open("rb") as stream:
                raw = stream.read(MAX_SETTINGS_BYTES + 1)
            if len(raw) > MAX_SETTINGS_BYTES:
                return JAVA_ME
            data = json.loads(raw.decode("utf-8"), object_pairs_hook=_reject_duplicate_fields)
            if (not isinstance(data, dict) or set(data) != {"schema", "visible_system"}
                    or type(data["schema"]) is not int or data["schema"] != SCHEMA):
                return JAVA_ME
            return validate_system(data["visible_system"])
        except (OSError, UnicodeError, json.JSONDecodeError, TypeError, ValueError):
            return JAVA_ME

    def save_visible_system(self, system: str) -> None:
        selected = validate_system(system)
        if selected not in (JAVA_ME, NES):
            raise ValueError("Unknown visible game system.")
        self._write_json(self.path, {"schema": SCHEMA, "visible_system": selected})
