//! HD tile recording for Enhanced playback (VENDORED-DELTA).
//!
//! This module is an addition to the upstream Mesen 0.9.9 port. It only
//! *observes* the PPU: when `active`, each drawn pixel appends one [`HdPixel`]
//! describing the winning tile. Nothing here feeds back into emulation, so
//! with `active == false` the PPU behaves bit-identically to upstream.
//!
//! Matching semantics mirror Mesen's HD packs (`HdNesPpu.h`):
//! * CHR ROM tiles key on the absolute tile number (`tile_addr / 16`).
//! * CHR RAM tiles are reported with `ram = true`; v1 compositors must treat
//!   them as misses (their 16 pattern bytes are not captured).
//! * `pal` packs four NES color indices big-endian: background tiles use
//!   `[pal_ram[0], pal_ram[p+1], pal_ram[p+2], pal_ram[p+3]]` and sprites use
//!   `[0xFF, pal_ram[o+1], pal_ram[o+2], pal_ram[o+3]]`, exactly like Mesen's
//!   `PaletteColors`. Our import pipeline stores the same value as the
//!   `Tile.palette` int, so keys compare equal.
//! * `col`/`row` are the flip-resolved pixel coordinates inside the 8x8 tile,
//!   while mirror flags orient the subpixels within each enlarged pixel.

/// One winning pixel's tile. Records are pushed in scanline order, but each
/// also carries its own coordinates so compositing never depends on counts
/// (scanlines with rendering disabled simply contribute no records).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HdPixel {
    /// Native screen position of this pixel.
    pub x: u16,
    pub y: u16,
    /// Absolute CHR tile number (`tile_addr / 16`). Undefined when `ram`.
    pub chr: u32,
    /// Packed palette key, Mesen `PaletteColors` layout (big-endian bytes).
    pub pal: u32,
    /// Flip-resolved column inside the 8x8 tile (0..8).
    pub col: u8,
    /// Displayed row inside the 8x8 tile (0..8).
    pub row: u8,
    /// True for sprite pixels, false for background pixels.
    pub sprite: bool,
    /// True for CHR RAM tiles (v1: compositor must treat as a miss).
    pub ram: bool,
    /// Sprite OAM flags (palette bits 0-1, mirror bits 6-7); 0 for background.
    pub sprite_flags: u8,
}

/// Packs four palette-RAM entries into a Mesen-compatible background key.
#[inline]
pub fn bg_palette_key(universal: u8, c1: u8, c2: u8, c3: u8) -> u32 {
    ((universal as u32) << 24) | ((c1 as u32) << 16) | ((c2 as u32) << 8) | (c3 as u32)
}

/// Packs four palette-RAM entries into a Mesen-compatible sprite key.
#[inline]
pub fn sprite_palette_key(c1: u8, c2: u8, c3: u8) -> u32 {
    0xFF00_0000 | ((c1 as u32) << 16) | ((c2 as u32) << 8) | (c3 as u32)
}

/// Absolute CHR tile number from a PPU pattern-table address.
///
/// `tile_addr` carries the fine-Y row in bits 0-2, which integer division
/// discards, matching Mesen's `TileIndex = AbsoluteTileAddr / 16`.
#[inline]
pub fn chr_index(tile_addr: u16) -> u32 {
    (tile_addr / 16) as u32
}

/// Background tile span for one screen pixel, mirroring `get_pixel_color`'s
/// tile selection exactly.
///
/// Returns the in-effect tile and the pixel's column inside it. `x` is the
/// screen dot (0..256), `fine_x` is the PPU fine-X scroll (0..8).
#[inline]
pub fn bg_span< T: Copy>(previous: T, current: T, x: u32, fine_x: u32) -> (T, u32) {
    let rel = fine_x + (x & 7);
    if rel < 8 {
        // Dots [x - rel .. x] belong to `previous`, column = rel.
        (previous, rel)
    } else {
        // Dots continue into `current`, column restarts at rel - 8.
        (current, rel - 8)
    }
}

/// Flip-resolved sprite column for one screen pixel.
///
/// `shift` is `cycle - sprite_x - 1` (0..8), matching `get_pixel_color`.
#[inline]
pub fn sprite_column(shift: u32, horizontal_mirror: bool) -> u32 {
    if horizontal_mirror {
        7 - shift
    } else {
        shift
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_keys_match_mesen_layout() {
        // Mesen: pal[0]<<24 | pal[tp+1]<<16 | pal[tp+2]<<8 | pal[tp+3].
        assert_eq!(bg_palette_key(0x0F, 0x16, 0x27, 0x38), 0x0F162738);
        // Mesen sprite key: 0xFF in the high byte.
        assert_eq!(sprite_palette_key(0x16, 0x27, 0x38), 0xFF162738);
    }

    #[test]
    fn chr_index_drops_fine_y_row_bits() {
        // Pattern table $0000, tile 5, fine-Y row 3.
        assert_eq!(chr_index(0x0053), 5);
        // Pattern table $1000, tile 5, fine-Y row 3.
        assert_eq!(chr_index(0x1053), 0x105);
        // High-plane address maps to the same tile.
        assert_eq!(chr_index(0x005B), 5);
    }

    #[test]
    fn bg_span_selection_matches_shifter_rule() {
        // fine_x = 0: the shifter pipeline has 8 dots of latency, so every
        // dot uses the previously latched tile, columns 0..8.
        for x in [0u32, 7, 8, 15, 255] {
            let (tile, col) = bg_span('p', 'c', x, 0);
            assert_eq!((tile, col), ('p', x & 7));
        }
        // fine_x = 3: first 5 dots of each 8-dot window come from previous.
        let (tile, col) = bg_span('p', 'c', 0, 3);
        assert_eq!((tile, col), ('p', 3));
        let (tile, col) = bg_span('p', 'c', 4, 3);
        assert_eq!((tile, col), ('p', 7));
        let (tile, col) = bg_span('p', 'c', 5, 3);
        assert_eq!((tile, col), ('c', 0));
        let (tile, col) = bg_span('p', 'c', 7, 3);
        assert_eq!((tile, col), ('c', 2));
        // Window boundary: x=8 behaves like x=0 of the next window.
        let (tile, col) = bg_span('p', 'c', 8, 3);
        assert_eq!((tile, col), ('p', 3));
    }

    #[test]
    fn sprite_column_mirrors_like_get_pixel_color() {
        // Compare texture coordinates with the actual PPU bit extraction;
        // column zero in a decoded tile is bit seven, not bit zero.
        for pattern in [0x80u32, 0x01, 0xB1, 0x5A] {
            for mirrored in [false, true] {
                for shift in 0..8 {
                    let visible = if mirrored {
                        (pattern >> shift) & 1
                    } else {
                        ((pattern << shift) & 0x80) >> 7
                    };
                    let col = sprite_column(shift, mirrored);
                    let decoded = (pattern >> (7 - col)) & 1;
                    assert_eq!(visible, decoded, "pattern={pattern:#x} mirrored={mirrored} shift={shift}");
                }
            }
        }
    }
}
