//! HD tile compositing for Enhanced playback.
//!
//! The core (`mesen-core::hd`) records one [`HdPixel`] per drawn framebuffer
//! pixel in scanline order. This module owns the replacement table and paints
//! the HD frame: nearest-scaled native pixels first, then HD art over each
//! recorded pixel whose tile is in the table. Only the *winning* tile per
//! pixel is ever recorded, so paint order cannot change the result and no
//! occlusion test is needed.

use std::collections::HashMap;

use mesen_core::hd::HdPixel;

const NATIVE_W: usize = 256;
const NATIVE_H: usize = 240;

/// Replacement art for one 8x8 source tile, stored at `scale * 8` pixels.
pub struct HdArt {
    /// RGBA bytes, row-major, `size` x `size`.
    pub rgba: Vec<u8>,
    /// Edge length in pixels (== 8 * pack scale).
    pub size: usize,
}

/// Key: bit 63 = sprite flag, bits 62-32 = CHR tile number, bits 31-0 = palette.
#[inline]
fn table_key(sprite: bool, chr: u32, pal: u32) -> u64 {
    ((sprite as u64) << 63) | ((chr as u64) << 32) | (pal as u64)
}

pub struct HdTable {
    scale: u32,
    map: HashMap<u64, HdArt>,
    pub lookups: u64,
    pub hits: u64,
}

impl HdTable {
    pub fn new(scale: u32) -> Self {
        Self {
            scale: scale.max(1),
            map: HashMap::new(),
            lookups: 0,
            hits: 0,
        }
    }

    pub fn scale(&self) -> u32 {
        self.scale
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }

    /// Insert one tile's art. `rgba` must hold `size * size` RGBA bytes with
    /// `size == 8 * table scale`; mismatched art is rejected (`false`).
    pub fn add(&mut self, sprite: bool, chr: u32, pal: u32, rgba: Vec<u8>, size: usize) -> bool {
        if size != (8 * self.scale) as usize || rgba.len() != size * size * 4 {
            return false;
        }
        self.map.insert(
            table_key(sprite, chr, pal),
            HdArt { rgba, size },
        );
        true
    }

    /// Composite one frame.
    ///
    /// `records` holds the winning tile for pixels the PPU drew this frame;
    /// scanlines with rendering disabled simply contribute nothing. Each
    /// record carries its own coordinates, so no count assumptions are made.
    /// Unmatched tiles fall back to the scaled native pixel underneath.
    pub fn composite(
        &mut self,
        records: &[HdPixel],
        native: &[u32],
        out: &mut [u32],
    ) -> bool {
        let s = self.scale as usize;
        let out_w = NATIVE_W * s;
        let out_h = NATIVE_H * s;
        if self.map.is_empty() || native.len() < NATIVE_W * NATIVE_H || out.len() != out_w * out_h {
            return false;
        }
        // Nearest-neighbor upscale of the native frame. Alpha is forced opaque:
        // the PPU framebuffer's top byte is padding, not transparency.
        for y in 0..out_h {
            let row = &native[(y / s) * NATIVE_W..(y / s) * NATIVE_W + NATIVE_W];
            let dest = &mut out[y * out_w..(y + 1) * out_w];
            for x in 0..out_w {
                dest[x] = row[x / s] | 0xFF00_0000;
            }
        }
        // Overdraw recorded tiles whose art is present.
        for record in records.iter() {
            if record.ram {
                continue;
            }
            let key = table_key(record.sprite, record.chr, record.pal);
            self.lookups += 1;
            let art = match self.map.get(&key) {
                Some(art) => {
                    self.hits += 1;
                    art
                }
                None => continue,
            };
            let x = record.x as usize;
            let y = record.y as usize;
            if x >= NATIVE_W || y >= NATIVE_H {
                continue;
            }
            let sx = (record.col as usize) * s;
            let sy = (record.row as usize) * s;
            for dy in 0..s {
                let dest_y = y * s + dy;
                let src_y = sy + dy;
                for dx in 0..s {
                    let dest_x = x * s + dx;
                    let o = (src_y * art.size + sx + dx) * 4;
                    let alpha = art.rgba[o + 3] as u32;
                    if alpha == 0 {
                        continue;
                    }
                    let dst = &mut out[dest_y * out_w + dest_x];
                    if alpha == 255 {
                        *dst = ((art.rgba[o] as u32) << 16)
                            | ((art.rgba[o + 1] as u32) << 8)
                            | (art.rgba[o + 2] as u32)
                            | 0xFF00_0000;
                    } else {
                        let inv = 255 - alpha;
                        let dr = (*dst >> 16) & 0xFF;
                        let dg = (*dst >> 8) & 0xFF;
                        let db = *dst & 0xFF;
                        *dst = (((art.rgba[o] as u32 * alpha + dr * inv) / 255) << 16)
                            | (((art.rgba[o + 1] as u32 * alpha + dg * inv) / 255) << 8)
                            | ((art.rgba[o + 2] as u32 * alpha + db * inv) / 255)
                            | 0xFF00_0000;
                    }
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesen_core::hd::HdPixel;

    const BLACK: u32 = 0x00000000;
    const WHITE: u32 = 0x00FFFFFF;

    fn solid_art(size: usize, r: u8, g: u8, b: u8, a: u8) -> Vec<u8> {
        let mut px = Vec::with_capacity(size * size * 4);
        for _ in 0..size * size {
            px.extend_from_slice(&[r, g, b, a]);
        }
        px
    }

    fn blank_records() -> Vec<HdPixel> {
        (0..NATIVE_W * NATIVE_H)
            .map(|i| HdPixel {
                x: (i % NATIVE_W) as u16,
                y: (i / NATIVE_W) as u16,
                chr: 0,
                pal: 0,
                col: 0,
                row: 0,
                sprite: false,
                ram: false,
                sprite_flags: 0,
            })
            .collect()
    }

    fn blank_native(color: u32) -> Vec<u32> {
        vec![color; NATIVE_W * NATIVE_H]
    }

    #[test]
    fn empty_table_reports_miss_for_plain_scaled_fallback() {
        let mut table = HdTable::new(4);
        let records = blank_records();
        let native = blank_native(BLACK);
        let mut out = vec![0u32; 1024 * 960];
        assert!(!table.composite(&records, &native, &mut out));
        assert!(out.iter().all(|&p| p == 0));
    }

    #[test]
    fn partial_record_lists_still_composite() {
        // Records carry their own coordinates, so a sparse list (e.g. after
        // scanlines with rendering disabled) composites fine.
        let mut table = HdTable::new(2);
        assert!(table.add(false, 9, 0x0F162738, solid_art(16, 255, 0, 0, 255), 16));
        let records = vec![HdPixel {
            x: 3,
            y: 5,
            chr: 9,
            pal: 0x0F162738,
            col: 4,
            row: 6,
            sprite: false,
            ram: false,
                sprite_flags: 0,
        }];
        let native = blank_native(BLACK);
        let mut out = vec![0u32; 512 * 480];
        assert!(table.composite(&records, &native, &mut out));
        assert_eq!(out[(5 * 2) * 512 + 3 * 2], 0xFFFF_0000);
    }

    #[test]
    fn bg_hit_replaces_all_subpixels_with_art() {
        let mut table = HdTable::new(2);
        // Art is 16x16 solid red; record covers native pixel (3, 5).
        assert!(table.add(false, 9, 0x0F162738, solid_art(16, 255, 0, 0, 255), 16));
        let mut records = blank_records();
        let at = 5 * NATIVE_W + 3;
        records[at] = HdPixel {
            x: 3,
            y: 5,
            chr: 9,
            pal: 0x0F162738,
            col: 4,
            row: 6,
            sprite: false,
            ram: false,
                sprite_flags: 0,
        };
        let native = blank_native(BLACK);
        let mut out = vec![0u32; 512 * 480];
        assert!(table.composite(&records, &native, &mut out));
        // Native pixel (3,5) maps to out block (6..8, 10..12); texel (4,6)*2.
        for dy in 0..2 {
            for dx in 0..2 {
                assert_eq!(out[(5 * 2 + dy) * 512 + 3 * 2 + dx], 0xFFFF_0000);
            }
        }
        // Untouched area stays scaled native.
        assert_eq!(out[0], 0xFF00_0000);
        assert_eq!(out[479 * 512 + 511], 0xFF00_0000);
    }

    #[test]
    fn miss_falls_back_to_scaled_native() {
        let mut table = HdTable::new(2);
        assert!(table.add(false, 9, 0x0F162738, solid_art(16, 9, 9, 9, 255), 16));
        let mut records = blank_records();
        // Same CHR, different palette -> miss.
        records[100] = HdPixel {
            x: 100,
            y: 0,
            chr: 9,
            pal: 0x0F162739,
            col: 0,
            row: 0,
            sprite: false,
            ram: false,
                sprite_flags: 0,
        };
        let mut native = blank_native(BLACK);
        native[100] = WHITE;
        let mut out = vec![0u32; 512 * 480];
        assert!(table.composite(&records, &native, &mut out));
        // Scaled native white block, no HD paint.
        for dy in 0..2 {
            for dx in 0..2 {
                assert_eq!(out[(0 * 2 + dy) * 512 + 100 * 2 + dx], 0xFFFF_FFFF);
            }
        }
    }

    #[test]
    fn sprite_and_bg_keys_do_not_collide() {
        let mut table = HdTable::new(1);
        assert!(table.add(true, 9, 0xFF162738, solid_art(8, 10, 20, 30, 255), 8));
        let mut records = blank_records();
        // Same chr/pal as a sprite tile but as background: must miss
        // (sprite flag differs).
        records[0] = HdPixel {
            x: 0,
            y: 0,
            chr: 9,
            pal: 0xFF162738,
            col: 0,
            row: 0,
            sprite: false,
            ram: false,
                sprite_flags: 0,
        };
        let native = blank_native(0x00112233);
        let mut out = vec![0u32; 256 * 240];
        assert!(table.composite(&records, &native, &mut out));
        assert_eq!(out[0], 0xFF11_2233);
    }

    #[test]
    fn transparent_hd_texels_keep_canvas() {
        let mut table = HdTable::new(1);
        let mut art = solid_art(8, 200, 100, 50, 0);
        // First texel fully transparent (alpha 0 keeps canvas)...
        art[0] = 1;
        art[1] = 2;
        art[2] = 3;
        art[3] = 0;
        assert!(table.add(false, 3, 0, art, 8));
        let mut records = blank_records();
        records[7] = HdPixel {
            x: 7,
            y: 0,
            chr: 3,
            pal: 0,
            col: 0,
            row: 0,
            sprite: false,
            ram: false,
                sprite_flags: 0,
        };
        let mut native = blank_native(BLACK);
        native[7] = 0x00AABBCC;
        let mut out = vec![0u32; 256 * 240];
        assert!(table.composite(&records, &native, &mut out));
        // Fully transparent texel (alpha 0) keeps canvas...
        assert_eq!(out[7], 0xFFAA_BBCC);
    }

    #[test]
    fn partial_alpha_blends_over_canvas() {
        let mut table = HdTable::new(1);
        // 50% red over black -> ~(127,0,0).
        assert!(table.add(false, 3, 0, solid_art(8, 255, 0, 0, 128), 8));
        let mut records = blank_records();
        records[0] = HdPixel {
            x: 0,
            y: 0,
            chr: 3,
            pal: 0,
            col: 1,
            row: 2,
            sprite: false,
            ram: false,
                sprite_flags: 0,
        };
        let native = blank_native(BLACK);
        let mut out = vec![0u32; 256 * 240];
        assert!(table.composite(&records, &native, &mut out));
        let got = out[0];
        let r = ((got >> 16) & 0xFF) as i32;
        assert!((125..=130).contains(&r), "r={r:#x}");
        assert_eq!(got & 0xFFFF, 0);
    }

    #[test]
    fn wrong_sized_art_is_rejected() {
        let mut table = HdTable::new(4);
        assert!(!table.add(false, 1, 2, solid_art(16, 1, 1, 1, 255), 16));
        assert!(!table.add(false, 1, 2, vec![0u8; 10], 32));
        assert_eq!(table.len(), 0);
    }

    #[test]
    fn ram_tiles_are_skipped_by_compositor() {
        let mut table = HdTable::new(1);
        // Even with matching chr/pal present, a ram-flagged record is a miss.
        assert!(table.add(false, 9, 0x0F162738, solid_art(8, 9, 9, 9, 255), 8));
        let mut records = blank_records();
        let mut rec = HdPixel {
            x: 0,
            y: 0,
            chr: 9,
            pal: 0x0F162738,
            col: 0,
            row: 0,
            sprite: false,
            ram: false,
                sprite_flags: 0,
        };
        rec.ram = true;
        records[0] = rec;
        let native = blank_native(0x00445566);
        let mut out = vec![0u32; 256 * 240];
        assert!(table.composite(&records, &native, &mut out));
        assert_eq!(out[0], 0xFF44_5566);
    }
}
