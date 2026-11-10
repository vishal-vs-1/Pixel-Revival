"""Store original NES cartridges and own their workspace lifecycle."""

import hashlib
import json
import os
import threading
from pathlib import Path
from tempfile import NamedTemporaryFile

from game_storage import GameStorage
from persistence import atomic_json
from systems.contracts import GameRef, NES
from systems.presentation import SystemPresentation
from .controls import (
    EMPTY_NES_PROFILE_RESULT,
    NES_CUSTOM_SLOT_IDS,
    NES_DEFAULT_PROFILE,
    NES_DEFAULT_SLOT_ID,
    NesProfileResolution,
    resolve_nes_selection,
)
from .metadata import new_metadata, validate_metadata
from .rom import NesRomInspector

MAX_METADATA_BYTES = 65536
PREFERENCES = ("name", "backend", "render_scale", "aspect", "capture_policy", "palette_profile")


def _quiet(_message):
    pass


class NesLibrary:
    system = NES
    presentation = SystemPresentation("Choose an NES cartridge", (("NES cartridge", "*.nes"),))

    def __init__(self, root, *, asset_service, runtime=None, register_artwork=None, inspector=None,
                 write_metadata=atomic_json, control_profiles=None):
        root = Path(root)
        self._storage = GameStorage(root / "library", root / "rendered-games", source_filename="game.nes")
        self._service = asset_service
        self._runtime = runtime
        self._register_artwork = register_artwork
        self._inspector = inspector if inspector is not None else NesRomInspector()
        self._write_metadata = write_metadata
        self._control_profiles = control_profiles
        self._lock = threading.RLock()
        self._deleting = set()
        self._players = {}

    @property
    def control_profiles(self):
        return self._control_profiles

    def enhanced_available(self):
        capability = getattr(self._runtime, "capture_available", None)
        return bool(self._runtime is not None and callable(capability) and capability())

    def workspace(self, identity):
        return self._storage.game_paths(identity)[1]

    def texture_dir(self, identity, artwork_backend):
        if artwork_backend not in ("pixel", "ai"):
            raise ValueError("Unknown artwork backend.")
        return self.workspace(identity) / ("textures-ai" if artwork_backend == "ai" else "textures")

    def _metadata_path(self, identity):
        folder = self.workspace(identity)
        return self._storage._owned(folder / "game.json", folder)

    def _read(self, identity):
        with self._metadata_path(identity).open("rb") as stream:
            raw = stream.read(MAX_METADATA_BYTES + 1)
        if len(raw) > MAX_METADATA_BYTES:
            raise ValueError("NES metadata exceeds 64 KiB.")
        return validate_metadata(json.loads(raw), identity)

    def _status(self, game):
        if self._runtime is None or not self._runtime.available():
            return "setup-needed"
        status = game["runtime_status"]
        return "not-probed" if status == "setup-needed" else status

    def _view(self, game):
        return {**game, "runtime_status": self._status(game)}

    def games(self):
        with self._lock:
            result = []
            for file in self._storage.rendered.glob("*/game.json"):
                identity = file.parent.name
                if identity.startswith(".") or identity in self._deleting:
                    continue
                try:
                    if self._storage.source_path(identity).is_file():
                        result.append(self._view(self._read(identity)))
                except (OSError, ValueError):
                    continue
            return sorted(result, key=lambda game: game["name"].casefold())

    def _require_idle(self, identity):
        if identity in self._deleting:
            raise RuntimeError("This NES game is being deleted.")
        process = self._players.get(identity)
        if process is not None and process.poll() is None:
            raise RuntimeError("Close the NES player before changing or deleting this game.")

    def _register(self, folder, game, *, recovering=False):
        if self._register_artwork is not None:
            self._register_artwork(folder, game, recovering=recovering)

    @staticmethod
    def _install(source, destination, info):
        temporary = None
        try:
            digest = hashlib.sha256()
            remaining = info.file_bytes
            with source.open("rb") as original, NamedTemporaryFile(
                    prefix=".import-", dir=destination.parent, delete=False) as staging:
                temporary = Path(staging.name)
                while remaining:
                    chunk = original.read(min(remaining, 1024 * 1024))
                    if not chunk:
                        break
                    digest.update(chunk)
                    staging.write(chunk)
                    remaining -= len(chunk)
                if remaining or original.read(1) or digest.hexdigest() != info.source_sha256:
                    raise ValueError("NES cartridge changed during import.")
            os.replace(temporary, destination)
        finally:
            if temporary is not None:
                temporary.unlink(missing_ok=True)

    def import_game(self, request, progress=_quiet):
        if request.system != NES:
            raise ValueError("Choose an NES import request.")
        info = self._inspector.inspect(request.path)
        with self._lock:
            self._require_idle(info.id)
            destination = self._storage.source_path(info.id)
            metadata = self._metadata_path(info.id)
            existing = metadata.is_file()
            game = (self._read(info.id) if existing else
                    new_metadata(info, request.path.name, request.artwork_backend))
            if existing:
                game["backend"] = request.artwork_backend
            game["runtime_status"] = self._status(game)
            created = not destination.exists()
            if created:
                destination.parent.mkdir(parents=True, exist_ok=True)
                self._install(request.path, destination, info)
            elif self._inspector.inspect(destination).source_sha256 != info.source_sha256:
                raise ValueError("The existing NES cartridge no longer matches its stored identity.")
            try:
                self._write_metadata(metadata, game)
            except Exception:
                if created:
                    destination.unlink(missing_ok=True)
                raise
            self._register(metadata.parent, game)
        progress("NES cartridge saved.")
        return game

    def save(self, game):
        ref = GameRef.from_metadata(game)
        if ref.system != NES:
            raise ValueError("Choose NES game metadata.")
        with self._lock:
            self._require_idle(ref.id)
            current = self._read(ref.id)
            previous_pack_settings = (current["backend"], current["render_scale"])
            current.update({key: game[key] for key in PREFERENCES if key in game})
            current = validate_metadata(current, ref.id)
            self._write_metadata(self._metadata_path(ref.id), current)
            if previous_pack_settings != (current["backend"], current["render_scale"]):
                self._register(self.workspace(ref.id), current)

    def prepare_style(self, game, backend, progress=_quiet):
        identity = game["id"]
        with self._lock:
            self.save({**game, "backend": backend})
            current = self._read(identity)
        progress("NES artwork style saved.")
        return self._view(current)

    def set_control_profile_id(self, identity, slot_id):
        with self._lock:
            if identity in self._deleting:
                raise RuntimeError("This NES game is being deleted.")
            current = self._read(identity)
            if slot_id is None or slot_id == NES_DEFAULT_SLOT_ID:
                current.pop("control_profile_id", None)
            else:
                if slot_id not in NES_CUSTOM_SLOT_IDS or not isinstance(slot_id, str):
                    raise ValueError("Choose Default or a saved NES control profile slot.")
                if self._control_profiles is None:
                    raise ValueError("NES control profiles are not available.")
                saved = self._control_profiles.load()
                if saved.problem or saved.get(slot_id) is None:
                    raise ValueError("Choose an existing saved NES control profile slot.")
                current["control_profile_id"] = slot_id
            self._write_metadata(
                self._metadata_path(identity), validate_metadata(current, identity)
            )

    def recover_assets(self, on_error, should_stop):
        if self._register_artwork is None:
            return
        for game in self.games():
            identity = game["id"]
            with self._lock:
                if should_stop():
                    return
                if identity in self._deleting:
                    continue
                try:
                    if self._storage.source_path(identity).is_file():
                        self._register(self.workspace(identity), self._read(identity), recovering=True)
                except (OSError, ValueError, RuntimeError) as error:
                    on_error(str(error))

    def launch(self, game, original=False, **kwargs):
        identity = game["id"]
        with self._lock:
            self._require_idle(identity)
            current = self._read(identity)
            if self._status(current) == "setup-needed":
                raise RuntimeError("Set up the NES player before playing this game.")
            if current["runtime_status"] == "unsupported":
                raise RuntimeError("The NES player reported this cartridge as unsupported.")
            source = self._storage.source_path(identity)
            if self._inspector.inspect(source).source_sha256 != current["source_sha256"]:
                raise ValueError("The stored NES cartridge no longer matches its identity.")
            selection = current.get("control_profile_id")
            profiles = (self._control_profiles.load() if self._control_profiles is not None
                        else EMPTY_NES_PROFILE_RESULT)
            resolution = resolve_nes_selection(selection, profiles)
            if self._control_profiles is None and selection in NES_CUSTOM_SLOT_IDS:
                resolution = NesProfileResolution(
                    NES_DEFAULT_PROFILE, "NES control profile store is unavailable"
                )
            controls = dict(resolution.profile.bindings)
            runtime_options = dict(kwargs)
            runtime_options["controls"] = controls
            runtime_options["controls_fallback_reason"] = resolution.reason
            process, log = self._runtime.launch(
                current, source, self.workspace(identity), original=original, **runtime_options
            )
            self._players[identity] = process
            return process, log

    def delete_game(self, identity, progress=_quiet, *, timeout=60.0):
        with self._lock:
            self._require_idle(identity)
            game = self._read(identity)
            self._storage.deletion_paths(identity)
            self._deleting.add(identity)
        unregistered = False
        try:
            progress("Removing NES cartridge and cache while keeping saved progress.")
            if identity in self._players:
                try:
                    self._service.set_player_state(identity, False)
                except KeyError:
                    pass
            unregistered = self._service.unregister_game(identity, timeout=timeout)
            saves = self._storage.delete(identity)
            with self._lock:
                self._players.pop(identity, None)
            return saves
        except Exception as error:
            if unregistered:
                try:
                    if self._metadata_path(identity).is_file():
                        self._register(self.workspace(identity), game, recovering=True)
                except Exception as recovery_error:
                    error.add_note(f"Artwork recovery failed: {recovery_error}")
            raise
        finally:
            with self._lock:
                self._deleting.discard(identity)
