"""Opt-in analysis of autonomous sprite candidates in a NES gameplay journal."""

from collections import Counter
from dataclasses import dataclass, field

from .live_pose import LivePose
from .live_player_tracking import (
    _filter_pose_fragments, _groups, _has_visible_body, _link, _pose, find_poses,
)


@dataclass
class _Track:
    group: object
    missed: int = 0
    observations: int = 0
    quiet_motion: int = 0
    active_motion: int = 0
    travel: int = 0
    player_seen: bool = False
    poses: Counter = field(default_factory=Counter)


@dataclass(frozen=True)
class EnemyCandidate:
    pose: LivePose
    observations: int
    quiet_motion: int
    active_motion: int


def find_enemy_candidates(frames, *, ram, limit=24):
    """Return review candidates once the input-responsive player can be excluded."""
    frames = tuple(frames)
    player = find_poses(frames, ram=ram, max_poses=192)
    if not player:
        return ()
    player_tiles = {(cell.pattern, cell.palette) for pose in player for cell in pose
                    if int(cell.pattern, 16)}
    player_palettes = {palette for _, palette in player_tiles}
    previous = None
    tracks = []
    candidates = {}

    def finish(track):
        if (track.player_seen or track.observations < 16 or track.quiet_motion < 4
                or track.active_motion < 4 or track.travel < 14):
            return
        recurring = tuple(pose for pose, seen in track.poses.most_common() if seen >= 2)
        for pose in _filter_pose_fragments(recurring)[:8]:
            if not _has_visible_body(pose):
                continue
            candidate = EnemyCandidate(pose, track.observations, track.quiet_motion, track.active_motion)
            old = candidates.get(pose)
            if old is None or old.observations < candidate.observations:
                candidates[pose] = candidate

    for frame in frames:
        if previous is not None and frame.number - previous.number > 12:
            for track in tracks:
                finish(track)
            tracks, previous = [], None
        groups = _groups(frame, previous)
        previous = frame
        matches, retained = _link(tracks, groups)
        live_ids = {id(track) for track in retained}
        live_ids.update(id(track) for track, _, _ in matches.values())
        for track in tracks:
            if id(track) not in live_ids:
                finish(track)
        for track in retained:
            track.missed += 1

        # Remove a displacement shared by a majority of tracked objects.
        deltas = Counter((dx, dy) for track, dx, dy in matches.values() if not track.missed)
        scroll, votes = deltas.most_common(1)[0] if deltas else ((0, 0), 0)
        if votes < 3 or 2 * votes <= deltas.total():
            scroll = (0, 0)
        tracks = retained
        for index, group in enumerate(groups):
            matched = matches.get(index)
            if matched is None:
                track = _Track(group)
            else:
                track, dx, dy = matched
                distance = abs(dx - scroll[0]) + abs(dy - scroll[1])
                if not track.missed and 0 < distance <= 9:
                    track.travel += distance
                    if frame.buttons == 0:
                        track.quiet_motion += 1
                    elif frame.buttons & 0xf0:
                        track.active_motion += 1
                track.group, track.missed = group, 0
            track.observations += 1
            pose = _pose(frame, group, ram=ram)
            if pose is not None:
                # A known player tile can also occur in a merged player/enemy group.
                track.player_seen |= any((cell.pattern, cell.palette) in player_tiles for cell in pose)
                palettes = {cell.palette for cell in pose if int(cell.pattern, 16)}
                # Shared player colors or layered groups leave ownership ambiguous.
                if (pose.replaceable and len(palettes) == 1 and palettes.isdisjoint(player_palettes)
                        and (len(track.poses) < 64 or pose in track.poses)):
                    track.poses[pose] += 1
            tracks.append(track)

    for track in tracks:
        finish(track)
    return tuple(sorted(candidates.values(), key=lambda item: item.observations, reverse=True)[:limit])
