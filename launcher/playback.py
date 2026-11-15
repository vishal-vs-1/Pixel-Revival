"""Start selected players and present their runtime measurements."""

import json
import time
from pathlib import Path
from tkinter import messagebox
from systems.contracts import GameRef, NES


class LauncherPlayback:
    def play(self, original=False, game=None):
        if self.busy:
            self.status.set('Finish preparing artwork before starting the player.')
            return
        game = game or self.game()
        if not game:
            return
        ref=GameRef.from_metadata(game)
        capture=False
        if ref.system==NES:
            if game.get('runtime_status')=='setup-needed':
                self.status.set('The NES player is not installed; this cartridge remains saved in your library.')
                return
            if game.get('runtime_status')=='unsupported':
                self.status.set('This NES cartridge is reported as unsupported by the installed player.')
                return
            capture=not original
            if capture and not self.library.enhanced_available(NES):
                self.status.set('This NES player build cannot capture tiles; Original playback is still available.')
                return
        self.launch_game(game,original,capture=capture)

    def launch_game(self,game,original=False,*,capture=False,player_replacement=False,player_recording=False,
                    player_regenerated=False,pose_mode=None):
        ref=GameRef.from_metadata(game)
        try:
            old = self.players.get(ref.id)
            if old and old[0].poll() is None:
                messagebox.showinfo('Already running', 'This game is already open. Close its player window before launching it again.')
                return
            if ref.system==NES:
                options = {'player_regenerated': True} if player_regenerated else {}
                process,log=self.library.launch(game,original,capture=capture,
                                                host_parent=self.root, player_replacement=player_replacement,
                                                player_recording=player_recording, **options)
            else:
                options = {'pose_mode': pose_mode} if pose_mode else {}
                process,log=self.library.launch(game, original, **options)
            self.players[ref.id] = (process,log)
            if player_recording and ref.system == NES:
                self._recording_players.add(ref.id)
            self.player_refs[ref.id]=ref
            self._notify_player_started(ref,log)
            hint=' '+game.get('startup_hint','')
            if ref.system==NES:
                mode=('Regenerated poses' if player_regenerated else 'Pose recording' if player_recording
                      else 'Player replacement' if player_replacement else 'Original' if original else 'Enhanced')
                suffix='; captured tiles will be prepared after close' if capture else ''
                self.status.set(f"Playing {game['name']} in {mode} NES mode{suffix}."+hint)
            elif pose_mode == 'regenerated':
                self.status.set(f"Playing {game['name']} with regenerated player poses."+hint)
            else:
                self.status.set(f"Playing {game['name']}. Small live image jobs need fresh frame and input headroom; larger work waits until play ends."+hint)
        except Exception as error:
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
            self.library.set_player_state(ref,True,log.with_suffix('.stats.json'),
                                          allow_live=(ref.system!=NES))
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
        try:
            report=json.loads(Path(str(log.with_suffix('.stats.json'))+'.gpu.json').read_text(encoding='utf-8'))
            suffix='' if report.get('matched',True) else '\nWindows selected a different GPU from the requested preference.'
            self.active_gpu.set('Actual playback: '+report['device']+suffix)
        except (OSError,ValueError,KeyError):pass
        try:
            metrics=json.loads(log.with_suffix('.stats.json').read_text(encoding='utf-8'))
            if time.time()-metrics['updated']<4:
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
        except (OSError,ValueError,KeyError):
            self.metrics.set('Player starting… waiting for frame measurements.')
        try:
            with log.open('rb') as file:
                file.seek(0,2);end=file.tell();file.seek(max(0,end-8192));text=file.read().decode('utf-8',errors='replace')
            if 'Exception in thread' in text or 'MIDlet event failed' in text:
                self.compat.set('Compatibility: an error was detected — see saved logs')
        except OSError:pass
