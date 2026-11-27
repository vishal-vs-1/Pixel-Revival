"""Compose and start the Pixel Revival desktop launcher."""

import queue
import threading
from collections import deque
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import tkinter as tk
from tkinter import messagebox
from systems.jme.library import Library, ROOT
from storage.atomic import atomic_json
from storage.paths import APP_ROOT, DATA_ROOT, RESOURCE_ROOT
from graphics.gpu_policy import devices as gpu_devices
from systems.nes.controls import NesControlProfileStore
from systems.application import ApplicationLibrary
from systems.comparison import ComparisonPlayers
from systems.jme.system import JavaMeSystem
from systems.nes.library import NesLibrary
from systems.nes.integration import NesCaptureIntegration
from systems.nes.playable_assets import PlayableAssets
from systems.nes.metadata import DEFAULT_RENDER_SCALE
from systems.app_settings import ApplicationSettingsStore
from runtime_assets.recipes import ArtworkRecipes
from launcher.styles import LauncherStyles
from launcher.catalog import LauncherCatalog
from launcher.settings import LauncherSettings
from launcher.operations import LauncherOperations
from launcher.playback import LauncherPlayback
from launcher.events import LauncherEvents
from launcher.layout import build_window


class App(LauncherStyles, LauncherCatalog, LauncherSettings, LauncherOperations, LauncherPlayback, LauncherEvents):
    def __init__(self, root, *, recover_previous_artwork=False):
        self.root = root
        self.nes_control_profiles = NesControlProfileStore(
            DATA_ROOT / 'config' / 'nes-control-profiles.json', atomic_json)
        # NES loads our own Rust library directly through its C ABI. A missing
        # build keeps the library usable; playback stays disabled until built.
        try:
            from systems.nes.rust_player import RustNesRuntime
            rust_runtime = RustNesRuntime(app_root=RESOURCE_ROOT)
        except (OSError, RuntimeError):
            rust_runtime = None
        self.nes_runtime = rust_runtime
        artwork_recipes = ArtworkRecipes()
        java_library = Library(artwork_recipes=artwork_recipes,
                               recover_previous_artwork=recover_previous_artwork)
        java = JavaMeSystem(java_library, demo=ROOT / 'fixtures' / 'RevivalDemo.jar')

        def register_nes_artwork(folder, game, recovering=False):
            recipe, ai_recipe = artwork_recipes.for_backend(game["backend"])
            integration = NesCaptureIntegration(
                game["id"], recipe,
                expected_palette_profile=game.get("palette_profile"),
                pack_scale=game.get("render_scale", DEFAULT_RENDER_SCALE),
            )
            return java_library.asset_service.register_game(
                folder, recipe, game=game, ai_recipe=ai_recipe,
                integration=integration, defer_maintenance_during_play=True,
            )

        # Keep the JME paths intact and give NES its independently owned storage tree.
        nes = NesLibrary(DATA_ROOT / 'nes', asset_service=java_library.asset_service,
                         control_profiles=self.nes_control_profiles, runtime=rust_runtime,
                         register_artwork=register_nes_artwork,
                         playable_assets=PlayableAssets(APP_ROOT))
        self.nes_library = nes
        self.library = ApplicationLibrary((java, nes), java_library.asset_service)
        self.app_settings = ApplicationSettingsStore(
            DATA_ROOT / 'config' / 'application.json', atomic_json)
        self.visible_system_id = self.app_settings.load_visible_system()
        self.messages = queue.Queue()
        self._asset_stats = {}
        self._catalog = []
        self._catalog_filter_pending = None
        self._preview_worker = ThreadPoolExecutor(max_workers=1, thread_name_prefix='library-preview')
        self._preview_request = None
        self._preview_future = None
        self._preview_count = None
        self.library.set_asset_listener(self._on_asset_stats)
        self.busy = False
        self.busy_reason = ''
        self.deleting_games = {}
        self.player_refs = {}
        self._recording_players = {}
        self._pending_player_states = set()
        self._delete_workers = ThreadPoolExecutor(max_workers=2,thread_name_prefix='delete-game')
        self.pending_imports = deque()
        self.preparation_paused = False
        self.players: dict[str, tuple[object, Path]] = {}
        self.settings_dialog = None
        self.control_profile_choice_ids = {}
        self.control_profile_problem = None
        build_window(self, root, java_library)
        self._last_metrics=0
        self._preview_key=None
        self._show_system_details()
        self._refresh_system_presentation()
        self.refresh()
        root.protocol('WM_DELETE_WINDOW', self.close)
        root.after(120, self.poll)
        root.after(1500, self.capture_tick)
        threading.Thread(target=self.detect_gpus,daemon=True).start()
        threading.Thread(target=self.bootstrap_asset_service,daemon=True).start()

    def close(self):
        comparisons = [process.view for process, _log in self.players.values()
                       if isinstance(process, ComparisonPlayers) and hasattr(process, 'view')
                       and process.poll() is None]
        if comparisons:
            for view in comparisons:
                view.close()
            self.root.after(100, self.close)
            return
        for process, _log in list(self.players.values()):
            view = getattr(process, 'view', None)
            if view is None:
                continue
            close = getattr(view, 'shutdown', None) or getattr(view, 'close', None)
            if callable(close):
                try:
                    close()
                except Exception:
                    pass
        shutdown_nes = getattr(self.nes_runtime, 'shutdown', None)
        if callable(shutdown_nes):
            try:
                shutdown_nes()
            except Exception as error:
                messagebox.showerror('NES player is still closing',
                                     f'The NES player could not finish saving and close safely.\n\n{error}')
                return
        self._delete_workers.shutdown(wait=False)
        self._preview_worker.shutdown(wait=False, cancel_futures=True)
        self.library.close(timeout=1.0)
        self.root.destroy()

    def detect_gpus(self):
        try:
            found=gpu_devices()
            text='\n'.join(f"{device['kind'].capitalize()}: {device['name']}" for device in found) or 'No Vulkan devices detected. Playback depends on the Windows graphics driver.'
            self.messages.put(('gpu_inventory',text))
        except Exception as error:self.messages.put(('gpu_inventory',f'GPU detection unavailable: {error}'))


def main():
    # A second launcher could otherwise keep converting on the same GPU during play.
    import msvcrt
    lock_path=DATA_ROOT/'library/.launcher.lock'
    lock_path.parent.mkdir(parents=True,exist_ok=True)
    with lock_path.open('a+b') as lock:
        lock.seek(0,2)
        if not lock.tell():lock.write(b'1');lock.flush()
        lock.seek(0)
        try:msvcrt.locking(lock.fileno(),msvcrt.LK_NBLCK,1)
        except OSError:
            notice=tk.Tk();notice.withdraw()
            messagebox.showinfo('JAR Revival is already open','Use the existing JAR Revival window. Only one library runs at a time to avoid competing conversion jobs.')
            notice.destroy();return
        root=tk.Tk()
        # The launcher lock remains held throughout recovery and the UI lifetime.
        App(root, recover_previous_artwork=True)
        root.mainloop()


if __name__ == '__main__':
    from multiprocessing import freeze_support
    freeze_support()
    main()
