"""Handle per-game preferences and shared controller profiles."""

from tkinter import messagebox
from graphics.gpu_policy import save_settings as save_gpu_settings
from launcher.palette import java_color, nes_color
from controls.java_me import DEFAULT_SLOT_ID
from controls.schema import JME_CONTROL_SCHEMA, NES_CONTROL_SCHEMA
from launcher.control_profile_dialog import ControlProfileDialog
from systems.contracts import GameRef, JAVA_ME, NES
from launcher.labels import GPU_CHOICES


class LauncherSettings:
    def set_gpu(self,key,label):
        save_gpu_settings(**{key:GPU_CHOICES[label]})
        self.status.set('GPU preference saved. Playback applies on next launch; conversion applies to the next artwork batch.')

    def _refresh_control_profile_controls(self, game=None):
        system=GameRef.from_metadata(game).system if game else self.visible_system_id
        store=self._profile_store_for(system)
        result=store.load()
        self.control_profile_problem=result.problem
        labels={'Default':DEFAULT_SLOT_ID}
        for profile in result.slots:
            labels[f'{profile.slot_id[-1]} · {profile.name}']=profile.slot_id
        self.control_profile_choice_ids=labels
        self.control_profile_combo.configure(values=tuple(labels))
        default_id=DEFAULT_SLOT_ID
        selected_id=game.get('control_profile_id',default_id) if game else default_id
        label=next((label for label,slot_id in labels.items() if slot_id==selected_id),'Default')
        self.control_profile.set(label)
        enabled=bool(game) and not self.busy and game['id'] not in self.deleting_games
        self.control_profile_combo.configure(state='readonly' if enabled else 'disabled')
        if result.problem:
            self.control_profile_info.set(f'Profiles could not be read. Default remains available. {result.problem}')
        elif game and selected_id != DEFAULT_SLOT_ID and label=='Default':
            self.control_profile_info.set('This game’s saved profile is missing. Default controls are selected.')
        elif not result.slots:
            self.control_profile_info.set('Create shared profiles in Settings.')
        else:
            self.control_profile_info.set('Profiles are managed in Settings. Choice applies next launch.')

    def open_settings(self):
        """Open or raise the one global keyboard-settings window."""
        current=self.settings_dialog
        if current is not None and current.top.winfo_exists():
            current.top.deiconify()
            current.top.lift()
            return
        system=self.visible_system_id
        stores={key:self._profile_store_for(key) for key in (JAVA_ME,NES)}
        self.settings_dialog=ControlProfileDialog(
            self.root,stores[system],self._control_profiles_changed,
            schema=NES_CONTROL_SCHEMA if system==NES else JME_CONTROL_SCHEMA,
            color=nes_color if system==NES else java_color,stores=stores)

    def _profile_store_for(self,system):
        if hasattr(self.library,'control_profiles_for'):
            return self.library.control_profiles_for(system)
        return self.nes_control_profiles if system==NES else self.library.control_profiles

    def _control_profiles_changed(self, _slot_id):
        # A shared-slot save changes available labels, never a game's chosen ID.
        self._refresh_control_profile_controls(self.game_if_selected())

    def set_control_profile(self,event=None):
        game=self.game_if_selected()
        label=self.control_profile.get()
        slot_id=self.control_profile_choice_ids.get(label)
        if (game is None or slot_id is None or self.busy
                or game['id'] in self.deleting_games):
            self._refresh_control_profile_controls(game)
            return
        try:
            self.library.set_control_profile_id(GameRef.from_metadata(game),slot_id)
        except (OSError,RuntimeError,ValueError) as error:
            self._refresh_control_profile_controls(game)
            self.status.set(f'Could not save control profile: {error}')
            return
        if slot_id==DEFAULT_SLOT_ID:
            game.pop('control_profile_id',None)
        else:
            game['control_profile_id']=slot_id
        self._refresh_control_profile_controls(game)
        self.status.set('Control profile preference saved for this game.')

    def set_quality(self,event=None):
        game=self.game()
        if not game:return
        old=self.players.get(game['id'])
        if old and old[0].poll() is None:
            messagebox.showinfo('Game is running','Close the player before changing display detail.')
            self.selected();return
        game['render_scale']=4 if self.quality.get().startswith('Detailed') else 2
        self.library.save(game)
        self.status.set('Display detail saved. Existing 4× artwork will be reused; no conversion is required.')

    def set_aspect(self,event=None):
        game=self.game()
        if game is None or GameRef.from_metadata(game).system!=NES:
            return
        game['aspect']=self.aspect.get()
        try:
            self.library.save(game)
        except (OSError,RuntimeError,ValueError) as error:
            self.status.set(f'Could not save NES aspect: {error}')
            self.selected()
            return
        self.status.set('NES aspect preference saved. The current native player still uses its built-in presentation ratio.')

    def set_renderer(self,event=None):
        game=self.game()
        if not game or GameRef.from_metadata(game).system!=JAVA_ME:return
        old=self.players.get(game['id'])
        if old and old[0].poll() is None:
            messagebox.showinfo('Game is running','Close the player before changing the renderer.');self.selected();return
        game['renderer']='cpu' if self.renderer.get().startswith('CPU') else 'gpu'
        self.library.save(game)
        self.status.set('Renderer saved. The next launch uses this setting.')

    def set_execution_mode(self):
        game=self.game()
        if not game or GameRef.from_metadata(game).system!=JAVA_ME:return
        player=self.players.get(game['id'])
        if player and player[0].poll() is None:
            messagebox.showinfo('Game is running','Close the player before changing Java compatibility mode.')
            self.selected();return
        game['execution_mode']='compatible' if self.execution_compatibility.get() else 'normal'
        self.library.save(game)
        self.status.set('Java execution setting saved for this game. Applies on the next launch; prepared artwork is reused.')

    def set_startup_cleanup(self):
        game=self.game()
        if not game or GameRef.from_metadata(game).system!=JAVA_ME:return
        game['remove_startup_wrappers']=bool(self.remove_wrappers.get())
        self.library.save(game);self.selected()
        self.status.set('Startup option saved. Applies next time you play; saved artwork is reused.')

    def set_resolution(self, event=None):
        game = self.game()
        if game and GameRef.from_metadata(game).system==JAVA_ME:
            if game['id'] in self.players and self.players[game['id']][0].poll() is None:
                messagebox.showinfo('Game is running', 'Close the game before changing its phone screen size.')
                self.selected()
                return
            game['width'], game['height'] = map(int, self.resolution.get().split(' × '))
            game['screen_source']='manual'
            self.library.save(game)
            self.refresh()
