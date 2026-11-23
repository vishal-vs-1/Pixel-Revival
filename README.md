# Pixel Revival

[![License: GPL-3.0-or-later](https://img.shields.io/badge/License-GPL--3.0--or--later-blue.svg)](LICENSE)

**A Windows emulator for Java ME and NES games with optional local artwork enhancement.**

Import your own `.jar` or `.nes` files, switch between the two libraries, and compare original artwork with cached enhancements. Pixel Revival uses [FreeJ2ME-Plus](https://github.com/TASEmulators/freej2me-plus) for Java ME and our own in-tool Rust NES player (a from-scratch port of Mesen 0.9.9) for NES. The Windows launcher appears as **JAR Revival**.

<p align="center">
  <img src="docs/images/revival-demo-gameplay.png" alt="Gameplay from the included synthetic Java ME demo" width="360">
  <br><em>The included synthetic Java ME demo.</em>
</p>

## Play on Windows

1. Extract the entire Windows preview ZIP and open **JAR Revival.exe**. Keep the `app/` and `data/` folders next to it. The preview includes the Java runtime, synthetic demo, and the Rust NES library.
2. Use the **Java ME / NES** switch in the sidebar. In Java ME mode, choose **Import Java ME demo** or **Add game**. In NES mode, add a `.nes` file. From source, build the Rust player first as described in the [development guide](docs/DEVELOPMENT.md#nes-setup).
3. Click **Play** to use original artwork, or check **Enhance play** before clicking **Play** to use prepared artwork and capture more during gameplay.
4. Open **Settings** to edit up to three keyboard profiles per system. Pick a saved profile or **Default** on each game's **Play** tab.

Saves, settings and cached artwork live in `data/` in the portable app. Keep that folder when upgrading. Bring your own games. The package includes a synthetic demo.

## Artwork and display

| Option | What it does |
| --- | --- |
| **Original** | Plays with the game's original artwork, in our window. |
| **Fast Pixel** | Uses Scale2x for sharper, smoother edges with the original colors. It works offline. |
| **AI Cartoon** | Uses optional local Real-ESRGAN to redraw supported artwork. It can change lettering and fine details. |
| **Redraw (no outlines)** | Traces color regions with the offline pixel-art tool. Choose it under Artwork and select Prepare for NES or Java ME. |

Choose a style in **Artwork** and click **Prepare** to change it for an imported game. Use **AI setup** to install the optional Vulkan runtime and model. The download happens once and inference runs locally. See the [artwork comparisons](docs/ARTWORK.md) for examples of how the two styles differ.

Java ME prepares supported resources on import and can discover additional artwork during play. NES Enhanced uses the prepared pack while you play in-tool; new tiles are recorded during Enhanced play and prepared after the game closes. Reopen the game to use the ready pack. Newly encountered tiles can use the original artwork until replacements are ready. Visual results and compatibility vary by game.

### Compare side by side

On the **Play** tab, choose **Compare side by side**. Both panes open in one window: Original on the left and Enhanced on the right using the current artwork cache. Each pane keeps a private copy of the same saved progress, and one keyboard controls both. Press **F11** for fullscreen; **Esc** closes the pair.

### Character artwork

Check **Enhance play**, then click **Play** to use the prepared NES pack in our window. The window opens maximized and fits the game at whole-number scale (4x when the screen allows); **F11** toggles fullscreen. Tiles seen during Enhanced play are prepared for later launches.

Java ME character artwork: review the captured artwork and confirm the player poses, then use **Regenerate all poses**, **Choose image**, **Play with regenerated poses**, or **Play with image**. Both playback modes fit each replacement to the pose's original visible height. See [supported replays and current coverage](docs/PROJECT_FLOW.md#playable-sprite-replacement).

Java ME uses its compatibility GPU renderer. **GPUs** selects the playback device, and **Compatibility** offers per-game CPU playback. NES plays and composites Enhanced frames in-tool on the Rust core.

## Run from source

Use Windows 10 or 11, Python 3.12+ with Tkinter, a JDK 17+ with `javac` and `jlink`, Git, and the stable Rust toolchain for the NES player.

~~~powershell
py -m venv .venv
.\.venv\Scripts\python.exe -m pip install -r requirements.txt
.\.venv\Scripts\python.exe tools/build.py
$env:CARGO_TARGET_DIR = "build/rust-nes-target"
cargo build --release --manifest-path rust-nes/Cargo.toml -p mesen-player
.\.venv\Scripts\python.exe app.py
~~~

Follow the [development guide](docs/DEVELOPMENT.md) to build the shared native graphics runtime, build the Rust NES player, run checks, or create the portable ZIP. [Project flow](docs/PROJECT_FLOW.md) describes the two emulators and shared pipeline. See also [asset discovery](docs/ASSET_DECODING.md), [performance notes](docs/PERFORMANCE.md), and the [CI workflow](.github/workflows/ci.yml).

See the [experiment history](docs/EXPERIMENTS.md) for what was tried and what the results showed. The detailed local records are indexed in `local-work/README.md` when present.

## License

The project is GPL-3.0-or-later. FreeJ2ME-Plus, the Rust NES core (Mesen 0.9.9 port), the optional AI model, and bundled dependencies have their own notices and terms. Import only games you are allowed to use. Java ME JARs contain executable code and should come from a source you trust.
