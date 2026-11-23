"""Own paired playback and private copies of gameplay saves."""

from pathlib import Path
import os
import shutil
import subprocess


def copy_playback_files(source: Path, destination: Path):
    if source.is_symlink() or source.is_junction():
        raise ValueError("Comparison files must stay inside the game workspace.")
    if source.is_dir():
        destination.mkdir(parents=True, exist_ok=True)
        for child in source.iterdir():
            copy_playback_files(child, destination / child.name)
    elif source.is_file():
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)


def comparison_startup_options():
    options = {"stdin": subprocess.PIPE}
    if os.name == "nt":
        startup = subprocess.STARTUPINFO()
        startup.dwFlags |= subprocess.STARTF_USESHOWWINDOW
        startup.wShowWindow = subprocess.SW_HIDE
        options["startupinfo"] = startup
    return options


class ComparisonPlayers:
    def __init__(self, launch, *, ready_paths=()):
        original, original_log = launch(True)
        self.players = [original]
        self.logs = [original_log]
        self.startup_error = None
        self.ready_paths = tuple(ready_paths)
        # Closing the comparison terminates its players; that is not a crash.
        self.expected_exit = False
        try:
            enhanced, enhanced_log = launch(False)
        except (OSError, RuntimeError, ValueError) as error:
            self.startup_error = str(error)
        else:
            self.players.append(enhanced)
            self.logs.append(enhanced_log)

    def poll(self):
        codes = [player.poll() for player in self.players]
        if any(code is None for code in codes):
            return None
        if self.expected_exit:
            return 0
        return next((code for code in codes if code != 0), 0)

    def send_key(self, token, pressed):
        message = (("+" if pressed else "-") + token + "\n").encode("ascii")
        for player in self.players:
            stream = player.stdin
            if stream is None:
                raise BrokenPipeError("Comparison player input is unavailable")
            stream.write(message)
            stream.flush()
