# Asset discovery and persistence

Current contracts, 26 November 2026. See [Project flow](PROJECT_FLOW.md) for playback and [Status](STATUS.md) for coverage limits.

## Java ME sources

Import reads recognized data without executing the game. Runtime hooks let the game decode its own resources and observe eligible images afterward.

| Source | Contract |
|---|---|
| Standard images | Decode with Pillow through the common registry |
| Embedded/header-swapped PNGs | Validate signature, complete chunks, CRCs and decoded bounds |
| Recognized packed containers/sprites | Validate indexes, lengths, decompression and palette/frame limits |
| Explicit raw indexed layouts | Require dimensions, palette and valid pixel indexes |
| Runtime draws | Observe eligible immutable images, regions, offscreen snapshots or sampled RGB data |

Readers live in `systems/jme/resources/`; normalized images pass through `AssetSink`. Pixel identity deduplicates source images and recipe jobs. Keep archive/module/frame/palette provenance; archive paths must not become output paths. Unknown formats stay opaque. Coverage of recognized containers does not mean every resource or future scene is decoded.

`asset-decoding.json` records source/pipeline identity and decoder results. `catalogue.json` retains source provenance; `original-assets/` stores normalized inputs. SQLite owns job state, while validated outputs and ready manifests feed the player. Recipe changes reuse source pixels without relabeling old outputs.

## Updates and recovery

Java ME capture events register sources directly. A successful validated conversion records its result in SQLite before publication. Available decoded Java ME artwork can be queued before play; live work is admitted only through resource/performance guards.

There is no automatic repeated full original-image catalog scan. Event polling, database recovery and bounded integrated NES ingestion still run where enabled. A missed or legacy file can be recovered through import/reconciliation or **Artwork > Sync** while players are closed. Sync scans the selected game's originals/pending captures and deduplicates existing entries. Interrupted jobs recover from stored rows; completed outputs are reused.

## NES sources

The Rust player captures tiles and palettes into bounded chunks. `systems/nes/` converts them into source images plus bindings; the shared queue prepares artwork. After play, ready CHR-ROM replacements can be published into an HD pack for a later launch. The loaded pack is fixed for that session. General CHR-RAM tile enhancement remains limited.

Pose capture is a separate contract: the optional native observer retains original sprite bytes and palettes at fetch time, including RAM-held patterns. It is bounded to 128 complete eight-pixel cells per frame. Journal version 2 includes optional per-cell palettes and camera data; version 1 remains readable. Pose candidates are not proof of player identity. See [current limitations](STATUS.md).

## Standalone extraction

```powershell
python -m tools.decode_jme_artwork game.jar build/decoded-art
```

For raw palette indexes, pass `--indexed-layout layout.json` with an array of entries such as:

```json
[{"resource":"art.bin","palette":"palette.bin","width":32,"height":24,"frames":3,"transparent_index":0}]
```

Pixels index RGB triples; frames are contiguous and row-major. `frames` defaults to one and transparency is optional. Add shared readers only when structure and bounds establish the format; keep decoding independent from the enhancer and renderer.
