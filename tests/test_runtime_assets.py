import json
from pathlib import Path
import tempfile

import pytest
from PIL import Image

from runtime_assets.contracts import image_source_id as image_id
from runtime_assets.backends import PixelBackend
from runtime_assets.contracts import (Recipe, SourceRecord, image_source_id,
                                      ncnn_recipe, pixel_recipe)
from runtime_assets.ingest import LegacyCacheImporter
from runtime_assets.packed_sources import PACKED_SPRITE_DECODER, LARGE_PACKED_CATALOGUE
from runtime_assets.store import AssetStore


@pytest.fixture
def tmp_path():
    """Keep test writes inside the project workspace in restricted environments."""
    parent = Path(__file__).resolve().parents[1] / "build" / "test-runtime-assets"
    parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="case-", dir=parent) as directory:
        yield Path(directory)


def _image(path: Path, size=(3, 2), color=(30, 80, 140, 255)):
    path.parent.mkdir(parents=True, exist_ok=True)
    image = Image.new("RGBA", size, color)
    image.save(path)
    return image


def _source(image: Image.Image, relative="captures/source.png"):
    return SourceRecord(image_source_id(image), image.width, image.height,
                        relative, "legacy-capture")


def test_redraw_publishes_java_texture_without_added_ink(tmp_path):
    from threading import Lock

    from runtime_assets.publication import publish_compatibility
    from runtime_assets.recipes import ArtworkRecipes
    from runtime_assets.work_state import _Game

    recipes = ArtworkRecipes()
    recipe, ai_recipe = recipes.for_backend("redraw")
    assert ai_recipe is None and recipe.key != pixel_recipe().key
    source = Image.new("RGBA", (130, 130), (220, 90, 30, 255))
    source.putpixel((124, 124), (0, 0, 0, 0))
    result = recipes.backend_for_recipe(recipe).enhance(source, recipe, tmp_path)
    assert result.size == (520, 520)
    assert result.getpixel((0, 0)) == (220, 90, 30, 255)
    assert result.getpixel((498, 498))[3] == 0

    store = AssetStore(tmp_path)
    record = _source(source)
    store.register_recipe(recipe)
    store.register_source(record)
    output = f"textures-cache/{recipe.key}/{record.source_id}.png"
    path = store.resolve_path(output)
    path.parent.mkdir(parents=True)
    result.save(path)
    assert store.mark_imported_ready(record.source_id, recipe.key, output)
    publish_compatibility(_Game(tmp_path, store, recipe), limit=1, commit_lock=Lock())
    path = tmp_path / "textures-redraw" / f"{record.source_id}.png"
    with Image.open(path) as published:
        assert published.tobytes() == result.tobytes()


def test_recipe_is_canonical_and_deeply_immutable():
    recipe = Recipe(1, "test", "v1", "", 2, "keep", "edge",
                    {"b": [1, {"a": True}], "a": 3})
    equivalent = Recipe(1, "test", "v1", "", 2, "keep", "edge",
                        {"a": 3, "b": [1, {"a": True}]})
    assert recipe.key == equivalent.key
    assert Recipe.from_json(recipe.canonical_json()) == recipe
    with pytest.raises(TypeError):
        recipe.settings["new"] = 1
    with pytest.raises(TypeError):
        recipe.settings["b"][1]["a"] = False


def test_shared_identity_matches_engine_and_ignores_hidden_transparent_rgb():
    first = Image.new("RGBA", (2, 2), (0, 0, 0, 0))
    second = Image.new("RGBA", (2, 2), (200, 80, 10, 0))
    assert image_source_id(first) == image_source_id(second)
    assert image_id(first) == image_source_id(first)
    second.putpixel((1, 1), (200, 80, 10, 255))
    assert image_source_id(first) != image_source_id(second)


def test_job_is_deduplicated_and_expired_lease_is_recovered(tmp_path):
    store = AssetStore(tmp_path)
    image = Image.new("RGBA", (2, 2), (1, 2, 3, 255))
    source = _source(image)
    recipe = pixel_recipe()
    store.register_recipe(recipe)
    store.register_source(source)

    assert store.enqueue(source.source_id, recipe.key, now=100)
    assert not store.enqueue(source.source_id, recipe.key, now=101)
    stale = store.claim("worker-a", now=102, lease_seconds=5)
    assert stale is not None and stale.attempts == 1
    assert store.recover_expired(now=108) == 1

    current = store.claim("worker-b", now=109, lease_seconds=30)
    assert current is not None and current.attempts == 2
    output = tmp_path / "textures" / "source.png"
    _image(output, (8, 8))
    assert not store.mark_ready(stale, "textures/source.png", now=110)
    assert store.mark_ready(current, "textures/source.png", now=110)
    assert store.get_job(source.source_id, recipe.key).state == "READY"


def test_duplicate_pixel_source_keeps_one_durable_job(tmp_path):
    store = AssetStore(tmp_path)
    image = Image.new("RGBA", (2, 2), (1, 2, 3, 255))
    source_id = image_source_id(image)
    store.register_recipe(pixel_recipe())
    store.register_source(SourceRecord(source_id, 2, 2, "original-assets/a.png", "original"))
    store.register_source(SourceRecord(source_id, 2, 2, "captures/b.png", "capture"))
    assert store.enqueue(source_id, pixel_recipe().key)
    assert not store.enqueue(source_id, pixel_recipe().key)
    assert store.pending_count() == 1


def test_batch_claim_respects_budget_without_spending_skipped_attempts(tmp_path):
    store=AssetStore(tmp_path)
    recipe=pixel_recipe()
    store.register_recipe(recipe)
    sources=[]
    for index,size in enumerate(((2,2),(10,10),(2,2))):
        source=_source(Image.new('RGBA',size,(index,50,100,255)),f'original-assets/{index}.png')
        store.register_source(source)
        store.enqueue(source.source_id,recipe.key,now=100+index)
        sources.append(source)
    leases=store.claim_batch('batch-one',now=200,lease_seconds=30,
                            recipe_key=recipe.key,max_count=5,max_pixels=8)
    assert [lease.source_id for lease in leases]==[sources[0].source_id,sources[2].source_id]
    skipped=store.get_job(sources[1].source_id,recipe.key)
    assert skipped.state=='QUEUED' and skipped.attempts==0
    # A large job at the front must run alone instead of being starved by the budget.
    large=store.claim_batch('batch-two',now=201,lease_seconds=30,
                           recipe_key=recipe.key,max_count=5,max_pixels=8)
    assert len(large)==1 and large[0].source_id==sources[1].source_id
    assert len({lease.token for lease in leases+large})==3
    assert store.recover_expired(now=232)==3
    assert store.counts(recipe.key)['QUEUED']==3


def test_parallel_batch_claims_do_not_share_jobs(tmp_path):
    from concurrent.futures import ThreadPoolExecutor
    store=AssetStore(tmp_path)
    recipe=pixel_recipe()
    store.register_recipe(recipe)
    for index in range(20):
        source=_source(Image.new('RGBA',(2,2),(index,40,90,255)),f'original-assets/{index}.png')
        store.register_source(source)
        store.enqueue(source.source_id,recipe.key)
    with ThreadPoolExecutor(max_workers=2) as pool:
        batches=list(pool.map(lambda owner:store.claim_batch(owner,recipe_key=recipe.key,max_count=5),
                              ('first-worker','second-worker')))
    assert [len(batch) for batch in batches]==[5,5]
    assert len({lease.source_id for batch in batches for lease in batch})==10
    assert store.counts(recipe.key)['RUNNING']==10
    assert store.counts(recipe.key)['QUEUED']==10


def test_capture_event_and_job_registration_are_atomic_and_idempotent(tmp_path):
    store = AssetStore(tmp_path)
    source = _source(Image.new("RGBA", (2, 2), (3, 4, 5, 255)))
    recipe = pixel_recipe()
    assert store.register_capture_event("capture-1", source, recipe, now=1)
    assert not store.register_capture_event("capture-1", source, recipe, now=2)
    assert store.pending_count() == 1


def test_import_batch_is_atomic_and_preserves_ready_jobs(tmp_path):
    store = AssetStore(tmp_path)
    recipe = pixel_recipe()
    store.register_recipe(recipe)
    ready = _source(Image.new("RGBA", (2, 2), (3, 4, 5, 255)))
    new_source = _source(Image.new("RGBA", (3, 3), (5, 6, 7, 255)))
    assert store.register_import_batch([ready], recipe.key, 0, queue_missing=True) == 1
    relative = f"textures-cache/{recipe.key}/{ready.source_id}.png"
    _image(tmp_path / relative, (8, 8))
    store.mark_imported_ready(ready.source_id, recipe.key, relative)

    conflicting = SourceRecord(ready.source_id, 3, 3, ready.relative_path, ready.origin)
    with pytest.raises(ValueError, match="conflicting dimensions"):
        store.register_import_batch([new_source, conflicting], recipe.key, 0, queue_missing=True)
    assert store.get_source(new_source.source_id) is None
    assert store.register_import_batch([ready, new_source], recipe.key, 0, queue_missing=True) == 1
    assert store.get_job(ready.source_id, recipe.key).state == "READY"
    assert store.get_job(new_source.source_id, recipe.key).state == "QUEUED"


def test_large_packed_catalogue_waits_for_capture_only_for_ai(tmp_path):
    store = AssetStore(tmp_path)
    ai = ncnn_recipe('a' * 64)
    pixel = pixel_recipe()
    files = tmp_path / 'original-assets'
    files.mkdir()
    rows = []
    for index in range(3):
        image = Image.new('RGBA', (2, 2), (index, 9, 8, 255))
        key = image_source_id(image)
        image.save(files / f'{key}.png')
        rows.append({'image_id': key, 'decoder': PACKED_SPRITE_DECODER})
    rows.append(dict(rows[-1], decoder='standard-images-v1'))
    (tmp_path / 'catalogue.json').write_text(json.dumps(rows), encoding='utf-8')
    game = {'id': tmp_path.name}
    importer = LegacyCacheImporter(tmp_path, store)
    importer.import_workspace(game, ai)

    # An older library can already have queued work for all three sources.
    game['asset_decoding'] = {'outcomes': {PACKED_SPRITE_DECODER: LARGE_PACKED_CATALOGUE}}
    importer.import_workspace(game, ai, queue_missing=False)
    assert store.counts(ai.key, hold_packed=True)['on_demand'] == 2
    first = store.claim_batch('worker', recipe_key=ai.key, hold_packed=True)
    assert [job.source_id for job in first] == [rows[-1]['image_id']]
    assert not store.has_waiting(ai.key, hold_packed=True)

    key = rows[0]['image_id']
    captured = SourceRecord(key, 2, 2, f'original-assets/{key}.png', 'RGB')
    store.register_capture_event('seen-in-game', captured, ai)
    assert store.get_source(key).origin == 'RGB'
    assert store.counts(ai.key, hold_packed=True)['on_demand'] == 1
    assert store.claim_batch('worker', recipe_key=ai.key, hold_packed=True)[0].source_id == key
    importer.import_workspace(game, pixel)
    assert store.counts(pixel.key)['QUEUED'] == 3


def test_legacy_import_reuses_cache_only_with_matching_known_recipe(tmp_path):
    workspace = tmp_path / "game-a"
    image = _image(workspace / "original-assets" / "placeholder.png")
    source_id = image_source_id(image)
    original = workspace / "original-assets" / "placeholder.png"
    original.rename(original.with_name(source_id + ".png"))
    _image(workspace / "textures" / (source_id + ".png"), (image.width * 4, image.height * 4))
    store = AssetStore(workspace)
    importer = LegacyCacheImporter(workspace, store)

    report = importer.import_workspace(
        {"id": workspace.name, "backend": "pixel", "recipe": "scale2x-rgba-v1-4x"},
        pixel_recipe(),
    )
    job = store.get_job(source_id, pixel_recipe().key)
    assert report.pixel_cache_reused == 1
    assert report.jobs_queued == 0
    assert job.state == "READY"
    assert job.relative_output == f"textures-cache/{pixel_recipe().key}/{source_id}.png"
    assert (workspace / "textures" / f"{source_id}.png").is_file()


def test_unknown_legacy_cache_is_preserved_but_not_adopted(tmp_path):
    workspace = tmp_path / "game-b"
    image = _image(workspace / "original-assets" / "placeholder.png")
    source_id = image_source_id(image)
    original = workspace / "original-assets" / "placeholder.png"
    original.rename(original.with_name(source_id + ".png"))
    old_texture = workspace / "textures" / (source_id + ".png")
    _image(old_texture, (image.width * 4, image.height * 4))
    store = AssetStore(workspace)
    importer = LegacyCacheImporter(workspace, store)

    report = importer.import_workspace({"id": workspace.name}, pixel_recipe())
    job = store.get_job(source_id, pixel_recipe().key)
    assert report.pixel_cache_reused == 0
    assert report.jobs_queued == 1
    assert job.state == "QUEUED"
    assert old_texture.is_file()
    assert report.legacy_unknown_cache_files == 1


def test_importer_queues_if_known_cache_dimensions_are_wrong(tmp_path):
    workspace = tmp_path / "game-c"
    image = _image(workspace / "original-assets" / "placeholder.png")
    source_id = image_source_id(image)
    original = workspace / "original-assets" / "placeholder.png"
    original.rename(original.with_name(source_id + ".png"))
    old_texture = workspace / "textures" / (source_id + ".png")
    _image(old_texture, (image.width * 2, image.height * 2))
    store = AssetStore(workspace)
    importer = LegacyCacheImporter(workspace, store)

    report = importer.import_workspace(
        {"id": workspace.name, "backend": "pixel", "recipe": "scale2x-rgba-v1-4x"},
        pixel_recipe(),
    )
    assert report.pixel_cache_reused == 0
    assert report.jobs_queued == 1
    assert old_texture.is_file()


def test_legacy_ai_cache_requires_recorded_matching_model_digest(tmp_path):
    workspace = tmp_path / "game-ai"
    image = _image(workspace / "original-assets" / "placeholder.png")
    source_id = image_source_id(image)
    original = workspace / "original-assets" / "placeholder.png"
    original.rename(original.with_name(source_id + ".png"))
    old_texture = workspace / "textures-ai" / (source_id + ".png")
    _image(old_texture, (image.width * 4, image.height * 4))
    model_digest = "a" * 64
    current = ncnn_recipe(model_digest)
    legacy = ncnn_recipe(model_digest, legacy_direct=True)
    store = AssetStore(workspace)
    importer = LegacyCacheImporter(workspace, store)

    metadata = {
        "id": workspace.name,
        "backend": "ai",
        "recipe": "realesrgan-anime-alpha-v1-4x",
        "model_digest": model_digest,
    }
    report = importer.import_workspace(metadata, current, ai_recipe=current)

    assert report.direct_ai_cache_reused == 1
    assert report.jobs_queued == 1
    assert store.get_job(source_id, legacy.key).state == "READY"
    assert store.get_job(source_id, current.key).state == "QUEUED"


def test_legacy_ai_cache_without_model_provenance_is_not_adopted(tmp_path):
    workspace = tmp_path / "game-ai-unknown"
    image = _image(workspace / "original-assets" / "placeholder.png")
    source_id = image_source_id(image)
    original = workspace / "original-assets" / "placeholder.png"
    original.rename(original.with_name(source_id + ".png"))
    old_texture = workspace / "textures-ai" / (source_id + ".png")
    _image(old_texture, (image.width * 4, image.height * 4))
    current = ncnn_recipe("b" * 64)
    store = AssetStore(workspace)
    importer = LegacyCacheImporter(workspace, store)

    report = importer.import_workspace(
        {"id": workspace.name, "backend": "ai", "recipe": "realesrgan-anime-alpha-v1-4x"},
        current,
        ai_recipe=current,
    )

    assert report.direct_ai_cache_reused == 0
    assert report.jobs_queued == 1
    assert report.legacy_unknown_cache_files == 1
    assert store.get_job(source_id, current.key).state == "QUEUED"
    assert old_texture.is_file()


def test_pixel_backend_accepts_only_its_canonical_recipe():
    backend = PixelBackend(lambda image: image.resize((image.width * 4, image.height * 4)))
    source = Image.new("RGBA", (2, 3))
    result = backend.enhance(source, pixel_recipe(), Path("."))
    assert result.size == (8, 12)
    wrong = Recipe(1, "scale2x-rgba", "other-version", "", 4,
                   "transparent-rgb-zero-v1", "edge-repeat-v1", {"passes": 2})
    with pytest.raises(ValueError):
        backend.enhance(source, wrong, Path("."))


@pytest.mark.parametrize("unsafe", ["../outside.png", "C:/outside.png", "textures/../../x.png"])
def test_store_rejects_paths_that_can_escape_workspace(tmp_path, unsafe):
    store = AssetStore(tmp_path)
    with pytest.raises(ValueError):
        store.resolve_path(unsafe)


def _ready_redraw_job(tmp_path, size=(8, 8), color=(10, 200, 90, 255)):
    from runtime_assets.publication import publish_ready
    from runtime_assets.recipes import ArtworkRecipes
    from runtime_assets.work_state import _Game

    recipes = ArtworkRecipes()
    recipe, _ = recipes.for_backend("redraw")
    store = AssetStore(tmp_path)
    store.register_recipe(recipe)
    image = Image.new("RGBA", size, color)
    record = _source(image)
    (tmp_path / "original-assets").mkdir(exist_ok=True)
    image.save(tmp_path / "original-assets" / f"{record.source_id}.png")
    store.register_source(record)
    output = f"textures-cache/{recipe.key}/{record.source_id}.png"
    path = store.resolve_path(output)
    path.parent.mkdir(parents=True, exist_ok=True)
    Image.new("RGBA", (size[0] * 4, size[1] * 4), color).save(path)
    assert store.mark_imported_ready(record.source_id, recipe.key, output)
    game = _Game(tmp_path, store, recipe)
    manifest = tmp_path / ".runtime" / "ready" / f"{recipe.key}.json"
    return recipe, store, game, manifest, record, path


def test_publish_ready_skips_rewrite_when_nothing_changed(tmp_path):
    recipe, store, game, manifest, record, path = _ready_redraw_job(tmp_path)
    from runtime_assets.publication import publish_ready
    from runtime_assets.store import AssetStore as _Store

    publish_ready(game)
    first = manifest.read_bytes()
    assert json.loads(first)["sources"] == [record.source_id]

    publish_ready(game)
    assert manifest.read_bytes() == first

    path.write_bytes(b"not a png")
    publish_ready(game)
    second = json.loads(manifest.read_text(encoding="utf-8"))
    assert second["sources"] == []
    assert _Store(tmp_path).get_job(record.source_id, recipe.key).state == "QUEUED"


def test_publish_ready_bounds_verification_on_a_first_large_publish(tmp_path, monkeypatch):
    """A first publish must not decode a whole catalogue before the app responds."""
    from runtime_assets import publication
    from runtime_assets.recipes import ArtworkRecipes
    from runtime_assets.work_state import _Game

    recipe, _ = ArtworkRecipes().for_backend("redraw")
    store = AssetStore(tmp_path)
    store.register_recipe(recipe)
    (tmp_path / "original-assets").mkdir()
    for index in range(200):
        image = Image.new("RGBA", (4, 4), (index, 40, 90, 255))
        record = SourceRecord(image_source_id(image), 4, 4,
                              f"original-assets/{index}.png", "legacy-capture")
        image.save(tmp_path / record.relative_path)
        store.register_source(record)
        store.enqueue(record.source_id, recipe.key)
        output = f"textures-cache/{recipe.key}/{record.source_id}.png"
        target = store.resolve_path(output)
        target.parent.mkdir(parents=True, exist_ok=True)
        Image.new("RGBA", (16, 16), (index, 40, 90, 255)).save(target)
        assert store.mark_imported_ready(record.source_id, recipe.key, output)

    checked = []
    original = publication._verify_output
    monkeypatch.setattr(publication, "_verify_output",
                        lambda game, row: checked.append(row.source_id) or original(game, row))
    game = _Game(tmp_path, store, recipe)
    publication.publish_ready(game)

    manifest = json.loads((tmp_path / ".runtime" / "ready" / f"{recipe.key}.json")
                          .read_text(encoding="utf-8"))
    assert len(manifest["sources"]) == 200
    assert len(checked) <= publication._VERIFY_SAMPLE


def test_publish_compatibility_reads_sources_in_one_query(tmp_path):
    """The legacy copy pass must not open a database read per ready image."""
    from threading import Lock

    from runtime_assets.publication import publish_compatibility
    from runtime_assets.recipes import ArtworkRecipes
    from runtime_assets.work_state import _Game

    recipe, _ = ArtworkRecipes().for_backend("redraw")
    store = AssetStore(tmp_path)
    store.register_recipe(recipe)
    (tmp_path / "original-assets").mkdir()
    for index in range(40):
        image = Image.new("RGBA", (4, 4), (index, 10, 60, 255))
        record = SourceRecord(image_source_id(image), 4, 4,
                              f"original-assets/{index}.png", "legacy-capture")
        image.save(tmp_path / record.relative_path)
        store.register_source(record)
        store.enqueue(record.source_id, recipe.key)
        output = f"textures-cache/{recipe.key}/{record.source_id}.png"
        target = store.resolve_path(output)
        target.parent.mkdir(parents=True, exist_ok=True)
        Image.new("RGBA", (16, 16), (index, 10, 60, 255)).save(target)
        assert store.mark_imported_ready(record.source_id, recipe.key, output)

    def forbidden(_source_id):
        raise AssertionError("compatibility publishing must not look up sources one by one")

    store.get_source = forbidden
    game = _Game(tmp_path, store, recipe)
    publish_compatibility(game, limit=8, commit_lock=Lock())
    assert len(list((tmp_path / "textures-redraw").glob("*.png"))) == 8

    # A restart must resume from the files already copied, not overwrite the
    # first eight rows forever because the completion marker was never written.
    restarted = _Game(tmp_path, store, recipe)
    publish_compatibility(restarted, limit=8, commit_lock=Lock())
    assert len(list((tmp_path / "textures-redraw").glob("*.png"))) == 16


def test_mark_ready_batch_uses_one_transaction(tmp_path, monkeypatch):
    from runtime_assets.recipes import ArtworkRecipes

    recipe, _ = ArtworkRecipes().for_backend("redraw")
    store = AssetStore(tmp_path)
    store.register_recipe(recipe)
    (tmp_path / "original-assets").mkdir()
    for index in range(5):
        image = Image.new("RGBA", (4, 4), (index, 3, 7, 255))
        record = SourceRecord(image_source_id(image), 4, 4,
                              f"original-assets/{index}.png", "legacy-capture")
        image.save(tmp_path / record.relative_path)
        store.register_source(record)
        store.enqueue(record.source_id, recipe.key)
    leases = store.claim_batch("test-owner", recipe_key=recipe.key, max_count=5)
    assert len(leases) == 5

    entries = []
    for lease in leases:
        output = f"textures-cache/{recipe.key}/{lease.source_id}.png"
        target = store.resolve_path(output)
        target.parent.mkdir(parents=True, exist_ok=True)
        Image.new("RGBA", (16, 16), (1, 2, 3, 255)).save(target)
        entries.append((lease, output))

    transactions = []
    original = store._transaction

    def counted():
        transactions.append(1)
        return original()

    monkeypatch.setattr(store, "_transaction", counted)
    assert store.mark_ready_batch(entries) == 5
    assert len(transactions) == 1
    states = {store.get_job(lease.source_id, recipe.key).state for lease in leases}
    assert states == {"READY"}
    assert store.mark_ready_batch(entries) == 0


def test_redraw_batch_budget_prefers_throughput(monkeypatch):
    from runtime_assets import resources
    from runtime_assets.backends import RedrawBackend

    monkeypatch.setattr(resources, "host_memory",
                        lambda: {"available_bytes": 8 * 1024**3, "logical_cpus": 9})
    budget = RedrawBackend().batch_limits()
    assert (budget.max_images, budget.max_pixels, budget.processing_threads) == (1024, 1_048_576, 8)

    monkeypatch.setattr(resources, "host_memory",
                        lambda: {"available_bytes": 0, "logical_cpus": 1})
    budget = RedrawBackend().batch_limits()
    assert (budget.max_images, budget.processing_threads) == (1, 1)
