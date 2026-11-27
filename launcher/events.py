"""Drain worker messages and update the launcher on the UI thread."""

import queue
import time
import threading
from tkinter import messagebox
from systems.contracts import GameRef, JAVA_ME, NES
from systems.ui_events import ApplicationEvent
from systems.playback_session import PlaybackSession
from launcher.labels import SYSTEM_LABELS


class LauncherEvents:
    def _on_asset_stats(self, game_id, stats):
        try:
            ref=self.library.reference(game_id)
        except (KeyError, LookupError, ValueError):
            return
        self.messages.put(('asset_stats',(ref,stats)))

    def bootstrap_asset_service(self):
        def report(error):
            self.messages.put(('background_status', f'Artwork cache recovery skipped: {error}'))
        try:
            self.library.recover_assets(report)
            self.messages.put(('refresh', None))
        except (OSError, ValueError, LookupError) as error:
            report(str(error))
        finally:
            completed = getattr(self, '_asset_recovery_done', None)
            if completed is not None:
                completed.set()

    def poll(self):
        # Drain bursts without spending the entire UI tick on progress messages.
        artwork_updates=set()
        for _ in range(100):
            try:
                kind, value = self.messages.get_nowait()
            except queue.Empty:
                break
            if kind == 'status':
                self.status.set(value+(f' · {len(self.pending_imports)} games queued' if self.pending_imports else ''))
            elif kind == 'background_status':
                self.preparation_info.set(value)
            elif kind == 'asset_stats':
                ref,stats=value
                if not isinstance(ref,GameRef):continue
                self._asset_stats[ref]=stats
                if ref.system==self.visible_system_id and ref.id in self.games:
                    self.games[ref.id]['asset_work']=stats
                artwork_updates.add(ref)
            elif kind == 'gpu_inventory':
                self.gpu_inventory.set(value)
            elif kind == 'imported':
                self._handle_imported_event(value)
            elif kind == 'deleted':
                self._handle_deleted_event(value)
            elif kind == 'delete_error':
                self._handle_delete_error_event(value)
            elif kind == 'comparison_started':
                self.comparison_started(value)
            elif kind == 'comparison_failed':
                self.comparison_failed(value)
            elif kind == 'idle':
                self.busy = False
                self.busy_reason = ''
                self.progress.stop()
                self.refresh()
            elif kind == 'poses_regenerated':
                self.playable_artwork.regenerated(*value)
            elif kind == 'poses_imported':
                self._finish_pose_import(*value)
            elif kind == 'error':
                if isinstance(value,ApplicationEvent):
                    label=SYSTEM_LABELS[value.system]
                    self.status.set(f'{label} preparation failed. Your original file has not been changed.')
                    messagebox.showerror(f'{label} preparation failed',value.detail)
                else:
                    self.status.set('Preparation failed. Your original file has not been changed.')
                    messagebox.showerror('Preparation failed', value)
            elif kind == 'scoped_error':
                label=SYSTEM_LABELS[value.system]
                self.status.set(f'{label} operation failed. Your original file was not changed.')
                messagebox.showerror(f'{label} operation failed',value.detail)
            elif kind == 'scoped_status':
                self.status.set(f"{SYSTEM_LABELS[value.system]}: {value.detail}")
            elif kind == 'refresh':
                self.refresh()
        game=self.game_if_selected()
        if game is not None and (ref:=GameRef.from_metadata(game)) in artwork_updates:
            if ref.system==NES:
                self._show_nes_artwork(game)
            else:
                self._show_java_artwork(game)
        self._poll_java_preview()
        if time.monotonic()-self._last_metrics>1:
            self._last_metrics=time.monotonic()
            self.update_telemetry()
        for key, (process, log) in list(self.players.items()):
            result = process.poll()
            if result is not None:
                session = self.players[key]
                del self.players[key]
                legacy_ref = getattr(self, 'player_refs', {}).pop(key, None)
                ref = session.ref if isinstance(session, PlaybackSession) else legacy_ref
                self._pending_player_states.discard(ref)
                try:self.library.set_player_state(ref or key,False)
                except KeyError:pass
                if (ref and ref.system == self.visible_system_id
                        and (selected := self.game_if_selected()) and selected['id'] == key):
                    self.playable_artwork.refresh(selected)
                legacy_recording = getattr(self, '_recording_players', {}).pop(key, False)
                if (session.recording if isinstance(session, PlaybackSession) else legacy_recording):
                    self._start_pose_import(key)
                if result:
                    reason = ('The GPU player could not continue. Try CPU · compatibility in the Compatibility tab, or check your graphics driver.'
                              if result == 71 and ref is not None and ref.system==JAVA_ME else
                              'The NES player could not continue; check its saved runtime log and setup.'
                              if result == 71 and ref is not None and ref.system==NES else
                              'The game could not continue. Its diagnostic log may show a compatibility error.'
                              if result == 72 and ref is not None and ref.system==JAVA_ME else
                              f'The player exited with code {result}.')
                    messagebox.showerror('Player stopped', f'{reason}\n\nDetails: {log}')
        if not getattr(self, '_closing', False):
            self.start_pending()
        self.row_actions.set_enabled(not self.busy and not self.pending_imports)
        self.root.after(120, self.poll)

    def _start_pose_import(self, key):
        if not hasattr(self, '_pose_importing'):
            self._pose_importing = set()
        self._pose_importing.add(key)
        self.status.set('Finding player poses in the recorded gameplay.')

        def worker():
            try:
                count = self.nes_library.import_playable_recording(key)
            except (OSError, ValueError, KeyError, RuntimeError, TypeError) as error:
                self.messages.put(('poses_imported', (key, 0, str(error))))
            else:
                self.messages.put(('poses_imported', (key, count, None)))

        threading.Thread(target=worker, name='nes-pose-import', daemon=True).start()

    def _finish_pose_import(self, key, count, error):
        self._pose_importing.discard(key)
        if error:
            self.status.set(f'Could not import recorded player poses. {error}')
        elif count:
            self.status.set(f'Added {count} player poses from gameplay.')
        else:
            self.status.set('No new player poses. Move back and forth several times, then close the game.')
        selected = self.game_if_selected()
        if self.visible_system_id == NES and selected and selected['id'] == key:
            self.playable_artwork.refresh(selected)

    def _handle_imported_event(self,event):
        ref=event.ref
        if ref.system==self.visible_system_id:
            self.search.set('')
            self.refresh()
            if self.tree.exists(ref.id):
                self.tree.selection_set(ref.id)
                self.tree.see(ref.id)
            self.status.set(f"{event.detail} is ready in the {SYSTEM_LABELS[ref.system]} library.")
        else:
            self.status.set(f"{event.detail} was added to the {SYSTEM_LABELS[ref.system]} library. The open screen was left unchanged.")

    def _handle_deleted_event(self,event):
        ref=event.ref
        self._catalog=[game for game in self._catalog if GameRef.from_metadata(game)!=ref]
        self.deleting_games.pop(ref.id,None)
        self._asset_stats.pop(ref,None)
        self.players.pop(ref.id,None)
        getattr(self, 'player_refs', {}).pop(ref.id,None)
        if ref.system==self.visible_system_id:
            self._show_catalog()
        self.status.set(f"{event.detail} deleted from the {SYSTEM_LABELS[ref.system]} library. Saved progress kept."+
            (f' {len(self.deleting_games)} deletion(s) pending.' if self.deleting_games else ''))

    def _handle_delete_error_event(self,event):
        ref=event.ref
        self.deleting_games.pop(ref.id,None)
        if ref.system==self.visible_system_id:
            self._show_catalog()
        title=event.detail.splitlines()[0]
        self.status.set(f"Could not finish deleting {title}. Saved progress is kept.")
        messagebox.showerror('Could not delete game',event.detail,parent=self.root)
