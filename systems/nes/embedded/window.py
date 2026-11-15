"""Host one native Mesen session in a Tk window and handle its controls."""
from __future__ import annotations

import json
import os
import uuid
from tkinter import ttk
import tkinter as tk
from .keys import _key_event_name


class NesInProcessPlayer:
    """App-owned window, controls and close signal for one native Mesen session."""

    def __init__(self, runtime, session, *, parent, original: bool, capture: bool,
                 on_finished):
        self.runtime = runtime
        self.session = session
        self.original = original
        self.capture = capture
        self.on_finished = on_finished
        self.log = session.home.parent / "runtime.log"
        self._state = "starting"
        self._exit_code = None
        self._fallback_process = None
        self._fallback_log = None
        self._launch_spec = None
        self._start_future: Future | None = None
        self._stop_future: Future | None = None
        self._key_codes = {}
        self._event_actions = {
            _key_event_name(token): action for action, token in session.controls.items()
        }
        self._held = set()
        self._paused = False
        self._resize_job = None

        self.window = tk.Toplevel(parent)
        self.window.title(f"{session.game_id[:8]} · NES · {'Original' if original else 'Enhanced'}")
        self.window.geometry("768x660")
        self.window.minsize(512, 480)
        self.window.configure(bg="#0c121c")
        toolbar = ttk.Frame(self.window, padding=(10, 8))
        toolbar.pack(fill="x")
        self.pause_button = ttk.Button(toolbar, text="Pause", command=self.toggle_pause)
        self.pause_button.pack(side="left")
        ttk.Button(toolbar, text="Close game", command=self.close).pack(side="right")
        self.status = tk.StringVar(value="Starting Mesen inside JAR Revival…")
        ttk.Label(toolbar, textvariable=self.status).pack(side="left", padx=12)
        self.view = tk.Frame(self.window, bg="black", takefocus=True)
        self.view.pack(fill="both", expand=True, padx=10, pady=(0, 10))
        self.view.bind("<Configure>", self._on_resize)
        self.window.bind("<KeyPress>", self._on_key_down)
        self.window.bind("<KeyRelease>", self._on_key_up)
        self.view.bind("<FocusOut>", self._focus_lost)
        self.window.protocol("WM_DELETE_WINDOW", self.close)
        self.window.after(120, self._begin)
        self.window.after(50, self._poll)

    def poll(self):
        if self._fallback_process is not None:
            return self._fallback_process.poll()
        return self._exit_code

    def _begin(self):
        if self._state != "starting":
            return
        self.view.focus_set()
        self.view.update_idletasks()
        try:
            width = max(256, self.view.winfo_width())
            height = max(240, self.view.winfo_height())
            self._start_future = self.runtime.host.start(
                self.session, self.window.winfo_id(), self.view.winfo_id(),
                original=self.original, capture=self.capture)
            self.runtime.host.resize(width, height)
        except Exception as error:
            self._finish_start_error(error)

    def _start_complete(self):
        future = self._start_future
        if future is None or self._state != "starting":
            return
        try:
            self._key_codes, status = future.result()
            if status is not None:
                build_id = bytes(status.graphics_build_id).split(b"\0", 1)[0].decode("ascii", "replace")
                self._write_log(f"Enhanced view ready; renderer build {build_id}; frames presented {status.presented_frames}.")
            else:
                self._write_log("Mesen loaded the NES cartridge in the app-owned game view.")
            self._state = "running"
            self.runtime.host.resize(max(1, self.view.winfo_width()), max(1, self.view.winfo_height()))
            self.status.set("Game running in JAR Revival")
            self.pause_button.configure(state="normal")
        except Exception as error:
            self._finish_start_error(error)

    def _finish_start_error(self, error):
        self._write_log(f"In-process Mesen startup failed: {error}")
        try:
            self._write_status("error", str(error))
        except OSError:
            pass
        try:
            self._fallback_process, self._fallback_log = self.runtime._launch_managed_fallback(self)
            self._write_log(f"The managed Mesen fallback is running; its log is {self._fallback_log}.")
            self.status.set("In-app view could not start; opened the managed Mesen fallback")
            self._state = "fallback"
            self.on_finished(self)
            self.window.after(800, self.window.destroy)
            return
        except Exception as fallback_error:
            self._write_log(f"Managed Mesen fallback also failed: {fallback_error}")
        self._exit_code = 71
        self._state = "finished"
        self.status.set(f"Could not start Mesen: {error}")
        self.pause_button.configure(state="disabled")
        self.on_finished(self)
        self.window.after(800, self.window.destroy)

    def _poll(self):
        if self._state == "starting" and self._start_future is not None and self._start_future.done():
            self._start_complete()
        if self._state == "closing" and self._stop_future is not None and self._stop_future.done():
            startup_error = None
            if self._start_future is not None:
                try:
                    self._start_future.result()
                except Exception as error:
                    startup_error = error
            try:
                self._stop_future.result()
                if startup_error is not None:
                    self._write_status("error", str(startup_error))
                    self._write_log(f"Mesen startup failed before close: {startup_error}")
                    self._exit_code = 71
                else:
                    self._write_status("stopped")
                    self._write_log("Mesen stopped and save work finished.")
                    self._exit_code = 0
            except Exception as error:
                self._write_status("error", str(error))
                self._write_log(f"Mesen close failed: {error}")
                self._exit_code = 71
            self._state = "finished"
            self.on_finished(self)
            self.window.destroy()
            return
        if self._state in ("starting", "running", "closing"):
            self.window.after(50, self._poll)

    def _on_resize(self, event):
        if event.widget is not self.view or self._state not in ("starting", "running"):
            return
        if self._resize_job is not None:
            self.window.after_cancel(self._resize_job)
        self._resize_job = self.window.after(80, lambda: self.runtime.host.resize(
            max(1, self.view.winfo_width()), max(1, self.view.winfo_height())))

    def _prepare_keys(self):
        if not self._key_codes:
            raise RuntimeError("Mesen controls are not ready")

    def _on_key_down(self, event):
        if self._state != "running":
            return None
        self._prepare_keys()
        key = event.keysym.casefold()
        action = self._event_actions.get(key)
        if action is None or key in self._held:
            return None
        self._held.add(key)
        self.runtime.host.key_state(self._key_codes[action], True)
        return "break"

    def _on_key_up(self, event):
        if self._state != "running":
            return None
        key = event.keysym.casefold()
        action = self._event_actions.get(key)
        if action is None or key not in self._held:
            return None
        self._held.discard(key)
        self.runtime.host.key_state(self._key_codes[action], False)
        return "break"

    def _focus_lost(self, _event):
        if self._held and self._state == "running":
            self._held.clear()
            self.runtime.host.reset_keys()

    def toggle_pause(self):
        if self._state != "running":
            return
        self._paused = not self._paused
        self.runtime.host.pause(self._paused)
        self.pause_button.configure(text="Resume" if self._paused else "Pause")
        self.status.set("Game paused" if self._paused else "Game running in JAR Revival")
        self.view.focus_set()

    def close(self):
        if self._state in ("closing", "finished"):
            return
        self._state = "closing"
        self.status.set("Stopping Mesen and saving progress…")
        self.pause_button.configure(state="disabled")
        self._held.clear()
        self._stop_future = self.runtime.host.stop()

    def close_blocking(self, timeout=60.0):
        if self._state not in ("closing", "finished"):
            self._state = "closing"
            self._stop_future = self.runtime.host.stop()
        startup_error = None
        if self._start_future is not None:
            try:
                self._start_future.result(timeout=timeout)
            except Exception as error:
                startup_error = error
        if self._stop_future is not None:
            self._stop_future.result(timeout=timeout)
        if self._exit_code is None:
            if startup_error is not None:
                self._write_status("error", str(startup_error))
                self._write_log(f"Mesen startup failed before close: {startup_error}")
                self._exit_code = 71
            else:
                self._write_status("stopped")
                self._exit_code = 0
        self._state = "finished"
        self.on_finished(self)
        try:
            self.window.destroy()
        except tk.TclError:
            pass

    def _write_log(self, message):
        with self.log.open("a", encoding="utf-8") as stream:
            stream.write(" ".join(str(message).split())[:1000] + "\n")

    def _write_status(self, phase, reason=None):
        document = {"schema": 1, "session_id": self.session.session_id, "phase": phase}
        if reason:
            document["reason"] = " ".join(str(reason).split())[:256]
        destination = self.session.status
        temporary = destination.with_name(destination.name + ".tmp-" + uuid.uuid4().hex)
        temporary.write_text(json.dumps(document, sort_keys=True, separators=(",", ":")), encoding="utf-8")
        os.replace(temporary, destination)
