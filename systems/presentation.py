"""File-picker and demo capabilities independent of Tk."""

from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class SystemPresentation:
    picker_title: str
    filetypes: tuple[tuple[str, str], ...]
    demo: Path | None = None
