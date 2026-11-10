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
    L --> E
    D --> M[Shared GPU presentation]
    E --> M
~~~

## System paths

| Responsibility | Java ME | NES |
| --- | --- | --- |
| Import | `engine.py` validates and copies a JAR. Recognized image resources are discovered during import. | `systems/nes/library.py` copies a validated cartridge into the NES library and records its hash and mapper metadata. |
| Playback | Patched FreeJ2ME-Plus runs the game. Java ME offers shared GPU, Legacy GPU, and CPU compatibility paths. | A private launch manifest starts managed MesenCE. Original uses the stock presenter. Enhanced loads a prepared HD pack and uses shared GPU presentation. |
| Discover artwork | Eligible decoded images can enter the artwork queue while the game runs. | Enhanced observes tiles and palettes during play. The artwork service prepares replacements after the player closes. |
| Apply artwork | The Java renderer looks up prepared textures by source identity and recipe. | The NES pack generator publishes a complete pack generation. A launch stages that pack for the session. |
| Controls | The player receives the selected Java ME binding snapshot. | The managed session receives the selected NES binding snapshot. |

## Shared components

- `app.py` and `systems/application.py` route library operations to the active system. Browsing filters the loaded catalog, and cover preparation runs in the background.
- `runtime_assets/` validates sources, schedules work, and caches outputs by original pixel identity and artwork recipe. Fast Pixel runs deterministic Scale2x. AI Cartoon uses local Real-ESRGAN through NCNN and Vulkan.
- `native-graphics/` implements the shared graphics ABI, texture operations, and frame presentation. The Java player reaches it through a JNI shim, and the NES bridge calls it from the managed runtime's native core.
- `graphics/` validates runtime manifests and launch policy. `emulator-src/` contains the Java integration, and `nes-bridge/` contains the native and managed NES adapter.
- `game_storage.py` and the system libraries own game data paths. Java ME and NES have separate libraries and control profile files. Each game stores its own profile choice, and deletion keeps saves.

Both systems retain their imported game files. Enhancements are generated from known original artwork and can be reused across launches. Unsupported or uncaptured artwork follows the original rendering path. See [asset discovery](ASSET_DECODING.md) for details and the [development guide](DEVELOPMENT.md) for the source and test layout.
