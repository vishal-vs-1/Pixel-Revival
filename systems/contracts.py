"""Stable game and import identities shared by the application and its backends."""

from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Literal, Protocol

from game_storage import validate_game_id
from .presentation import SystemPresentation

SystemId = Literal["j2me", "nes"]
JAVA_ME: SystemId = "j2me"
NES: SystemId = "nes"


def validate_system(system: str) -> SystemId:
    if not isinstance(system, str):
        raise ValueError(f"Unknown game system: {system!r}")
    if system == JAVA_ME:
        return JAVA_ME
    if system == NES:
        return NES
    raise ValueError(f"Unknown game system: {system!r}")


@dataclass(frozen=True)
class GameRef:
    system: SystemId
    id: str

    def __post_init__(self):
        validate_system(self.system)
        validate_game_id(self.id)

    @classmethod
    def from_metadata(cls, game):
        return cls(game.get("system", JAVA_ME), game["id"])


@dataclass(frozen=True)
class ImportRequest:
    system: SystemId
    path: Path
    artwork_backend: str

    def __post_init__(self):
        validate_system(self.system)
        if self.artwork_backend not in ("pixel", "ai"):
            raise ValueError(f"Unknown artwork backend: {self.artwork_backend!r}")
        if self.path == "":
            raise ValueError("Choose a game file.")
        object.__setattr__(self, "path", Path(self.path).resolve())


class SystemBackend(Protocol):
    system: SystemId
    presentation: SystemPresentation

    def games(self) -> list[dict]: ...
    def import_game(self, request: ImportRequest, progress: Callable[[str], None]) -> dict: ...
    def recover_assets(self, on_error: Callable[[str], None], should_stop: Callable[[], bool]) -> None: ...
    def workspace(self, identity: str) -> Path: ...
    def texture_dir(self, identity: str, artwork_backend: str) -> Path: ...
    def save(self, game: dict) -> None: ...
    def set_control_profile_id(self, identity: str, slot_id: str | None) -> None: ...
    def delete_game(self, identity: str, *args, **kwargs) -> Path: ...
    def prepare_style(self, game: dict, backend: str, progress: Callable[[str], None]) -> dict: ...
    def launch(self, game: dict, original: bool = False, **kwargs) -> tuple: ...
