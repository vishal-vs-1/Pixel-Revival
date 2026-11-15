# Project flow

Pixel Revival runs two emulators from one Windows library. Both systems share artwork preparation and use system-specific presentation paths.

The [experiment history](EXPERIMENTS.md) records the trials and decisions that led to these paths.

~~~mermaid
flowchart TD
    A[Launcher and system switch] --> B[Java ME library and JAR import]
    A --> C[NES library and ROM import]
    B --> D[Patched FreeJ2ME-Plus]
    C --> E[Managed MesenCE]
    B --> F[Supported resources from the JAR]
    D --> G[Stable images discovered during play]
    E --> H[Tile capture during NES Enhanced play]
    F --> I[Shared source and recipe cache]
    G --> I
    H --> I
    I --> J[Fast Pixel or local AI Cartoon]
    J --> K[Java ME texture replacement]
    J --> L[NES pack for a later launch]
    K --> D
    L -->|Mesen loads the pack on the next launch| E
    D --> N[Java ME compatibility presenter]
    E -->|Composed NES frame| M[Shared GPU presentation]
~~~

## System paths

| Responsibility | Java ME | NES |
| --- | --- | --- |
| Import | `systems/jme/library.py` validates and copies a JAR. Recognized image resources are discovered during import. | `systems/nes/library.py` copies a validated cartridge into the NES library and records its hash and mapper metadata. |
| Playback | `systems/jme/playback.py` starts patched FreeJ2ME-Plus. Java ME offers GPU and CPU compatibility paths. | `systems/nes/runtime.py` starts managed MesenCE with a private launch manifest. Original uses the stock presenter. Enhanced loads a prepared HD pack and uses shared GPU presentation. |
| Discover artwork | Eligible decoded images can enter the artwork queue while the game runs. | Enhanced observes tiles and palettes during play. The artwork service prepares replacements after the player closes. |
| Apply artwork | The Java renderer looks up prepared textures by source identity and recipe. | MesenCE draws matching tiles from the staged HD pack while composing the frame. |
| Controls | The player receives the selected Java ME binding snapshot. | The managed session receives the selected NES binding snapshot. |

## Java ME character artwork

NES and Java ME use the same **Playable sprite** panel in Artwork. **Play & record poses** collects candidate artwork. NES can also detect player poses through a controller replay or scan a checkpoint. In Java ME, open **Review captured artwork**, preview each candidate, and mark **This is a player pose** for the character's frames. Movement suggestions are candidates for review. The default image applies only to confirmed player poses. Individual images take priority until you choose **Use default for all**. Captured terrain, effects, and other artwork stay outside the replacement set unless explicitly selected.

## How NES artwork reaches the screen

Pixel Revival creates a Mesen HD pack from captured NES tiles. MesenCE reads that pack and uses its replacement images when composing sprites and backgrounds. The shared renderer receives the finished frame and fits it to the player window.

1. **Capture during Enhanced play.** The NES bridge observes 8×8 tiles and the palettes used to draw them. It records a tile index for cartridge graphics stored in ROM, or the pattern bytes for graphics held in RAM. A first launch can use a transparent placeholder pack to enable capture while the visible artwork stays original.
2. **Prepare replacements after play.** The capture adapter converts observed tiles into original RGBA PNGs and saves their tile and palette bindings. The shared artwork service runs the selected recipe when the player is closed. Fast Pixel applies Scale2x twice to an 8×8 source. AI Cartoon uses Real-ESRGAN. Both cache 32×32 outputs at 4×.
3. **Build the pack.** The generator puts ready replacements into PNG atlas pages. Its `hires.txt` maps each tile and palette to a rectangle on a page. A 4× pack uses the cached tile size, and a 2× pack derives 16×16 tiles from those same cached results. Publication makes one complete pack available for a later launch.
4. **Stage and draw on the next Enhanced launch.** The launcher selects a pack snapshot. The managed runtime validates it and stages `hires.txt` and its PNGs in the session's private `HdPacks` folder before loading the ROM. MesenCE's HD video filter looks up each tile as it draws the frame. A matching tile uses the replacement, and a tile without a match uses its original pixels. The loaded pack is fixed for that session.
5. **Present the frame.** The NES bridge hands the composed frame to the shared graphics DLL. A 4× pack can produce a 1024×960 frame from the nominal 256×240 NES image, before window fitting or any overscan adjustment. The presenter uses the frame at the scale Mesen supplied and performs the final display sizing.

**Play Original** leaves HD packs disabled and uses Mesen's stock presentation path. The imported ROM remains intact in both modes.

The embedded Mesen host in `systems/nes/embedded/` is a separate experiment. It requires an opt-in runtime build and the `J2ME_REVIVAL_NES_INPROCESS=1` setting. The supported default uses the managed Mesen process. Both routes receive Mesen's composed frame. The current pose feature observes sprite tiles and layout data to build replacement packs.

## Playable sprite replacement

**NES → Artwork → Detect player** runs short, isolated Mesen replays. The detector compares sprite positions under neutral and directional input to find the controllable character. It records each observed pose as a layout of sprite tiles, palettes, and flips. The initial replays are verified for specific cartridge revisions of Super Mario Bros., Donkey Kong, and Road Fighter.

| Action | Result |
| --- | --- |
| **Detect player** | Runs a replay from startup and opens the image picker when detection succeeds. |
| **Choose image** | Assigns an image to the selected pose or sets the default for every unassigned pose. |
| **Scan latest** | Uses the newest Mesen close checkpoint or save slot for the selected game. It adds newly detected poses and keeps existing assignments. |
| **Choose save state** | Scans a particular Mesen save slot when you want to capture an earlier moment. |
| **Play & record poses** | Records new sprite layouts during play and adds them after the player exits. A recorded form stays in the catalog even if the character changes form again. |
| **Play with image** | Uses uploaded artwork. NES builds a private HD pack with the game's controls and save folders. Java ME uses the existing draw-time replacement hooks. |
| **Regenerate all poses** | Draws all currently detected NES poses or confirmed Java ME poses offline and saves them as a separate artwork set. |
| **Play with regenerated poses** | Starts the game using that artwork set and fits each drawing to the original pose's visible height. |

The pack matches the sprite layout at its current position and checks larger layouts first. A pose-specific image takes priority over the default. Replacement artwork matches the original pose's visible height. Wide images are narrowed to fit its width. If the layout or image is missing, Mesen draws the original sprite. The checkpoint flow was verified with Mario's 16×32 powered-up form. The mushroom pickup transition and real ladder play still need verification.

The regenerated set uses black outlines and stronger colors. Uploaded images have their own playback button. Run regeneration again after detecting or confirming more poses. Unmatched poses use the original sprite.

The renderer also accepts standalone sprite PNGs with `python -m runtime_assets.sprite_paint input.png output.png`.

### Computer-controlled enemies

Enemy replacement can reuse the sprite observations, pose previews, image assignments, and HD pack renderer. It needs a detector that tracks tiles moving together across frames and distinguishes enemies from the player, moving platforms, projectiles, and effects. Several enemies can share an identical pose. The current HD pack rules can replace that pose wherever it matches, so targeting one individual would need additional instance information. Enemy detection is a future extension.

## Shared components

| Package | Responsibility |
| --- | --- |
| `app.py`, `launcher/` | Start the app and bind the library, settings, artwork, and player views. |
| `systems/application.py`, `systems/jme/`, `systems/nes/` | Route requests by game system. Each system owns imports, pose discovery, and its player launch. |
| `runtime_assets/` | Schedule conversion, run Scale2x or local Real-ESRGAN, and publish ready artwork by source and recipe. It also contains the standalone pose drawing algorithm. |
| `systems/generated_poses.py` | Store complete generated pose sets and verify that they match the current pose catalog before play. |
| `storage/`, `controls/` | Resolve portable paths, retain saves during deletion, and own keyboard profile data. Java ME and NES have separate libraries and profiles. |
| `graphics/`, `native-graphics/` | Resolve graphics settings and runtime builds. The shared native renderer handles NES Enhanced presentation, and the Java JNI shim is retained for diagnostics. |
| `emulator-src/`, `nes-bridge/`, `tools/patches/` | Hold the Java integration, native and managed NES bridge, and build-time patches for the pinned upstream sources. |

Both systems retain their imported game files. Enhancements are generated from known original artwork and can be reused across launches. Unsupported or uncaptured artwork follows the original rendering path. See [asset discovery](ASSET_DECODING.md) for details and the [development guide](DEVELOPMENT.md) for the source and test layout.
