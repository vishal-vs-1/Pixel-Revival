"""Durable NES tile bindings kept separate from the generic artwork database."""

from __future__ import annotations

from contextlib import contextmanager
from dataclasses import dataclass
import sqlite3
import time
from pathlib import Path

from runtime_assets.contracts import Recipe, SourceRecord, image_source_id
from runtime_assets.store import AssetStore

from .capture import CaptureChunk


@dataclass(frozen=True)
class TileBinding:
    binding_id: str
    selector: str
    source_id: str
    transparency_required: bool


class NesBindingStore:
    """Own palette, selector and capture-ingress state for one NES workspace."""

    def __init__(self, workspace: Path, *, busy_timeout_ms: int = 3000):
        self.workspace = Path(workspace).resolve()
        self.database = self.workspace / "nes" / "bindings.sqlite"
        self.database.parent.mkdir(parents=True, exist_ok=True)
        self.busy_timeout_ms = max(100, min(int(busy_timeout_ms), 30000))
        self._initialize()

    def _connect(self):
        db = sqlite3.connect(self.database, timeout=self.busy_timeout_ms / 1000,
                             isolation_level=None)
        db.row_factory = sqlite3.Row
        db.execute(f"PRAGMA busy_timeout={self.busy_timeout_ms}")
        return db

    @contextmanager
    def _transaction(self):
        db = self._connect()
        db.execute("BEGIN IMMEDIATE")
        try:
            yield db
            db.commit()
        except BaseException:
            db.rollback()
            raise
        finally:
            db.close()

    def _initialize(self):
        db = self._connect()
        try:
            version = db.execute("PRAGMA user_version").fetchone()[0]
            if version > 1:
                raise RuntimeError("NES bindings database is newer than this application")
            if version == 0:
                db.executescript("""
                    BEGIN IMMEDIATE;
                    CREATE TABLE palette_profiles (
                      profile_id TEXT PRIMARY KEY,
                      rgb BLOB NOT NULL CHECK(length(rgb)=192),
                      created_at REAL NOT NULL
                    );
                    CREATE TABLE tile_bindings (
                      binding_id TEXT PRIMARY KEY,
                      selector TEXT NOT NULL,
                      selector_json TEXT NOT NULL,
                      source_id TEXT NOT NULL,
                      transparency_required INTEGER NOT NULL CHECK(transparency_required IN (0,1)),
                      first_seen INTEGER NOT NULL,
                      last_seen INTEGER NOT NULL
                    );
                    CREATE TABLE ingested_chunks (
                      session_id TEXT NOT NULL,
                      sequence INTEGER NOT NULL,
                      chunk_digest TEXT NOT NULL,
                      ingested_at REAL NOT NULL,
                      PRIMARY KEY(session_id, sequence)
                    );
                    CREATE TABLE pack_generations (
                      generation TEXT PRIMARY KEY,
                      manifest_digest TEXT NOT NULL,
                      created_at REAL NOT NULL
                    );
                    PRAGMA user_version=1;
                    COMMIT;
                """)
        finally:
            db.close()

    def palette(self, profile_id: str) -> bytes | None:
        with self._connect() as db:
            row = db.execute("SELECT rgb FROM palette_profiles WHERE profile_id=?",
                             (profile_id,)).fetchone()
            return None if row is None else bytes(row["rgb"])

    def remember_palette(self, profile_id: str, rgb: bytes):
        if not isinstance(profile_id, str) or len(profile_id) != 64:
            raise ValueError("Invalid NES palette profile")
        if not isinstance(rgb, bytes) or len(rgb) != 192:
            raise ValueError("NES palette must contain 64 RGB entries")
        with self._transaction() as db:
            row = db.execute("SELECT rgb FROM palette_profiles WHERE profile_id=?",
                             (profile_id,)).fetchone()
            if row is not None and bytes(row["rgb"]) != rgb:
                raise ValueError("NES palette profile changed")
            db.execute("INSERT OR IGNORE INTO palette_profiles(profile_id,rgb,created_at) "
                       "VALUES (?,?,?)", (profile_id, rgb, time.time()))

    def binding(self, binding_id: str) -> TileBinding | None:
        with self._connect() as db:
            row = db.execute("SELECT * FROM tile_bindings WHERE binding_id=?",
                             (binding_id,)).fetchone()
            if row is None:
                return None
            return TileBinding(row["binding_id"], row["selector"], row["source_id"],
                               bool(row["transparency_required"]))

    def list_bindings(self) -> list[TileBinding]:
        with self._connect() as db:
            rows = db.execute("SELECT * FROM tile_bindings ORDER BY binding_id").fetchall()
            return [TileBinding(row["binding_id"], row["selector"], row["source_id"],
                                bool(row["transparency_required"])) for row in rows]

    def ingested(self, session_id: str, sequence: int, digest: str) -> bool:
        with self._connect() as db:
            row = db.execute("SELECT chunk_digest FROM ingested_chunks "
                             "WHERE session_id=? AND sequence=?",
                             (session_id, sequence)).fetchone()
            if row is None:
                return False
            if row["chunk_digest"] != digest:
                raise ValueError("NES capture sequence was reused with different data")
            return True

    def ingest(self, chunk: CaptureChunk, store: AssetStore, recipe: Recipe) -> int:
        """Register originals before bindings and acknowledge a chunk last."""
        self.remember_palette(chunk.palette_profile, chunk.palette_rgb)
        if self.ingested(chunk.session, chunk.sequence, chunk.digest):
            return 0
        assets = self.workspace / "original-assets"
        assets.mkdir(parents=True, exist_ok=True)
        added = 0
        for tile in chunk.tiles:
            binding_id = f"{tile.selector}|{'sprite' if tile.sprite else 'background'}"
            previous = self.binding(binding_id)
            transparency = tile.transparent or (previous.transparency_required if previous else False)
            image = tile.image(chunk.palette_rgb, transparent=transparency)
            source_id = image_source_id(image)
            relative = f"original-assets/{source_id}.png"
            destination = store.resolve_path(relative)
            if not destination.is_file():
                temporary = destination.with_name(destination.name + ".part")
                image.save(temporary, format="PNG")
                temporary.replace(destination)
            source = SourceRecord(source_id, 8, 8, relative, "nes-tile-v1")
            event_id = f"nes:{chunk.session}:{chunk.sequence}:{binding_id}"
            if store.register_capture_event(event_id, source, recipe):
                added += 1
            now = int(time.time())
            with self._transaction() as db:
                db.execute(
                    "INSERT INTO tile_bindings(binding_id,selector,selector_json,source_id,"
                    "transparency_required,first_seen,last_seen) VALUES (?,?,?,?,?,?,?) "
                    "ON CONFLICT(binding_id) DO UPDATE SET source_id=excluded.source_id,"
                    "transparency_required=max(tile_bindings.transparency_required,"
                    "excluded.transparency_required),last_seen=excluded.last_seen",
                    (binding_id, tile.selector,
                     f'{{"selector":"{tile.selector}","sprite":{str(tile.sprite).lower()}}}',
                     source_id, int(transparency), now, now))
        with self._transaction() as db:
            row = db.execute("SELECT chunk_digest FROM ingested_chunks "
                             "WHERE session_id=? AND sequence=?",
                             (chunk.session, chunk.sequence)).fetchone()
            if row is not None and row["chunk_digest"] != chunk.digest:
                raise ValueError("NES capture sequence was reused with different data")
            db.execute("INSERT OR IGNORE INTO ingested_chunks(session_id,sequence,chunk_digest,ingested_at) "
                       "VALUES (?,?,?,?)", (chunk.session, chunk.sequence, chunk.digest, time.time()))
        return added
