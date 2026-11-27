"""Find control-responsive sprite groups in a live NES recording."""

from collections import Counter, deque
from dataclasses import dataclass, field, replace
from itertools import accumulate
from functools import cached_property, lru_cache
from statistics import median

from .live_pose import LivePose


@dataclass(frozen=True)
class Group:
    sprites: tuple
    x: int
    y: int

    @cached_property
    def appearance(self):
        return {sprite.pattern for sprite in self.sprites}


@dataclass
class Track:
    group: Group
    evidence: deque = field(default_factory=lambda: deque(maxlen=64))
    releases: deque = field(default_factory=lambda: deque(maxlen=16))
    motion: deque = field(default_factory=lambda: deque(maxlen=2))
    observed: deque = field(default_factory=lambda: deque(maxlen=12))
    pose: LivePose | None = None
    onset_motion: tuple = (0, 0)
    missed: int = 0
    screen_steps: deque = field(default_factory=lambda: deque(maxlen=64))
    appearances: deque = field(default_factory=lambda: deque(maxlen=16))
    layouts: deque = field(default_factory=lambda: deque(maxlen=32))
    camera_motion: bool = False


def _tile_motion(frame, previous):
    if previous is None or frame.height != previous.height:
        return {}
    by_pattern = {}
    for sprite in previous.sprites:
        by_pattern.setdefault(sprite.pattern, []).append(sprite)
    motion = {}
    for sprite in frame.sprites:
        nearby = sorted((abs(sprite.x - old.x) + abs(sprite.y - old.y),
                         sprite.x - old.x, sprite.y - old.y)
                        for old in by_pattern.get(sprite.pattern, ()))
        if nearby and nearby[0][0] <= 12 and (len(nearby) == 1 or nearby[0][0] < nearby[1][0]):
            motion[sprite] = nearby[0][1:]
    return motion


def _moves_together(first, second, motion):
    a, b = motion.get(first), motion.get(second)
    return a is None or b is None or max(abs(a[0] - b[0]), abs(a[1] - b[1])) <= 2


def _groups(frame, previous=None):
    # Rebuild the layout every frame. Sprite slots can be reassigned when
    # animation, flicker or mapper state changes.
    motion = _tile_motion(frame, previous)
    groups = []
    for sprite in sorted(frame.sprites, key=lambda s: (s.y, s.x)):
        adjacent = []
        for index, members in enumerate(groups):
            if any(((abs(s.x - sprite.x) <= 8 and abs(s.y - sprite.y) <= 2)
                    or (abs(s.x - sprite.x) <= 2 and abs(s.y - sprite.y) <= frame.height))
                   and _moves_together(s, sprite, motion) for s in members):
                adjacent.append(index)
        joined = [sprite] + [s for index in adjacent for s in groups[index]]
        if (len(joined) * (frame.height // 8) <= 16
                and max(s.x for s in joined) - min(s.x for s in joined) <= 40
                and max(s.y for s in joined) - min(s.y for s in joined) <= 48):
            groups = [members for index, members in enumerate(groups) if index not in adjacent]
            groups.append(joined)
        else:
            groups.append([sprite])
    return [Group(tuple(members), min(s.x for s in members), max(s.y for s in members) + frame.height)
            for members in groups]


def _link(tracks, groups):
    matches = {}
    used = set()
    pairs = []
    appearances = [group.appearance for group in groups]
    for i, track in enumerate(tracks):
        previous = track.group
        appearance = previous.appearance
        for j, group in enumerate(groups):
            dx, dy = group.x - previous.x, group.y - previous.y
            distance = abs(dx) + abs(dy)
            shared = appearance & appearances[j]
            if distance > 16 + 5 * track.missed or (track.missed and not shared):
                continue
            old_colors = {s.colors for s in previous.sprites if s.colors is not None}
            new_colors = {s.colors for s in group.sprites if s.colors is not None}
            if old_colors and new_colors and old_colors.isdisjoint(new_colors) and not shared:
                continue
            # Match layout identity across OAM reorderings and short motion gaps.
            similarity = len(shared) / max(len(appearance), len(appearances[j]))
            cost = distance + 2 * abs(len(group.sprites) - len(previous.sprites)) - 9 * similarity
            pairs.append((cost, i, j, dx, dy))
    for _, i, j, dx, dy in sorted(pairs):
        if j not in matches and i not in used:
            matches[j] = (tracks[i], dx, dy)
            used.add(i)
    return matches, [track for i, track in enumerate(tracks) if i not in used and track.missed < 2]


def _pose(frame, group, *, ram):
    origin_x = min(s.x for s in group.sprites)
    origin_y = min(s.y for s in group.sprites)
    if any(s.x + 8 > 256 or s.y + frame.height > 239 for s in group.sprites):
        return None
    records = []
    for sprite in group.sprites:
        palette_offset = (sprite.flags & 3) * 8 + 2
        colors = sprite.colors or 'FF' + frame.palette[palette_offset:palette_offset + 6].upper()
        mirrors = sprite.flags >> 6
        for i in range(frame.height // 8):
            index = sprite.indices[i]
            if (ram and index != -1) or (not ram and index < 0):
                return None
            position = 1 - i if frame.height == 16 and mirrors & 2 else i
            records.append([sprite.x - origin_x, sprite.y - origin_y + position * 8,
                            None if ram else index, colors, mirrors, sprite.pattern[i * 32:i * 32 + 32]])
    records.sort(key=lambda record: (record[1], record[0]))
    return LivePose.parse(records)


_DIRECTIONS = {128: (1, 0), 64: (-1, 0), 16: (0, -1), 32: (0, 1),
               144: (1, -1), 160: (1, 1), 80: (-1, -1), 96: (-1, 1)}
_ACTIONS = {1: (0, -1), 2: (0, -1)}
_CONTROLS = _DIRECTIONS | _ACTIONS


def _input(buttons):
    direction = buttons & 240
    if direction in _DIRECTIONS:
        return direction
    return (buttons & 3) if (buttons & 3) in _ACTIONS else 0


def _score(track):
    # Compare the first few samples after a press with the object's motion
    # just before it. Allow a short delay for acceleration and animation.
    episodes = {}
    for episode, direction, motion, baseline in track.evidence:
        episodes.setdefault((episode, direction), []).append((motion, baseline))
    responses = []
    for (episode, direction), samples in episodes.items():
        adjusted = [motion - max(0, baseline) for motion, baseline in samples]
        positive = sum(delta >= 1 for delta in adjusted)
        # A jump can return to its starting point within the onset window.
        total = max(accumulate(adjusted)) if direction in _ACTIONS else sum(adjusted)
        if positive >= 2 and total >= 4:
            responses.append((episode, direction, min(20, total)))
    if len(responses) < 2:
        return 0
    vectors = [_DIRECTIONS[control] for _, control, _ in responses if control in _DIRECTIONS]
    reverses = any(ax * bx + ay * by < 0 for ax, ay in vectors for bx, by in vectors)
    # A runner or jumper can demonstrate control by repeatedly starting and
    # settling on release even when it cannot travel in the opposite direction.
    released = sum(episode in track.releases for episode, _, _ in responses)
    if not reverses and (len(responses) < 3 or released < 2):
        return 0
    if len(responses) == 2 and (not reverses or min(value for _, _, value in responses) < 7):
        return 0
    return sum(amount for _, _, amount in responses)


def _shares_identity(first, second):
    # Moving close to a lost player is insufficient to take over its track.
    shared = first.appearance & second.appearance
    return len(shared) >= min(2, len(first.appearance), len(second.appearance)) and bool(shared)


def find_poses(frames, *, ram, max_poses, diagnostics=None, known_poses=()):
    """Keep recurring poses from an unambiguous input-responsive track."""
    counts = diagnostics if diagnostics is not None else Counter()
    # Reuse established native identities instead of proving control again each run.
    from .pose_tracking import KnownPoseTracker
    known_tracker = KnownPoseTracker(known_poses, ram=ram, limit=max_poses)
    tracks = []
    previous_direction = 0
    previous_frame = None
    previous_observation = None
    episode = age = 0
    released_episode = None
    last_input = action = action_until = 0
    selected = {}
    provisional = {}
    provisional_owner = None
    provisional_palette = None
    observing = None
    recent = deque(maxlen=5)
    owner = None
    owner_palette = None
    owner_pattern = None
    for frame in frames:
        counts['frames'] += 1
        tracked = known_tracker.observe(frame)
        if tracked is not None:
            counts['known_player_frames'] += 1
        if known_tracker.anchored_frames:
            counts['known_player_unmatched'] += tracked is None
            continue
        if previous_frame is not None and frame.number - previous_frame > 12:
            tracks, previous_direction, age = [], 0, 0
            released_episode = None
            last_input = action = action_until = 0
            recent.clear()
            previous_observation = None
        previous_frame = frame.number
        control = _input(frame.buttons)
        if control in _ACTIONS and control != last_input:
            action, action_until = control, frame.number + 18
        elif control in _DIRECTIONS:
            action_until = 0
        last_input = control
        direction = control or (action if frame.number < action_until else 0)
        if direction != previous_direction:
            released_episode = episode if previous_direction in _CONTROLS and not direction else None
            episode += 1
            if direction in _DIRECTIONS:
                counts[f'press_{direction}'] += 1
            age = 0
        age += 1
        ax, ay = _CONTROLS.get(direction, (0, 0))
        previous_direction = direction
        groups = _groups(frame, previous_observation)
        camera_delta = None
        if (previous_observation is not None and frame.camera is not None
                and previous_observation.camera is not None):
            camera_delta = tuple((new - old + period // 2) % period - period // 2
                                 for new, old, period in zip(frame.camera, previous_observation.camera, (512, 480)))
        previous_observation = frame
        counts['sprite_frames'] += bool(groups)
        matches, unmatched = _link(tracks, groups)
        deltas = [(dx, dy) for track, dx, dy in matches.values() if not track.missed]
        scroll_x = median(dx for dx, _ in deltas) if len(deltas) >= 4 else 0
        scroll_y = median(dy for _, dy in deltas) if len(deltas) >= 4 else 0
        if camera_delta is not None and max(map(abs, camera_delta)) <= 24:
            scroll_x, scroll_y = (-value for value in camera_delta)
        for track in unmatched:
            track.missed += 1
        tracks = list(unmatched)
        visible = []
        for index, group in enumerate(groups):
            track, dx, dy = matches.get(index, (None, 0, 0))
            if track is None:
                track = Track(group)
            else:
                motion = (dx - scroll_x, dy - scroll_y)
                track.screen_steps.append(abs(dx) + abs(dy))
                track.camera_motion |= camera_delta is not None and any(camera_delta)
                if age == 1:
                    track.onset_motion = (tuple(median(axis) for axis in zip(*track.motion))
                                          if track.motion else (0, 0))
                if direction in _CONTROLS and 1 <= age <= 8 and not track.missed:
                    baseline = track.onset_motion[0] * ax + track.onset_motion[1] * ay
                    track.evidence.append((episode, direction, motion[0] * ax + motion[1] * ay, baseline))
                if (released_episode is not None and 2 <= age <= 8 and not track.missed
                        and max(map(abs, motion)) <= 1 and track.motion
                        and max(map(abs, track.motion[-1])) <= 1
                        and released_episode not in track.releases):
                    track.releases.append(released_episode)
                if track.missed:
                    track.motion.clear()
                else:
                    track.motion.append(motion)
                track.group, track.missed = group, 0
            pose = _pose(frame, group, ram=ram)
            counts['mapped_groups'] += pose is not None
            track.pose = pose
            if pose is not None:
                track.layouts.append(pose)
                appearance = tuple(cell.pattern for cell in pose)
                if not track.appearances or track.appearances[-1] != appearance:
                    track.appearances.append(appearance)
            tracks.append(track)
            visible.append(track)
        ranked = []
        for index, track in enumerate(visible):
            score = _score(track)
            # Camera motion also moves fixed HUD items in world coordinates.
            # Require actual screen travel or a recurring animation cycle.
            if (score >= 16 and track.camera_motion and sum(track.screen_steps) < 32
                    and len(set(track.appearances)) == len(track.appearances)):
                counts['static_camera_response'] += 1
                continue
            # An invalid high-scoring glyph must not hide a valid player track.
            if score >= 16 and (track.pose is None or not track.pose.overlayable
                               or not _has_visible_body(track.pose)):
                counts['non_body_response'] += 1
                continue
            ranked.append((score, index))
        ranked.sort(reverse=True)
        score, index = ranked[0] if ranked else (0, 0)
        if score < 16:
            counts['insufficient_response'] += 1
            recent.clear()
            continue
        counts['qualified_response'] += 1
        if len(ranked) > 1 and score - ranked[1][0] < 8:
            counts['ambiguous_response'] += 1
            recent.clear()
            continue
        winner = visible[index]
        if (owner is not None and winner is not owner
                and (any(track is owner for track in tracks)
                     or not _shares_identity(owner_pattern, winner.group))):
            counts['different_track'] += 1
            recent.clear()
            continue
        recent.append(winner)
        if len(recent) == recent.maxlen and all(track is winner for track in recent):
            pose = winner.pose
            if pose is None:
                counts['missing_layout'] += 1
                continue
            dominant_palette = max((cell.palette for cell in pose), key=lambda palette: sum(
                int(cell.pattern, 16).bit_count() for cell in pose if cell.palette == palette))
            if owner_palette is not None and dominant_palette != owner_palette:
                counts['palette_change'] += 1
                continue
            if owner is None and winner is not provisional_owner:
                provisional.clear()
                provisional_owner = winner
                provisional_palette = dominant_palette
            if owner is None and dominant_palette != provisional_palette:
                counts['provisional_palette_change'] += 1
                recent.clear()
                continue
            if winner is not observing:
                winner.observed.clear()
                observing = winner
            counts['selected_track'] += 1
            winner.observed.append(pose)
            if winner.observed.count(pose) < 2:
                continue
            if not _has_visible_body(pose):
                counts['disconnected_graphics'] += 1
                continue
            if owner is None:
                # A static menu pointer may follow the pad. Keep it provisional
                # so it cannot block a later animated character in the same run.
                pattern = tuple(cell.pattern for cell in pose)
                provisional.setdefault(pattern, pose)
                # Control proof arrives after movement has begun. Retain recurring
                # appearances from this same track instead of only its stopped pose.
                for observed, count in Counter(winner.layouts).items():
                    if count < 2 or not observed.overlayable or not _has_visible_body(observed):
                        continue
                    colors = max((cell.palette for cell in observed), key=lambda palette: sum(
                        int(cell.pattern, 16).bit_count() for cell in observed if cell.palette == palette))
                    if colors == dominant_palette:
                        provisional.setdefault(tuple(cell.pattern for cell in observed), observed)
                if len(provisional) < 2:
                    continue
                selected.update((item.key(), item) for item in provisional.values())
            owner = winner
            owner_palette = dominant_palette
            owner_pattern = winner.group
            selected.setdefault(pose.key(), pose)
            if len(selected) >= max_poses:
                break
    if known_tracker.anchored_frames:
        counts['learned_from_known_player'] = len(known_tracker.learned)
        return _filter_pose_fragments(tuple(known_tracker.learned.values())[:max_poses])
    if provisional and not selected:
        counts['insufficient_pose_variation'] += 1
    return _filter_pose_fragments(tuple(selected.values())[:max_poses])


def _filter_pose_fragments(poses):
    if not poses:
        return ()
    # Drop recurring limbs and isolated effects when the same continuing track
    # yielded a much larger body. Preserve differently sized character forms.
    areas = [pose.image(bytes(192)).getchannel('A').histogram()[255] for pose in poses]
    typical_body = sorted(areas, reverse=True)[len(areas) // 5]
    sizes = Counter(pose.size for pose, area in zip(poses, areas) if area >= typical_body)
    full_width_by_height = {}
    for (width, height), _ in sizes.most_common():
        full_width_by_height.setdefault(height, width)

    kept = []
    for pose, area in zip(poses, areas):
        if 3 * area < typical_body:
            continue
        # An entire column can disappear during an occlusion. Compare widths
        # at the same height so a shorter power-up form can still be kept.
        width, height = pose.size
        if 5 * area < 3 * typical_body and width * 2 <= full_width_by_height.get(height, 0):
            continue
        kept.append(pose)
    # Whole-pose overlays retain genuine layers; the legacy tile-pack writer
    # applies its separate non-overlap requirement at its own boundary.
    return tuple(kept)


def _contains_layout(pose, layout):
    cells = set(pose)
    first = layout[0]
    for anchor in pose:
        if anchor != replace(first, x=anchor.x, y=anchor.y):
            continue
        dx, dy = anchor.x - first.x, anchor.y - first.y
        if all(replace(cell, x=cell.x + dx, y=cell.y + dy) in cells for cell in layout):
            return True
    return False


@lru_cache(maxsize=512)
def _has_visible_body(pose):
    # Disconnected pieces of text can move with a menu or scrolling scene.
    alpha = pose.image(bytes(192)).getchannel('A')
    bounds = alpha.getbbox()
    if bounds is None:
        return False
    _, top, _, bottom = bounds
    minimum = 28 if bottom - top <= 8 else 16
    remaining = {(x, y) for y in range(alpha.height) for x in range(alpha.width)
                 if alpha.getpixel((x, y))}
    while len(remaining) >= minimum:
        pending = [remaining.pop()]
        connected = 0
        while pending:
            x, y = pending.pop()
            connected += 1
            if connected >= minimum:
                return True
            for dx, dy in ((-1,-1), (-1,0), (-1,1), (0,-1), (0,1), (1,-1), (1,0), (1,1)):
                neighbor = x + dx, y + dy
                if neighbor in remaining:
                    remaining.remove(neighbor)
                    pending.append(neighbor)
    return False
