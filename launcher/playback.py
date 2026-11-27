"""Start selected players and present their runtime measurements."""

import json
import threading
import time
from pathlib import Path
from tkinter import messagebox
from systems.comparison import ComparisonPlayers
from systems.contracts import GameRef, NES
from systems.playback_session import PlaybackSession
from .comparison import ComparisonWindow


def _read_json_limited(path, limit=65536):
    """Read a small player report without letting a huge file stall the UI."""
    try:
        if path.is_symlink() or not path.is_file() or path.stat().st_size > limit:
            return None
        document = json.loads(path.read_text(encoding="utf-8"))
        return document if isinstance(document, dict) else None
    except (OSError, ValueError):
        return None


class LauncherPlayback:
    def select_sprite_playback(self, selected):
        if selected == 'image' and self.image_play.get():
            self.poses_play.set(False)
        elif selected == 'poses' and self.poses_play.get():
            self.image_play.set(False)
        self._refresh_readiness()

    def set_record_artwork(self):
        game = self.game_if_selected()
        if game is None:
            return
        previous = game.get('record_artwork', True)
        running = self.players.get(game['id'])
        if self.busy or (running and running[0].poll() is None):
            self.record_artwork.set(previous)
            self.status.set('Close this game before changing recording.')
            return
        updated = {**game, 'record_artwork': bool(self.record_artwork.get())}
        try:
            self.library.save(updated)
        except (OSError, ValueError, RuntimeError) as error:
            self.record_artwork.set(previous)
            self.status.set(f'Could not save recording choice: {error}')
            return
        game.update(updated)
        self._refresh_readiness()

    def _refresh_readiness(self):
        if not hasattr(self, 'readiness'):
            return
        from .readiness import describe_readiness
        game = self.game_if_selected()
        self.readiness.set(describe_readiness(game,
            image=self.image_play.get(), poses=self.poses_play.get()) if game else 'Select a game to play.')

    def set_sprite_playback_options(self, image_available, poses_available, *, enabled=True):
        for variable, checkbox, available in (
                (self.image_play, self.image_play_checkbox, image_available),
                (self.poses_play, self.poses_play_checkbox, poses_available)):
            checkbox.configure(state='normal' if enabled and available else 'disabled')
            if not available:
                variable.set(False)

    def _sprite_playback_options(self, game):
        image = bool(getattr(self, 'image_play', None) and self.image_play.get())
        poses = bool(getattr(self, 'poses_play', None) and self.poses_play.get())
        panel = getattr(self, 'playable_artwork', None)
        if panel is not None and panel.identity != GameRef.from_metadata(game):
            image = poses = False
        if GameRef.from_metadata(game).system == NES:
            return {'player_replacement': image and not poses,
                    'player_regenerated': poses}
        recording = game.get('record_artwork', True)
        return {'pose_mode': ('regenerated' if poses else
                             ('record-replace' if recording else 'replace') if image else
                             'record' if recording else None)}

    def _blocked_message(self):
        reason = getattr(self, 'busy_reason', '')
        if reason:
            return f'Finish {reason} before starting the player.'
        return 'Finish preparing artwork before starting the player.'

    def play_comparison(self):
        if getattr(self, 'busy', False) or not (game := self.game()):
            if getattr(self, 'busy', False):
                self.status.set(self._blocked_message())
            return
        if GameRef.from_metadata(game).system == NES and not self.library.enhanced_available(NES):
            self.status.set('Side-by-side comparison requires an Enhanced-capable NES player.')
            return
        self.start_comparison(game)

    def start_comparison(self, game):
        """Launch both players; the Rust pair starts inline on the window thread."""
        if getattr(self, '_closing', False):
            return
        ref = GameRef.from_metadata(game)
        if ref.system == NES and ref.id in getattr(self, '_pose_importing', ()):
            self.status.set('Wait for the recorded player poses to finish importing.')
            return
        old = self.players.get(ref.id)
        if old and old[0].poll() is None:
            messagebox.showinfo('Already running',
                                'This game is already open. Close its player window before launching it again.')
            return
        playback_options = self._sprite_playback_options(game)
        self.busy = True
        self.busy_reason = 'starting the comparison players'
        self.status.set('Starting both comparison players…')
        self.progress.start()
        in_tool = (ref.system == NES and
                   getattr(getattr(self, 'nes_runtime', None),
                           'supports_in_tool_comparison', False) is True)
        if in_tool:
            try:
                process, log = self.library.launch(game, comparison=True,
                                                   host_parent=self.root, **playback_options)
            except Exception as error:
                self.comparison_failed((game, error))
            else:
                self.comparison_started((game, process, log))
            return

        def worker():
            try:
                process, log = self.library.launch(game, comparison=True,
                                                   host_parent=self.root, **playback_options)
            except Exception as error:
                self.messages.put(('comparison_failed', (game, error)))
            else:
                self.messages.put(('comparison_started', (game, process, log)))

        threading.Thread(target=worker, name='comparison-launch', daemon=True).start()

    def _finish_comparison_launch(self):
        self.busy = False
        self.busy_reason = ''
        self.progress.stop()

    def comparison_started(self, value):
        game, process, log = value
        ref = GameRef.from_metadata(game)
        self.players[ref.id] = PlaybackSession(ref, process, log,
            recording=(ref.system == NES and game.get('record_artwork', True)))
        if getattr(process, 'owns_window', False):
            # The Rust pair already owns one Tk window with both panes.
            process.view = process
            self._notify_player_started(ref, log)
            self.status.set('Comparison opened in this window with shared keyboard controls.')
            self._finish_comparison_launch()
            return
        try:
            process.view = ComparisonWindow(
                self.root, process, game['name'],
                embed_players=(ref.system != NES))
        except (OSError, RuntimeError, ValueError) as error:
            for player in getattr(process, 'players', ()):
                if player.poll() is None:
                    try:
                        player.terminate()
                    except OSError:
                        pass
            self.comparison_failed((game, error))
            return
        self._notify_player_started(ref, log)
        self.status.set('Comparison opened with shared keyboard controls.')
        self._finish_comparison_launch()

    def comparison_failed(self, value):
        game, error = value
        ref = GameRef.from_metadata(game)
        self.players.pop(ref.id, None)
        getattr(self, 'player_refs', {}).pop(ref.id, None)
        getattr(self, '_recording_players', {}).pop(ref.id, None)
        self._finish_comparison_launch()
        try:
            self.library.set_player_state(ref, False)
        except KeyError:
            pass
        if not any(process.poll() is None for process, _ in self.players.values()) and not self.preparation_paused:
            self.library.resume_all()
        messagebox.showerror('Could not start comparison', str(error))

    def play(self, original=None, game=None):
        if getattr(self, '_closing', False):
            return
        if self.busy:
            self.status.set(self._blocked_message())
            return
        game = game or self.game()
        if not game:
            return
        ref=GameRef.from_metadata(game)
        if original is None:
            original = not self.enhance_play.get()
        capture=False
        if ref.system==NES:
            if game.get('runtime_status')=='setup-needed':
                self.status.set('The Rust NES player is not built; this cartridge remains saved in your library.')
                return
            if game.get('runtime_status')=='unsupported':
                self.status.set('This NES cartridge is reported as unsupported by the Rust player.')
                return
            capture=game.get('record_artwork', True)
            if not original and not self.library.enhanced_available(NES):
                original = True
        self.launch_game(game, original, capture=capture,
                         capture_poses=(ref.system == NES and game.get('record_artwork', True)),
                         **self._sprite_playback_options(game))

    def launch_game(self,game,original=False,*,capture=False,pose_mode=None,comparison=False,
                    player_replacement=False,player_recording=False,player_regenerated=False,
                    capture_poses=False):
        if getattr(self, '_closing', False):
            return
        ref=GameRef.from_metadata(game)
        if ref.system == NES and ref.id in getattr(self, '_pose_importing', ()):
            self.status.set('Wait for the recorded player poses to finish importing.')
            return
        try:
            old = self.players.get(ref.id)
            if old and old[0].poll() is None:
                messagebox.showinfo('Already running', 'This game is already open. Close its player window before launching it again.')
                return
            if comparison:
                process, log = self.library.launch(game, comparison=True,
                                                   host_parent=self.root,
                                                   **self._sprite_playback_options(game))
            elif ref.system==NES:
                options = {}
                if player_replacement:
                    options['player_replacement'] = True
                if player_regenerated:
                    options['player_regenerated'] = True
                if player_recording:
                    options['player_recording'] = True
                if capture_poses:
                    options['capture_poses'] = True
                process,log=self.library.launch(game,original,capture=capture,
                                                host_parent=self.root, **options)
            else:
                options = {'pose_mode': pose_mode} if pose_mode else {}
                process,log=self.library.launch(game, original, **options)
            self.players[ref.id] = PlaybackSession(ref, process, log,
                recording=(ref.system == NES and (player_recording or capture_poses or comparison)))
            self._notify_player_started(ref,log)
            if comparison:
                if getattr(process, 'owns_window', False):
                    process.view = process
                    self.status.set('Comparison opened in this window with shared keyboard controls.')
                    return
                try:
                    process.view = ComparisonWindow(
                        self.root, process, game['name'],
                        embed_players=(ref.system != NES))
                except (OSError, RuntimeError, ValueError):
                    for player in process.players:
                        if player.poll() is None:
                            player.terminate()
                    raise
                self.status.set('Comparison opened with shared keyboard controls.')
                return
            if ref.system == NES and getattr(process, 'owns_window', False) is True:
                process.view = process
            hint=' '+game.get('startup_hint','')
            if ref.system==NES:
                mode='Original' if original else 'Enhanced'
                capability = getattr(self.nes_runtime, 'capture_available', None)
                can_capture = bool(callable(capability) and capability())
                if capture and can_capture:
                    suffix='; captured tiles will be prepared after close'
                elif capture:
                    suffix='; using the prepared pack in this window'
                else:
                    suffix=''
                self.status.set(f"Playing {game['name']} in {mode} NES mode{suffix}."+hint)
            elif pose_mode == 'regenerated':
                self.status.set(f"Playing {game['name']} with regenerated player poses."+hint)
            else:
                self.status.set(f"Playing {game['name']}. Small live image jobs need fresh frame and input headroom; larger work waits until play ends."+hint)
        except Exception as error:
            # A comparison window can fail after the pair was registered.
            # Close and forget it so Play does not stay stuck on
            # "Already running" with a leaked player process.
            active = self.players.pop(ref.id, None)
            getattr(self, 'player_refs', {}).pop(ref.id, None)
            getattr(self, '_recording_players', {}).pop(ref.id, None)
            if active is not None:
                process = active[0]
                players = getattr(process, 'players', None)
                if players is None or not hasattr(players, '__iter__'):
                    players = (process,)
                for player in players:
                    try:
                        if getattr(player, 'poll', lambda: 0)() is None:
                            player.terminate()
                    except (OSError, RuntimeError):
                        pass
            try:self.library.set_player_state(ref,False)
            except KeyError:pass
            if not any(process.poll() is None for process,_ in self.players.values()) and not self.preparation_paused:
                self.library.resume_all()
            messagebox.showerror('Could not start game', str(error))

    def capture_tick(self):
        # A player can start before recovery registers its artwork workspace.
        for ref in tuple(self._pending_player_states):
            player=self.players.get(ref.id)
            if player is None or player[0].poll() is not None:
                self._pending_player_states.discard(ref)
            else:
                self._notify_player_started(ref,player[1])
        self.root.after(1500,self.capture_tick)

    def _notify_player_started(self, ref, log):
        try:
            player = self.players[ref.id][0]
            self.library.set_player_state(ref,True,log.with_suffix('.stats.json'),
                                          allow_live=(ref.system != NES and not isinstance(player, ComparisonPlayers)))
        except KeyError:
            self._pending_player_states.add(ref)
        else:
            self._pending_player_states.discard(ref)

    def update_telemetry(self):
        selection=self.tree.selection()
        if not selection:return
        key=selection[0]
        game=self.games.get(key)
        if game and GameRef.from_metadata(game).system==NES:
            self.metrics.set('NES frame and input measurements are not connected to the library screen yet.')
            return
        active=self.players.get(key)
        if not active or active[0].poll() is not None:
            self.metrics.set('Saved artwork is reused. Temporary resized sprites do not need AI conversion.')
            return
        _,log=active
        report=_read_json_limited(Path(str(log.with_suffix('.stats.json'))+'.gpu.json'))
        if report is not None:
            suffix='' if report.get('matched',True) else '\nWindows selected a different GPU from the requested preference.'
            self.active_gpu.set('Actual playback: '+str(report.get('device','Player'))+suffix)
        metrics=_read_json_limited(log.with_suffix('.stats.json'))
        try:
            if metrics is not None and time.time()-metrics['updated']<4:
                live=self.games.get(key,{}).get('asset_work',{}).get('live_reason')
                scaler=metrics.get('fast_scaler_submit_ms',0.0)
                actual_backend=metrics.get('backend','Player')
                if actual_backend=='legacy-java':
                    backend_label='GPU · compatibility'
                elif actual_backend=='Java2D CPU':
                    backend_label='CPU · compatibility'
                else:
                    backend_label=actual_backend
                detail=f"{backend_label}  ·  {metrics['fps']:.1f} game FPS\nInput queue p95 {metrics['input_queue_p95_ms']:.1f} ms  ·  {metrics['scale']}× detail, desktop fit"
                if scaler:detail+=f'\nGPU scaler CPU submission {scaler:.2f} ms (last texture)'
                if metrics.get('gpu_scale_fallbacks',0):
                    detail+='\nGPU filter fallback is in use; details are in the player log.'
                elif metrics.get('gpu_scale_passes',0):
                    detail+='\nLive GPU filter active; detailed artwork prepares in the background.'
                self.metrics.set(detail+(f'\nLive artwork: {live}' if live else ''))
            else:self.metrics.set('Waiting for screen updates; menus may redraw only after input.')
        except (KeyError,TypeError):
            self.metrics.set('Player starting… waiting for frame measurements.')
        now = time.monotonic()
        if now - getattr(self, '_log_tail_at', 0.0) >= 5.0:
            self._log_tail_at = now
            try:
                with log.open('rb') as file:
                    file.seek(0,2);end=file.tell();file.seek(max(0,end-8192));text=file.read().decode('utf-8',errors='replace')
                if 'Exception in thread' in text or 'MIDlet event failed' in text:
                    self.compat.set('Compatibility: an error was detected — see saved logs')
            except OSError:pass
