# Project flow

Pixel Revival runs two emulators from one Windows library. Both systems share artwork preparation and the native GPU presentation implementation for Enhanced play.

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
    D --> M[Shared GPU presentation]
    E -->|Composed NES frame| M
~~~

## System paths

| Responsibility | Java ME | NES |
| --- | --- | --- |
| Import | `engine.py` validates and copies a JAR. Recognized image resources are discovered during import. | `systems/nes/library.py` copies a validated cartridge into the NES library and records its hash and mapper metadata. |
| Playback | Patched FreeJ2ME-Plus runs the game. Java ME offers shared GPU, Legacy GPU, and CPU compatibility paths. | A private launch manifest starts managed MesenCE. Original uses the stock presenter. Enhanced loads a prepared HD pack and uses shared GPU presentation. |
| Discover artwork | Eligible decoded images can enter the artwork queue while the game runs. | Enhanced observes tiles and palettes during play. The artwork service prepares replacements after the player closes. |
| Apply artwork | The Java renderer looks up prepared textures by source identity and recipe. | MesenCE draws matching tiles from the staged HD pack while composing the frame. |
| Controls | The player receives the selected Java ME binding snapshot. | The managed session receives the selected NES binding snapshot. |

## How NES artwork reaches the screen

Pixel Revival creates a Mesen HD pack from captured NES tiles. MesenCE reads that pack and uses its replacement images when composing sprites and backgrounds. The shared renderer receives the finished frame and fits it to the player window.

1. **Capture during Enhanced play.** The NES bridge observes 8×8 tiles and the palettes used to draw them. It records a tile index for cartridge graphics stored in ROM, or the pattern bytes for graphics held in RAM. A first launch can use a transparent placeholder pack to enable capture while the visible artwork stays original.
2. **Prepare replacements after play.** The capture adapter converts observed tiles into original RGBA PNGs and saves their tile and palette bindings. The shared artwork service runs the selected recipe when the player is closed. Fast Pixel applies Scale2x twice to an 8×8 source. AI Cartoon uses Real-ESRGAN. Both cache 32×32 outputs at 4×.
3. **Build the pack.** The generator puts ready replacements into PNG atlas pages. Its `hires.txt` maps each tile and palette to a rectangle on a page. A 4× pack uses the cached tile size, and a 2× pack derives 16×16 tiles from those same cached results. Publication makes one complete pack available for a later launch.
4. **Stage and draw on the next Enhanced launch.** The launcher selects a pack snapshot. The managed runtime validates it and stages `hires.txt` and its PNGs in the session's private `HdPacks` folder before loading the ROM. MesenCE's HD video filter looks up each tile as it draws the frame. A matching tile uses the replacement, and a tile without a match uses its original pixels. The loaded pack is fixed for that session.
5. **Present the frame.** The NES bridge hands the composed frame to the shared graphics DLL. A 4× pack can produce a 1024×960 frame from the nominal 256×240 NES image, before window fitting or any overscan adjustment. The presenter uses the frame at the scale Mesen supplied and performs the final display sizing.

**Play Original** leaves HD packs disabled and uses Mesen's stock presentation path. The imported ROM remains intact in both modes.

## Shared components

- `app.py` and `systems/application.py` route library operations to the active system. Browsing filters the loaded catalog, and cover preparation runs in the background.
- `runtime_assets/` validates sources, schedules work, and caches outputs by original pixel identity and artwork recipe. Fast Pixel runs deterministic Scale2x. AI Cartoon uses local Real-ESRGAN through NCNN and Vulkan.
- `native-graphics/` implements the shared graphics ABI, texture operations, and frame presentation. The Java player reaches it through a JNI shim, and the NES bridge calls it from the managed runtime's native core.
- `graphics/` validates runtime manifests and launch policy. `emulator-src/` contains the Java integration, and `nes-bridge/` contains the native and managed NES adapter.
- `game_storage.py` and the system libraries own game data paths. Java ME and NES have separate libraries and control profile files. Each game stores its own profile choice, and deletion keeps saves.

Both systems retain their imported game files. Enhancements are generated from known original artwork and can be reused across launches. Unsupported or uncaptured artwork follows the original rendering path. See [asset discovery](ASSET_DECODING.md) for details and the [development guide](DEVELOPMENT.md) for the source and test layout.
