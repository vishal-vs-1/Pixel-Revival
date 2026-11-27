"""Own imported Java ME games and their workspace lifecycle."""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import shutil
import threading
import time
import zipfile
from systems.jme.profiles import PROFILES
from runtime_assets.recipes import ArtworkRecipes
from runtime_assets.service import AssetService
from storage.games import GameStorage, validate_game_id
from storage.atomic import atomic_json
from storage.paths import RESOURCE_ROOT, DATA_ROOT
from controls.java_me import CUSTOM_SLOT_IDS, DEFAULT_SLOT_ID, ControlProfileStore, ProfileValidationError
from systems.jme.jdk import locate_java
from systems.jme.resources.jar import read_manifest
from systems.contracts import DEFAULT_IMPORT_BACKEND
from systems.jme.artwork import JavaMeArtwork, RECIPE_LABELS
from systems.jme.playback import JavaMePlayback

ROOT = RESOURCE_ROOT
LIBRARY = DATA_ROOT / 'library'
RENDERED = DATA_ROOT / 'rendered-games'
# Seven save threads measured about 1.5x faster than a serial writer on
# 512-image batches without oversubscribing the redraw CPU budget badly.
SAVE_THREADS = 7
MAX_METADATA_BYTES = 65536


class Library(JavaMeArtwork, JavaMePlayback):
    def __init__(self, library=LIBRARY, rendered=RENDERED, workers=2, *, control_profiles=None,
                 artwork_recipes=None, recover_previous_artwork=False):
        self.library, self.rendered = Path(library), Path(rendered)
        self.library.mkdir(parents=True, exist_ok=True)
        self.rendered.mkdir(parents=True, exist_ok=True)
        self.lock = threading.RLock()
        self.workers = max(1, min(8, int(workers)))
        self._artwork_recipes = artwork_recipes if artwork_recipes is not None else ArtworkRecipes()
        self._players = {}
        self._deleting = set()
        self._registering = set()
        self.control_profiles = control_profiles or ControlProfileStore(
            DATA_ROOT / 'config' / 'control-profiles.json', atomic_json)
        self.asset_service = AssetService(self._artwork_recipes.backend_for_recipe,
                                          save_workers=SAVE_THREADS,
                                          recover_previous_artwork=recover_previous_artwork)

    def games(self):
        result = []
        for file in self.rendered.glob('*/game.json'):
            if (file.parent.name.startswith('.') or file.parent.name in self._deleting
                    or file.is_symlink() or file.parent.is_symlink()
                    or file.parent.is_junction()):
                continue
            try:
                game=self._read_java_metadata(file)
                # Old builds saved a stale READY/WAITING snapshot here; only the
                # asset service's live counters may drive the library display.
                game.pop('asset_work', None)
                profile=PROFILES.get(game['id'])
                if profile and 'width' in profile and not game.get('screen_source') and (game['width'],game['height'])==(240,320):
                    game['previous_screen']=[game['width'],game['height']]
                    game.update(profile)
                    self._write_if_current(file,game)
                if game.get('ui_recipe')!='smooth-text-v1' and (file.parent/'catalogue.json').exists():
                    self.prepare_ui_assets(file.parent)
                    game['ui_recipe']='smooth-text-v1'
                    self._write_if_current(file,game)
                from systems.jme.resources.inspection import inspect,VERSION as INSPECTION_VERSION
                inspection_file=file.parent/'graphics-inspection.json'
                try:inspection_version=json.loads(inspection_file.read_text()).get('inspection_version',0)
                except (OSError,ValueError):inspection_version=0
                if inspection_version<INSPECTION_VERSION and (self.library/game['id']/'game.jar').exists():
                    try:report=inspect(self.library/game['id']/'game.jar',game.get('assets'))
                    except (OSError,zipfile.BadZipFile) as error:report={'inspection_version':INSPECTION_VERSION,'error':str(error)}
                    self._write_if_current(inspection_file, report)
                if (file.parent/'graphics-inspection.json').exists():
                    game['graphics_inspection']=json.loads((file.parent/'graphics-inspection.json').read_text())
                result.append(game)
            except (ValueError, OSError):
                pass
        return sorted(result, key=lambda g: g['name'].casefold())

    def _write_if_current(self, file, document) -> bool:
        """Write only while the game is not being deleted (delete holds lock)."""
        with self.lock:
            if file.parent.name in self._deleting:
                return False
            atomic_json(file, document)
            return True

    @staticmethod
    def _read_java_metadata(file):
        if (file.is_symlink() or not file.is_file()
                or file.stat().st_size > MAX_METADATA_BYTES):
            raise ValueError('Java ME metadata is missing or too large.')
        game = json.loads(file.read_text(encoding='utf-8'))
        if (not isinstance(game, dict) or game.get('system', 'j2me') != 'j2me'
                or validate_game_id(game.get('id')) != file.parent.name
                or not isinstance(game.get('name'), str)):
            raise ValueError('Game metadata does not belong to this Java ME workspace.')
        return game

    def recover_assets(self, on_error, should_stop):
        """Recover saved recipes behind the Java ME storage and deletion boundary."""
        from runtime_assets.contracts import Recipe
        from runtime_assets.store import AssetStore
        from systems.jme.resources.decoding import VERSION as DECODING_VERSION
        for metadata_path in self.rendered.glob('*/game.json'):
            registering = None
            try:
                # A deletion can happen after the scan. Check again under the same lock.
                with self.lock:
                    if should_stop():
                        return
                    folder = metadata_path.parent
                    if (folder.name.startswith('.') or folder.name in self._deleting
                            or not metadata_path.is_file() or metadata_path.is_symlink()
                            or folder.is_symlink() or folder.is_junction()):
                        continue
                    game = self._read_java_metadata(metadata_path)
                    recipe_key = game.get('recipe_key')
                    if not recipe_key:
                        continue
                    raw = AssetStore(folder).recipe_json(recipe_key)
                    if raw is None:
                        continue
                    recipe = Recipe.from_json(raw)
                    ai_recipe = recipe if recipe.algorithm == 'realesrgan-x4plus-anime' else None
                    if should_stop():
                        return
                    # Indexing a large catalogue can take minutes. Holding the
                    # library-wide lock here froze Play and comparison launches,
                    # so only guard this game against deletion while registering.
                    self._registering.add(folder.name)
                    registering = folder.name
                try:
                    self.asset_service.register_game(
                        folder, recipe, game=game, ai_recipe=ai_recipe,
                        queue_missing=False,
                        recover=(game.get('status') == 'Ready' and
                                 game.get('asset_decoding', {}).get('version') == DECODING_VERSION))
                finally:
                    with self.lock:
                        self._registering.discard(registering)
            except Exception as error:
                on_error(str(error))

    def workspace(self, game_id):
        return self.rendered / validate_game_id(game_id)

    def save(self, game):
        with self.lock:
            self._check_not_deleting(game['id'])
            # Runtime counters are a live snapshot, never metadata. Persisting
            # them made the library show old numbers after every restart.
            clean = {key: value for key, value in game.items() if key != 'asset_work'}
            atomic_json(self.workspace(game['id']) / 'game.json', clean)

    def set_control_profile_id(self, game_id, slot_id):
        """Persist only this game's profile selection without replacing fresh metadata."""
        with self.lock:
            self._check_not_deleting(game_id)
            game_file = self.workspace(game_id) / 'game.json'
            if not game_file.is_file():
                raise FileNotFoundError('This game is no longer in the library.')
            if slot_id is None or slot_id == DEFAULT_SLOT_ID:
                slot_id = DEFAULT_SLOT_ID
            elif slot_id not in CUSTOM_SLOT_IDS:
                raise ProfileValidationError('Choose a saved control profile or Default.')
            else:
                profiles = self.control_profiles.load()
                if profiles.problem:
                    raise ProfileValidationError(
                        f'Cannot select a profile because the profile file could not be read: {profiles.problem}')
                if profiles.get(slot_id) is None:
                    raise ProfileValidationError('This control profile is no longer saved.')
            current = json.loads(game_file.read_text(encoding='utf-8'))
            if not isinstance(current, dict) or current.get('id') != game_id:
                raise RuntimeError('Game metadata is invalid or no longer matches this library entry.')
            if slot_id == DEFAULT_SLOT_ID:
                current.pop('control_profile_id', None)
            else:
                current['control_profile_id'] = slot_id
            atomic_json(game_file, current)

    def _check_not_deleting(self,game_id):
        if game_id in self._deleting:raise RuntimeError('This game is being deleted. Wait for deletion to finish before importing or changing it.')

    def delete_game(self,game_id,progress=lambda message:None,*,timeout=60.0):
        """Stop scheduling this game, delete its owned data, and retain saves for reimport."""
        with self.lock:
            if game_id in self._registering:
                raise RuntimeError('This game is still being prepared. Try again in a moment.')
            self._check_not_deleting(game_id)
            storage=GameStorage(self.library,self.rendered)
            storage.deletion_paths(game_id)
            folder=self.workspace(game_id)
            metadata=json.loads((folder/'game.json').read_text(encoding='utf-8')) if (folder/'game.json').is_file() else None
            player=self._players.get(game_id)
            if player is not None:
                if player.poll() is None:
                    raise RuntimeError('Close the game before deleting it so its saves can finish writing.')
                try:self.asset_service.set_player_state(game_id,False)
                except KeyError:pass
            self._deleting.add(game_id)
        # Waiting and filesystem cleanup never hold the library-wide lock.
        cleanup_started=False
        try:
            progress('Deleting in background; waiting for any current artwork batch…')
            self.asset_service.unregister_game(game_id,timeout=timeout)
            progress('Deleting game files and artwork. Keeping saved progress…')
            cleanup_started=True
            saves=storage.delete(game_id)
            self._players.pop(game_id,None)
            return saves
        except Exception:
            if cleanup_started and metadata and (folder/'game.json').is_file():
                try:
                    # Rebuild archive-derived data and reconcile missing cache
                    # outputs before making a failed deletion playable again.
                    self.decode_assets(metadata)
                    recipe,ai_recipe=self.recipe_for_backend(metadata.get('backend','pixel'))
                    self.asset_service.register_game(folder,recipe,game=metadata,ai_recipe=ai_recipe)
                except Exception:pass
            raise
        finally:
            with self.lock:self._deleting.discard(game_id)

    def prepare_startup(self, game, progress=lambda message: None):
        """Startup cleanup has its own handler/cache, independent of artwork/input."""
        from systems.jme.startup_cleaner import select_startup
        source = self.library / game['id'] / 'game.jar'
        enabled=bool(game.get('remove_startup_wrappers', False))
        playable, report = select_startup(source, self.workspace(game['id']) / 'startup',
            enabled=enabled, javac=locate_java('javac') if enabled else None,
            player=ROOT/'build/jme-player.jar', progress=progress)
        if game.get('startup') != report:
            game['startup'] = report
            game['startup_hint'] = ('Promotional startup screens removed. Starts directly in the game.'
                if report['status'] == 'cleaned' else PROFILES.get(game['id'],{}).get('startup_hint',''))
            self.save(game)
        return playable

    def import_game(self, source, progress=lambda message: None, backend=DEFAULT_IMPORT_BACKEND):
        source = Path(source).resolve()
        if source.suffix.lower() != '.jar':
            raise ValueError('Select the actual .jar game. A .jad is only its descriptor.')
        if source.stat().st_size > 64 * 1024 * 1024:
            raise ValueError('JAR exceeds the 64 MB experiment limit.')
        identity = hashlib.sha256(source.read_bytes()).hexdigest()
        folder = self.workspace(identity)
        with self.lock:
            self._check_not_deleting(identity)
            existing = folder / 'game.json'
            if existing.exists():
                game = json.loads(existing.read_text(encoding='utf-8'))
                if game.get('status') == 'Ready' and (self.library / identity / 'game.jar').exists():
                    from systems.jme.resources.decoding import VERSION as DECODING_VERSION, default_registry
                    report=game.get('asset_decoding',{})
                    if report.get('version')==DECODING_VERSION and report.get('registry')==[getattr(d,'cache_key',d.name) for d in default_registry().decoders]:
                        if game.get('backend') != backend:
                            return self.prepare_style(game, backend, progress)
                        migration_game = dict(game)
                        recipe, ai_recipe = self.recipe_for_backend(game.get('backend', backend))
                        game['recipe_key'] = recipe.key
                        if ai_recipe is not None:
                            game['model_digest'] = recipe.model_digest
                        self.save(game)
                        self.asset_service.register_game(folder, recipe, game=migration_game,
                                                         ai_recipe=ai_recipe, progress=progress)
                        game['asset_work'] = self.asset_service.stats(identity)
                        progress('Already prepared — reusing saved assets.')
                        return game
                    return self.prepare_style(game,backend,progress)
            with zipfile.ZipFile(source) as archive:
                infos = archive.infolist()
                if len(infos) > 20_000 or sum(i.file_size for i in infos) > 128 * 1024 * 1024:
                    raise ValueError('Archive expands beyond the supported limit.')
                try:
                    manifest = {k: v.strip() for k, v in read_manifest(archive.read('META-INF/MANIFEST.MF')).items()}
                except KeyError:
                    raise ValueError('Not a Java ME game: META-INF/MANIFEST.MF is missing.')
                if 'MIDlet-1' not in manifest:
                    raise ValueError('No MIDlet-1 entry found; this is not a supported Java ME game.')
                folder.mkdir(parents=True, exist_ok=True)
                dest = self.library / identity / 'game.jar'
                dest.parent.mkdir(parents=True, exist_ok=True)
                if source != dest.resolve():
                    shutil.copy2(source, dest)
                for sub in ['original-assets', 'textures', 'logs', 'saves']:
                    (folder / sub).mkdir(exist_ok=True)
                game = {'id': identity, 'name': manifest.get('MIDlet-Name', source.stem),
                        'vendor': manifest.get('MIDlet-Vendor', ''), 'status': 'Preparing',
                        'width': 240, 'height': 320, 'scale': 4, 'recipe': RECIPE_LABELS[backend],
                        'backend': backend, 'workers': self.workers,
                        'assets': 0, 'imported': time.time(),
                        'original_filename': source.name, 'manifest': manifest}
                game.update(PROFILES.get(identity,{}))
                self.save(game)
                self.prepare_startup(game, progress)
                fresh_sources = {}
                report,catalogue = self.decode_assets(game, progress, fresh_sources=fresh_sources)
                seen={row['image_id'] for row in catalogue}
                self.prepare_ui_assets(folder)
                from systems.jme.resources.inspection import inspect
                atomic_json(folder/'graphics-inspection.json',inspect(dest,report['outcomes'].get('standard-images-v1',0)))
                self.record_decoding_coverage(game)
                game.update(assets=len(seen), ui_recipe='smooth-text-v1')
                recipe, ai_recipe = self.recipe_for_backend(backend)
                game['recipe_key'] = recipe.key
                if ai_recipe is not None:
                    game['model_digest'] = recipe.model_digest
                migration_game = dict(game)
                if ai_recipe is not None:
                    # Do not attribute pre-existing unindexed AI files to weights
                    # that this fresh import has not yet used.
                    migration_game.pop('model_digest', None)
                migration = self.asset_service.register_game(folder, recipe, game=migration_game,
                                                             ai_recipe=ai_recipe, progress=progress,
                                                             fresh_sources=fresh_sources)
                game.update(status='Ready', prepared=time.time())
                game['asset_work'] = self.asset_service.stats(identity)
                self.save(game)
                progress(f'Ready to play — {len(seen)} sources registered; '
                         f'{migration.jobs_queued} enhancement jobs continue in the background.')
                return game
