"""Check installed playback dependencies without opening a game or Tk window."""

from pathlib import Path
import zipfile


def check_runtime(resource_root):
    from systems.nes.rust_core import RustCore
    root = Path(resource_root)
    java = root / 'tools/runtime/bin/java.exe'
    if not java.is_file():
        raise RuntimeError('Bundled Java runtime is missing.')
    with zipfile.ZipFile(root / 'build/jme-player.jar') as jar:
        control = jar.read('org/recompile/mobile/ComparisonInput.class')
        jar.getinfo('org/recompile/mobile/GpuPlayer.class')
        if b'jme.launcherControl' not in control:
            raise RuntimeError('Java player needs rebuilding for cooperative launcher shutdown.')
    core = RustCore(app_root=root)
    return core.identity
