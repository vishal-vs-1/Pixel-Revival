# Vendored Rust NES backend (library packaging decision: copy, not reference).
#
# Source: C:\Users\Vishal\Documents\test\mesen-rs\backend\mesen-core
# (from-scratch Rust port of Mesen 0.9.9 by Sour; NES-only core, no UI)
# Copied: 2026-11-24, pristine except this workspace layout.
# Upstream license: GPL-3.0-or-later (attribution headers kept in every file).
#
# Layout:
#   mesen-core/    vendored core: upstream plus the documented VENDORED-DELTA
#                  HD-recording hooks in ppu.rs/hd.rs/emulator.rs. Do not change
#                  palette, audio or timing behavior here; fix those in our frontend.
#   mesen-player/  OUR crate: safe C ABI (`cdylib`) over the core. This is the
#                  library the Python tool loads. Edit freely here.
#
# Why copy instead of referencing the outside path:
# - pinned and reproducible on any machine (no dependency on Documents\test),
# - versioned with this repo, builds offline, cargo locks to this tree.
#
# Build (keeps target dir inside ignored build/):
#   $env:CARGO_TARGET_DIR = "..\..\build\rust-nes-target"
#   cargo build --release -p mesen-player
# Output: build\rust-nes-target\release\mesen_player.dll
