"""Bounded decoding and ownership checks for committed native capture chunks."""

from dataclasses import dataclass
from hashlib import sha256
import os
from pathlib import Path
import re
import stat
from struct import Struct

from .tiles import Tile, palette_id

CAPTURE_DIRECTORY = ".runtime/nes-capture"
CHUNK_RECORDS = 128
MAX_CHUNKS = 512
_HEADER = Struct("<4s64s32sIHH192s")
_TILE = Struct("<QiI16sBBBB")
_MAX_BYTES = _HEADER.size + _TILE.size * CHUNK_RECORDS
_NAME = re.compile(r"([0-9a-f]{32})-([0-9a-f]{8})\.rnt")
_QUARANTINE_LIMIT = 32


@dataclass(frozen=True)
class CaptureChunk:
    session: str
    sequence: int
    digest: str
    palette_profile: str
    palette_rgb: bytes
    tiles: tuple[Tile, ...]


def owned_path(workspace: Path, relative: str) -> Path:
    path = workspace
    for component in Path(relative).parts:
        path = path / component
        if path.is_symlink() or path.is_junction():
            raise ValueError("NES artwork path is redirected")
    if not path.resolve().is_relative_to(workspace.resolve()):
        raise ValueError("NES artwork path escapes its workspace")
    return path


def read_chunk(path: Path, identity: str) -> CaptureChunk:
    match = _NAME.fullmatch(path.name)
    if match is None or not stat.S_ISREG(path.lstat().st_mode):
        raise ValueError("Invalid NES capture file")
    with path.open("rb") as stream:
        data = stream.read(_MAX_BYTES + 1)
    if not _HEADER.size <= len(data) <= _MAX_BYTES:
        raise ValueError("NES capture file size is out of bounds")
    magic, game, session, sequence, count, reserved, palette = _HEADER.unpack_from(data)
    if (magic != b"RNT1" or game != identity.encode("ascii") or reserved or
            session != match[1].encode("ascii") or sequence != int(match[2], 16) or
            sequence >= MAX_CHUNKS or not 1 <= count <= CHUNK_RECORDS or
            len(data) != _HEADER.size + count * _TILE.size):
        raise ValueError("Invalid NES capture header")
    tiles = []
    for frame, index, colors, pattern, ram, sprite, transparent, reserved in _TILE.iter_unpack(
            memoryview(data)[_HEADER.size:]):
        if reserved or any(flag > 1 for flag in (ram, sprite, transparent)):
            raise ValueError("Invalid NES capture flags")
        tiles.append(Tile(frame, index, colors, pattern, bool(ram), bool(sprite), bool(transparent)))
    return CaptureChunk(match[1], sequence, sha256(data).hexdigest(), palette_id(palette),
                        palette, tuple(tiles))


def committed_chunks(workspace, limit=4):
    folder = owned_path(workspace, CAPTURE_DIRECTORY)
    if not folder.exists():
        return []
    pending = []
    with os.scandir(folder) as entries:
        for scanned, entry in enumerate(entries):
            if scanned >= MAX_CHUNKS * 2:
                raise ValueError("NES capture inbox has too many entries")
            if _NAME.fullmatch(entry.name):
                pending.append(entry.name)
    return [folder / name for name in sorted(pending)[:limit]]


def quarantine(path):
    folder = path.parent
    # Keep a small diagnostic sample without allowing bad captures to fill the disk.
    for index in range(_QUARANTINE_LIMIT):
        destination = folder / f"invalid-{index:02d}.bad"
        if not destination.exists() and not destination.is_symlink():
            path.rename(destination)
            return
    path.unlink()
