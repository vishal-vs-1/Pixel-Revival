Pixel Revival - Windows preview

START
Extract the whole ZIP into a writable folder and open Pixel Revival.exe.
Keep app/ and data/ beside it. Select Java ME or NES and add your own game.
The Java ME demo is synthetic. Packaged previews may predate newer source fixes.

PLAY
Select a game and open Play. Enhance play uses cached general artwork.
Missing replacements keep the original pixels. AI does not run on every frame.
Settings contains separate keyboard profiles for Java ME and NES.
Java ME supports GPU playback and per-game CPU compatibility.
NES uses the bundled in-app Rust library.

CHARACTER ART
Use image and Use regenerated poses are mutually exclusive Play checkboxes.
They are independent of Enhance play. Choose an image or regenerate a pose
set in the artwork controls first. Java ME player poses need review/confirmation.
NES player identification is experimental: it can miss poses or select enemies.
Regenerate again when the saved pose set changes.

CAPTURE AND COMPARISON
Standalone Original and Enhanced capture artwork and pose candidates.
Comparison uses shared keys and private save copies. Only Enhanced captures
and applies the selected character artwork in comparison.
F11 toggles comparison fullscreen. Java ME Escape leaves fullscreen before
closing the windowed pair; NES Escape closes the pair directly.
Java ME comparison game clocks are independent.
Close players before using Artwork > Sync to recover missed/legacy captures.
NES prepares captured tile artwork for a later Enhanced launch.

ARTWORK STYLES
Fast Pixel uses offline Scale2x. Redraw uses offline pixel tracing.
AI Cartoon needs AI setup plus a working Vulkan driver. Its runtime/model
download once; processing then runs locally. AI can change letters/details.

FILES
Keep data/ when upgrading: games, saves, settings, artwork and AI downloads
live there. Keep an old installation until the new one shows your saves.
License and dependency notices are bundled under app/.

Compatibility, capture coverage and performance vary by game and hardware.
Import only games you are allowed to use; Java ME JARs execute code.
