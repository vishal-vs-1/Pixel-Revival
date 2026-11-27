"""Queue imports, deletion, and artwork preparation."""

import os
import threading
from tkinter import filedialog, messagebox
from systems.contracts import GameRef, ImportRequest, NES
from systems.ui_events import ApplicationEvent
from launcher.labels import SYSTEM_LABELS, ARTWORK_BACKENDS


class LauncherOperations:
    def delete_game(self,game=None):
        if self.busy or self.pending_imports:
            self.status.set('Finish the current library operation before deleting a game.')
            return
        if game is None:
            game=self.game()
        if game is None:return
        ref=GameRef.from_metadata(game)
        game_name=game['name']
        if ref.id in self.deleting_games:return
        old=self.players.get(ref.id)
        if old and old[0].poll() is None:
            messagebox.showinfo('Game is running','Close this game before deleting it so its saves can finish writing.')
            return
        if not messagebox.askyesno('Delete game?',
                f"Delete {game['name']} from your {SYSTEM_LABELS[ref.system]} collection?\n\n"
                f"Removes the imported {'cartridge' if ref.system==NES else 'JAR'}, artwork, caches and logs.\n"
                'Keeps all saved progress. Import the same game again to reuse it.',parent=self.root):return
        self.deleting_games[ref.id]=game
        self._show_catalog()
        self._update_row_actions()
        self.status.set(f'Deleting {len(self.deleting_games)} game(s) in background. Saved progress is kept.')
        def worker():
            try:
                self.library.delete_game(ref)
                self.messages.put(('deleted',ApplicationEvent('deleted',ref.system,ref.id,game_name)))
            except Exception as error:
                self.messages.put(('delete_error',ApplicationEvent('delete_error',ref.system,ref.id,
                                                                     f'{game_name}\n{error}')))
        self._delete_workers.submit(worker)

    def toggle_preparation(self):
        self.preparation_paused=not self.preparation_paused
        self.pause_button.configure(text='Resume background' if self.preparation_paused else 'Pause background')
        if self.preparation_paused:
            self.library.pause_all()
        else:
            self.library.resume_all()
        self.preparation_info.set('Preparation paused after current images finish.' if self.preparation_paused else 'Preparation resumed.')

    def import_demo(self):
        presentation = self.library.presentation(self.visible_system_id)
        demo = presentation.demo
        if demo is not None and demo.exists():
            self.import_game(demo)
        elif self.visible_system_id==NES:
            messagebox.showinfo('No NES demo available','No synthetic NES demo is included yet. Choose your own .nes cartridge.')
        else:
            messagebox.showerror('Demo not built', 'Run python tools/build.py to build the player and demo.')

    def import_game(self, path=None):
        if not path:
            presentation = self.library.presentation(self.visible_system_id)
            path = filedialog.askopenfilename(title=presentation.picker_title, filetypes=presentation.filetypes)
        if not path:
            return
        request=ImportRequest(self.visible_system_id, path)
        self.pending_imports.append(request)
        self.status.set(f'Queued {os.path.basename(path)}. You can add more games while preparation finishes.')
        self.start_pending()

    def start_pending(self):
        if self.busy:return
        if not self.pending_imports or any(p.poll() is None for p,_ in self.players.values()):return
        request=self.pending_imports.popleft()
        self.busy=True
        self.busy_reason = f'importing {request.path.name}'
        self.status.set(f'Preparing {SYSTEM_LABELS[request.system]} · {request.path.name} · {len(self.pending_imports)} more games queued')
        self.progress.start()
        def worker():
            try:
                game = self.library.import_game(request, lambda msg: self.messages.put((
                    'scoped_status',ApplicationEvent('status',request.system,detail=msg))))
                ref=GameRef.from_metadata(game)
                self.messages.put(('imported',ApplicationEvent('imported',ref.system,ref.id,game['name'])))
            except Exception as error:
                self.messages.put(('error',ApplicationEvent('error',request.system,detail=str(error))))
            finally:
                self.messages.put(('idle',ApplicationEvent('idle',request.system)))
        threading.Thread(target=worker, daemon=True).start()

    def install_ai(self):
        if self.busy: return
        self.busy=True
        self.busy_reason = 'installing the AI backend'
        self.progress.start()
        def worker():
            try:
                from tools.setup_ai import install
                install(lambda msg:self.messages.put(('status',msg)))
            except Exception as error:
                self.messages.put(('error',str(error)))
            finally:
                self.messages.put(('idle',None))
        threading.Thread(target=worker,daemon=True).start()

    def prepare_style(self):
        if self.busy: return
        game=self.game()
        if not game: return
        old=self.players.get(game['id'])
        if old and old[0].poll() is None:
            messagebox.showinfo('Game is running','Close the player before changing its artwork style.')
            return
        backend=ARTWORK_BACKENDS.get(self.backend.get(), 'pixel')
        ref=GameRef.from_metadata(game)
        self.busy=True; self.progress.start()
        self.busy_reason = f'preparing {game.get("name", "artwork")} artwork style'
        def worker():
            try:
                game_ready=self.library.prepare_style(game,backend,lambda msg:self.messages.put((
                    'scoped_status',ApplicationEvent('status',ref.system,ref.id,msg))))
                self.messages.put(('imported',ApplicationEvent('imported',ref.system,ref.id,game_ready['name'])))
            except Exception as error:
                self.messages.put(('scoped_error',ApplicationEvent('error',ref.system,ref.id,str(error))))
            finally:
                self.messages.put(('idle',ApplicationEvent('idle',ref.system,ref.id)))
        threading.Thread(target=worker,daemon=True).start()

    def sync_artwork(self):
        if self.busy or self.pending_imports:
            self.status.set('Finish the current library operation before syncing artwork.')
            return
        game = self.game()
        if not game:
            return
        game = dict(game)
        if any(player.poll() is None for player, _ in self.players.values()):
            self.status.set('Close the game players before syncing artwork.')
            return
        ref = GameRef.from_metadata(game)
        if ref.id in self.deleting_games or ref.id in getattr(self, '_pose_importing', ()):
            self.status.set('Wait for this game to finish updating before syncing artwork.')
            return
        self.busy = True
        self.busy_reason = f'syncing {game["name"]} artwork'
        self.status.set(f'Syncing artwork for {game["name"]}.')
        self.progress.start()

        def worker():
            try:
                count = self.library.sync_artwork(game)
                detail = f'Sync complete for {game["name"]}. {count} new artwork items queued.'
                self.messages.put(('scoped_status', ApplicationEvent(
                    'status', ref.system, ref.id, detail)))
            except Exception as error:
                self.messages.put(('scoped_error', ApplicationEvent(
                    'error', ref.system, ref.id, str(error))))
            finally:
                self.messages.put(('idle', ApplicationEvent('idle', ref.system, ref.id)))

        threading.Thread(target=worker, name='artwork-sync', daemon=True).start()

    def open_folder(self):
        game = self.game()
        if game:
            os.startfile(self.library.workspace(GameRef.from_metadata(game)))
