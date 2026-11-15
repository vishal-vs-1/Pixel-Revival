"""Locate Java executables used by the player and build tools."""
from __future__ import annotations

import os
from pathlib import Path
import shutil


def locate_java(tool='java') -> str:
    override = os.environ.get('JME_JAVA_HOME')
    candidates = [Path(override) / 'bin' / (tool + '.exe')] if override else []
    # Avoid Oracle javapath launcher shims, which can hang on stale installations.
    for base in [Path('C:/Program Files/Java'), Path('C:/Program Files/Eclipse Adoptium')]:
        if base.exists():
            candidates.extend(p / 'bin' / (tool + '.exe') for p in sorted(base.iterdir(), reverse=True))
    for candidate in candidates:
        if candidate.is_file():
            return str(candidate)
    found = shutil.which(tool)
    if found:
        return found
    raise RuntimeError('A Java JDK is required. Install JDK 17 or newer, or set JME_JAVA_HOME.')
