"""Translate the stored renderer choice into verified Java launch arguments."""

from __future__ import annotations

from dataclasses import dataclass

from .backend_preference import COMMON_BACKEND, LEGACY_BACKEND


@dataclass(frozen=True, slots=True)
class GraphicsLaunchDecision:
    requested_backend: str
    arguments: tuple[str, ...]
    fallback_reason: str | None = None


class JmeGraphicsLaunchPolicy:
    """Apply the shared renderer preference only to GPU Java ME launches."""

    def __init__(self, preference_store, runtime_locator):
        self.preference_store = preference_store
        self.runtime_locator = runtime_locator

    def for_renderer(self, renderer: str) -> GraphicsLaunchDecision | None:
        if renderer != "gpu":
            return None

        preference = self.preference_store.load()
        if preference.warning:
            return GraphicsLaunchDecision(
                LEGACY_BACKEND,
                (f"-Djme.graphicsBackend={LEGACY_BACKEND}",),
                preference.warning,
            )
        if preference.backend == LEGACY_BACKEND:
            return GraphicsLaunchDecision(
                LEGACY_BACKEND, (f"-Djme.graphicsBackend={LEGACY_BACKEND}",))

        arguments = [f"-Djme.graphicsBackend={COMMON_BACKEND}"]
        runtime = self.runtime_locator.locate()
        if runtime is not None:
            arguments.extend((f"-Djme.graphicsCore={runtime.core}",
                              f"-Djme.graphicsShim={runtime.shim}"))
            return GraphicsLaunchDecision(COMMON_BACKEND, tuple(arguments))

        # Keep gameplay available; GpuPlayer records the failed common request and
        # reports the actual legacy renderer in its regular player metrics.
        return GraphicsLaunchDecision(
            COMMON_BACKEND, tuple(arguments),
            "No verified shared graphics runtime was found; the player will use Legacy GPU compatibility. "
            + (self.runtime_locator.last_error or "Runtime lookup returned no candidate."),
        )
