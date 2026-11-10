import json
from pathlib import Path
from PIL import Image

from runtime_assets.contracts import pixel_recipe
from runtime_assets.store import AssetStore
from systems.nes.capture import _HEADER, _TILE
from systems.nes.integration import NesCaptureIntegration
from systems.nes.pack import NesPackGenerator


def _capture(workspace: Path, identity: str):
    inbox = workspace / ".runtime" / "nes-capture"
    inbox.mkdir(parents=True)
    session = "1" * 32
    palette = bytes(value for value in range(192))
    pattern = bytes([0xFF] * 8 + [0] * 8)
    payload = _HEADER.pack(b"RNT1", identity.encode(), session.encode(), 0, 1, 0, palette)
    payload += _TILE.pack(1, 2, 0x00112233, pattern, 0, 0, 0, 0)
    (inbox / f"{session}-00000000.rnt").write_bytes(payload)


def _ready_tile(workspace, store, recipe):
    source = store.list_sources()[0]
    output = store.resolve_path(f"textures-cache/{recipe.key}/{source.source_id}.png")
    output.parent.mkdir(parents=True, exist_ok=True)
    Image.new("RGBA", (32, 32), (25, 50, 75, 255)).save(output)
    assert store.mark_imported_ready(source.source_id, recipe.key,
                                     output.relative_to(workspace).as_posix())


def test_pack_is_deterministic_and_publishes_pointer(tmp_path):
    identity = "a" * 64
    workspace = tmp_path / identity
    store = AssetStore(workspace)
    recipe = pixel_recipe()
    _capture(workspace, identity)
    NesCaptureIntegration(identity, recipe).admit(workspace, store, {"id": identity}, recipe, True)
    _ready_tile(workspace, store, recipe)
    generator = NesPackGenerator(workspace)
    first = generator.publish(store, recipe, pack_scale=4)
    second = generator.publish(store, recipe, pack_scale=4)
    assert first == second
    assert first.source_count == 1
    assert (first.directory / "hires.txt").read_text(encoding="ascii") == (
        "<ver>109\n<scale>4\n<img>page-000.png\n"
        "<tile>0,2,00112233,0,0,1,N\n")
    assert json.loads((workspace / ".runtime" / "nes-pack.json").read_text())[
        "generation"] == first.generation
    assert Image.open(first.directory / "page-000.png").size == (32, 32)


def test_pack_scale_two_uses_cached_four_x_without_new_job(tmp_path):
    identity = "b" * 64
    workspace = tmp_path / identity
    store = AssetStore(workspace)
    recipe = pixel_recipe()
    _capture(workspace, identity)
    NesCaptureIntegration(identity, recipe).admit(workspace, store, {"id": identity}, recipe, True)
    _ready_tile(workspace, store, recipe)
    generation = NesPackGenerator(workspace).publish(store, recipe, pack_scale=2)
    assert generation is not None
    assert Image.open(generation.directory / "page-000.png").size == (16, 16)
    assert store.counts(recipe.key)["QUEUED"] == 0


def test_invalid_ready_output_is_omitted_and_previous_pack_is_kept(tmp_path):
    identity = "c" * 64
    workspace = tmp_path / identity
    store = AssetStore(workspace)
    recipe = pixel_recipe()
    _capture(workspace, identity)
    NesCaptureIntegration(identity, recipe).admit(workspace, store, {"id": identity}, recipe, True)
    source = store.list_sources()[0]
    output = store.resolve_path(f"textures-cache/{recipe.key}/{source.source_id}.png")
    output.parent.mkdir(parents=True, exist_ok=True)
    Image.new("RGBA", (7, 7), (1, 2, 3, 255)).save(output)
    assert NesPackGenerator(workspace).publish(store, recipe) is None


def test_capture_pack_bootstrap_is_transparent_stable_and_scale_specific(tmp_path):
    identity = "d" * 64
    workspace = tmp_path / identity
    recipe = pixel_recipe()
    generator = NesPackGenerator(workspace)

    first = generator.ensure_capture_pack(recipe, pack_scale=2)
    second = generator.ensure_capture_pack(recipe, pack_scale=2)

    assert first == second
    manifest = json.loads(first.manifest.read_text(encoding="utf-8"))
    assert manifest["recipe_key"] == recipe.key
    assert manifest["pack_scale"] == 2
    assert len(manifest["bindings"]) == len(manifest["tiles"]) == 1
    assert manifest["bindings"][0]["selector"] == "rom:7FFFFFFF,FFFFFFFF"
    assert (first.directory / "hires.txt").read_text(encoding="ascii") == (
        "<ver>109\n<scale>2\n<img>page-000.png\n"
        "<tile>0,7FFFFFFF,FFFFFFFF,0,0,1,N\n")
    with Image.open(first.directory / "page-000.png") as image:
        assert image.size == (16, 16)
        assert image.getchannel("A").getbbox() is None

    at_four = generator.ensure_capture_pack(recipe, pack_scale=4)
    assert at_four.generation != first.generation
    assert json.loads((workspace / ".runtime" / "nes-pack.json").read_text())[
        "pack_scale"] == 4
