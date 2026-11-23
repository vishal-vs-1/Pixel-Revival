import json
from dataclasses import replace
from pathlib import Path
import sqlite3
import pytest
from PIL import Image

from runtime_assets.contracts import pixel_recipe
from runtime_assets.recipes import ArtworkRecipes
from runtime_assets.store import AssetStore
from systems.nes.capture import _HEADER, _TILE
from systems.nes.integration import NesCaptureIntegration
from systems.nes.pack import NesPackGenerator

PACK_MANIFEST_FIELDS = {"schema", "recipe_key", "pack_scale", "bindings", "generation", "pages", "tiles"}


def _capture(workspace: Path, identity: str, *, has_color_zero=False, session="1" * 32):
    inbox = workspace / ".runtime" / "nes-capture"
    inbox.mkdir(parents=True, exist_ok=True)
    palette = bytes(value for value in range(192))
    pattern = bytes([0xFE if has_color_zero else 0xFF] * 8 + [0] * 8)
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


@pytest.mark.parametrize("backend", ["pixel", "redraw"])
def test_pack_is_deterministic_and_publishes_pointer(tmp_path, backend):
    identity = "a" * 64
    workspace = tmp_path / identity
    store = AssetStore(workspace)
    recipes = ArtworkRecipes()
    recipe, _ = recipes.for_backend(backend)
    _capture(workspace, identity)
    NesCaptureIntegration(identity, recipe).admit(workspace, store, {"id": identity}, recipe, True)
    if backend == "redraw":
        source = store.list_sources()[0]
        output = f"textures-cache/{recipe.key}/{source.source_id}.png"
        path = store.resolve_path(output)
        path.parent.mkdir(parents=True)
        with Image.open(store.resolve_path(source.relative_path)) as image:
            recipes.backend_for_recipe(recipe).enhance(image, recipe, workspace).save(path)
        assert store.mark_imported_ready(source.source_id, recipe.key, output)
    else:
        _ready_tile(workspace, store, recipe)
    generator = NesPackGenerator(workspace)
    first = generator.publish(store, recipe, pack_scale=4)
    second = generator.publish(store, recipe, pack_scale=4)
    assert first == second
    assert first.source_count == 1
    assert set(json.loads(first.manifest.read_text(encoding="utf-8"))) == PACK_MANIFEST_FIELDS
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


@pytest.mark.parametrize("pack_scale", [2, 4])
def test_background_pack_preserves_color_zero_for_behind_priority_sprites(tmp_path, pack_scale):
    identity = "f" * 64
    workspace = tmp_path / identity
    store = AssetStore(workspace)
    recipe = pixel_recipe()
    _capture(workspace, identity, has_color_zero=True)
    NesCaptureIntegration(identity, recipe).admit(workspace, store, {"id": identity}, recipe, True)
    # Simulate an enhancement result that filled the originally transparent pixels.
    _ready_tile(workspace, store, recipe)
    generation = NesPackGenerator(workspace).publish(store, recipe, pack_scale=pack_scale)
    with Image.open(generation.directory / "page-000.png") as page:
        assert page.getchannel("A").getbbox() == (0, 0, 7 * pack_scale, 8 * pack_scale)


def test_old_background_pack_falls_back_until_tile_is_recaptured(tmp_path):
    identity = "1" * 64
    workspace = tmp_path / identity
    store = AssetStore(workspace)
    recipe = pixel_recipe()
    _capture(workspace, identity)
    NesCaptureIntegration(identity, recipe).admit(workspace, store, {"id": identity}, recipe, True)
    _ready_tile(workspace, store, recipe)
    generator = NesPackGenerator(workspace)
    previous = generator.publish(store, recipe)
    pointer = workspace / ".runtime" / "nes-pack.json"
    document = json.loads(pointer.read_text(encoding="utf-8"))
    document.pop("background_priority")
    pointer.write_text(json.dumps(document), encoding="utf-8")
    with sqlite3.connect(workspace / "nes" / "bindings.sqlite") as database:
        database.execute("UPDATE tile_bindings SET transparency_required=0")
    bootstrap = generator.ensure_capture_pack(recipe)
    assert bootstrap.generation != previous.generation
    assert generator.publish(store, recipe) is None

    _capture(workspace, identity, session="2" * 32)
    NesCaptureIntegration(identity, recipe).discover(workspace, store, recipe)
    assert generator.publish(store, recipe) == previous
    assert json.loads(pointer.read_text(encoding="utf-8"))["generation"] == previous.generation


def test_recipe_change_queues_saved_nes_tiles_without_a_new_capture(tmp_path):
    identity = "e" * 64
    workspace = tmp_path / identity
    store = AssetStore(workspace)
    recipe = pixel_recipe()
    _capture(workspace, identity)
    NesCaptureIntegration(identity, recipe).admit(workspace, store, {"id": identity}, recipe, True)

    replacement = replace(recipe, implementation_version="test-replacement")
    store.register_recipe(replacement)
    NesCaptureIntegration(identity, replacement).admit(
        workspace, store, {"id": identity}, replacement, True)
    assert store.counts(replacement.key)["QUEUED"] == 1


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
    assert set(manifest) == PACK_MANIFEST_FIELDS
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
