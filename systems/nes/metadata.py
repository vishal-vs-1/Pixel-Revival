"""Versioned NES library metadata and validation."""

from dataclasses import asdict
from pathlib import Path

from game_storage import validate_game_id
from systems.contracts import GameRef, NES
from .rom import NesRomInfo, nes_game_id

SCHEMA = 1
RUNTIME_STATUSES = ("setup-needed", "not-probed", "ready", "unsupported")


def new_metadata(info: NesRomInfo, filename: str, backend: str,
                 runtime_status: str = "setup-needed") -> dict:
    details = asdict(info)
    for key in ("id", "source_sha256"):
        del details[key]
    details["warnings"] = list(info.warnings)
    details["chr_kind"] = info.chr_kind
    return validate_metadata({
        "schema": SCHEMA, "system": NES, "id": info.id, "name": Path(filename).stem,
        "source_sha256": info.source_sha256, "original_filename": filename,
        "backend": backend, "render_scale": 2, "width": 256, "height": 240,
        "aspect": "4:3", "palette_profile": None, "runtime_status": runtime_status,
        "capture_policy": "disabled", "nes": details,
    })


def validate_metadata(game: dict, expected_id: str | None = None) -> dict:
    if (not isinstance(game, dict) or type(game.get("schema")) is not int
            or game.get("schema") != SCHEMA or game.get("system") != NES):
        raise ValueError("Unsupported NES game metadata.")
    ref = GameRef(NES, game.get("id"))
    digest = validate_game_id(game.get("source_sha256"))
    if ref.id != nes_game_id(bytes.fromhex(digest)) or (expected_id is not None and ref.id != expected_id):
        raise ValueError("NES metadata does not match its cartridge or workspace.")
    if not isinstance(game.get("name"), str) or not game["name"].strip():
        raise ValueError("Invalid NES title.")
    filename = game.get("original_filename")
    if (not isinstance(filename, str) or not filename
            or any(char in filename for char in "/\\:\0") or filename in (".", "..")):
        raise ValueError("Invalid NES original filename.")
    if (game.get("backend") not in ("pixel", "ai") or game.get("render_scale") not in (2, 4)
            or (game.get("width"), game.get("height")) != (256, 240)
            or game.get("aspect") not in ("4:3", "16:15")
            or game.get("runtime_status") not in RUNTIME_STATUSES
            or game.get("capture_policy") not in ("disabled", "enabled")):
        raise ValueError("Invalid NES display, artwork or runtime settings.")
    palette = game.get("palette_profile")
    if palette is not None:
        validate_game_id(palette)
    details = game.get("nes")
    if (not isinstance(details, dict) or details.get("container") not in ("ines", "nes2")
            or details.get("chr_kind") not in ("rom", "ram")
            or details.get("region") not in ("unknown", "ntsc", "pal", "multi", "dendy")
            or type(details.get("mapper")) is not int or not 0 <= details["mapper"] <= 4095
            or type(details.get("submapper")) is not int or not 0 <= details["submapper"] <= 15):
        raise ValueError("Invalid NES cartridge metadata.")
    return game
