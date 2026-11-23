"""Side-by-side Original/Enhanced NES comparison inside our own window.

Both panes are real `RustNesPlayer` instances embedded in one Tk window. Each
pane gets a private copy of the cartridge, saves and (for Enhanced) the
prepared pack.
"""

from __future__ import annotations

import threading
import time
import tkinter as tk
from pathlib import Path
from tkinter import ttk
from uuid import uuid4

from storage.atomic import atomic_json
from systems.comparison import copy_playback_files

from .metadata import DEFAULT_RENDER_SCALE
from .pack import PRIORITY_SAFE_PACK
from .rust_hd import owned_path, read_pack_snapshot
from .rust_player import _TK_TO_ACTION, _tk_keysym, RustNesPlayer


def _run_on_tk(root, callback, timeout=30.0):
    """Run widget work on the Tk thread; comparison launches start in a worker."""
    if threading.current_thread() is threading.main_thread():
        return callback()
    done = threading.Event()
    result = {}

    def call():
        try:
            result["value"] = callback()
        except BaseException as error:  # surfaced on the launching thread
            result["error"] = error
        finally:
            done.set()

    root.after(0, call)
    if not done.wait(timeout):
        raise RuntimeError("Timed out opening the comparison window")
    if "error" in result:
        raise result["error"]
    return result.get("value")


def _copy_pack(workspace: Path, target: Path):
    pack, scale = read_pack_snapshot(workspace)
    relative = pack.relative_to(workspace)
    copy_playback_files(pack.parent, target / relative.parent)
    atomic_json(target / ".runtime" / "nes-pack.json", {
        "schema": 1, "generation": pack.parent.name,
        "manifest": relative.as_posix(), "pack_scale": scale,
        "background_priority": PRIORITY_SAFE_PACK,
    })


class RustComparisonPlayers:
    """Own the comparison window and its two in-tool players."""

    owns_window = True

    def __init__(self, root, title):
        self.parent = root
        self.players = []
        self.logs = []
        self.ready_paths = ()
        self.expected_exit = False
        self.startup_error = None
        self.held = set()
        self.window = tk.Toplevel(root)
        self.window.title(f"{title} · Original / Enhanced")
        self.window.geometry("1280x760")
        self.window.minsize(640, 380)
        self.window.configure(bg="#0c121c")
        self.window.protocol("WM_DELETE_WINDOW", self.close)
        self.window.bind("<KeyPress>", self._press)
        self.window.bind("<KeyRelease>", self._release)
        self.window.bind("<FocusOut>", lambda _event: self.release_all())
        self.window.bind("<F11>", lambda _event: self.toggle_fullscreen())
        try:
            self.window.state("zoomed")
        except tk.TclError:
            pass
        board = tk.Frame(self.window, bg="#0c121c")
        board.pack(fill="both", expand=True)
        board.rowconfigure(1, weight=1)
        board.columnconfigure(0, weight=1, uniform="pane")
        board.columnconfigure(1, weight=1, uniform="pane")
        self.panes = []
        for index, label in enumerate(("Original", "Enhanced")):
            ttk.Label(board, text=label, anchor="center").grid(
                row=0, column=index, sticky="ew")
            pane = tk.Frame(board, bg="black")
            pane.grid(row=1, column=index, sticky="nsew",
                      padx=(0, 1) if index == 0 else (1, 0))
            self.panes.append(pane)
        self.hint = tk.StringVar(master=self.window, value=(
            "Shared controls · Esc closes both players"))
        ttk.Label(self.window, textvariable=self.hint, padding=(10, 4),
                  anchor="w").pack(fill="x")
        self.board = board
        self.view = self
        self._watch_job = self.window.after(250, self._watch)

    def _watch(self):
        self._watch_job = None
        if self.expected_exit:
            return
        if self.players and all(player.poll() is not None for player in self.players):
            self.close()
            return
        try:
            self._watch_job = self.window.after(250, self._watch)
        except tk.TclError:
            self._watch_job = None

    def add(self, index, player, log):
        self.players.append(player)
        self.logs.append(log)

    def start(self):
        for player in self.players:
            player.start()
        self.board.focus_set()

    def poll(self):
        codes = [player.poll() for player in self.players]
        if any(code is None for code in codes):
            return None
        if self.expected_exit:
            return 0
        return next((code for code in codes if code != 0), 0)

    def _action_for(self, keysym):
        """Prefer each pane's selected NES profile over the default map."""
        for player in self.players:
            action = getattr(player, "_key_to_action", {}).get(keysym)
            if action is not None:
                return action
        return _TK_TO_ACTION.get(keysym)

    def send_key(self, token, pressed):
        keysym = _tk_keysym(token)
        action = self._action_for(keysym) if keysym else _COMPARISON_TOKENS.get(token)
        for player in self.players:
            if pressed:
                player.press(action)
            else:
                player.release(action)

    def release_all(self):
        for player in self.players:
            player.release_all()
        self.held.clear()

    def _press(self, event):
        if event.keysym == "Escape":
            self.close()
            return "break"
        action = self._action_for(event.keysym.lower())
        if action is None or action in self.held:
            return "break"
        self.held.add(action)
        for player in self.players:
            player.press(action)
        return "break"

    def toggle_fullscreen(self):
        self._fullscreen = not getattr(self, "_fullscreen", False)
        try:
            self.window.attributes("-fullscreen", self._fullscreen)
        except tk.TclError:
            self._fullscreen = False

    def _release(self, event):
        action = self._action_for(event.keysym.lower())
        if action in self.held:
            self.held.discard(action)
            for player in self.players:
                player.release(action)
        return "break"

    def close(self):
        if self.expected_exit:
            return
        self.expected_exit = True
        if self._watch_job is not None:
            try:
                self.window.after_cancel(self._watch_job)
            except tk.TclError:
                pass
            self._watch_job = None
        self.release_all()
        for player in self.players:
            try:
                player.close()
            except (OSError, RuntimeError):
                pass
        try:
            self.window.destroy()
        except tk.TclError:
            pass

    def shutdown(self, timeout=8.0):
        self.close()
        deadline = time.monotonic() + timeout
        while any(player.poll() is None for player in self.players):
            if time.monotonic() >= deadline:
                break
            time.sleep(0.05)

    terminate = close


# Shared comparison key tokens as a convenience seam for callers.
_COMPARISON_TOKENS = {
    "ENTER": "START", "SPACE": "SELECT", "ARROW_UP": "UP",
    "ARROW_DOWN": "DOWN", "ARROW_LEFT": "LEFT", "ARROW_RIGHT": "RIGHT",
    "KEY_X": "A", "KEY_Z": "B",
}


def launch_comparison(runtime, game, source, workspace, *, host_parent=None,
                      controls=None, **options):
    """Prepare isolated runs and open both in-tool players side by side."""
    if host_parent is None:
        raise RuntimeError("In-tool comparison needs the application window")
    workspace = Path(workspace)
    identity = game["id"]
    pack, scale = read_pack_snapshot(workspace)
    requested = int(game.get("render_scale", DEFAULT_RENDER_SCALE))
    if scale != requested:
        raise ValueError("Wait for the enhanced artwork pack at the selected display scale.")
    # Keep nested comparison paths short on Windows.
    data_root = workspace.parent.parent
    comparison = owned_path(data_root / ".compare" / uuid4().hex[:12],
                            data_root, directory=True)
    comparison.mkdir(parents=True)
    launches = {}
    for original, label in ((True, "original"), (False, "enhanced")):
        root = comparison / label
        rom = root / "library" / identity / "game.nes"
        copy_playback_files(source, rom)
        target = root / "rendered-games" / identity
        copy_playback_files(workspace / "saves" / "mesen",
                            target / "saves" / "mesen")
        if not original:
            _copy_pack(workspace, target)
        launches[original] = rom, target

    def build():
        pair = RustComparisonPlayers(host_parent, game.get("name", "NES"))
        try:
            for index, original in enumerate((True, False)):
                rom, target = launches[original]
                player = RustNesPlayer(
                    runtime._core, game, rom, target, original=original,
                    controls=controls, embedded_parent=pair.panes[index])
                # Both panes must render at the same physical size so tiles and
                # HUD line up; exact fit overrides integer scaling.
                player._fit_exact = True
                pair.add(index, player, player.log_path)
            pair.start()
        except BaseException:
            pair.close()
            raise
        return pair

    pair = _run_on_tk(host_parent, build)
    return pair, pair.logs[-1]
