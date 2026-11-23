# Asset discovery and decoder contracts

Java ME import and play use the same generic asset pipeline. Import reads standard images, embedded PNGs, PNGs with a pair-swapped header, and supported palette-based sprites from validated containers. At runtime, emulator hooks observe decoded immutable images, stable offscreen snapshots, and sampled raw RGB blocks. The game continues to run its own decoder.

| Responsibility | Implementation | Contract |
|---|---|---|
| Select common import readers | `systems.jme.resources.decoding.DecoderRegistry` | Readers accept recognized, bounded formats. Import reads data only. |
| Normalize and persist source images | `AssetSink`, `runtime_assets` | Pixel identity, dimensions, alpha policy, and provenance are shared by import and runtime discovery. Archive paths never become output paths. |
| Read ordinary images | `StandardImages` | Pillow decodes supported images without executing game code. |
| Extract embedded PNGs | `EmbeddedImages` | Complete chunk boundaries and CRCs must validate. |
| Restore a pair-swapped PNG header | `SwappedHeaderImages` | The stored signature and header must match. The restored PNG must decode and its chunk CRCs must validate. |
| Unpack a recognized container | `systems.jme.resources.gameloft` | Index tables, bounds, and decompressed output are validated and limited. Unknown payloads stay opaque. |
| Extract packed Gameloft sprites | `GameloftSpriteImages` | Parse the declared module, frame, animation and palette tables. Decode supported indexed, run-length and alpha streams and preserve archive, block, module and palette provenance. |
| Decode raw 8-bit palette indices offline | `IndexedImages` | An explicit layout identifies dimensions, frame count, RGB palette, and any transparent palette index. Payload lengths and index ranges must match. |
| Enhance prepared or observed images | `AssetService` and backend implementations | Work is deduplicated by source identity and recipe, persists across launches, and runs outside the game/render thread. |
| Display cached artwork | `TextureRepository`, `GpuRenderer`, `Enhancement` | Async cache lookup; updates are staged at frame boundaries and rejected when source revision or recipe is stale. |

## Persistence

`asset-decoding.json` records the JAR hash, pipeline version, registered common readers, recognized stages, and unresolved resource information. `catalogue.json` records normalized sources and their provenance. `original-assets/` contains source PNGs; `textures-cache/` and the ready manifest contain validated recipe-specific outputs. Legacy source images remain valid inputs even when they were produced by an older extractor.

Source preparation is reused only when the JAR, reader versions, and indexed files still match. Enhancement recipe changes reuse known source pixels and create separate cached variants. Historical cache files with unknown provenance remain legacy variants; they are not silently relabeled as results of a new recipe.

Adding the same JAR again or preparing its artwork updates a catalogue created before the packed sprite reader. Sprite palette variants are included in the catalogue, and identical decoded pixels share one PNG and one job per recipe. The decoder limits each block and each import by pixel count and image count. `asset-decoding.json` reports decoded, empty, unsupported and budget-limited sprite blocks under `features.gameloft_sprites`.

AI Cartoon prepares ordinary image sources immediately. When an import contains at least 512 packed sprite records, it indexes every decoded sprite and queues packed-only sprites when Enhanced Play observes those pixels. The Artwork status shows how many sources are available on demand. Existing textures and observed sources retain their priority. AI batches for these catalogues are capped at 32 images so pause and playback can take effect between batches.

## Standalone extraction

Run this from the project root to extract artwork independently of the launcher and emulator.

```powershell
python -m tools.decode_jme_artwork game.jar build/decoded-art
```

Supply the dimensions and palette for raw pixel indices with `--indexed-layout layout.json`. The JSON file contains an array of entries.

```json
[
  {
    "resource": "art.bin",
    "palette": "palette.bin",
    "width": 32,
    "height": 24,
    "frames": 3,
    "transparent_index": 0
  }
]
```

Each pixel byte indexes an RGB triple. Frames are contiguous and row-major. `frames` defaults to one, and omitting `transparent_index` makes all entries opaque. The standalone command accepts layouts and writes PNGs with the artwork pipeline's standard source IDs.

## Coverage and extension

This pipeline recognizes stored images and supported sprite modules, including modules for scenes that have yet to be played. Composed frames, unsupported binary layouts and effects can still be discovered during play. Packed binary data may also represent maps or sound. Continuously changing screen buffers remain on the fast render path.

Add new shared readers only for formats validated by structure and bounds. Emit normalized images through `AssetSink`; preserve animation and frame provenance without changing game coordinates. Keep format support independent from the upscaler, renderer, UI, and input handling.

## NES tile discovery

NES Enhanced observes tiles and palettes from the Rust core. The player writes bounded capture chunks and the NES adapter turns those records into source PNGs and tile bindings. The same shared artwork queue prepares Fast Pixel or AI Cartoon outputs. Once the player closes, the pack generator can publish ready replacements for the next Enhanced launch. Pack bindings preserve tile and palette identity. Coverage depends on the scenes captured during play.
