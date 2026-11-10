"""Registered game backends and a globally unique ID index."""

from collections.abc import Iterable

from .contracts import GameRef, SystemBackend, validate_system


class SystemRegistry:
    def __init__(self, backends: Iterable[SystemBackend]):
        self._backends = {}
        for backend in backends:
            system = validate_system(backend.system)
            if system in self._backends:
                raise ValueError(f"Duplicate backend for {system!r}")
            self._backends[system] = backend

    def get(self, system):
        system = validate_system(system)
        if system not in self._backends:
            raise LookupError(f"No backend registered for {system!r}")
        return self._backends[system]

    def __iter__(self):
        return iter(self._backends.values())


class GameIndex:
    def __init__(self):
        self._refs = {}

    def add(self, ref: GameRef, *, allow_existing=False):
        existing = self._refs.get(ref.id)
        if existing is not None and (not allow_existing or existing != ref):
            raise ValueError(f"Duplicate game identifier: {ref.id}")
        self._refs[ref.id] = ref

    def resolve(self, game: GameRef | str) -> GameRef:
        ref = self._refs.get(game.id if isinstance(game, GameRef) else game)
        if ref is not None and (isinstance(game, str) or ref == game):
            return ref
        raise KeyError(f"Unknown game reference: {game!r}")

    def discard(self, ref):
        self._refs.pop(ref.id, None)
