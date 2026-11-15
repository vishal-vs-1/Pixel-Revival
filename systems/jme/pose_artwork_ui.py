"""Java ME artwork adapter for the shared player pose panel."""
from launcher.playable_artwork_ui import ArtworkView, PoseChoice
from .pose_artwork import PoseArtwork


class JavaPlayableArtwork:
    def __init__(self, app, library):
        self.app = app
        self.library = library

    def artwork(self, game):
        return PoseArtwork(self.app.library.workspace(game['id']))

    def view(self, game):
        if not game:
            return ArtworkView('Select a game.')
        artwork = self.artwork(game)
        suggestions = artwork.folder('suggested')
        previews = artwork.folder('poses')
        poses = []
        for key in artwork.poses():
            confirmed = artwork.is_confirmed(key)
            label = ('Player pose' if confirmed else 'Suggested pose' if (suggestions / key).is_file()
                     else 'Drawn candidate' if artwork.is_drawn(key) else 'Captured candidate')
            poses.append(PoseChoice(key, label, previews / f'{key}.png', artwork.assigned(key),
                                    confirmed))
        poses.sort(key=lambda pose: (not pose.confirmed, not (suggestions / pose.key).is_file()))
        keys = tuple(pose.key for pose in poses if pose.confirmed)
        count = len(keys)
        hint = (f'{count} confirmed player poses. Regenerate them or choose your own images.'
                if count else 'Open Review captured artwork and confirm the player poses.'
                if poses else 'Play & record poses to capture artwork for review.')
        return ArtworkView(hint, tuple(poses), artwork.assigned(None), record=True,
                           regenerated=artwork.regenerated.current(keys))

    def save(self, game, key, path):
        artwork = self.artwork(game)
        artwork.assign(key, path)
        if key is not None and path is not None:
            artwork.set_selected(key, True)

    def confirm(self, game, key, enabled):
        artwork = self.artwork(game)
        if not enabled and artwork.assigned(key).is_file():
            raise ValueError('Clear the custom image before removing this player pose.')
        artwork.set_selected(key, enabled)

    def clear_overrides(self, game):
        self.artwork(game).clear_overrides()

    def play(self, game):
        self.app.launch_game(game, pose_mode='replace')

    def regenerate(self, game):
        return self.library.regenerate_playable_poses(game['id'])

    def play_regenerated(self, game):
        self.app.launch_game(game, pose_mode='regenerated')

    def record(self, game):
        self.app.launch_game(game, pose_mode='record')
