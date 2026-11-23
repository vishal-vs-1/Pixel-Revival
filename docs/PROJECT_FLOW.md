# Project flow

Pixel Revival runs two emulators from one Windows library. Both systems share artwork preparation and use system-specific presentation paths.

The [experiment history](EXPERIMENTS.md) records the trials and decisions that led to these paths.

## General artwork preparation

Both systems can use Fast Pixel, AI Cartoon, or Redraw (no outlines) to prepare artwork. The worker saves results for the current game and recipe. Each player consumes its own game's results. While the game is closed, Redraw can process several assets at once using up to four CPU workers, subject to available memory.

~~~mermaid
flowchart TD
    A["General artwork"] -->|Java ME| J["Images from the JAR or gameplay"]
    A -->|NES| N["Tiles captured during Enhanced play"]
    J --> JR["Prepare with Fast Pixel, AI Cartoon, or Redraw (no outlines)"]
    JR --> JC["Cache this game's Java ME textures"]
    JC --> JP["FreeJ2ME applies ready textures during Enhanced play"]
    JP --> JD["Java ME GPU or CPU compatibility presenter"]
    N --> NR["Prepare with Fast Pixel, AI Cartoon, or Redraw (no outlines) after close"]
    NR --> NP["Build this game's NES HD pack"]
    NP --> NN["Rust compositor paints replacements on a later Enhanced frame"]
    NN --> ND["In-tool Rust NES window"]
~~~

Original play skips general artwork replacement. Java ME plays at its original artwork scale through its compatibility player. NES plays original frames in-tool on the Rust core. With **Enhance play** checked, the same in-tool player composites the game's prepared pack; when no pack is ready it shows scaled original frames instead of failing.

## System paths

| Responsibility | Java ME | NES |
| --- | --- | --- |
| Import | `systems/jme/library.py` validates and copies a JAR. Recognized image resources are discovered during import. | `systems/nes/library.py` copies a validated cartridge into the NES library and records its hash and mapper metadata. |
| Normal playback | `systems/jme/playback.py` starts patched FreeJ2ME-Plus. Java ME offers GPU and CPU compatibility paths. | `systems/nes/rust_player.py` runs the cartridge in our own Tk window on the vendored Rust core (`rust-nes/mesen-player`). |
| Discover artwork | Eligible decoded images can enter the artwork queue while the game runs. | The Rust player observes tiles and palettes during Enhanced play. The artwork service prepares replacements after the player closes. |
| Apply artwork | The Java renderer looks up prepared textures by source identity and recipe. | The Rust compositor looks up each recorded tile in the staged HD pack and paints its replacement over the scaled native frame. |
| Controls | The player receives the selected Java ME binding snapshot. | The Rust player receives the selected NES binding snapshot; both comparison panes share the same keys. |

## How NES artwork reaches the screen

Pixel Revival creates a Mesen-compatible HD pack from captured NES tiles. The Rust compositor reads that pack and paints replacements over the scaled original frame.

1. **Capture during Enhanced play.** The Rust player observes 8×8 tiles and the palettes used to draw them. It records a tile index for cartridge graphics stored in ROM; RAM-held graphics stay on the original path in this version. A first launch can use an empty pack so capture runs while the visible artwork stays original.
2. **Prepare replacements after play.** The capture adapter converts observed tiles into original RGBA PNGs and saves their tile and palette bindings. The shared artwork service runs the selected recipe when the player is closed. Fast Pixel applies Scale2x twice to an 8×8 source. AI Cartoon uses Real-ESRGAN. Redraw uses the pixel tracer's color fills. All three cache 32×32 outputs at 4×.
3. **Build the pack.** The generator puts ready replacements into PNG atlas pages. Its `hires.txt` maps each tile and palette to a rectangle on a page. A 4× pack uses the cached tile size, and a 2× pack derives 16×16 tiles from those same cached results. Publication makes one complete pack available for a later launch.
4. **Draw on the next Enhanced launch.** The in-tool Rust player validates the pack pointer, loads each tile's art into its replacement table, and composites every frame: it scales the original frame, then paints each recorded tile whose art is present. A matching tile uses the replacement, and a tile without a match keeps its original pixels. The loaded pack is fixed for that session.
5. **Present the frame.** A 4× pack produces a 1024×960 frame from the nominal 256×240 NES image. The window opens maximized and fits the frame at a whole-number scale when the screen allows (otherwise it resizes smoothly), centered with 4:3 preserved; **F11** toggles fullscreen.

**Play** with **Enhance play** unchecked leaves HD packs disabled and shows original frames in our window. With it checked, the Rust player loads the prepared pack and composites replacements in-tool; no pack means scaled original frames. The imported ROM remains intact in both modes.

The Rust core renders original and enhanced frames directly. The Python process loads our `mesen_player.dll`; there is no external emulator process and no managed fallback. If the DLL is not built, NES playback stays disabled while the library and saved settings remain usable.

## Side-by-side playback

Comparison opens an independent Original and Enhanced player. For NES both are Rust panes in one in-tool window, with two private save copies and shared keyboard controls; Enhanced uses the prepared cache. Java ME keeps its existing embedded pair. Controls stay independent between panes.

## Playable sprite replacement

The **Playable sprite** panel is available for Java ME. This feature fits a replacement to a known character pose. **Play with image** uses the uploaded images, and **Play with regenerated poses** uses the separate set made by the offline pose renderer.

~~~mermaid
flowchart TD
    J["Java ME recording of image regions or drawn sprites"] --> C["Review and confirm the player poses"]
    C --> P["Player poses for the selected game"]
    P --> U["Assign a default image or an image per pose"]
    P --> R["Regenerate all poses with the offline renderer"]
    U --> UP["Play with image"]
    R --> RP["Play with regenerated poses"]
    UP --> F["Fit the replacement to the original visible bounds"]
    RP --> F
    F --> JP["FreeJ2ME substitutes the pose at draw time"]
~~~

In Java ME, open **Review captured artwork** and mark **This is a player pose** on the character's frames. Movement suggestions help with the review. The default image applies only to confirmed poses. Individual uploads take priority until you choose **Use default for all**. Captured terrain and effects stay outside the replacement set unless explicitly selected.

| Action | Result |
| --- | --- |
| **Choose image** | Assigns an image to the selected pose or sets the default for every unassigned pose. |
| **Play** with **Enhance play** checked | Records new sprite layouts during play and adds them after the player exits. A recorded form stays in the catalog even if the character changes form again. |
| **Play with image** | Uses uploaded artwork through Java ME's draw-time replacement hooks. |
| **Regenerate all poses** | Draws all confirmed Java ME poses offline and saves them as a separate artwork set. |
| **Play with regenerated poses** | Starts the game using that artwork set and fits each drawing to the original pose's visible height. |

The regenerated set uses black outlines and stronger colors. Uploaded images have their own playback button. Run regeneration again after recording or confirming more poses. Unmatched poses use the original sprite.

The renderer also accepts standalone sprite PNGs with `python -m runtime_assets.sprite_paint input.png output.png`.

### Computer-controlled enemies

Enemy replacement would need a detector that tracks tiles moving together across frames and distinguishes enemies from the player, moving platforms, projectiles, and effects. Several enemies can share an identical pose, so targeting one individual would need additional instance information. Enemy detection is a future extension; the current NES pipeline replaces tiles through the prepared HD pack.

## Shared components

| Package | Responsibility |
| --- | --- |
| `app.py`, `launcher/` | Start the app and bind the library, settings, artwork, and player views. |
| `systems/application.py`, `systems/jme/`, `systems/nes/` | Route requests by game system. Each system owns imports, artwork discovery, and its player launch. |
| `runtime_assets/` | Schedule conversion, run Scale2x or local Real-ESRGAN, and publish ready artwork by source and recipe. It also contains the standalone pose drawing algorithm. |
| `systems/generated_poses.py` | Store complete generated pose sets and verify that they match the current pose catalog before play. |
| `storage/`, `controls/` | Resolve portable paths, retain saves during deletion, and own keyboard profile data. Java ME and NES have separate libraries and profiles. |
| `graphics/`, `native-graphics/` | Resolve Java ME graphics settings and runtime builds. NES renders through its own Rust compositor and window. |
| `emulator-src/`, `tools/patches/` | Hold the Java ME integration sources and build-time patches for the pinned upstream player. |

Both systems retain their imported game files. Enhancements are generated from known original artwork and can be reused across launches. Unsupported or uncaptured artwork follows the original rendering path. See [asset discovery](ASSET_DECODING.md) for details and the [development guide](DEVELOPMENT.md) for the source and test layout.

The NES window is our own Tk window presenting the Rust core; no managed Mesen process, C# bridge, or external player is part of the runtime.
