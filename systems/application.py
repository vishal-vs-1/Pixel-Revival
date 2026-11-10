"""Route application operations by a stable system identity."""

from collections.abc import Iterable
from threading import Event, Lock, RLock

from game_storage import validate_game_id
from .contracts import GameRef, ImportRequest, JAVA_ME, SystemBackend
from .registry import GameIndex, SystemRegistry


def _ignore(_message):
    pass


class ApplicationLibrary:
    def __init__(self, backends: Iterable[SystemBackend], asset_service):
        self._systems = SystemRegistry(backends)
        self.asset_service = asset_service
        self._index = None
        self._lock = RLock()
        self._close_lock = Lock()
        self._closed = Event()

    @staticmethod
    def _view(backend, game):
        ref = GameRef.from_metadata(game)
        if ref.system != backend.system:
            raise ValueError(f"Game {ref.id} does not belong to {backend.system!r}")
        return ref, {**game, "system": ref.system}

    def games(self, system=None):
        if system is not None:
            self._systems.get(system)
        with self._lock:
            # Publish the full index even for a filtered view. IDs are shared by
            # the artwork service, and hidden entries must retain their owners.
            self._index = None
            index = GameIndex()
            result = []
            for backend in self._systems:
                for game in backend.games():
                    ref, view = self._view(backend, game)
                    index.add(ref)
                    if system is None or ref.system == system:
                        result.append(view)
            result.sort(key=lambda game: game["name"].casefold())
            self._index = index
            return result

    def _route(self, game):
        if not isinstance(game, GameRef):
            validate_game_id(game)
        else:
            self._systems.get(game.system)
        with self._lock:
            if self._index is None:
                self.games()
            ref = self._index.resolve(game)
        return ref, self._systems.get(ref.system)

    def import_game(self, request: ImportRequest, progress=_ignore):
        if not isinstance(request, ImportRequest):
            raise TypeError("Import requires an ImportRequest")
        backend = self._systems.get(request.system)
        # Validate the existing catalog before importing into any system.
        with self._lock:
            if self._index is None:
                self.games()
        ref, view = self._view(backend, backend.import_game(request, progress))
        with self._lock:
            if self._index is None:
                self.games()
            try:
                self._index.add(ref, allow_existing=True)
            except ValueError:
                # Force subsequent lookups to detect a persisted collision.
                self._index = None
                raise
        return view

    def presentation(self, system):
        return self._systems.get(system).presentation

    @property
    def control_profiles(self):
        return self._systems.get(JAVA_ME).control_profiles

    def control_profiles_for(self, system):
        """Return the profile store owned by one system, without UI-mode routing."""
        backend = self._systems.get(system)
        profiles = getattr(backend, "control_profiles", None)
        if profiles is None:
            raise LookupError(f"No control profile store is registered for {system!r}")
        return profiles

    def reference(self, game):
        """Resolve an ID to its indexed system identity for asynchronous callbacks."""
        return self._route(game)[0]

    def workspace(self, game):
        ref, backend = self._route(game)
        return backend.workspace(ref.id)

    def texture_dir(self, game, artwork_backend):
        ref, backend = self._route(game)
        return backend.texture_dir(ref.id, artwork_backend)

    def delete_game(self, game, *args, **kwargs):
        ref, backend = self._route(game)
        result = backend.delete_game(ref.id, *args, **kwargs)
        with self._lock:
            if self._index is not None:
                self._index.discard(ref)
        return result

    def save(self, game):
        _, backend = self._route(GameRef.from_metadata(game))
        return backend.save(game)

    def set_control_profile_id(self, game, slot_id):
        ref, backend = self._route(game)
        return backend.set_control_profile_id(ref.id, slot_id)

    def prepare_style(self, game, artwork_backend, progress=_ignore):
        ref, backend = self._route(GameRef.from_metadata(game))
        ready_ref, view = self._view(backend, backend.prepare_style(game, artwork_backend, progress))
        if ready_ref != ref:
            raise ValueError("Artwork preparation changed the game identity")
        return view

    def launch(self, game, original=False, **kwargs):
        _, backend = self._route(GameRef.from_metadata(game))
        return backend.launch(game, original, **kwargs)

    def enhanced_available(self, system):
        backend = self._systems.get(system)
        capability = getattr(backend, "enhanced_available", None)
        return bool(callable(capability) and capability())

    def set_asset_listener(self, listener):
        self.asset_service.set_listener(listener)

    def set_player_state(self, game, *args, **kwargs):
        ref, _ = self._route(game)
        self.asset_service.set_player_state(ref.id, *args, **kwargs)

    def pause_all(self):
        self.asset_service.pause_all()

    def resume_all(self):
        self.asset_service.resume_all()

    def recover_assets(self, on_error=_ignore):
        if self._closed.is_set():
            return
        with self._lock:
            if self._index is None:
                self.games()
        for backend in self._systems:
            if self._closed.is_set():
                return
            backend.recover_assets(on_error, self._closed.is_set)

    def close(self, timeout=1.0):
        with self._close_lock:
            if self._closed.is_set():
                return
            self._closed.set()
        self.asset_service.close(timeout=timeout)
