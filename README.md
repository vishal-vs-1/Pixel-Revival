# Pixel Revival

A Windows launcher for Java ME and NES games with optional local artwork enhancement. The desktop app is named **JAR Revival**.

Java ME uses patched FreeJ2ME-Plus. NES uses the vendored Rust core through `mesen_player.dll` inside the app. Bring your own `.jar` or `.nes` files; the included Java ME demo is synthetic.

## Why the NES player changed

We replaced the separate managed Mesen player and its bridge with a Rust library called directly by Python. This gives the app ownership of the player window, controls, comparison, audio delivery and artwork hooks through one integration path.

The emulator core is a reused Rust port of Mesen 0.9.9. Our work is the C API wrapper, in-app player, HD artwork integration and capture/pose extensions. The core remains separate from the launcher in `rust-nes/mesen-core/`; it was not rewritten from scratch for this app. We vendor its source so builds do not depend on an external checkout. See [architecture and migration details](docs/PROJECT_FLOW.md#why-the-nes-player-changed).

## Use the app

1. Select **Java ME** or **NES**, add a game, and open **Play**.
2. **Enhance play** selects prepared artwork. Missing replacements keep original pixels.
3. Optionally select **Use image** or **Use regenerated poses** for character replacement. These choices are mutually exclusive and independent of Enhance play.
4. **Compare side by side** opens Original and Enhanced with shared keys. Only Enhanced captures artwork and applies character replacements in comparison.
5. Use **Settings** for keyboard profiles, **Artwork** to prepare images, and **Sync** to reconcile missed captures while players are closed.

Normal standalone Original and Enhanced capture artwork and pose candidates. Java ME player poses require review and confirmation. NES identification is experimental and can miss the player or select enemies. See [current status](docs/STATUS.md).

Keep portable `data/` when upgrading: it contains games, saves, settings and artwork. An existing preview ZIP may predate the current source behavior; see its bundled guide.

## Artwork styles

| Style | Processing |
|---|---|
| Fast Pixel | Offline Scale2x |
| AI Cartoon | Optional local Real-ESRGAN; requires setup and a working Vulkan driver |
| Redraw (no outlines) | Offline pixel tracing |

Prepared artwork is cached. Enhancement is not performed on every game frame. AI can change lettering and details; [examples](docs/ARTWORK.md) show saved outputs.

## Run from source

Windows, Python 3.12+ with Tkinter, JDK 17+ and a stable Rust toolchain are required for the respective builds.

```powershell
py -m venv .venv
.\.venv\Scripts\python.exe -m pip install -r requirements.txt
.\.venv\Scripts\python.exe tools/build.py
cargo build --release --manifest-path rust-nes/Cargo.toml -p mesen-player --target-dir build/rust-nes-target
.\.venv\Scripts\python.exe app.py
```

The [documentation index](docs/README.md) links the current architecture, status, setup, asset contracts and performance notes. Older designs and measurements are historical records, not active architecture specifications.

## License

GPL-3.0-or-later; see [LICENSE](LICENSE) and dependency notices. Import games you are allowed to use. Java ME JARs execute code and should come from a source you trust.
