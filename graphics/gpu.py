"""Advisory per-executable GPU preference with an injectable writer."""

from dataclasses import dataclass
from pathlib import Path
from typing import Protocol

MODES = frozenset({"auto", "dedicated", "integrated", "system"})


class PreferenceWriter(Protocol):
    def set(self, executable: Path, mode: str) -> None: ...
    def clear(self, executable: Path) -> None: ...


@dataclass(frozen=True)
class PreferenceResult:
    executable: Path
    mode: str
    applied: bool
    reason: str = ""


def apply_advisory_preference(executable: Path, mode: str, writer: PreferenceWriter) -> PreferenceResult:
    executable = Path(executable).resolve()
    if mode not in MODES:
        raise ValueError("Unknown GPU preference")
    if not executable.is_file() or executable.is_symlink():
        raise ValueError("GPU preference target must be an existing executable")
    try:
        if mode in ("auto", "system"):
            writer.clear(executable)
        else:
            writer.set(executable, mode)
        return PreferenceResult(executable, mode, True)
    except OSError as error:
        return PreferenceResult(executable, mode, False, str(error))
