"""Apply integration patches to the pinned Java ME source checkout."""

from pathlib import Path
import sys

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tools.patches.java_me.frontend import patch_frontend
from tools.patches.java_me.graphics import patch_platform_graphics
from tools.patches.java_me.image import patch_platform_image
from tools.patches.java_me.lifecycle import patch_midp_lifecycle, patch_platform_lifecycle
from tools.patches.java_me.m3g import patch_m3g
from tools.patches.java_me.sources import install_sources


ROOT = Path(__file__).resolve().parents[1]


def patch():
    java_sources = ROOT / 'vendor/freej2me-plus/src'
    source = java_sources / 'org/recompile'
    patch_m3g(java_sources / 'javax/microedition/m3g/Graphics3D.java')
    patch_platform_image(source)
    patch_platform_graphics(source)
    patch_frontend(source)
    install_sources(ROOT, source)
    patch_platform_lifecycle(source)
    patch_midp_lifecycle(source)


if __name__ == '__main__':
    patch()
