"""Publish one complete set of regenerated player poses in a game workspace."""

import json
from pathlib import Path
import re
import shutil
from uuid import uuid4

from graphics.runtime import is_redirected_path
from storage.atomic import atomic_json
from runtime_assets.sprite_paint import paint_pose


MAX_POSES = 192
RECIPE = 'sprite-paint-ink-v1'
GENERATION = re.compile('[0-9a-f]{32}')
POSE_KEY = re.compile('[0-9a-f]{32}(?:[0-9a-f]{32})?')


class GeneratedPoses:
    def __init__(self, workspace, relative, *, keep_sources=False):
        self.workspace = workspace
        self.relative = Path(relative)
        self.keep_sources = keep_sources

    def _path(self, *parts):
        path = self.workspace
        if is_redirected_path(path):
            raise ValueError('The game workspace is redirected.')
        for component in (*self.relative.parts, *parts):
            path /= component
            if is_redirected_path(path):
                raise ValueError('The regenerated artwork folder is redirected.')
        if not path.resolve().is_relative_to(self.workspace.resolve()):
            raise ValueError('The regenerated artwork path escapes the game folder.')
        return path

    @staticmethod
    def _keys(keys):
        keys = sorted(keys)
        if (len(keys) > MAX_POSES or len(set(keys)) != len(keys)
                or any(not POSE_KEY.fullmatch(key) for key in keys)):
            raise ValueError('The saved player pose list is invalid.')
        return keys

    def _manifest(self):
        manifest = self._path('current.json')
        try:
            with manifest.open('rb') as stream:
                raw = stream.read(16_385)
            data = json.loads(raw) if len(raw) <= 16_384 else None
        except (FileNotFoundError, ValueError):
            return None
        if not isinstance(data, dict):
            return None
        name = data.get('generation')
        return data if isinstance(name, str) and GENERATION.fullmatch(name) else None

    def current(self, keys):
        keys = self._keys(keys)
        try:
            data = self._manifest() if keys else None
            if data is None or data.get('recipe') != RECIPE or data.get('keys') != keys:
                return None
            name = data['generation']
            folders = ('images', 'poses') if self.keep_sources else ('images',)
            if all(self._path(name, folder, f'{key}.png').is_file()
                   for folder in folders for key in keys):
                return self._path(name)
        except (OSError, ValueError):
            # An unreadable generated cache must not disable the image workflow.
            return None
        return None

    def regenerate(self, keys, read_pose):
        keys = self._keys(keys)
        if not keys:
            raise ValueError('Record or detect player poses before regenerating them.')
        previous = self._manifest()
        name = uuid4().hex
        folder = self._path(name)
        folder.mkdir(parents=True)
        try:
            (folder / 'images').mkdir()
            if self.keep_sources:
                (folder / 'poses').mkdir()
            for key in keys:
                source = read_pose(key)
                # The Java ME replacement reader accepts images up to 256 pixels.
                scale = min(4, 256 // max(source.size))
                drawing = paint_pose(source, scale=scale)
                if drawing.getchannel('A').getbbox() is None:
                    raise ValueError('A saved player pose is empty. Review the captured poses and try again.')
                drawing.save(folder / 'images' / f'{key}.png', optimize=True)
                if self.keep_sources:
                    source.save(folder / 'poses' / f'{key}.png')
            # Readers see the new set only after every pose has been written.
            atomic_json(self._path('current.json'), {'recipe': RECIPE, 'generation': name, 'keys': keys})
        except Exception:
            self._discard(name)
            raise
        if previous:
            # System libraries serialize generation with playback. Remove the
            # old cache only after the new set has been published.
            self._discard(previous['generation'])
        return len(keys)

    def _discard(self, name):
        if not GENERATION.fullmatch(name):
            return
        try:
            folder = self._path(name)
            parent = self._path().resolve()
            if folder.resolve().parent == parent and folder.resolve().is_relative_to(self.workspace.resolve()):
                shutil.rmtree(folder, ignore_errors=True)
        except (OSError, ValueError):
            pass
