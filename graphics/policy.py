"""Validated, renderer-neutral graphics preferences and capabilities."""

from dataclasses import dataclass
import re

from .protocol import ABI_VERSION, MAX_ASPECT_COMPONENT


_BACKENDS = frozenset({"legacy", "common-v1"})
_FITS = frozenset({"desktop-height-preserve-aspect"})
_FILTERS = frozenset({"nearest", "linear"})
_GPU_PREFERENCES = frozenset({"auto", "dedicated", "integrated", "system"})
_BUILD_ID = re.compile(r"[0-9a-f]{64}\Z")


def _choice(name: str, value: object, choices: frozenset[str]) -> None:
    if not isinstance(value, str) or value not in choices:
        raise ValueError(f"{name} must be one of: {', '.join(sorted(choices))}")


def _positive_bounded_integer(name: str, value: object, maximum: int) -> None:
    if type(value) is not int or not 1 <= value <= maximum:
        raise ValueError(f"{name} must be an integer from 1 to {maximum}")


@dataclass(frozen=True, slots=True)
class GraphicsPolicy:
    """A requested policy; it never claims which physical GPU was selected."""

    backend: str
    detail_scale: int
    aspect_num: int
    aspect_den: int
    fit: str
    texture_filter: str
    gpu_preference: str

    def __post_init__(self) -> None:
        _choice("backend", self.backend, _BACKENDS)
        if type(self.detail_scale) is not int or self.detail_scale not in (2, 4):
            raise ValueError("detail_scale must be 2 or 4")
        _positive_bounded_integer("aspect_num", self.aspect_num, MAX_ASPECT_COMPONENT)
        _positive_bounded_integer("aspect_den", self.aspect_den, MAX_ASPECT_COMPONENT)
        _choice("fit", self.fit, _FITS)
        _choice("texture_filter", self.texture_filter, _FILTERS)
        _choice("gpu_preference", self.gpu_preference, _GPU_PREFERENCES)


@dataclass(frozen=True, slots=True)
class GraphicsCapabilities:
    """Reported implementation support, separate from requested policy."""

    abi_version: int
    build_id: str
    accepts_surface_commands: bool
    accepts_completed_frames: bool
    supports_scale2x: bool
    reports_actual_device: bool

    def __post_init__(self) -> None:
        _positive_bounded_integer("abi_version", self.abi_version, 0xFFFFFFFF)
        if not isinstance(self.build_id, str) or _BUILD_ID.fullmatch(self.build_id) is None:
            raise ValueError("build_id must be 64 lowercase hexadecimal characters")
        for name in (
            "accepts_surface_commands",
            "accepts_completed_frames",
            "supports_scale2x",
            "reports_actual_device",
        ):
            if type(getattr(self, name)) is not bool:
                raise ValueError(f"{name} must be a bool")
