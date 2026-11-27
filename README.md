# Pixel Revival

A Windows launcher for Java ME and NES games with optional local artwork enhancement.

Bring your own `.jar` or `.nes` files; the included Java ME demo is synthetic.

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

## Emulation backends

Java ME runs on patched FreeJ2ME-Plus. NES runs on a vendored Rust port of Mesen 0.9.9, integrated through `mesen_player.dll`. Pixel Revival provides the launcher, in-app NES player, artwork pipeline and capture/replacement hooks. See the [architecture](docs/PROJECT_FLOW.md) for component boundaries and the [NES backend notes](rust-nes/README.md) for provenance.

## Run from source

Windows, Python 3.12+ with Tkinter, JDK 17+ and a stable Rust toolchain are required for the respective builds.

```powershell
py -m venv .venv
.\.venv\Scripts\python.exe -m pip install -r requirements.txt
.\.venv\Scripts\python.exe tools/build.py
cargo build --release --manifest-path rust-nes/Cargo.toml -p mesen-player --target-dir build/rust-nes-target
.\.venv\Scripts\python.exe app.py
```

The [high-level design](docs/PROJECT_FLOW.md) describes components, process boundaries, data ownership, runtime flows and design decisions. The [low-level design](docs/LOW_LEVEL_DESIGN.md) maps those boundaries to implementation contracts. The [documentation index](docs/README.md) also links status, setup, asset contracts and performance notes.

## License

GPL-3.0-or-later; see [LICENSE](LICENSE) and dependency notices. Import games you are allowed to use. Java ME JARs execute code and should come from a source you trust.
