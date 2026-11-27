"""Follow known native player poses through bounded animation transitions."""

from collections import OrderedDict
from dataclasses import dataclass

from .live_pose import LivePose
from .live_player_tracking import _groups, _pose


@dataclass(frozen=True)
class TrackedPose:
    pose: LivePose
    x: int
    y: int


def native_seed(pose, chr_rom=None):
    if isinstance(pose, LivePose):
        return pose
    from .pose_layouts import _pattern, offsets_for
    if chr_rom is None:
        return None
    return LivePose.parse([
        [x, y, cell.index, cell.palette, cell.mirrors, _pattern(chr_rom, cell.index).hex()]
        for cell, (x, y) in zip(pose, offsets_for(pose))])


class KnownPoseTracker:
    """Require a native anchor before following a nearby animation change."""

    def __init__(self, poses, *, ram=False, limit=192):
        self.seeds = tuple(p for p in poses if p is not None and p.overlayable)[:limit]
        self.ram = ram
        self.limit = limit
        self._known = {tuple(p.cells) for p in self.seeds}
        self._patterns = {c.pattern for p in self.seeds for c in p if int(c.pattern, 16)}
        self._palettes = {c.palette for p in self.seeds for c in p}
        self._width = max((p.size[0] for p in self.seeds), default=0)
        self._height = max((p.size[1] for p in self.seeds), default=0)
        self._counts = OrderedDict()
        self.learned = OrderedDict()
        self._previous = None
        self._owner = None
        self._anchor = None
        self._last_number = None
        self.anchored_frames = 0

    def observe(self, frame):
        if not self.seeds:
            return None
        gap = frame.number - self._last_number if self._last_number is not None else 1
        if gap < 1 or gap > 12:
            self._owner = self._anchor = self._previous = None
            self._counts.clear()
        candidates = []
        for group in _groups(frame, self._previous):
            pose = _pose(frame, group, ram=self.ram)
            if pose is None or not pose.overlayable:
                continue
            width, height = pose.size
            if width > self._width + 8 or height > self._height + 16:
                continue
            signature = tuple(pose.cells)
            exact = signature in self._known or signature in self.learned
            shared = len({c.pattern for c in pose} & self._patterns)
            # A palette change is acceptable only while native tile identity persists.
            if not exact and not all(c.palette in self._palettes for c in pose) and not (self._owner and shared >= 2):
                continue
            candidates.append((group, pose, signature, exact, shared))
        self._previous = frame
        self._last_number = frame.number
        chosen = None
        if self._owner is not None:
            old_group, old_pose = self._owner
            old_width, old_height = old_pose.size
            nearby = []
            for candidate in candidates:
                group, pose, _, exact, shared = candidate
                width, height = pose.size
                # Missing body tiles must not become a new head-only replacement.
                if (not exact and len(pose) * 2 <= len(old_pose)
                        and width * height * 2 <= old_width * old_height):
                    continue
                dx = abs(group.x + width / 2 - old_group.x - old_width / 2)
                # Group.y already denotes the bottom of the native sprite group.
                dy = abs(group.y - old_group.y)
                if max(dx, dy) > min(32, max(12, gap * 6)):
                    continue
                # A wholly new appearance can bridge only a short animation gap.
                if not exact and shared < min(2, len(pose)) and (self._anchor is None or frame.number - self._anchor > 12):
                    continue
                cost = dx + dy + abs(width - old_width) / 2 + abs(height - old_height) / 4
                nearby.append((cost - min(2, shared) * 3 - (4 if exact else 0), candidate))
            nearby.sort(key=lambda item: item[0])
            if nearby and (len(nearby) == 1 or nearby[1][0] - nearby[0][0] >= 4):
                chosen = nearby[0][1]
        if chosen is None:
            anchors = [candidate for candidate in candidates if candidate[3]]
            if len(anchors) == 1:
                chosen = anchors[0]
        if chosen is None:
            self._owner = None
            self._counts.clear()
            return None
        group, pose, signature, exact, shared = chosen
        trusted = exact or shared >= min(2, len(pose))
        if trusted:
            self._anchor = frame.number
        self._owner = group, pose
        self.anchored_frames += 1
        if not exact:
            count = self._counts.get(signature, (pose, 0))[1] + 1
            self._counts[signature] = pose, count
            self._counts.move_to_end(signature)
            while len(self._counts) > self.limit:
                self._counts.popitem(last=False)
        # An entirely changed appearance must reconnect to a trusted identity.
        # Lost or ambiguous tracks discard their unconfirmed transitions.
        if trusted:
            for key, (observed, count) in tuple(self._counts.items()):
                if count >= 2 and len(self.learned) < self.limit:
                    self.learned.setdefault(key, observed)
                    self._counts.pop(key)
        return TrackedPose(pose, group.x, group.y - pose.size[1] + 1)
