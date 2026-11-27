"""Manage NES player pose detection, assignments, regeneration, and playback."""

import json
import os
from pathlib import Path
import re
from uuid import uuid4

from PIL import Image

from storage.atomic import atomic_json
from systems.generated_poses import GeneratedPoses
from .capture import owned_path
from .playable_recording import read_recording
from .live_player_capture import read_frames
from .live_player_tracking import find_poses
from .live_pose import LivePose
from .rom import NesRomInspector
from .pose_layouts import (
    Cell, Detection, _pattern, _pose_image, offsets_for, owned_cartridge, pose_key, read_chr_rom,
)
from .pose_pack import CELL, _fit_canvas, _fit_pose, _read_image
from .pose_overlay import PoseOverlay

SECTION = 'playable-assets'
MAX_POSES = 192
MAX_DETECTION_BYTES = 256 * 1024


class PlayableAssets:
    def __init__(self, app_root):
        self.app_root = Path(app_root)

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

    def import_recording(self, identity, source, workspace, *, session_journal=None, observed_poses=()):
        if session_journal is not None:
            journal = owned_path(workspace, session_journal.relative_to(workspace).as_posix())
            return self._import_live_recording(identity, source, workspace, journal) if journal.is_file() else 0
        live = owned_path(workspace, f'{SECTION}/live-recording.tsv')
        if live.is_file():
            return self._import_live_recording(identity, source, workspace, live, observed_poses=observed_poses)
        journal = owned_path(workspace, f'{SECTION}/recorded-poses.txt')
        if not journal.is_file():
            return 0
        previous = self._legacy_detection(identity, workspace)
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

    def _import_live_recording(self, identity, source, workspace, journal, *, observed_poses=()):
        owned_cartridge(source, identity)
        info = NesRomInspector().inspect(source)
        legacy = self._legacy_detection(identity, workspace)
        known = {pose.key(): pose for pose in self._live_poses(identity, workspace)}
        capacity = MAX_POSES - len(legacy.poses if legacy else ()) - len(known)
        if capacity <= 0:
            return 0
        legacy_layouts = {tuple((x, y, cell.index, cell.palette, cell.mirrors)
                                for (x, y), cell in zip(offsets_for(pose), pose))
                          for pose in legacy.poses} if legacy else set()
        # Counter-like default values let the detector report why a recording
        # failed without relaxing the evidence needed to replace a sprite.
        from collections import Counter
        from .pose_tracking import native_seed
        diagnostics = Counter()
        raw = read_chr_rom(source) if legacy else None
        seeds = tuple(known.values())
        if legacy:
            seeds += tuple(native_seed(pose, raw) for pose in legacy.poses)
        candidates = find_poses(read_frames(journal), ram=info.chr_kind == 'ram',
                                max_poses=MAX_POSES, diagnostics=diagnostics, known_poses=seeds)
        # Per-frame replacement can see animations between journal samples.
        # Merge only recurring, native poses retained by its bounded tracker.
        observed = tuple(LivePose.parse(pose.document()) for pose in observed_poses[:MAX_POSES]
                         if isinstance(pose, LivePose) and pose.overlayable)
        if observed:
            diagnostics['learned_during_play'] = len(observed)
            candidates = tuple({pose.key(): pose for pose in (*candidates, *observed)}.values())[:MAX_POSES]
        added = [pose for pose in candidates if pose.key() not in known and
                 tuple((cell.x, cell.y, cell.index, cell.palette, cell.mirrors)
                       for cell in pose) not in legacy_layouts][:capacity]
        atomic_json(owned_path(workspace, f'{SECTION}/recording-report.json'), {
            'schema': 1, 'game': identity, 'detected': len(candidates),
            'added': len(added), 'observations': dict(diagnostics),
        })
        if not added:
            return 0
        for pose in added:
            image = _pose_image(pose, None)
            image.resize((image.width * 4, image.height * 4), Image.Resampling.NEAREST).save(
                self.pose_preview(workspace, pose))
            known[pose.key()] = pose
        atomic_json(owned_path(workspace, f'{SECTION}/live-poses.json'),
                    {'game': identity, 'poses': [pose.document() for pose in known.values()]})
        return len(added)

    @staticmethod
    def _live_poses(identity, workspace):
        path = owned_path(workspace, f'{SECTION}/live-poses.json')
        if not path.is_file():
            return ()
        with path.open('rb') as stream:
            payload = stream.read(512 * 1024 + 1)
        if len(payload) > 512 * 1024:
            raise ValueError('The saved live player poses are too large.')
        document = json.loads(payload)
        if (not isinstance(document, dict) or document.get('game') != identity
                or not isinstance(document.get('poses'), list) or len(document['poses']) > MAX_POSES):
            raise ValueError('The live player poses do not match this game.')
        poses = tuple(LivePose.parse(records) for records in document['poses'])
        if len(set(poses)) != len(poses):
            raise ValueError('Duplicate live player poses.')
        return poses

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
        legacy = self._legacy_detection(identity, workspace)
        live = self._live_poses(identity, workspace)
        if not live:
            return legacy
        poses = (*(legacy.poses if legacy else ()), *live)
        tags = (*(legacy.tags if legacy else ()), *('Recorded in play' for _ in live))
        if len(poses) > MAX_POSES:
            raise ValueError('This cartridge has too many recorded poses.')
        return Detection(poses, legacy.preview if legacy else self.pose_preview(workspace, live[0]),
                         tags, legacy.slots if legacy else ())

    def _legacy_detection(self, identity, workspace):
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
            raise ValueError('Record player poses during gameplay first.')
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
            raise ValueError('Record player poses before regenerating them.')
        poses = {pose_key(pose): pose for pose in detection.poses}
        chr_rom = read_chr_rom(source) if any(not isinstance(pose, LivePose) for pose in poses.values()) else None
        return self.regenerated(workspace).regenerate(poses, lambda key: _pose_image(poses[key], chr_rom))

    def pose_art(self, identity, source, workspace, *, regenerated=False):
        """Return (pose, 4x RGBA artwork) pairs for Rust pose overlay."""
        detection = self.detection(identity, workspace)
        if detection is None:
            raise ValueError('Record player poses and choose artwork first.')
        poses = tuple(pose for pose in detection.poses
                      if not isinstance(pose, LivePose) or pose.overlayable)
        if not poses:
            raise ValueError('The recorded poses are available as artwork previews.')
        chr_rom = (read_chr_rom(source)
                   if any(not isinstance(pose, LivePose) for pose in poses) else None)
        if regenerated:
            keys = tuple(map(pose_key, detection.poses))
            generation = self.regenerated(workspace).current(keys)
            if generation is None:
                raise ValueError('Regenerate all recorded player poses before playing.')
            pairs = []
            for pose in poses:
                path = generation / 'images' / f'{pose_key(pose)}.png'
                artwork = self._assigned_image(path, generated=True)
                if artwork is not None:
                    pairs.append((pose, artwork))
            if not pairs:
                raise ValueError('Regenerate at least one player pose before playing.')
            return tuple((pose, _fit_pose(artwork, pose, chr_rom))
                         for pose, artwork in pairs)
        default = self._assigned_image(self.image_path(workspace))
        pairs = []
        for pose in poses:
            key = pose_key(pose)
            artwork = self._assigned_image(self.image_path(workspace, key)) or default
            if artwork is not None:
                pairs.append((pose, _fit_pose(artwork, pose, chr_rom)))
        if not pairs:
            raise ValueError('Assign an image to at least one pose before playing.')
        return tuple(pairs)

    def pose_overlay(self, identity, source, workspace, *, regenerated=False):
        from .pose_tracking import native_seed
        pairs = self.pose_art(identity, source, workspace, regenerated=regenerated)
        detection = self.detection(identity, workspace)
        poses = detection.poses if detection else ()
        raw = read_chr_rom(source) if any(not isinstance(p, LivePose) for p in poses) else None
        seeds = tuple(native_seed(pose, raw) for pose in poses)
        default = None if regenerated else self._assigned_image(self.image_path(workspace))
        return PoseOverlay(pairs, default_art=default, known_poses=seeds)

    def enemy_candidates(self, identity, source, workspace):
        """Return likely computer-controlled sprite layouts from the journal."""
        from .enemy_candidates import find_enemy_candidates

        info = NesRomInspector().inspect(source)
        journal = owned_path(workspace, f'{SECTION}/live-recording.tsv')
        if not journal.is_file():
            return ()
        return find_enemy_candidates(
            read_frames(journal), ram=info.chr_kind == 'ram')

    @staticmethod
    def _assigned_image(path, *, generated=False):
        if not path.is_file():
            return None
        with Image.open(path, formats=['PNG']) as image:
            valid = max(image.size) <= 256 if generated else image.size in (
                (2 * CELL, 2 * CELL), (2 * CELL, 4 * CELL))
            if not valid:
                raise ValueError('A saved replacement image must be chosen again.')
            return image.convert('RGBA')
