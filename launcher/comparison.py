"""Own one comparison window and route the same controls to both sessions."""

from queue import Empty, Full, Queue
import threading
import time
import tkinter as tk
from tkinter import messagebox, ttk

from .comparison_native import ComparisonNativeWindows


_KEYS = {
    "up": "ARROW_UP", "down": "ARROW_DOWN", "left": "ARROW_LEFT", "right": "ARROW_RIGHT",
    "return": "ENTER", "kp_enter": "NUMPAD_ENTER", "space": "SPACE",
}
_KEYPAD_NAVIGATION = dict(zip(
    ("insert", "end", "down", "next", "left", "begin", "right", "home", "up", "prior"),
    "0123456789",
))


def comparison_key(keysym):
    key = keysym.lower()
    if key in _KEYS:
        return _KEYS[key]
    if len(key) == 1:
        if "a" <= key <= "z":
            return f"KEY_{key.upper()}"
        if "0" <= key <= "9":
            return f"DIGIT_{key}"
    if key.startswith("kp_"):
        suffix = key[3:]
        digit = suffix if len(suffix) == 1 and "0" <= suffix <= "9" else _KEYPAD_NAVIGATION.get(suffix)
        if digit is not None:
            return f"NUMPAD_{digit}"
    return None


class ComparisonWindow:
    def __init__(self, root, pair, title, *, embed_players=True):
        self.native = ComparisonNativeWindows()
        self.pair = pair
        self.parent = root
        # External players keep their own top-level windows, tiled side
        # by side; players owned by the tool (Java ME) embed into our panes.
        self.embed_players = bool(embed_players)
        self.windows = {}
        self.attached = False
        self.input_ready = False
        self.closing = False
        self.input_error = None
        self.held = set()
        self.events = Queue(maxsize=128)
        self.stop_input = threading.Event()
        self.deadline = time.monotonic() + 35
        self.enforce_counter = 0
        self.enqueued_count = 0
        self._log_path = self._comparison_log_path(pair)
        mode = "embedded" if self.embed_players else "tiled"
        self._log(f"comparison-window v4 {mode} watching "
                  f"original={pair.players[0].pid} enhanced={pair.players[-1].pid}")

        self.window = tk.Toplevel(root)
        self.window.title(f"{title} · Original / Enhanced")
        if self.embed_players:
            width = min(1400, self.window.winfo_screenwidth() * 9 // 10)
            height = min(820, self.window.winfo_screenheight() * 4 // 5)
            self.window.geometry(f"{width}x{height}")
            self.window.minsize(540, 360)
        self.window.protocol("WM_DELETE_WINDOW", self.close)
        self.window.bind("<Destroy>", self._destroyed)
        if self.embed_players:
            hint = "Starting both players…"
        else:
            hint = ("Starting both players…\n"
                    "Click this window, then use the keys; both games follow.")
        self.hint = tk.StringVar(master=self.window, value=hint)
        ttk.Label(self.window, textvariable=self.hint, padding=(10, 6),
                  wraplength=460, justify="left").pack(anchor="w")
        self.board = tk.Frame(self.window, bg="black", takefocus=True)
        self.board.pack(fill="both", expand=True)
        self.panes = []
        self.side_status = []
        if self.embed_players:
            self.board.rowconfigure(1, weight=1)
            for index, label in enumerate(("Original", "Enhanced")):
                self.board.columnconfigure(index, weight=1, uniform="comparison")
                ttk.Label(self.board, text=label, padding=(10, 5), anchor="center").grid(
                    row=0, column=index, sticky="ew")
                pane = tk.Frame(self.board, bg="black")
                pane.grid(row=1, column=index, sticky="nsew", padx=(0 if index == 0 else 1, 0))
                pane.bind("<Configure>", lambda event, i=index: self._resize(i, event.width, event.height))
                self.panes.append(pane)
        else:
            for label in ("Original", "Enhanced"):
                status = tk.StringVar(master=self.window, value=f"{label}: starting…")
                self.side_status.append(status)
                ttk.Label(self.board, textvariable=status, padding=(10, 4),
                          anchor="w").pack(fill="x")
        self.window.bind("<KeyPress>", self._press)
        self.window.bind("<KeyRelease>", self._release)
        self.tick_job = self.window.after(35, self._tick)
        threading.Thread(target=self._write_inputs, daemon=True, name="comparison-input").start()

    @staticmethod
    def _comparison_log_path(pair):
        try:
            ready = getattr(pair, "ready_paths", ())
            if ready:
                from pathlib import Path
                return str(Path(ready[0]).parent / "comparison-window.log")
        except (OSError, ValueError, TypeError):
            pass
        return None

    def _log(self, message):
        path = getattr(self, "_log_path", None)
        if not path:
            return
        try:
            with open(path, "a", encoding="utf-8") as stream:
                stream.write(f"{time.monotonic():.2f} {message}\n")
        except OSError:
            pass

    def _resize(self, index, width, height):
        if not getattr(self, "embed_players", True):
            return
        if self.attached and not self.closing:
            try:
                self.native.fit(self.windows[self.pair.players[index].pid], width, height)
            except OSError as error:
                self._fail(str(error))

    def _write_inputs(self):
        while not self.stop_input.is_set():
            try:
                token, pressed = self.events.get(timeout=0.1)
            except Empty:
                continue
            try:
                self.pair.send_key(token, pressed)
            except (OSError, ValueError) as error:
                self.input_error = str(error)
                return

    def _enqueue(self, token, pressed):
        try:
            self.events.put_nowait((token, pressed))
        except Full:
            self.input_error = "A comparison player stopped reading keyboard input."
        else:
            self.enqueued_count = getattr(self, "enqueued_count", 0) + 1

    def _press(self, event):
        if event.keysym == "Escape":
            self.close()
            return "break"
        token = comparison_key(event.keysym)
        if self.input_ready and not self.closing and token and token not in self.held:
            self.held.add(token)
            self._enqueue(token, True)
        return "break"

    def _release(self, event):
        token = comparison_key(event.keysym)
        if token in self.held:
            self.held.remove(token)
            self._enqueue(token, False)
        return "break"

    def _release_all(self):
        for token in self.held:
            self._enqueue(token, False)
        self.held.clear()

    def _tick(self):
        self.tick_job = None
        alive = [player for player in self.pair.players if player.poll() is None]
        if not alive:
            self.window.destroy()
            return
        if self.closing:
            if time.monotonic() >= self.deadline:
                for player in alive:
                    try:
                        player.terminate()
                    except OSError:
                        pass
            self.tick_job = self.window.after(75, self._tick)
            return
        if self.pair.startup_error or self.input_error:
            self._fail(self.pair.startup_error or self.input_error)
            return
        if len(alive) != 2:
            self.close(expected=False)
            return
        if not self.input_ready:
            try:
                if not self.attached:
                    self._attach(alive)
                if self.attached and all(path.is_file() for path in self.pair.ready_paths):
                    self.input_ready = True
                    self._log("input ready; shared controls active")
                    if getattr(self, "embed_players", True):
                        self.hint.set("Shared controls · Esc closes both players")
                    else:
                        self.hint.set("Shared controls · click here, then use the keys · Esc closes both players")
                    self.board.focus_set()
                elif time.monotonic() >= self.deadline:
                    self._fail("Both comparison players did not become ready. Check the player logs.")
                    return
            except OSError as error:
                self._fail(str(error))
                return
        if self.native.is_foreground(self.window.winfo_id()):
            if not self.native.has_focus(self.board.winfo_id()):
                self.board.focus_force()
        else:
            self._release_all()
        if not self.embed_players:
            self._refresh_side_status(alive)
        self.enforce_counter += 1
        if self.attached and self.enforce_counter % 15 == 0:
            try:
                self._enforce(alive)
            except OSError as error:
                self._fail(str(error))
                return
        self.tick_job = self.window.after(35, self._tick)

    def _refresh_side_status(self, alive):
        running = {player.pid for player in alive}
        for index, player in enumerate(self.pair.players):
            label = "Original" if index == 0 else "Enhanced"
            if player.pid not in running:
                state = "closed"
            elif self.input_ready:
                state = "running · following shared keys"
            elif player.pid in self.windows:
                state = "open · waiting for the other player"
            else:
                state = "starting…"
            self.side_status[index].set(f"{label}: {state}")

    def _attach(self, players):
        self.windows = self.native.find_players({player.pid for player in players})
        if len(self.windows) != 2:
            return
        if not getattr(self, "embed_players", True):
            first = self.windows[players[0].pid]
            second = self.windows[players[1].pid]
            left, right = self.native.tile_pair(first, second)
            self._log(f"tile original={first} {left} enhanced={second} {right}")
            self.attached = True
            return
        for player, pane in zip(players, self.panes):
            window = self.windows[player.pid]
            self.native.embed(window, pane.winfo_id())
            self.native.fit(window, pane.winfo_width(), pane.winfo_height(), show=True)
            self._log(f"attach pid={player.pid} window={window} "
                      f"style={self.native.style(window):#x} parent={self.native.parent_of(window)}")
        self.attached = True

    def _enforce(self, players):
        """Keep embedded panes borderless; restore tiled windows if minimized."""
        found = self.native.find_players({player.pid for player in players})
        if len(found) != len(players):
            return
        if not getattr(self, "embed_players", True):
            left, right = self.native.halves()
            for player, rect in zip(self.pair.players, (left, right)):
                if player.poll() is not None:
                    continue
                window = found[player.pid]
                if self.windows.get(player.pid) != window:
                    self.native.place(window, rect)
                    self.windows[player.pid] = window
                    self._log(f"re-tile pid={player.pid} window={window} {rect}")
                elif self.native.minimized(window):
                    self.native.place(window, rect)
                    self._log(f"restore pid={player.pid} window={window}")
            return
        for player, pane in zip(self.pair.players, self.panes):
            if player.poll() is not None:
                continue
            window = found[player.pid]
            if self.windows.get(player.pid) != window:
                self.native.embed(window, pane.winfo_id())
                self.windows[player.pid] = window
                self._log(f"re-embed pid={player.pid} window={window}")
            actions = self.native.ensure(window, pane.winfo_id(), pane.winfo_width(), pane.winfo_height())
            if actions:
                self._log(f"ensure pid={player.pid} window={window} {'+'.join(actions)}")

    def _fail(self, error):
        self._log(f"failed: {error}")
        self.close()
        messagebox.showerror("Comparison could not run", error, parent=self.parent)

    def close(self, *, expected=True):
        if self.closing:
            return
        self.closing = True
        if expected:
            self.pair.expected_exit = True
        self._log(f"closing; keys sent={getattr(self, 'enqueued_count', 0)}")
        self.deadline = time.monotonic() + 8
        self._release_all()
        self.hint.set("Closing both players…")
        alive = {player.pid for player in self.pair.players if player.poll() is None}
        for handle in (self.native.find_players(alive) | self.windows).values():
            self.native.close(handle)
        if self.tick_job is None:
            self.tick_job = self.window.after(75, self._tick)

    def _destroyed(self, event):
        if event.widget is self.window:
            self.stop_input.set()
            if self.tick_job is not None:
                self.window.after_cancel(self.tick_job)
