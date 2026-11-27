"""Draw saved player-pose artwork over Rust NES frames.

The old Mesen pack used PPU position conditions. The Rust compositor only
knows tile identities, so poses are matched here from the per-frame OAM
snapshot and painted at the matched layout's origin.
"""

from __future__ import annotations

from PIL import Image

from .live_pose import LivePose, LiveCell as _Cell
from .pose_layouts import offsets_for
from .pose_observation import capture_frame, frame_cells
from .pose_tracking import KnownPoseTracker


def _saved_cells(pose):
    if isinstance(pose, LivePose):
        return tuple(_Cell(cell.x, cell.y, cell.index, cell.palette,
                           cell.mirrors, cell.pattern) for cell in pose)
    return tuple(_Cell(x, y, part.index, part.palette, part.mirrors, "")
                 for part, (x, y) in zip(pose, offsets_for(pose)))


class PoseOverlay:
    """Match recorded layouts against OAM and composite their 4x artwork."""

    def __init__(self, pairs, *, default_art=None, known_poses=()):
        pairs = tuple(pairs)
        self._entries = tuple(sorted((
            (tuple(_saved_cells(pose)), pose, art)
            for pose, art in pairs), key=lambda entry: len(entry[0]), reverse=True))
        self._core = None
        self._handle = None
        self._chr_ram = False
        self._scaled_art = {}
        self._default_art = default_art
        self._known_poses = tuple(known_poses) or tuple(p for p, _ in pairs if isinstance(p, LivePose))
        self._tracker = None
        self._live_art = {}
        self._frame_number = 0
        self.matched_frames = 0

    def bind(self, core, handle, chr_ram=False):
        self._core = core
        self._handle = handle
        self._chr_ram = bool(chr_ram)
        if self._known_poses:
            self._tracker = KnownPoseTracker(self._known_poses, ram=self._chr_ram)

    @property
    def learned_poses(self):
        return tuple(self._tracker.learned.values()) if self._tracker is not None else ()

    def _current_cells(self, observation):
        if self._core is None:
            return ()
        return frame_cells(capture_frame(self._core, self._handle, 0, 0, observation,
                                         ram=self._chr_ram))

    @staticmethod
    def _matches(saved, current, keyed=None):
        if keyed is None:
            keyed = {}
            for cell in current:
                keyed.setdefault((cell.x, cell.y), []).append(cell)
        for anchor in current:
            origin_x, origin_y = anchor.x - saved[0].x, anchor.y - saved[0].y
            ok = True
            used = set()
            for expected in saved:
                position = (origin_x + expected.x, origin_y + expected.y)
                candidates = keyed.get(position, ())
                match = next((index for index, found in enumerate(candidates)
                           if (position, index) not in used
                           and found.palette == expected.palette
                           and found.mirrors == expected.mirrors
                           and (expected.index is None or found.index == expected.index)
                           and (not expected.pattern or found.pattern == expected.pattern)), None)
                if match is None:
                    ok = False
                    break
                used.add((position, match))
            if ok:
                yield origin_x, origin_y
                return

    def apply(self, data, width, height, observation, *, frame=None, frame_number=None, buttons=0):
        if not self._entries or self._core is None:
            return data
        self._frame_number = frame_number if frame_number is not None else self._frame_number + 1
        if frame is None:
            frame = capture_frame(self._core, self._handle, self._frame_number,
                                  buttons, observation, ram=self._chr_ram)
        current = frame_cells(frame)
        if not current:
            if self._tracker is not None:
                self._tracker.observe(frame)
            return data
        matches = []
        seen = set()
        keyed = {}
        for cell in current:
            keyed.setdefault((cell.x, cell.y), []).append(cell)
        for saved, pose, art in self._entries:
            for x, y in self._matches(saved, current, keyed):
                if (x, y, id(art)) in seen:
                    continue
                seen.add((x, y, id(art)))
                matches.append((x, y, pose, art))
        tracked = self._tracker.observe(frame) if self._tracker is not None else None
        if tracked is not None and self._default_art is not None:
            signature = tuple(tracked.pose.cells)
            exact = any(isinstance(pose, LivePose) and tuple(pose.cells) == signature
                        and (x, y) == (tracked.x, tracked.y) for x, y, pose, _ in matches)
            if not exact:
                art = self._live_art.get(signature)
                if art is None and len(self._live_art) < 192:
                    from .pose_pack import _fit_pose
                    art = _fit_pose(self._default_art, tracked.pose, None)
                    self._live_art[signature] = art
                if art is not None:
                    # Replace partial saved matches at this origin with the full live pose.
                    pw, ph = tracked.pose.size
                    matches = [(x, y, pose, image) for x, y, pose, image in matches
                               if not (tracked.x <= x < tracked.x + pw and tracked.y <= y < tracked.y + ph)]
                    matches.append((tracked.x, tracked.y, tracked.pose, art))
        if not matches:
            return data
        self.matched_frames += 1
        output = bytearray(data)
        scale = max(1, width // 256)
        for x, y, _pose, art in matches:
            key = (id(art), scale)
            target = self._scaled_art.get(key)
            if target is None:
                target = art.resize(
                    (max(1, art.width * scale // 4), max(1, art.height * scale // 4)),
                    Image.Resampling.NEAREST)
                self._scaled_art[key] = target
            x, y = x * scale, y * scale
            left, top = max(0, x), max(0, y)
            right, bottom = min(width, x + target.width), min(height, y + target.height)
            if right <= left or bottom <= top:
                continue
            # Convert only the sprite rectangle. Converting the entire 4x
            # framebuffer twice per pose starves both pacing and audio.
            stride = width * 4
            rows = [row * stride + left * 4 for row in range(top, bottom)]
            row_bytes = (right - left) * 4
            region = b''.join(output[offset:offset + row_bytes] for offset in rows)
            image = Image.frombytes('RGB', (right - left, bottom - top), region,
                                    'raw', 'BGRX').convert('RGBA')
            image.alpha_composite(target.crop((left - x, top - y, right - x, bottom - y)))
            pixels = image.convert('RGB').tobytes('raw', 'BGRX')
            for row, offset in enumerate(rows):
                output[offset:offset + row_bytes] = pixels[row * row_bytes:(row + 1) * row_bytes]
        return bytes(output)
