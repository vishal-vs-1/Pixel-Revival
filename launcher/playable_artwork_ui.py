"""Shared character artwork controls for the active game system."""
from dataclasses import dataclass
from pathlib import Path
from threading import Thread
import tkinter as tk
from tkinter import filedialog, messagebox, ttk

from PIL import Image, ImageTk

from systems.contracts import NES, GameRef


@dataclass(frozen=True)
class PoseChoice:
    key: str
    label: str
    preview: Path
    image: Path
    confirmed: bool = True
    replaceable: bool = True


@dataclass(frozen=True)
class ArtworkView:
    hint: str
    poses: tuple = ()
    default: Path | None = None
    record: bool = False
    regenerated: Path | None = None


class PlayableArtworkPanel(ttk.Frame):
    def __init__(self, parent, app, nes, java, *, pack_before=None):
        super().__init__(parent)
        self.app = app
        self.nes = nes
        self.java = java
        self._pack_before = pack_before
        self._packed = True
        self.identity = None
        self.selection_key = None
        self.choices = {}
        self.show_candidates = tk.BooleanVar(master=self)
        self.confirmed = tk.BooleanVar(master=self)
        self.hint = tk.StringVar(master=self)
        ttk.Label(self, text='Playable sprite').pack(anchor='w')
        ttk.Label(self, textvariable=self.hint, wraplength=390).pack(fill='x', pady=(4, 8))

        actions = self._row()
        self.image_button = self._button(actions, 'Choose image', self.choose_image)
        self.play_button = self._button(actions, 'Play with image', self.play)

        generation = self._row()
        self.regenerate_button = self._button(generation, 'Regenerate all poses', self.regenerate)
        self.play_regenerated_button = self._button(generation, 'Play with regenerated poses', self.play_regenerated)

        self.review_tools = self._row()
        ttk.Checkbutton(self.review_tools, text='Review captured artwork', variable=self.show_candidates,
                        command=self.update_selection).pack(side='left', padx=(0, 6))
        self.confirm_button = ttk.Checkbutton(self.review_tools, text='This is a player pose',
                                             variable=self.confirmed, command=self.confirm)
        self.confirm_button.pack(side='left')

        defaults = self._row()
        self.default_button = self._button(defaults, 'Use default for all', self.clear_overrides)
        assignment = self._row()
        self.pose_picker = ttk.Combobox(assignment, state='disabled', width=29)
        self.pose_picker.pack(side='left', fill='x', expand=True)
        self.pose_picker.bind('<<ComboboxSelected>>', self._select_pose)
        self.clear_button = self._button(assignment, 'Clear image', self.clear_image)
        previews = self._row()
        self.preview = ttk.Label(previews)
        self.preview.pack(side='left', padx=(0, 12))
        self.replacement_preview = ttk.Label(previews)
        self.replacement_preview.pack(side='left', padx=(0, 12))
        self.regenerated_preview = ttk.Label(previews, compound='top')
        self.regenerated_preview.pack(side='left')

    def _row(self):
        row = ttk.Frame(self)
        row.pack(fill='x', pady=(4, 0))
        return row

    @staticmethod
    def _button(parent, label, command):
        button = ttk.Button(parent, text=label, command=command)
        button.pack(side='left', padx=(0, 6))
        return button

    @property
    def backend(self):
        return self.nes if self.app.visible_system_id == NES else self.java

    def refresh(self, game):
        if self.backend is None:
            self.review_tools.pack_forget()
            if self._packed:
                self.pack_forget()
                self._packed = False
            return
        if not self._packed:
            self.pack(fill='x', before=self._pack_before, pady=(14, 0))
            self._packed = True
        identity = GameRef.from_metadata(game) if game else None
        if identity != self.identity:
            self.identity, self.selection_key = identity, None
            self.show_candidates.set(False)
        nes = self.backend is self.nes
        if nes:
            self.review_tools.pack_forget()
        elif not self.review_tools.winfo_manager():
            self.review_tools.pack(fill='x', pady=(4, 0), after=self.play_button.master)
        idle = bool(game) and not self.app.busy
        player = self.app.players.get(identity.id) if identity else None
        can_generate = idle and not (player and player[0].poll() is None)
        try:
            view = self.backend.view(game)
            poses = [pose for pose in view.poses if pose.confirmed or self.show_candidates.get()]
            confirmed = [pose for pose in view.poses if pose.confirmed]
            replaceable = [pose for pose in confirmed if pose.replaceable]
            custom = any(pose.image.is_file() for pose in replaceable)
            default = bool(view.default and view.default.is_file())
            self.choices = {'Default · all player poses': None} if view.default else {}
            self.choices.update({f'{i:02d} · {pose.label}' + (' · custom' if pose.image.is_file() else ''): pose.key
                                 for i, pose in enumerate(poses, 1)})
            selected = next((pose for pose in poses if pose.key == self.selection_key), None)
            self.selection_key = selected.key if selected else None
            image = selected.image if selected else view.default
            self.hint.set(view.hint)
            self.pose_picker.configure(values=list(self.choices),
                                       state='readonly' if idle and self.choices else 'disabled')
            self.pose_picker.set(next((label for label, key in self.choices.items() if key == self.selection_key), ''))
            self.confirmed.set(bool(selected and selected.confirmed))
            self._show_image(self.preview, selected.preview if selected else confirmed[0].preview if confirmed else None)
            self._show_image(self.replacement_preview, image if image and image.is_file()
                             else view.default if selected and selected.confirmed else None)
            generated_pose = selected or (confirmed[0] if confirmed else None)
            generated_image = (view.regenerated / 'images' / f'{generated_pose.key}.png'
                               if view.regenerated and generated_pose and generated_pose.confirmed else None)
            try:
                self._show_image(self.regenerated_preview, generated_image)
            except OSError:
                self._show_image(self.regenerated_preview, None)
            self.regenerated_preview.configure(text='Regenerated' if self.regenerated_preview.picture else '')
            for button, available in (
                (self.image_button, bool(image and (not selected or selected.replaceable))), (self.clear_button, bool(image and image.is_file())),
                (self.play_button, custom or bool(replaceable and default)),
                (self.default_button, default and custom), (self.confirm_button, selected is not None),
            ):
                button.configure(state='normal' if idle and available else 'disabled')
            self.regenerate_button.configure(state='normal' if can_generate and confirmed else 'disabled')
            self.play_regenerated_button.configure(state='normal' if can_generate and view.regenerated and replaceable else 'disabled')
        except (OSError, ValueError, KeyError, TypeError) as error:
            self.hint.set(str(error))
            self.pose_picker.configure(state='disabled', values=())
            self.pose_picker.set('')
            self._show_image(self.preview, None)
            self._show_image(self.replacement_preview, None)
            self._show_image(self.regenerated_preview, None)
            self.regenerated_preview.configure(text='')
            for button in (self.image_button, self.clear_button, self.play_button, self.default_button,
                           self.confirm_button, self.regenerate_button, self.play_regenerated_button):
                button.configure(state='disabled')

    def _show_image(self, label, path):
        label.picture = None
        if path and path.is_file():
            with Image.open(path, formats=['PNG']) as image:
                if image.width <= 256 and image.height <= 256:
                    image.thumbnail((64, 96), Image.Resampling.NEAREST)
                    label.picture = ImageTk.PhotoImage(image, master=self)
        label.configure(image=label.picture or '')

    def update_selection(self):
        self.refresh(self.app.game_if_selected())

    def _select_pose(self, _event):
        self.selection_key = self.choices.get(self.pose_picker.get())
        self.update_selection()

    def _apply(self, action):
        game = self.app.game()
        if not game or self.app.busy or self.backend is None:
            return
        try:
            action(game)
        except (ValueError, OSError, RuntimeError) as error:
            messagebox.showerror('Playable sprite', str(error), parent=self)
        self.refresh(game)

    def choose_image(self):
        def save(game):
            path = filedialog.askopenfilename(parent=self, title='Choose a player replacement image',
                filetypes=[('Images', '*.png *.jpg *.jpeg *.webp *.bmp *.gif *.tif *.tiff *.ico'), ('All files', '*.*')])
            if path:
                self.backend.save(game, self.selection_key, Path(path))
        self._apply(save)

    def clear_image(self):
        self._apply(lambda game: self.backend.save(game, self.selection_key, None))

    def clear_overrides(self):
        if messagebox.askyesno('Use default for all', 'Clear individual images and use the default for all player poses?',
                               parent=self):
            self._apply(self.backend.clear_overrides)

    def confirm(self):
        self._apply(lambda game: self.java.confirm(game, self.selection_key, self.confirmed.get()))

    def record(self):
        self._apply(self.backend.record)

    def play(self):
        self._apply(self.backend.play)

    def regenerate(self):
        if self.regenerate_button.instate(['disabled']):
            return
        game = self.app.game()
        if not game:
            return
        backend = self.backend
        self.app.busy = True
        self.app.busy_reason = f'regenerating poses for {game["name"]}'
        self.app.status.set(f'Regenerating player poses for {game["name"]}.')
        self.app.progress.start()
        self.refresh(game)
        self.app._update_row_actions()

        def worker():
            try:
                result = (backend.regenerate(game), None)
            except Exception as error:
                result = (0, str(error))
            self.app.messages.put(('poses_regenerated', result))

        Thread(target=worker, daemon=True, name='regenerate-player-poses').start()

    def regenerated(self, count, error):
        self.app.busy = False
        self.app.busy_reason = ''
        self.app.progress.stop()
        self.refresh(self.app.game_if_selected())
        self.app._update_row_actions()
        if error:
            self.app.status.set('Player pose regeneration could not finish.')
            messagebox.showerror('Could not regenerate player poses', error, parent=self)
        else:
            self.app.status.set(f'Regenerated {count} player poses.')

    def play_regenerated(self):
        self._apply(self.backend.play_regenerated)
