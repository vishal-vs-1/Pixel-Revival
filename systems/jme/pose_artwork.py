"""Per-game Java ME sprite previews and user artwork."""

import os
import re
from uuid import uuid4

from PIL import Image

from graphics.runtime import is_redirected_path
from systems.generated_poses import GeneratedPoses


class PoseArtwork:
    def __init__(self, workspace):
        self.root = workspace / 'player-poses'
        self.regenerated = GeneratedPoses(workspace, 'player-poses/regenerated', keep_sources=True)

    def folder(self, name):
        path = self.root / name
        if is_redirected_path(self.root) or is_redirected_path(path):
            raise ValueError('The sprite artwork folder has been redirected.')
        return path

    def poses(self):
        return sorted(path.stem for path in self.folder('poses').glob('*.png')
                      if re.fullmatch('[0-9a-f]{64}', path.stem) and not is_redirected_path(path))

    def assigned(self, key):
        path = self.folder('images') / f'{key or "default"}.png'
        if is_redirected_path(path):
            raise ValueError('The replacement image has been redirected.')
        return path

    def is_drawn(self, key):
        return (self.folder('drawn') / key).is_file()

    def is_confirmed(self, key):
        return self.selected(key).is_file() or self.assigned(key).is_file()

    def confirmed_keys(self):
        return tuple(key for key in self.poses() if self.is_confirmed(key))

    def regenerate(self):
        poses = self.folder('poses')

        def read_pose(key):
            with Image.open(poses / f'{key}.png', formats=['PNG']) as source:
                if max(source.size) > 128:
                    raise ValueError('A captured player pose exceeds the 128 pixel size limit.')
                return source.convert('RGBA')

        return self.regenerated.regenerate(self.confirmed_keys(), read_pose)

    def has_assignments(self):
        default = self.assigned(None).is_file()
        return any(self.assigned(key).is_file()
                   or (default and self.selected(key).is_file()) for key in self.poses())

    def clear_overrides(self):
        for key in self.poses():
            if self.assigned(key).is_file():
                self.set_selected(key, True)
                self.assign(key, None)

    def selected(self, key):
        path = self.folder('selected') / key
        if is_redirected_path(path):
            raise ValueError('The pose selection has been redirected.')
        return path

    def set_selected(self, key, value):
        if key not in self.poses():
            raise ValueError('Select a recorded sprite pose.')
        path = self.selected(key)
        if value:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.touch()
        else:
            path.unlink(missing_ok=True)

    def assign(self, key, source):
        if key is not None and key not in self.poses():
            raise ValueError('Select a recorded sprite pose.')
        target = self.assigned(key)
        if source is None:
            target.unlink(missing_ok=True)
            return
        with Image.open(source) as opened:
            if opened.width * opened.height > 4_194_304:
                raise ValueError('Choose an image no larger than four megapixels.')
            image = opened.convert('RGBA')
        image.thumbnail((256, 256), Image.Resampling.LANCZOS)
        target.parent.mkdir(parents=True, exist_ok=True)
        temp = target.with_name(f'{uuid4().hex}.tmp')
        try:
            image.save(temp, format='PNG')
            os.replace(temp, target)
        finally:
            temp.unlink(missing_ok=True)

    def launch_arguments(self, record, *, regenerated=False):
        if regenerated:
            folder = self.regenerated.current(self.confirmed_keys())
            if folder is None:
                raise ValueError('Regenerate all confirmed player poses before playing.')
            # The existing player reads the same folder layout and fits each
            # drawing against its matching source, including procedural poses.
            return [f'-Djme.playerPoses={folder}', '-Djme.recordPoses=false']
        for name in ('poses', 'images', 'selected', 'suggested', 'drawn'):
            self.folder(name).mkdir(parents=True, exist_ok=True)
        return [f'-Djme.playerPoses={self.root}', f'-Djme.recordPoses={str(record).lower()}']
