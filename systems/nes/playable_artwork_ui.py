"""NES adapter for gameplay recording and player artwork."""
from launcher.playable_artwork_ui import ArtworkView, PoseChoice
from .live_pose import LivePose
from .pose_layouts import pose_key


class NesPlayableArtwork:
    def __init__(self, app, library):
        self.app = app
        self.library = library

    def view(self, game):
        if not game:
            return ArtworkView('Select a game.')
        identity = game['id']
        workspace = self.library.workspace(identity)
        service = self.library.playable_assets
        detection = service.detection(identity, workspace)
        if not detection:
            return ArtworkView(
                'Click Record poses, play with the pad, and repeat a few movements or jumps so the detector can isolate the character.',
                record=True)
        poses = []
        for pose, action in zip(detection.poses, detection.tags):
            key = pose_key(pose)
            live = isinstance(pose, LivePose)
            form = f'{pose.size[0]} x {pose.size[1]}' if live else 'Tall' if len(pose) == 8 else 'Small'
            replaceable = not live or pose.overlayable
            preview = service.pose_preview(workspace, pose)
            label = f'{form} - {action or "Pose"}'
            if not replaceable:
                label += ' (preview only)'
            poses.append(PoseChoice(key, label,
                                    preview if preview.is_file() else detection.preview,
                                    service.image_path(workspace, key), replaceable=replaceable))
        regenerated = service.regenerated(workspace).current(pose.key for pose in poses)
        return ArtworkView(f'{len(poses)} player poses. Record more during gameplay or choose artwork.',
                           tuple(poses), service.image_path(workspace), record=True, regenerated=regenerated)

    def save(self, game, key, path):
        self.library.save_playable_image(game['id'], path, key=key)

    def clear_overrides(self, game):
        self.library.clear_playable_overrides(game['id'])

    def play(self, game):
        self.app.launch_game(game, player_replacement=True)

    def regenerate(self, game):
        return self.library.regenerate_playable_poses(game['id'])

    def play_regenerated(self, game):
        self.app.launch_game(game, player_regenerated=True)

    def record(self, game):
        self.app.launch_game(game, player_recording=True)

    def find_enemies(self, game):
        return self.library.enemy_candidates(game['id'])
