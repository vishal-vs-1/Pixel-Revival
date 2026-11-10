"""Small immutable events for work that outlives a screen selection."""

from __future__ import annotations

from dataclasses import dataclass

from .contracts import GameRef, SystemId, validate_system


@dataclass(frozen=True)
class ApplicationEvent:
    kind: str
    system: SystemId
    game_id: str | None = None
    detail: str = ""

    def __post_init__(self) -> None:
        validate_system(self.system)
        if not isinstance(self.kind, str) or not self.kind:
            raise ValueError("Application events require a kind.")
        if not isinstance(self.detail, str):
            raise ValueError("Application event detail must be text.")
        if self.game_id is not None:
            GameRef(self.system, self.game_id)

    @property
    def ref(self) -> GameRef | None:
        return GameRef(self.system, self.game_id) if self.game_id is not None else None
