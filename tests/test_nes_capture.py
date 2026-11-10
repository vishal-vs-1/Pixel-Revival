from pathlib import Path
from struct import pack

import pytest

from runtime_assets.contracts import pixel_recipe
from runtime_assets.store import AssetStore
from systems.nes.bindings import NesBindingStore
from systems.nes.capture import _HEADER, _TILE, read_chunk
from systems.nes.integration import NesCaptureIntegration
from systems.nes.tiles import Tile, palette_id


def _chunk(root: Path, identity: str, session: str, sequence: int, *,
           transparent=False):
    palette = bytes(value for value in range(192))
    pattern = bytes([(0xFE if transparent else 0xFF)] + [0xFF] * 7 + [0] * 8)
    path = root / f"{session}-{sequence:08x}.rnt"
    payload = _HEADER.pack(b"RNT1", identity.encode(), session.encode(), sequence, 1, 0, palette)
    payload += _TILE.pack(4, 2, 0x00112233, pattern, 0, 0, int(transparent), 0)
    path.write_bytes(payload)
    return path


def test_tile_decoder_matches_nes_bitplane_order():
    palette = bytes(index for index in range(192))
    tile = Tile(1, 2, 0x00112233, bytes([0x80] + [0] * 7 + [0] * 8),
                False, False, False)
    image = tile.image(palette)
    assert image.getpixel((0, 0)) == (51, 52, 53, 255)
    assert image.getpixel((1, 0)) == (0, 1, 2, 255)


def test_capture_replay_registers_original_and_is_idempotent(tmp_path):
    identity = "a" * 64
    session = "b" * 32
    workspace = tmp_path / identity
    inbox = workspace / ".runtime" / "nes-capture"
    inbox.mkdir(parents=True)
    _chunk(inbox, identity, session, 0)
    store = AssetStore(workspace)
    integration = NesCaptureIntegration(identity, pixel_recipe())
    report = integration.admit(workspace, store, {"id": identity}, pixel_recipe(), True)
    assert report.sources_registered == 1
    assert not list(inbox.glob("*.rnt"))
    assert len(store.list_sources()) == 1
    assert integration.discover(workspace, store, pixel_recipe()) == 0
    next_chunk = read_chunk(_chunk(inbox, identity, session, 1), identity)
    assert NesBindingStore(workspace).ingested(session, 1, next_chunk.digest) is False


def test_capture_quarantines_bad_file_and_rejects_sequence_reuse(tmp_path):
    identity = "c" * 64
    session = "d" * 32
    workspace = tmp_path / identity
    inbox = workspace / ".runtime" / "nes-capture"
    inbox.mkdir(parents=True)
    (inbox / f"{session}-00000000.rnt").write_bytes(b"bad")
    integration = NesCaptureIntegration(identity, pixel_recipe())
    store = AssetStore(workspace)
    assert integration.discover(workspace, store, pixel_recipe()) == 0
    assert (inbox / "invalid-00.bad").is_file()
    _chunk(inbox, identity, session, 1)
    chunk = read_chunk(next(inbox.glob("*.rnt")), identity)
    bindings = NesBindingStore(workspace)
    bindings.ingest(chunk, store, pixel_recipe())
    with pytest.raises(ValueError):
        bindings.ingested(session, 1, "0" * 64)


def test_transparency_upgrade_rebinds_the_same_selector(tmp_path):
    identity = "e" * 64
    session = "f" * 32
    workspace = tmp_path / identity
    inbox = workspace / ".runtime" / "nes-capture"
    inbox.mkdir(parents=True)
    first = _chunk(inbox, identity, session, 0, transparent=False)
    store = AssetStore(workspace)
    integration = NesCaptureIntegration(identity, pixel_recipe())
    integration.admit(workspace, store, {"id": identity}, pixel_recipe(), True)
    bindings = NesBindingStore(workspace)
    first_binding = bindings.binding("rom:2,00112233|background")
    assert first_binding is not None and not first_binding.transparency_required
    _chunk(inbox, identity, session, 1, transparent=True)
    integration.discover(workspace, store, pixel_recipe())
    second_binding = bindings.binding("rom:2,00112233|background")
    assert second_binding is not None and second_binding.transparency_required
    assert second_binding.source_id != first_binding.source_id
    assert len(store.list_sources()) == 2
