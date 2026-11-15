"""Read folded metadata fields from a Java archive manifest."""
from __future__ import annotations




def read_manifest(raw: bytes) -> dict:
    text = raw.decode('utf-8', errors='replace').replace('\r\n', '\n')
    lines = []
    for line in text.split('\n'):
        if line.startswith(' ') and lines:
            lines[-1] += line[1:]
        else:
            lines.append(line)
    return dict(line.split(':', 1) for line in lines if ':' in line)
