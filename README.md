# Pixel Revival

[![License: GPL-3.0-or-later](https://img.shields.io/badge/License-GPL--3.0--or--later-blue.svg)](LICENSE)

**Play Java ME phone games and NES cartridges on Windows, with optional local artwork enhancement.**

Import your own `.jar` or `.nes` files, switch between the two libraries, and compare original artwork with cached enhancements. Pixel Revival uses [FreeJ2ME-Plus](https://github.com/TASEmulators/freej2me-plus) for Java ME and a pinned [MesenCE](https://github.com/nesdev-org/MesenCE) build for NES. The Windows launcher appears as **JAR Revival**.

<p align="center">
  <img src="docs/images/revival-demo-gameplay.png" alt="Gameplay from the included synthetic Java ME demo" width="360">
  <br><em>The included synthetic Java ME demo.</em>
</p>

## Play on Windows

1. Extract the entire Windows preview ZIP and open **JAR Revival.exe**. Keep the `app/` and `data/` folders next to it. The preview includes the Java runtime and synthetic demo. The managed NES player is a separate setup.
2. Use the **Java ME / NES** switch in the sidebar. In Java ME mode, choose **Import Java ME demo** or **Add game**. In NES mode, add a `.nes` file after [setting up the managed runtime](docs/DEVELOPMENT.md#managed-nes-runtime).
3. Choose **Play enhanced** to use prepared artwork. Use **Compare original** in Java ME or **Play original** in NES to see the game's original artwork.
4. Open **Settings** to edit up to three keyboard profiles per system. Pick a saved profile or **Default** on each game's **Play** tab.

Saves, settings and cached artwork live in `data/` in the portable app. Keep that folder when upgrading. Bring your own games. The package includes a synthetic demo.

## Artwork and display

| Option | What it does |
| --- | --- |
| **Original** | Plays with the game's original artwork. NES Original uses the stock Mesen presenter. |
| **Fast Pixel** | Uses Scale2x for sharper, smoother edges with the original colors. It works offline. |
| **AI Cartoon** | Uses optional local Real-ESRGAN to redraw supported artwork. It can change lettering and fine details. |

Choose a style in **Artwork** and click **Prepare** to change it for an imported game. Use **AI setup** to install the optional Vulkan runtime and model. The download happens once and inference runs locally. See the [artwork comparisons](docs/ARTWORK.md) for examples of how the two styles differ.

Java ME prepares supported resources on import and can discover additional artwork during play. NES Enhanced records tiles as you play and prepares replacements after the game closes. Reopen the game to use the ready pack. Newly encountered tiles can use the original artwork until replacements are ready. Visual results and compatibility vary by game.

Java ME GPU playback requests the shared renderer by default. **GPUs** offers Legacy GPU, and **Compatibility** offers per-game CPU playback. NES Enhanced uses the same shared graphics implementation through the managed NES runtime.

## Run from source

Use Windows 10 or 11, Python 3.12+ with Tkinter, a JDK 17+ with `javac` and `jlink`, and Git.

~~~powershell
py -m venv .venv
.\.venv\Scripts\python.exe -m pip install -r requirements.txt
.\.venv\Scripts\python.exe tools/build.py
.\.venv\Scripts\python.exe app.py
~~~

Follow the [development guide](docs/DEVELOPMENT.md) to build the shared native graphics runtime, set up NES, run checks, or create the portable ZIP. [Project flow](docs/PROJECT_FLOW.md) describes the two emulators and shared pipeline. See also [asset discovery](docs/ASSET_DECODING.md), [performance notes](docs/PERFORMANCE.md), and the [CI workflow](.github/workflows/ci.yml).

## License

The project is GPL-3.0-or-later. FreeJ2ME-Plus, MesenCE, the optional AI model, and bundled dependencies have their own notices and terms. Import only games you are allowed to use. Java ME JARs contain executable code and should come from a source you trust.
