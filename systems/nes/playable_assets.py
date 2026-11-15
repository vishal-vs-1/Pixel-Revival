"""Manage NES player pose detection, assignments, regeneration, and playback."""

from concurrent.futures import ThreadPoolExecutor
from hashlib import file_digest
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
from tempfile import TemporaryDirectory
from uuid import uuid4

from PIL import Image

from storage.atomic import atomic_json
from systems.generated_poses import GeneratedPoses
from .capture import owned_path
from .playable_checkpoints import latest_checkpoint
from .playable_recording import read_recording, write_recorder
from .portable_controls import set_keyboard
from .pose_layouts import (
    Cell, Detection, REPLAYS, ReplayStart, _pattern, _pose_image, collect_poses,
    find_larger_checkpoint_sprite, identify_player, owned_cartridge, pose_key, read_chr_rom,
)
from .pose_pack import CELL, _fit_canvas, _read_image, write_pack
from .runtime import MAX_BUILD_INFO_BYTES, NesManagedRuntime

SECTION = 'playable-assets'
MAX_POSES = 192
MAX_DETECTION_BYTES = 256 * 1024


class PlayableAssets:
    def __init__(self, app_root):
        self.app_root = Path(app_root)

    def runtime(self):
        for root in (self.app_root / 'build/common-runtime-auto-groups-repair', self.app_root / 'tools/nes-runtime'):
            manifest = root / 'nes/graphics-runtime.json'
            executable = root / 'nes/player/Mesen.exe'
            if not manifest.is_file() or not executable.is_file():
                continue
            runtime = NesManagedRuntime(executable, manifest)
            try:
                with (manifest.parent / 'build-info.json').open('rb') as stream:
                    raw = stream.read(MAX_BUILD_INFO_BYTES + 1)
                if len(raw) > MAX_BUILD_INFO_BYTES:
                    continue
                info = json.loads(raw)
                if not isinstance(info, dict) or info.get('schema') != 1 or not runtime.available():
                    continue
                with (executable.parent / 'MesenCore.dll').open('rb') as stream:
                    if file_digest(stream, 'sha256').hexdigest() == info.get('native_core_sha256'):
                        return runtime
            except (OSError, ValueError):
                continue
        raise RuntimeError('A verified Windows NES player is required for player-sprite replacement.')

    def detect(self, identity, source, workspace, *, state=None, latest=False):
        profile = REPLAYS.get(identity)
        if profile is None:
            raise ValueError('Player detection has no verified startup replay for this cartridge.')
        owned_cartridge(source, identity)
        raw = read_chr_rom(source)
        runtime = self.runtime()
        from .playable_probe import trace_player
        if latest:
            state = latest_checkpoint(workspace, identity)
        elif state is not None:
            state = self._checkpoint(workspace, state)
        if state is not None:
            profile = ReplayStart(24, 1000, profile.throttle)
            tracks = dict(left='checkpoint_left', right='checkpoint_right', neutral='neutral',
                          jump='jump', jump_left='jump_left', climb_up='climb_up', climb_down='climb_down')
        else:
            tracks = dict.fromkeys(('neutral', 'left', 'right', 'turn', 'jump', 'jump_left', 'turn_right'))
        previous = self.detection(identity, workspace)
        if state is not None and (previous is None or not previous.slots):
            raise ValueError('Run Detect player once before scanning a saved checkpoint.')
        scratch = owned_path(workspace, SECTION)
        scratch.mkdir(parents=True, exist_ok=True)
        with TemporaryDirectory(prefix='probe-', dir=scratch) as folder:
            if state is not None:
                from .playable_probe import _checkpoint_bytes
                snapshot = Path(folder) / 'checkpoint.mss'
                snapshot.write_bytes(_checkpoint_bytes(state))
            else:
                snapshot = None

            def replay(track):
                return trace_player(runtime, source, Path(folder) / track, profile, tracks[track] or track, state=snapshot)

            with ThreadPoolExecutor(max_workers=3) as workers:
                traces = dict(zip(tracks, workers.map(replay, tracks)))
        samples = {track: {f: row for f, row in trace.items() if (f - profile.first) % 5 == 0}
                   for track, trace in traces.items()}
        try:
            slots = identify_player(samples, raw)
        except ValueError:
            if state is None:
                raise
            try:
                slots = identify_player(samples, raw, vertical=True)
            except ValueError:
                slots = find_larger_checkpoint_sprite(traces['neutral'], previous.slots, raw)
                if slots is None:
                    raise ValueError('Could not re-identify the player from this checkpoint. Try a save while the player can move.') from None
        new_poses = collect_poses(traces, slots, raw)
        if not new_poses:
            raise ValueError('The detected player poses could not be isolated safely.')
        return self._save_poses(identity, workspace, raw, previous, new_poses,
                                previous.slots if state is not None else slots)

    def _save_poses(self, identity, workspace, raw, previous, new_poses, slots):
        scratch = owned_path(workspace, SECTION)
        combined = dict(zip(previous.poses, previous.tags)) if previous else {}
        for pose, actions in new_poses.items():
            combined[pose] = ', '.join(sorted(set(filter(None, combined.get(pose, '').split(', '))) | actions))
        if len(combined) > MAX_POSES:
            raise ValueError(f'This cartridge already has too many poses to add this scan ({MAX_POSES} maximum).')
        for pose in combined:
            image = _pose_image(pose, raw)
            image.resize((64, 16 * len(pose)), Image.Resampling.NEAREST).save(self.pose_preview(workspace, pose))
        atomic_json(owned_path(scratch, 'detected.json'), {'game': identity, 'poses': [
            [[c.index, c.palette, c.mirrors] for c in pose] for pose in combined], 'tags': list(combined.values()),
            'slots': list(slots)})
        poses = tuple(combined)
        return Detection(poses, self.pose_preview(workspace, poses[0]), tuple(combined.values()), slots)

    def import_recording(self, identity, source, workspace):
        journal = owned_path(workspace, f'{SECTION}/recorded-poses.txt')
        if not journal.is_file():
            return 0
        previous = self.detection(identity, workspace)
        if previous is None:
            return 0
        raw = read_chr_rom(source)
        known = set(previous.poses)
        observed = {}
        for record in read_recording(journal):
            pose = tuple(Cell(*part) for part in record)
            if pose not in known and all(any(_pattern(raw, part.index)) for part in pose):
                observed[pose] = {'Recorded in play'}
                if len(known) + len(observed) >= MAX_POSES:
                    break
        if not observed or len(known) >= MAX_POSES:
            return 0
        self._save_poses(identity, workspace, raw, previous, observed, previous.slots)
        return len(observed)

    @staticmethod
    def _checkpoint(workspace, state):
        folder = owned_path(workspace, 'saves/mesen/states')
        state = Path(state).absolute()
        if (state.parent != folder or state.suffix.casefold() != '.mss'
                or state.is_symlink() or state.is_junction() or not state.is_file()):
            raise ValueError('Choose a Mesen save state from the saved states folder for this game.')
        return state

    @staticmethod
    def pose_preview(workspace, pose):
        return owned_path(workspace, f'{SECTION}/preview-{pose_key(pose)}.png')

    @staticmethod
    def image_path(workspace, key=None):
        if key is not None and not re.fullmatch(r'[0-9a-f]{32}', key):
            raise ValueError('Invalid player pose identifier.')
        return owned_path(workspace, f'{SECTION}/image-{key}.png' if key else f'{SECTION}/replacement.png')

    def clear_overrides(self, identity, workspace):
        detection = self.detection(identity, workspace)
        if detection is None or not self.image_path(workspace).is_file():
            raise ValueError('Choose a default image first.')
        paths = [self.image_path(workspace, pose_key(pose)) for pose in detection.poses]
        for path in paths:
            path.unlink(missing_ok=True)

    def detection(self, identity, workspace):
        path = owned_path(workspace, f'{SECTION}/detected.json')
        if not path.is_file():
            return None
        if path.stat().st_size > MAX_DETECTION_BYTES:
            raise ValueError('The saved player detection is too large.')
        document = json.loads(path.read_bytes())
        poses = document['poses']
        if document['game'] != identity or not 1 <= len(poses) <= MAX_POSES:
            raise ValueError('The saved player detection does not match this game.')
        decoded = tuple(tuple(Cell(*part) for part in pose) for pose in poses)
        if any(len(pose) not in (4, 8) or any(type(cell.index) is not int or not 0 <= cell.index < 512
                                   or not isinstance(cell.palette, str)
                                   or not re.fullmatch(r'FF(?:[0-3][0-9A-F]){3}', cell.palette)
                                   or type(cell.mirrors) is not int or cell.mirrors not in range(4)
                                   for cell in pose) for pose in decoded):
            raise ValueError('The saved player detection contains invalid sprite data.')
        tags = document.get('tags', [''] * len(decoded))
        if (len(tags) != len(decoded) or any(not isinstance(tag, str) or len(tag) > 80 for tag in tags)
                or len(set(decoded)) != len(decoded)):
            raise ValueError('The saved player detection contains invalid pose labels.')
        preview = self.pose_preview(workspace, decoded[0])
        if not preview.is_file():
            preview = owned_path(workspace, f'{SECTION}/detected.png')
        slots = document.get('slots', [])
        if (not isinstance(slots, list) or len(slots) not in (0, 4, 8)
                or any(type(slot) is not int or not 0 <= slot < 64 for slot in slots) or len(set(slots)) != len(slots)):
            raise ValueError('The saved player detection contains invalid sprite slots.')
        return Detection(decoded, preview, tuple(tags), tuple(slots))

    def save_image(self, identity, workspace, path, *, key=None):
        detection = self.detection(identity, workspace)
        if detection is None:
            raise ValueError('Detect the playable sprite first.')
        if key is not None and all(pose_key(pose) != key for pose in detection.poses):
            raise ValueError('This player pose has not been detected.')
        target = self.image_path(workspace, key)
        if path is None:
            target.unlink(missing_ok=True)
            return
        image = _fit_canvas(_read_image(Path(path)), (2 * CELL, 4 * CELL))
        temporary = target.with_name(f'{uuid4().hex}.png')
        try:
            image.save(temporary)
            os.replace(temporary, target)
        finally:
            temporary.unlink(missing_ok=True)

    @staticmethod
    def regenerated(workspace):
        return GeneratedPoses(workspace, f'{SECTION}/regenerated')

    def regenerate(self, identity, source, workspace):
        owned_cartridge(source, identity)
        detection = self.detection(identity, workspace)
        if detection is None:
            raise ValueError('Detect the playable sprite before regenerating poses.')
        poses = {pose_key(pose): pose for pose in detection.poses}
        chr_rom = read_chr_rom(source)
        return self.regenerated(workspace).regenerate(poses, lambda key: _pose_image(poses[key], chr_rom))

    def launch(self, identity, source, workspace, controls, *, record=False, regenerated=False,
               process_factory=subprocess.Popen):
        owned_cartridge(source, identity)
        if record and regenerated:
            raise ValueError('Choose recording or playing with regenerated poses.')
        if record:
            self.import_recording(identity, source, workspace)
        detection = self.detection(identity, workspace)
        if record and (detection is None or not detection.slots):
            raise ValueError('Run Detect player before recording new poses.')
        if detection is None:
            raise ValueError('Detect the player and choose an image before playing.')
        if regenerated:
            keys = tuple(map(pose_key, detection.poses))
            folder = self.regenerated(workspace).current(keys)
            if folder is None:
                raise ValueError('Regenerate all detected player poses before playing.')
            image = None
            overrides = {key: self._assigned_image(folder / 'images' / f'{key}.png') for key in keys}
        else:
            image = self._assigned_image(self.image_path(workspace))
            overrides = {key: artwork for pose in detection.poses if (key := pose_key(pose))
                         and (artwork := self._assigned_image(self.image_path(workspace, key))) is not None}
        if image is None and not overrides and not record:
            raise ValueError('Assign an image to at least one pose before playing.')
        runtime = self.runtime()
        home = self._player_home(runtime, workspace, record=record)
        saves = owned_path(workspace, 'saves/mesen')
        battery = owned_path(saves, 'battery')
        states = owned_path(saves, 'states')
        for folder in (battery, states):
            folder.mkdir(parents=True, exist_ok=True)
        enhanced = image is not None or bool(overrides)
        settings = {'Nes': {'EnableHdPacks': enhanced}, 'Preferences': {
            'SingleInstance': False, 'AutomaticallyCheckForUpdates': False,
            'OverrideSaveDataFolder': True, 'SaveDataFolder': str(battery),
            'OverrideSaveStateFolder': True, 'SaveStateFolder': str(states)}}
        set_keyboard(settings, controls)
        args = [str(home / 'Mesen.exe'), str(source)]
        chr_rom = read_chr_rom(source) if record or enhanced else None
        if record:
            journal = owned_path(workspace, f'{SECTION}/recorded-poses.txt')
            journal.write_bytes(b'')
            args.append(str(write_recorder(home, detection.slots, journal, chr_rom, source)))
            settings['Debug'] = {'ScriptWindow': {'AllowIoOsAccess': True, 'AllowNetworkAccess': False,
                                                 'AutoReloadScriptWhenFileChanges': False,
                                                 'AutoRestartScriptAfterPowerCycle': False}}
        atomic_json(home / 'settings.json', settings)
        if enhanced:
            write_pack(home, detection.poses, image, overrides, chr_rom=chr_rom, artwork_follows_pose=regenerated)
        log = owned_path(home, 'runtime.log')
        with log.open('w', encoding='utf-8') as stream:
            environment = os.environ.copy()
            for name in ('J2ME_REVIVAL_ENABLE_NES_ENHANCED', 'J2ME_REVIVAL_NES_CAPTURE', 'J2ME_REVIVAL_NES_LAYOUTS'):
                environment.pop(name, None)
            process = process_factory(args, cwd=home, shell=False,
                                      env=environment, stdout=stream, stderr=subprocess.STDOUT)
        return process, log

    @staticmethod
    def _assigned_image(path):
        if not path.is_file():
            return None
        with Image.open(path, formats=['PNG']) as image:
            if image.size not in ((2 * CELL, 2 * CELL), (2 * CELL, 4 * CELL)):
                raise ValueError('A saved replacement image must be chosen again.')
            return image.convert('RGBA')

    @staticmethod
    def _player_home(runtime, workspace, *, record=False):
        graphics = runtime._graphics()
        with (runtime.executable.parent / 'MesenCore.dll').open('rb') as stream:
            core_id = file_digest(stream, 'sha256').hexdigest()
        prefix = 'recorder' if record else 'player'
        home = owned_path(workspace, f'{SECTION}/{prefix}-{core_id[:16]}-{graphics.build_id[:16]}')
        if not home.exists():
            # Publish the verified portable frontend only after the copy is complete.
            scratch = home.with_name(f'.player-{uuid4().hex}')
            try:
                shutil.copytree(runtime.executable.parent, scratch, ignore=shutil.ignore_patterns(
                    'settings.json', 'HdPacks', 'Saves', 'SaveStates', '*.pdb'))
                shutil.copyfile(graphics.dll, scratch / 'revival_graphics.dll')
                scratch.rename(home)
            finally:
                if (scratch.is_dir() and not scratch.is_symlink() and not scratch.is_junction()
                        and scratch.resolve().parent == home.parent.resolve()):
                    shutil.rmtree(scratch)
        return home
