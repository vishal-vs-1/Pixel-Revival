# Pixel Revival low-level design

Current source contract map, 27 November 2026. Read the [HLD](PROJECT_FLOW.md) for context and design decisions. This page identifies implementation boundaries and invariants; detailed algorithms remain beside their source, and unresolved behavior remains in [Status](STATUS.md).

## Composition and system routing

| Implementation | Contract |
|---|---|
| [app.py](../app.py) | Construct backends, recipes, shared service and views; acquire launcher lock; coordinate shutdown |
| [contracts.py](../systems/contracts.py) | `GameRef`, `ImportRequest` and `SystemBackend` protocol |
| [application.py](../systems/application.py) | Route workspace, launch, preparation and control operations by stable game identity |
| [registry.py](../systems/registry.py) | Register system adapters and resolve catalog ownership |
| [playback.py](../launcher/playback.py) | Translate UI choices into explicit backend launch options and track player lifecycle |
| [events.py](../launcher/events.py) | Consume background messages on the UI side |

System IDs remain `j2me` and `nes`. `GameRef` validates both system and game ID; older Java ME metadata without a system field resolves to `j2me`. `ImportRequest` accepts the `pixel`, `ai` and `redraw` artwork backends and normalizes its source path. UI labels and product branding do not change these values.

`ApplicationLibrary.games()` builds the complete identity index even when returning a filtered system view. It snapshots the catalog revision and retries publication after concurrent import/delete changes. Routing locks protect the index; backend calls occur outside them. Async UI work retains a `GameRef` so changing the visible system cannot redirect a completion to another backend.

## Playback option mapping

`LauncherPlayback._sprite_playback_options()` reads mutually exclusive image/generated choices only for the panel's selected identity. `Enhance play` separately determines the `original` argument. Choices reset when no longer available; temporary busy state can disable controls without discarding a valid choice.

| User choice | Java ME option | NES option |
|---|---|---|
| Neither sprite choice | `pose_mode='record'` | Capture poses, no replacement source |
| Use image | `pose_mode='record-replace'` | `player_replacement=True` |
| Use regenerated poses | `pose_mode='regenerated'` | `player_regenerated=True` |

Java ME recording-only launch explicitly passes `jme.replacePoses=false`. Generated playback separates the replacement reader from the main capture destination. Comparison supplies pose/replacement options only to Enhanced. NES standalone Play explicitly requests both tile capture and pose capture; those options must not be inferred from the Enhance checkbox. Backend defaults can differ from the normal launcher's explicit selection, so new callers must choose their intended mode.

## Java ME integration

[JavaMeSystem](../systems/jme/system.py) adapts the [Java ME library](../systems/jme/library.py) to the shared system contract. The library owns imported originals, metadata, control profiles and the shared service instance. [Resource readers](../systems/jme/resources/) inspect supported archive/image structures; [asset contracts](ASSET_DECODING.md) describe validation and provenance.

[JavaMePlayback](../systems/jme/playback.py) resolves Java, builds player arguments and launches patched FreeJ2ME-Plus. JVM properties select rendering, texture roots, capture and pose behavior. [PoseArtwork](../systems/jme/pose_artwork.py) owns saved pose arguments and artwork preparation. Integration code lives under [emulator-src](../emulator-src/) and [patches](../tools/patches/); it is built into the player rather than loaded from Python during each draw.

[ComparisonPlayers](../systems/comparison.py) wraps process pairs and sends key transitions through stdin. Comparison creates private save directories, readiness paths and startup options. [ComparisonWindow](../launcher/comparison.py) manages embedding and view lifetime. The two JVM clocks remain independent, even though key transitions are shared.

## NES native boundary and frame ownership

| Implementation | Contract |
|---|---|
| [NesLibrary](../systems/nes/library.py) | Own cartridge storage, validated metadata, launch state and after-close pose import guards |
| [RustCore](../systems/nes/rust_core.py) | Bind/check C ABI and expose native handles, frame/audio/save/HD/observation calls |
| [RustNesRuntime / player](../systems/nes/rust_player.py) | Runtime capability, player lifecycle, emulation worker and optional hooks |
| [FramePresenter](../systems/nes/frame_presenter.py) | Present bounded immutable frame work outside emulation |
| [RustAudioOut](../systems/nes/rust_audio.py) | Deliver drained samples to Windows audio |
| [Rust comparison](../systems/nes/rust_comparison.py) | Own paired NES playback, views and lifecycle |
| [Comparison clock](../systems/nes/comparison_clock.py) | Coordinate paired frame progression |

Base ABI version is 1. `RUST_NES_DLL` explicitly overrides discovery; otherwise the newest existing standard/`_next` DLL under `build/rust-nes-target/release/` is selected. The Windows packager copies the standard DLL. Restart is required to change an already loaded native library.

The player worker owns mutable native operations during emulation. Display receives copied frame data rather than a mutable core buffer. `FramePresenter` retains two pending frames when pacing is enabled and one otherwise, discarding the oldest on overflow. It records presentation exceptions and rejects further submission after failure or stop. Teardown must stop work before native handle destruction.

Native battery/state serialization stays separate from artwork and pose journals. Optional pose collection clears on native reset/load and is not added to save-state serialization. The reused core and wrapper provenance are documented in the [native README](../rust-nes/README.md).

## Artwork identities, storage and workers

[Recipe and SourceRecord](../runtime_assets/contracts.py) define immutable conversion inputs. Recipe canonical JSON is hashed with SHA-256; its identity includes implementation version and model digest. Input dimensions and pixel count are validated, with a shared maximum source area of 4,194,304 pixels. Source provenance and recipe identity survive independently of player-facing texture filenames.

[AssetStore](../runtime_assets/store.py) owns per-workspace `asset-cache.sqlite` with schema version 1. Tables include recipes, sources, jobs and consumed events. Jobs use `(source_id, recipe_key)` as the primary key and states `QUEUED`, `RUNNING`, `DEFERRED`, `READY`, `FAILED`. Owner/token/expiry fields implement reservations. Short connections enable foreign keys and busy timeout; writes use explicit transactions. A database with a newer schema is rejected rather than silently rewritten.

[AssetService](../runtime_assets/service.py) owns coordinator/worker threads and a one-slot work queue. Defaults are a maximum batch of 512 sources and a 1,048,576-pixel batch budget, with constructor bounds and resource admission limiting actual work. These are scheduler settings, not throughput guarantees. Recovery of another service owner's jobs is enabled only through the lock-owning launcher's explicit recovery flag.

[Discovery](../runtime_assets/discovery.py) ingests source events and explicit reconciliation. [Execution](../runtime_assets/execution.py) invokes injected backends in `.runtime/tmp`, validates results and saves output. [Publication](../runtime_assets/publication.py) exposes ready recipes and maintains legacy player texture views. [Resource policy](../runtime_assets/resources.py) and [admission](../runtime_assets/admission.py) keep pressure/live admission decisions out of image conversion logic.

`WorkspaceIntegration.admit/discover/publish_ready` is the extension boundary for system-specific source and output behavior. [NesCaptureIntegration](../systems/nes/integration.py) implements it with bounded chunk replay, palette checks, bindings and pack publication. The shared service does not identify cartridges or assemble NES sprites.

## NES tile capture and pack contract

[Capture](../systems/nes/capture.py) reads committed chunks; malformed chunks enter quarantine. [Bindings](../systems/nes/bindings.py) map original native tiles/palettes to normalized source images. [Pack generation](../systems/nes/pack.py) publishes ready outputs and updates the workspace pack pointer. Capture integration accepts pack scales 2 or 4.

The player loads one pack for its session. Captured tiles can produce a later pack after play; publication does not mutate the active core's loaded bindings. General tile enhancement supports CHR-ROM bindings, while CHR-RAM sprite identities can be observed by the separate pose path. Original-pixel fallback remains valid when a tile has no accepted replacement.

## Pose observation, tracking and replacement

| Implementation | Responsibility |
|---|---|
| [pose_observation.py](../systems/nes/pose_observation.py) | Decode native sprite observations; share a sampled snapshot between recorder and overlay |
| [pose_tracking.py](../systems/nes/pose_tracking.py) | Track an already known native player through nearby recurring layouts |
| [live_pose.py](../systems/nes/live_pose.py) | Validate cell geometry, native patterns, palettes and pose identity |
| [pose_overlay.py](../systems/nes/pose_overlay.py) | Match saved/live layouts, fit selected art and compose over the output frame |
| [playable_assets.py](../systems/nes/playable_assets.py) | Coordinate playable artwork operations and saved evidence |
| [generated_poses.py](../systems/generated_poses.py) | Validate/store generated pose sets independently of base tile enhancement |

The optional fetched-sprite protocol is version 2, separate from ABI 1. It retains at most 128 complete eight-pixel cells per frame with actual fetched patterns and palettes. Without it, Python retains the earlier debug-read observation path. Journals write version 2 and accept versions 1/2, with an every-third-frame recording cadence and a 2,400-sample cap. Live overlay observation can see frames between journal samples.

Initial discovery uses controller-response evidence and camera motion. Known-pose tracking requires a trusted saved native anchor, limits ambiguous appearance bridging and keeps bounded learned/fitted layouts. Matching reads native identities in both Original and Enhanced, not enhanced RGB artwork. A default image can fit a new tracked pose; generated art needs another regeneration to cover newly learned layouts.

Whole-pose composition permits overlapping native layers. Legacy tile-pack export has a stricter non-overlap contract; `LivePose.overlayable` and `replaceable` therefore express different capabilities. An overlay can blend artwork but cannot reconstruct hidden backgrounds. Correct geometry or matching alone does not prove the selected sprite is the player.

## Saved data and compatibility

[Paths](../storage/paths.py) defines `APP_ROOT`, `RESOURCE_ROOT`, `DATA_ROOT` for source and frozen layouts. [GameStorage](../storage/games.py) owns source/workspace boundaries. [atomic_json](../storage/atomic.py) supplies atomic JSON publication. [NES metadata](../systems/nes/metadata.py) remains schema 1, preserves runtime status and adapts recognized legacy artwork backend names when loading.

Product naming is Pixel Revival for windows, messages and future Windows packages. Existing `RevivalDemo.jar`, native class/ABI names, patch marker strings and persistent path names remain compatibility identifiers. No data migration is part of a branding change. Keeping `data/` beside the replacement executable retains the portable location contract.

## Focused verification map

| Boundary | Existing checks |
|---|---|
| Router, identities and backend isolation | `tests/test_systems_application.py` |
| Artwork state, leases and recovery | `tests/test_runtime_assets.py`, `tests/test_runtime_service.py` |
| Portable paths and package contents | `tests/test_packaging_paths.py`, `tests/test_runtime_package.py` |
| Native lifecycle and teardown | `tests/test_nes_player_safety.py` |
| Pose matching and tracking | `tests/test_nes_pose.py`, `tests/test_nes_pose_tracking.py` |

Confirm test filenames and fixture behavior before execution; some integration checks need built runtimes or open windows. Documentation changes require source/link review. Naming changes need package/path checks, without launching user applications. Evidence and unrun acceptance remain explicit in [Status](STATUS.md) and the append-only [experiment log](EXPERIMENTS.md).
