"""Mesen discovery and artwork adapter for the shared player pose panel."""
from pathlib import Path
from threading import Thread
from tkinter import filedialog, messagebox

from systems.contracts import NES
from launcher.playable_artwork_ui import ArtworkView, PoseChoice
from .capture import owned_path
from systems.nes.pose_layouts import REPLAYS, pose_key


class NesPlayableArtwork:
    def __init__(self, app, library):
        self.app = app
        self.library = library
        self.detecting = False

    def view(self, game):
        if not game:
            return ArtworkView('Select a game.')
        identity = game['id']
        if identity not in REPLAYS:
            return ArtworkView('Player detection is available for the tested Super Mario Bros., Donkey Kong, and Road Fighter cartridges.')
        workspace = self.library.workspace(identity)
        service = self.library.playable_assets
        detection = service.detection(identity, workspace)
        if not detection:
            return ArtworkView('Detect the player in a short controller replay.', detect=True)
        poses = []
        for pose, action in zip(detection.poses, detection.tags):
            key = pose_key(pose)
            form = 'Tall' if len(pose) == 8 else 'Small'
            preview = service.pose_preview(workspace, pose)
            poses.append(PoseChoice(key, f'{form} · {action or "Pose"}',
                                    preview if preview.is_file() else detection.preview,
                                    service.image_path(workspace, key)))
        regenerated = service.regenerated(workspace).current(pose.key for pose in poses)
        return ArtworkView(f'Detected {len(poses)} player poses. Regenerate them or choose your own images.',
                           tuple(poses), service.image_path(workspace), record=bool(detection.slots),
                           scan=bool(detection.slots), detect=True, regenerated=regenerated)

    def scan_state(self):
        game = self.app.game()
        if not game:
            return
        folder = owned_path(self.library.workspace(game['id']), 'saves/mesen/states')
        path = filedialog.askopenfilename(parent=self.app.playable_artwork, initialdir=folder, title='Choose this game’s Mesen save state',
                                         filetypes=[('Mesen save states', '*.mss')])
        if path:
            self.detect(state=Path(path))

    def detect(self, *, state=None, latest=False):
        if self.detecting:
            return
        game = self.app.game()
        if not game:
            return
        identity = game['id']
        self.detecting = True
        self.app.playable_artwork.refresh(game)
        self.app.status.set('Scanning the latest checkpoint…' if latest
                            else f'Detecting the playable sprite in {game["name"]}…')

        def worker():
            try:
                detection = self.library.detect_playable_asset(identity, state=state, latest=latest)
                self.app.messages.put(('playable_asset', (identity, len(detection.poses), None, state is None and not latest)))
            except Exception as error:
                self.app.messages.put(('playable_asset', (identity, 0, str(error), False)))

        Thread(target=worker, daemon=True, name='nes-player-detection').start()

    def detected(self, identity, count, error, choose_image=True):
        self.detecting = False
        game = self.app.game_if_selected()
        self.app.playable_artwork.refresh(game)
        self.app._update_row_actions()
        if error:
            self.app.status.set('The scan could not finish.')
            messagebox.showerror('Could not detect player', error, parent=self.app.playable_artwork)
        else:
            self.app.status.set(f'Detected {count} player sprite layouts.')
            if (choose_image and self.app.visible_system_id == NES and game
                    and game['id'] == identity and not self.app.busy):
                self.app.playable_artwork.selection_key = None
                self.app.playable_artwork.choose_image()

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
