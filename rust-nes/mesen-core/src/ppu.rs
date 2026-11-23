//! NES PPU (2C02).
//!
//! This is a direct translation of Mesen 0.9.9's `Core/PPU.cpp` / `Core/PPU.h`
//! (Copyright (C) 2014-2020 Sour, GPL-3.0). Rust port: GPL-3.0-or-later.
//!
//! OWNER: ppu agent. Contract: one call to `tick` advances one PPU dot. Return
//! `Some(scanline)` when a new scanline starts so the bus can notify the mapper.
//!
//! Implementation notes:
//! * Scanlines run `-1` (pre-render), `0..=239` (visible), `240` (post-render)
//!   and `241..=vblank_end` (vblank), with 341 dots per line. On NTSC an odd
//!   frame skips the last dot of the pre-render line (Visual 2C02 behaviour).
//! * All CHR/nametable traffic goes through [`Cartridge::ppu_read`] /
//!   [`Cartridge::ppu_write`] so mapper-side A12 watchers (MMC3/MMC5) observe
//!   the same fetch pattern as the real PPU.
//! * Palette RAM and OAM are internal to this struct.
//! * Register open-bus behaviour, the OAMADDR pre-render bug, the sprite
//!   overflow evaluation bug, sprite 0 hit masking and the `$2006` write delay
//!   are ported from upstream.

use crate::cartridge::Cartridge;
use crate::state::{StateReader, StateResult, StateWriter};
use crate::types::{palette_color, Region, SCREEN_PIXELS, SCREEN_WIDTH};

const PPU_PALETTE_BOOT: [u8; 0x20] = [
    0x09, 0x01, 0x00, 0x01, 0x00, 0x02, 0x02, 0x0D, 0x08, 0x10, 0x08, 0x24, 0x00, 0x00, 0x04, 0x2C,
    0x09, 0x01, 0x34, 0x03, 0x00, 0x04, 0x00, 0x14, 0x08, 0x3A, 0x00, 0x02, 0x00, 0x20, 0x2C, 0x08,
];

/// Horizontal dots in one PPU line (cycle 0 is the line-start dot).
#[cfg(test)]
const DOTS_PER_LINE: u32 = 341;

/// Mesen uses `OamDecayCycleCount = 3000` CPU cycles. We count PPU dots.
const OAM_DECAY_DOTS: u64 = 3000 * 3;

#[derive(Clone, Copy, Default)]
struct TileInfo {
    low_byte: u8,
    high_byte: u8,
    palette_offset: u16,
    tile_addr: u16,
    offset_y: u8,
}

#[derive(Clone, Copy, Default)]
struct SpriteInfo {
    low_byte: u8,
    high_byte: u8,
    palette_offset: u16,
    tile_addr: u16,
    offset_y: u8,
    sprite_x: u8,
    horizontal_mirror: bool,
    vertical_mirror: bool,
    background_priority: bool,
}

#[derive(Clone, Copy, Default)]
struct ControlFlags {
    vertical_write: bool,
    sprite_pattern_addr: u16,
    background_pattern_addr: u16,
    large_sprites: bool,
    vblank: bool,
    grayscale: bool,
    background_mask: bool,
    sprite_mask: bool,
    background_enabled: bool,
    sprites_enabled: bool,
    intensify_red: bool,
    intensify_green: bool,
    intensify_blue: bool,
}

#[derive(Clone, Copy, Default)]
struct StatusFlags {
    sprite_overflow: bool,
    sprite0_hit: bool,
    vertical_blank: bool,
}

pub struct Ppu {
    framebuffer: Vec<u32>,
    /// Raw palette indices + emphasis bits, in Mesen's 16-bit PPU output format
    /// (bits 0-5 color, 6 red, 7 green, 8 blue).
    output_buffer: Vec<u16>,

    scanline: i16,
    cycle: u32,
    frame_count: u64,
    region: Region,
    frame_ready: bool,

    nmi_scanline: i16,
    vblank_end: i16,
    pal_sprite_eval_scanline: i16,

    // PPU registers / loopy scroll registers.
    control: u8,
    mask: u8,
    status: u8,
    video_ram_addr: u16,     // v
    tmp_video_ram_addr: u16, // t
    x_scroll: u8,            // x
    write_toggle: bool,      // w
    sprite_ram_addr: u16,    // OAMADDR

    low_bit_shift: u16,
    high_bit_shift: u16,
    memory_read_buffer: u8,

    palette_ram: [u8; 0x20],
    sprite_ram: [u8; 0x100],
    secondary_sprite_ram: [u8; 0x20],
    has_sprite: [bool; 257],

    previous_tile: TileInfo,
    current_tile: TileInfo,
    next_tile: TileInfo,
    sprite_tiles: [SpriteInfo; 64],

    // VENDORED-DELTA (HD recording for Enhanced playback): observe-only tile
    // log for the HD compositor. Pushed only while `hd_active` is set; never
    // read back by emulation, so rendering is bit-identical when inactive.
    hd_active: bool,
    hd_is_ram: bool,
    hd_pixels: Vec<crate::hd::HdPixel>,

    sprite_count: u32,
    secondary_oam_addr: u32,
    sprite0_visible: bool,
    first_visible_sprite_addr: u8,
    last_visible_sprite_addr: u8,
    sprite_index: u32,

    open_bus: u8,
    open_bus_decay_stamp: [i32; 8],
    ignore_vram_read: u32,

    oam_copybuffer: u8,
    sprite_in_range: bool,
    sprite0_added: bool,
    sprite_addr_h: u8,
    sprite_addr_l: u8,
    oam_copy_done: bool,
    overflow_bug_counter: u8,

    need_state_update: bool,
    rendering_enabled: bool,
    prev_rendering_enabled: bool,
    prevent_vbl_flag: bool,

    update_vram_addr: u16,
    update_vram_addr_delay: u8,

    minimum_draw_bg_cycle: u32,
    minimum_draw_sprite_cycle: u32,
    minimum_draw_sprite_standard_cycle: u32,

    ppu_bus_address: u16,
    intensify_color_bits: u16,
    palette_ram_mask: u8,
    last_updated_pixel: i32,

    nmi_flag: bool,
    /// Monotonic PPU dot counter (used for OAM decay timestamps/save states).
    total_dots: u64,
    enable_oam_decay: bool,
    oam_decay_cycles: [u64; 0x40],

    flags: ControlFlags,
    status_flags: StatusFlags,
}

impl Ppu {
    pub fn new(region: Region) -> Self {
        let mut ppu = Self {
            framebuffer: vec![0; SCREEN_PIXELS],
            output_buffer: vec![0; SCREEN_PIXELS],
            scanline: -1,
            cycle: 340,
            frame_count: 0,
            region,
            frame_ready: false,
            nmi_scanline: 241,
            vblank_end: 260,
            pal_sprite_eval_scanline: 265,
            control: 0,
            mask: 0,
            status: 0,
            video_ram_addr: 0,
            tmp_video_ram_addr: 0,
            x_scroll: 0,
            write_toggle: false,
            sprite_ram_addr: 0,
            low_bit_shift: 0,
            high_bit_shift: 0,
            memory_read_buffer: 0,
            palette_ram: PPU_PALETTE_BOOT,
            sprite_ram: [0; 0x100],
            secondary_sprite_ram: [0; 0x20],
            has_sprite: [false; 257],
            previous_tile: TileInfo::default(),
            current_tile: TileInfo::default(),
            next_tile: TileInfo::default(),
            sprite_tiles: [SpriteInfo::default(); 64],
            hd_active: false,
            hd_is_ram: false,
            hd_pixels: Vec::new(),
            sprite_count: 0,
            secondary_oam_addr: 0,
            sprite0_visible: false,
            first_visible_sprite_addr: 0,
            last_visible_sprite_addr: 0,
            sprite_index: 0,
            open_bus: 0,
            open_bus_decay_stamp: [0; 8],
            ignore_vram_read: 0,
            oam_copybuffer: 0,
            sprite_in_range: false,
            sprite0_added: false,
            sprite_addr_h: 0,
            sprite_addr_l: 0,
            oam_copy_done: false,
            overflow_bug_counter: 0,
            need_state_update: false,
            rendering_enabled: false,
            prev_rendering_enabled: false,
            prevent_vbl_flag: false,
            update_vram_addr: 0,
            update_vram_addr_delay: 0,
            minimum_draw_bg_cycle: 300,
            minimum_draw_sprite_cycle: 300,
            minimum_draw_sprite_standard_cycle: 300,
            ppu_bus_address: 0,
            intensify_color_bits: 0,
            palette_ram_mask: 0x3F,
            last_updated_pixel: -1,
            nmi_flag: false,
            total_dots: 0,
            enable_oam_decay: false,
            oam_decay_cycles: [0; 0x40],
            flags: ControlFlags::default(),
            status_flags: StatusFlags::default(),
        };
        ppu.configure_region(region);
        ppu.reset();
        ppu
    }

    fn configure_region(&mut self, region: Region) {
        self.region = region;
        let (nmi_scanline, vblank_end) = match region {
            Region::Ntsc => (241i16, 260i16),
            Region::Pal => (241, 310),
            Region::Dendy => (291, 310),
        };
        self.nmi_scanline = nmi_scanline;
        self.vblank_end = vblank_end;
        // Mesen: PAL/Dendy sprite evaluation happens in the extended vblank on
        // PAL only; Dendy's value is past the end of the frame and unused.
        self.pal_sprite_eval_scanline = nmi_scanline + 24;
    }

    pub fn reset(&mut self) {
        self.scanline = -1;
        self.cycle = 340;
        self.frame_count = 0;
        self.frame_ready = false;
        self.total_dots = 0;
        self.nmi_flag = false;

        self.control = 0;
        self.mask = 0;
        self.status = 0;
        self.flags = ControlFlags::default();
        self.status_flags = StatusFlags::default();

        self.video_ram_addr = 0;
        self.tmp_video_ram_addr = 0;
        self.x_scroll = 0;
        self.write_toggle = false;
        self.sprite_ram_addr = 0;
        // VENDORED-DELTA (HD recording): drop any in-flight tile log.
        self.hd_pixels.clear();

        self.low_bit_shift = 0;
        self.high_bit_shift = 0;
        self.memory_read_buffer = 0;

        self.previous_tile = TileInfo::default();
        self.current_tile = TileInfo::default();
        self.next_tile = TileInfo::default();
        self.sprite_tiles = [SpriteInfo::default(); 64];

        self.sprite_count = 0;
        self.secondary_oam_addr = 0;
        self.sprite0_visible = false;
        self.first_visible_sprite_addr = 0;
        self.last_visible_sprite_addr = 0;
        self.sprite_index = 0;

        self.open_bus = 0;
        self.open_bus_decay_stamp = [0; 8];
        self.ignore_vram_read = 0;

        self.oam_copybuffer = 0;
        self.sprite_in_range = false;
        self.sprite0_added = false;
        self.sprite_addr_h = 0;
        self.sprite_addr_l = 0;
        self.oam_copy_done = false;
        self.overflow_bug_counter = 0;

        self.need_state_update = false;
        self.rendering_enabled = false;
        self.prev_rendering_enabled = false;
        self.prevent_vbl_flag = false;

        self.update_vram_addr = 0;
        self.update_vram_addr_delay = 0;

        self.ppu_bus_address = 0;
        self.intensify_color_bits = 0;
        self.palette_ram_mask = 0x3F;
        self.last_updated_pixel = -1;

        self.framebuffer.fill(0);
        self.output_buffer.fill(0);
        self.oam_decay_cycles = [0; 0x40];
        self.update_minimum_draw_cycles();
    }

    pub fn set_region(&mut self, region: Region) {
        self.configure_region(region);
    }

    pub fn region(&self) -> Region {
        self.region
    }

    /// Enable/disable simulated OAM decay (Mesen's `EnableOamDecay` flag; off
    /// by default).
    pub fn set_oam_decay(&mut self, enabled: bool) {
        self.enable_oam_decay = enabled;
    }

    // -----------------------------------------------------------------------
    // Main loop
    // -----------------------------------------------------------------------

    /// Advance one PPU dot. Returns the scanline index when a new line starts.
    pub fn tick(&mut self, cart: &mut Cartridge) -> Option<i16> {
        self.total_dots = self.total_dots.wrapping_add(1);
        let mut started_line = None;

        if self.cycle > 339 {
            self.cycle = 0;
            self.scanline += 1;
            if self.scanline > self.vblank_end {
                self.last_updated_pixel = -1;
                self.scanline = -1;

                // Force pre-render sprite fetches to load dummy $FF tiles.
                self.sprite_count = 0;
                self.update_minimum_draw_cycles();
            }
            started_line = Some(self.scanline);

            if self.scanline == -1 {
                self.status_flags.sprite_overflow = false;
                self.status_flags.sprite0_hit = false;
            } else if self.scanline == 240 {
                // At the start of vblank the PPU bus address returns to v.
                self.set_bus_address(cart, self.video_ram_addr);
                self.send_frame();
                self.frame_count = self.frame_count.wrapping_add(1);
            }
        } else {
            self.cycle += 1;

            if self.scanline < 240 {
                self.process_scanline(cart);
            } else if self.cycle == 1 && self.scanline == self.nmi_scanline {
                if !self.prevent_vbl_flag {
                    self.status_flags.vertical_blank = true;
                    self.begin_vblank();
                }
                self.prevent_vbl_flag = false;
            } else if self.region == Region::Pal
                && self.scanline >= self.pal_sprite_eval_scanline
            {
                // On PAL the PPU keeps refreshing OAM during the extended
                // vblank, even when rendering is disabled.
                if self.cycle <= 256 {
                    self.process_sprite_evaluation();
                } else if self.cycle >= 257 && self.cycle < 320 {
                    self.sprite_ram_addr = 0;
                }
            }
        }

        if self.need_state_update {
            self.update_state(cart);
        }

        started_line
    }

    fn begin_vblank(&mut self) {
        if self.flags.vblank {
            self.nmi_flag = true;
        }
    }

    fn send_frame(&mut self) {
        self.update_grayscale_and_intensify_bits();
        for i in 0..SCREEN_PIXELS {
            self.framebuffer[i] = emphasize_pixel(self.output_buffer[i]);
        }
        self.frame_ready = true;
    }

    pub fn read_register(&mut self, addr: u16, cart: &mut Cartridge) -> u8 {
        let mut open_bus_mask = 0xFFu8;
        let mut return_value = 0u8;

        match addr & 0x07 {
            0x02 => {
                self.write_toggle = false;
                self.update_status_flag();
                return_value = self.status;
                open_bus_mask = 0x1F;
            }
            0x04 => {
                if self.scanline <= 239 && self.rendering_enabled {
                    if self.cycle >= 257 && self.cycle <= 320 {
                        let step = (self.cycle - 257) % 8;
                        let step = if step > 3 { 3 } else { step };
                        self.secondary_oam_addr = (self.cycle - 257) / 8 * 4 + step;
                        self.oam_copybuffer =
                            self.secondary_sprite_ram[(self.secondary_oam_addr & 0x1F) as usize];
                    }
                    return_value = self.oam_copybuffer;
                } else {
                    let addr = self.sprite_ram_addr;
                    return_value = self.read_sprite_ram(addr as u8);
                }
                open_bus_mask = 0x00;
            }
            0x07 => {
                if self.ignore_vram_read != 0 {
                    // Two $2007 reads in quick succession: second read ignored.
                    open_bus_mask = 0xFF;
                } else {
                    return_value = self.memory_read_buffer;
                    let bus_addr = self.ppu_bus_address & 0x3FFF;
                    if bus_addr >= 0x3F00 {
                        // Palette reads do not load the read buffer with CHR
                        // data: hardware mirrors the access into the nametable
                        // at $2F00-$2FFF. Mesen achieves the same by mapping
                        // $3000-$3FFF onto nametable RAM.
                        self.set_bus_address(cart, bus_addr);
                        self.memory_read_buffer = match cart.ppu_read(bus_addr) {
                            Some(value) => value,
                            None => {
                                let mirror = bus_addr & 0x2FFF;
                                cart.ppu_read(mirror).unwrap_or(mirror as u8)
                            }
                        };
                    } else {
                        self.memory_read_buffer = self.read_vram(cart, bus_addr);
                    }

                    if (self.ppu_bus_address & 0x3FFF) >= 0x3F00 {
                        return_value =
                            self.read_palette_ram(self.ppu_bus_address) | (self.open_bus & 0xC0);
                        open_bus_mask = 0xC0;
                    } else {
                        open_bus_mask = 0x00;
                    }

                    self.update_video_ram_addr(cart);
                    self.ignore_vram_read = 6;
                    self.need_state_update = true;
                }
            }
            _ => {}
        }

        self.apply_open_bus(open_bus_mask, return_value)
    }

    pub fn write_register(&mut self, addr: u16, mut value: u8, cart: &mut Cartridge) {
        self.set_open_bus(0xFF, value);
        let reg = addr & 0x07;

        match reg {
            0x00 => self.set_control_register(value),
            0x01 => self.set_mask_register(value),
            0x03 => self.sprite_ram_addr = value as u16,
            0x04 => {
                let writable = (self.scanline >= 240
                    && (self.region != Region::Pal
                        || self.scanline < self.pal_sprite_eval_scanline))
                    || !self.rendering_enabled;
                if writable {
                    if (self.sprite_ram_addr & 0x03) == 0x02 {
                        // Unimplemented bits of sprite attribute byte read as 0.
                        value &= 0xE3;
                    }
                    let oam_addr = self.sprite_ram_addr as u8;
                    self.write_sprite_ram(oam_addr, value);
                    self.sprite_ram_addr = (self.sprite_ram_addr + 1) & 0xFF;
                } else {
                    // Writes during rendering bump only the high 6 bits.
                    self.sprite_ram_addr = (self.sprite_ram_addr + 4) & 0xFF;
                }
            }
            0x05 => {
                if self.write_toggle {
                    self.tmp_video_ram_addr = (self.tmp_video_ram_addr & !0x73E0)
                        | (((value as u16) & 0xF8) << 2)
                        | (((value as u16) & 0x07) << 12);
                } else {
                    self.x_scroll = value & 0x07;
                    let new_addr =
                        (self.tmp_video_ram_addr & !0x001F) | ((value as u16) >> 3);
                    self.process_tmp_addr_scroll_glitch(new_addr, 0, 0x001F);
                }
                self.write_toggle = !self.write_toggle;
            }
            0x06 => {
                if self.write_toggle {
                    self.tmp_video_ram_addr =
                        (self.tmp_video_ram_addr & !0x00FF) | value as u16;

                    // v is updated by the PPU 3 dots after the second write.
                    self.need_state_update = true;
                    self.update_vram_addr_delay = 3;
                    self.update_vram_addr = self.tmp_video_ram_addr;
                    // Let A12-watching mappers see manual $2006 address changes
                    // immediately (in addition to the delayed bus update).
                    cart.mapper_mut()
                        .ppu_bus_address_changed(self.update_vram_addr & 0x3FFF);
                } else {
                    let new_addr = (self.tmp_video_ram_addr & !0xFF00)
                        | (((value as u16) & 0x3F) << 8);
                    self.process_tmp_addr_scroll_glitch(new_addr, 0, 0x0C00);
                }
                self.write_toggle = !self.write_toggle;
            }
            0x07 => {
                if (self.ppu_bus_address & 0x3FFF) >= 0x3F00 {
                    self.write_palette_ram(self.ppu_bus_address, value);
                } else {
                    let bus_addr = self.ppu_bus_address & 0x3FFF;
                    if self.scanline >= 240 || !self.rendering_enabled {
                        cart.ppu_write(bus_addr, value);
                    } else {
                        // During rendering the written value is ignored and the
                        // address' LSB is used instead (Mesen/Visual NES quirk).
                        cart.ppu_write(bus_addr, (self.ppu_bus_address & 0xFF) as u8);
                    }
                }
                self.update_video_ram_addr(cart);
            }
            _ => {}
        }

        // Notify mappers that observe PPU register writes (MMC5 and friends).
        match reg {
            0x00 | 0x01 | 0x05 | 0x06 => {
                cart.mapper_mut()
                    .ppu_register_write(0x2000 | reg, value);
            }
            _ => {}
        }
    }

    /// Copy 256 bytes into OAM (the bus performs the CPU-side DMA reads).
    ///
    /// Hardware starts the DMA transfer at OAMADDR and wraps around; after 256
    /// writes OAMADDR is back at its starting value.
    pub fn oam_dma(&mut self, data: &[u8]) {
        let count = data.len().min(0x100);
        let start = self.sprite_ram_addr as usize & 0xFF;
        for (i, &byte) in data[..count].iter().enumerate() {
            self.write_sprite_ram(((start + i) & 0xFF) as u8, byte);
        }
        self.sprite_ram_addr = ((start + count) & 0xFF) as u16;
    }

    pub fn take_nmi(&mut self) -> bool {
        std::mem::take(&mut self.nmi_flag)
    }

    pub fn frame_ready(&self) -> bool {
        self.frame_ready
    }

    pub fn take_frame_ready(&mut self) -> bool {
        std::mem::take(&mut self.frame_ready)
    }

    pub fn framebuffer(&self) -> &[u32] {
        &self.framebuffer
    }

    pub fn scanline(&self) -> i16 {
        self.scanline
    }

    pub fn cycle(&self) -> u32 {
        self.cycle
    }

    pub fn frame_count(&self) -> u64 {
        self.frame_count
    }

    /// VENDORED-DELTA (HD recording): arm or disarm per-pixel tile logging.
    /// `is_ram` marks CHR-RAM games, whose tiles are reported as misses.
    pub(crate) fn set_hd(&mut self, active: bool, is_ram: bool) {
        self.hd_active = active;
        self.hd_is_ram = is_ram;
        if !active {
            self.hd_pixels.clear();
        }
    }

    /// VENDORED-DELTA (HD recording): drain the current frame's tile log.
    /// Records are in scanline order, so entry N is frame pixel N.
    pub(crate) fn take_hd_pixels(&mut self, out: &mut Vec<crate::hd::HdPixel>) {
        out.clear();
        out.append(&mut self.hd_pixels);
    }

    /// VENDORED-DELTA (HD debug): report arm state and pending log length
    /// without draining, so Python probes can tell "not armed" apart from
    /// "armed but no pixels" and "pixels drained but no table match".
    pub(crate) fn hd_debug(&self) -> (bool, bool, usize) {
        (self.hd_active, self.hd_is_ram, self.hd_pixels.len())
    }

    // -----------------------------------------------------------------------
    // Register helpers
    // -----------------------------------------------------------------------

    fn set_control_register(&mut self, value: u8) {
        let was_nmi_enabled = self.flags.vblank;
        self.control = value;

        let name_table = (value & 0x03) as u16;
        let normal_addr = (self.tmp_video_ram_addr & !0x0C00) | (name_table << 10);
        self.process_tmp_addr_scroll_glitch(normal_addr, 0, 0x0400);

        self.flags.vertical_write = (value & 0x04) == 0x04;
        self.flags.sprite_pattern_addr = if (value & 0x08) == 0x08 {
            0x1000
        } else {
            0x0000
        };
        self.flags.background_pattern_addr = if (value & 0x10) == 0x10 {
            0x1000
        } else {
            0x0000
        };
        self.flags.large_sprites = (value & 0x20) == 0x20;

        self.flags.vblank = (value & 0x80) == 0x80;
        if !self.flags.vblank {
            self.nmi_flag = false;
        } else if !was_nmi_enabled && self.status_flags.vertical_blank {
            // Enabling NMI while vblank is active pulls /NMI low (an edge).
            // Writing the same value again must not generate a second NMI.
            self.nmi_flag = true;
        }
    }

    /// Recompute register-derived flags without side effects (save-state load).
    fn rebuild_control_flags(&mut self) {
        let value = self.control;
        self.flags.vertical_write = (value & 0x04) == 0x04;
        self.flags.sprite_pattern_addr = if (value & 0x08) == 0x08 {
            0x1000
        } else {
            0x0000
        };
        self.flags.background_pattern_addr = if (value & 0x10) == 0x10 {
            0x1000
        } else {
            0x0000
        };
        self.flags.large_sprites = (value & 0x20) == 0x20;
        self.flags.vblank = (value & 0x80) == 0x80;
    }

    fn set_mask_register(&mut self, value: u8) {
        self.mask = value;
        self.flags.grayscale = (value & 0x01) == 0x01;
        self.flags.background_mask = (value & 0x02) == 0x02;
        self.flags.sprite_mask = (value & 0x04) == 0x04;
        self.flags.background_enabled = (value & 0x08) == 0x08;
        self.flags.sprites_enabled = (value & 0x10) == 0x10;
        self.flags.intensify_blue = (value & 0x80) == 0x80;

        if self.rendering_enabled
            != (self.flags.background_enabled | self.flags.sprites_enabled)
        {
            self.need_state_update = true;
        }

        self.update_minimum_draw_cycles();

        // Retroactively apply the *previous* grayscale/emphasis settings to the
        // pixels drawn since the last update, then adopt the new ones.
        self.update_grayscale_and_intensify_bits();

        self.palette_ram_mask = if self.flags.grayscale { 0x30 } else { 0x3F };

        match self.region {
            Region::Ntsc => {
                self.flags.intensify_red = (value & 0x20) == 0x20;
                self.flags.intensify_green = (value & 0x40) == 0x40;
                self.intensify_color_bits = ((value & 0xE0) as u16) << 1;
            }
            Region::Pal | Region::Dendy => {
                // On PAL/Dendy the green and red emphasis bits swap meaning.
                self.flags.intensify_red = (value & 0x40) == 0x40;
                self.flags.intensify_green = (value & 0x20) == 0x20;
                self.intensify_color_bits = (if self.flags.intensify_red {
                    0x40
                } else {
                    0
                }) | (if self.flags.intensify_green { 0x80 } else { 0 })
                    | (if self.flags.intensify_blue { 0x100 } else { 0 });
            }
        }
    }

    fn rebuild_mask_flags(&mut self) {
        let value = self.mask;
        self.flags.grayscale = (value & 0x01) == 0x01;
        self.flags.background_mask = (value & 0x02) == 0x02;
        self.flags.sprite_mask = (value & 0x04) == 0x04;
        self.flags.background_enabled = (value & 0x08) == 0x08;
        self.flags.sprites_enabled = (value & 0x10) == 0x10;
        match self.region {
            Region::Ntsc => {
                self.flags.intensify_red = (value & 0x20) == 0x20;
                self.flags.intensify_green = (value & 0x40) == 0x40;
                self.flags.intensify_blue = (value & 0x80) == 0x80;
            }
            Region::Pal | Region::Dendy => {
                self.flags.intensify_red = (value & 0x40) == 0x40;
                self.flags.intensify_green = (value & 0x20) == 0x20;
                self.flags.intensify_blue = (value & 0x80) == 0x80;
            }
        }
    }

    fn update_minimum_draw_cycles(&mut self) {
        self.minimum_draw_bg_cycle = if self.flags.background_enabled {
            if self.flags.background_mask {
                0
            } else {
                8
            }
        } else {
            300
        };
        self.minimum_draw_sprite_cycle = if self.flags.sprites_enabled {
            if self.flags.sprite_mask {
                0
            } else {
                8
            }
        } else {
            300
        };
        self.minimum_draw_sprite_standard_cycle = if self.flags.sprites_enabled {
            if self.flags.sprite_mask {
                0
            } else {
                8
            }
        } else {
            300
        };
    }

    fn update_status_flag(&mut self) {
        self.status = ((self.status_flags.sprite_overflow as u8) << 5)
            | ((self.status_flags.sprite0_hit as u8) << 6)
            | ((self.status_flags.vertical_blank as u8) << 7);
        self.status_flags.vertical_blank = false;
        self.nmi_flag = false;

        if self.scanline == self.nmi_scanline && self.cycle == 0 {
            // "Reading one PPU clock before [vblank] reads it as clear and
            // never sets the flag or generates NMI for that frame."
            self.prevent_vbl_flag = true;
        }
    }

    fn process_tmp_addr_scroll_glitch(&mut self, normal_addr: u16, _value: u16, _mask: u16) {
        // Mesen's PPU $2000/$2005/$2006 cycle-257 scroll glitches are gated by
        // optional accuracy flags that default to off.
        self.tmp_video_ram_addr = normal_addr;
    }

    // -----------------------------------------------------------------------
    // Open bus
    // -----------------------------------------------------------------------

    fn set_open_bus(&mut self, mask: u8, value: u8) {
        if mask == 0xFF {
            self.open_bus = value;
            for stamp in &mut self.open_bus_decay_stamp {
                *stamp = self.frame_count as i32;
            }
        } else {
            let mut open_bus = (self.open_bus as u16) << 8;
            let mut mask = mask;
            let mut value = value;
            for i in 0..8 {
                open_bus >>= 1;
                if mask & 0x01 != 0 {
                    if value & 0x01 != 0 {
                        open_bus |= 0x80;
                    } else {
                        open_bus &= 0xFF7F;
                    }
                    self.open_bus_decay_stamp[i] = self.frame_count as i32;
                } else if (self.frame_count as i64 - self.open_bus_decay_stamp[i] as i64) > 30 {
                    open_bus &= 0xFF7F;
                }
                value >>= 1;
                mask >>= 1;
            }
            self.open_bus = open_bus as u8;
        }
    }

    fn apply_open_bus(&mut self, mask: u8, value: u8) -> u8 {
        self.set_open_bus(!mask, value);
        value | (self.open_bus & mask)
    }

    // -----------------------------------------------------------------------
    // PPU memory bus
    // -----------------------------------------------------------------------

    fn set_bus_address(&mut self, cart: &mut Cartridge, addr: u16) {
        self.ppu_bus_address = addr;
        cart.mapper_mut()
            .ppu_bus_address_changed(addr & 0x3FFF);
    }

    fn read_vram(&mut self, cart: &mut Cartridge, addr: u16) -> u8 {
        self.set_bus_address(cart, addr);
        cart.ppu_read(addr).unwrap_or(addr as u8)
    }

    fn update_video_ram_addr(&mut self, cart: &mut Cartridge) {
        if self.scanline >= 240 || !self.rendering_enabled {
            self.video_ram_addr = self
                .video_ram_addr
                .wrapping_add(if self.flags.vertical_write { 32 } else { 1 })
                & 0x7FFF;
            self.set_bus_address(cart, self.video_ram_addr & 0x3FFF);
        } else {
            self.inc_horizontal_scrolling();
            self.inc_vertical_scrolling();
        }
    }

    fn inc_vertical_scrolling(&mut self) {
        let mut addr = self.video_ram_addr;
        if (addr & 0x7000) != 0x7000 {
            addr = addr.wrapping_add(0x1000);
        } else {
            addr &= !0x7000;
            let mut y = (addr & 0x03E0) >> 5;
            if y == 29 {
                y = 0;
                addr ^= 0x0800;
            } else if y == 31 {
                y = 0;
            } else {
                y += 1;
            }
            addr = (addr & !0x03E0) | (y << 5);
        }
        self.video_ram_addr = addr;
    }

    fn inc_horizontal_scrolling(&mut self) {
        let mut addr = self.video_ram_addr;
        if (addr & 0x001F) == 31 {
            addr = (addr & !0x001F) ^ 0x0400;
        } else {
            addr = addr.wrapping_add(1);
        }
        self.video_ram_addr = addr;
    }

    fn get_name_table_addr(&self) -> u16 {
        0x2000 | (self.video_ram_addr & 0x0FFF)
    }

    fn get_attribute_addr(&self) -> u16 {
        0x23C0
            | (self.video_ram_addr & 0x0C00)
            | ((self.video_ram_addr >> 4) & 0x0038)
            | ((self.video_ram_addr >> 2) & 0x0007)
    }

    // -----------------------------------------------------------------------
    // Palette / OAM memory
    // -----------------------------------------------------------------------

    fn read_palette_ram(&self, addr: u16) -> u8 {
        let mut a = (addr & 0x1F) as usize;
        if a == 0x10 || a == 0x14 || a == 0x18 || a == 0x1C {
            a &= !0x10;
        }
        self.palette_ram[a]
    }

    fn write_palette_ram(&mut self, addr: u16, value: u8) {
        let addr = (addr & 0x1F) as usize;
        let value = value & 0x3F;
        if addr == 0x00 || addr == 0x10 {
            self.palette_ram[0x00] = value;
            self.palette_ram[0x10] = value;
        } else if addr == 0x04 || addr == 0x14 {
            self.palette_ram[0x04] = value;
            self.palette_ram[0x14] = value;
        } else if addr == 0x08 || addr == 0x18 {
            self.palette_ram[0x08] = value;
            self.palette_ram[0x18] = value;
        } else if addr == 0x0C || addr == 0x1C {
            self.palette_ram[0x0C] = value;
            self.palette_ram[0x1C] = value;
        } else {
            self.palette_ram[addr] = value;
        }
    }

    fn read_sprite_ram(&mut self, addr: u8) -> u8 {
        if !self.enable_oam_decay {
            self.sprite_ram[addr as usize]
        } else {
            let elapsed = self
                .total_dots
                .saturating_sub(self.oam_decay_cycles[(addr >> 3) as usize]);
            if elapsed <= OAM_DECAY_DOTS {
                self.oam_decay_cycles[(addr >> 3) as usize] = self.total_dots;
                self.sprite_ram[addr as usize]
            } else {
                0x10
            }
        }
    }

    fn write_sprite_ram(&mut self, addr: u8, value: u8) {
        self.sprite_ram[addr as usize] = value;
        if self.enable_oam_decay {
            self.oam_decay_cycles[(addr >> 3) as usize] = self.total_dots;
        }
    }

    // -----------------------------------------------------------------------
    // Scanline processing
    // -----------------------------------------------------------------------

    fn process_scanline(&mut self, cart: &mut Cartridge) {
        if self.cycle <= 256 {
            self.load_tile_info(cart);

            if self.prev_rendering_enabled && (self.cycle & 0x07) == 0 {
                self.inc_horizontal_scrolling();
                if self.cycle == 256 {
                    self.inc_vertical_scrolling();
                }
            }

            if self.scanline >= 0 {
                self.draw_pixel();
                self.shift_tile_registers();
                self.process_sprite_evaluation();
            } else if self.cycle < 9 {
                if self.cycle == 1 {
                    self.status_flags.vertical_blank = false;
                    self.nmi_flag = false;
                }
                if self.sprite_ram_addr >= 0x08 && self.rendering_enabled {
                    // OAMADDR >= 8 copies bytes from OAMADDR & 0xF8 into the
                    // first eight OAM bytes when rendering starts.
                    let src = (((self.sprite_ram_addr & 0xF8) as u32) + self.cycle - 1) as u8;
                    let value = self.read_sprite_ram(src);
                    self.write_sprite_ram((self.cycle - 1) as u8, value);
                }
            }
        } else if self.cycle >= 257 && self.cycle <= 320 {
            if self.cycle == 257 {
                self.sprite_index = 0;
                self.has_sprite = [false; 257];
                if self.prev_rendering_enabled {
                    // Copy horizontal scrolling value from t.
                    self.video_ram_addr = (self.video_ram_addr & !0x041F)
                        | (self.tmp_video_ram_addr & 0x041F);
                }
            }

            if self.rendering_enabled {
                // OAMADDR is reset during the sprite tile loading interval.
                self.sprite_ram_addr = 0;

                if self.cycle >= 261 && (self.cycle - 261) % 8 == 0 {
                    // Cycles 261, 269, ... 317
                    self.load_sprite_tile_info(cart);
                } else if (self.cycle - 257) % 8 == 0 {
                    // Garbage NT sprite fetch (needed by MMC3 clones).
                    let addr = self.get_name_table_addr();
                    self.read_vram(cart, addr);
                } else if self.cycle >= 259 && (self.cycle - 259) % 8 == 0 {
                    // Garbage AT sprite fetch.
                    let addr = self.get_attribute_addr();
                    self.read_vram(cart, addr);
                }

                if self.scanline == -1 && self.cycle >= 280 && self.cycle <= 304 {
                    // Copy vertical scrolling value from t.
                    self.video_ram_addr = (self.video_ram_addr & !0x7BE0)
                        | (self.tmp_video_ram_addr & 0x7BE0);
                }
            }
        } else if self.cycle >= 321 && self.cycle <= 336 {
            if self.cycle == 321 {
                if self.rendering_enabled {
                    self.oam_copybuffer = self.secondary_sprite_ram[0];
                }
                self.load_tile_info(cart);
            } else if self.prev_rendering_enabled && (self.cycle == 328 || self.cycle == 336) {
                self.load_tile_info(cart);
                self.low_bit_shift <<= 8;
                self.high_bit_shift <<= 8;
                self.inc_horizontal_scrolling();
            } else {
                self.load_tile_info(cart);
            }
        } else if self.cycle == 337 || self.cycle == 339 {
            if self.rendering_enabled {
                let addr = self.get_name_table_addr();
                self.read_vram(cart, addr);

                if self.scanline == -1
                    && self.cycle == 339
                    && (self.frame_count & 0x01) != 0
                    && self.region == Region::Ntsc
                {
                    // NTSC odd frames skip the last dot of the pre-render line.
                    self.cycle = 340;
                }
            }
        }
    }

    fn load_tile_info(&mut self, cart: &mut Cartridge) {
        if !self.rendering_enabled {
            return;
        }

        match self.cycle & 0x07 {
            1 => {
                self.previous_tile = self.current_tile;
                self.current_tile = self.next_tile;

                self.low_bit_shift |= self.next_tile.low_byte as u16;
                self.high_bit_shift |= self.next_tile.high_byte as u16;

                let addr = self.get_name_table_addr();
                let tile_index = self.read_vram(cart, addr);
                self.next_tile.tile_addr = ((tile_index as u16) << 4)
                    | (self.video_ram_addr >> 12)
                    | self.flags.background_pattern_addr;
                self.next_tile.offset_y = (self.video_ram_addr >> 12) as u8;
            }
            3 => {
                let shift = (((self.video_ram_addr >> 4) & 0x04)
                    | (self.video_ram_addr & 0x02)) as u8;
                let addr = self.get_attribute_addr();
                let attr = self.read_vram(cart, addr);
                self.next_tile.palette_offset = (((attr >> shift) & 0x03) as u16) << 2;
            }
            5 => {
                let addr = self.next_tile.tile_addr;
                self.next_tile.low_byte = self.read_vram(cart, addr);
            }
            7 => {
                let addr = self.next_tile.tile_addr.wrapping_add(8);
                self.next_tile.high_byte = self.read_vram(cart, addr);
            }
            _ => {}
        }
    }

    fn shift_tile_registers(&mut self) {
        self.low_bit_shift <<= 1;
        self.high_bit_shift <<= 1;
    }

    fn get_pixel_color(&mut self) -> u16 {
        let offset = self.x_scroll;
        let mut background_color = 0u8;
        let mut sprite_bg_color = 0u8;

        if self.cycle > self.minimum_draw_bg_cycle {
            // BackgroundMask = false: hide background in leftmost 8 pixels.
            sprite_bg_color = ((((self.low_bit_shift as u32) << offset) & 0x8000) >> 15) as u8
                | ((((self.high_bit_shift as u32) << offset) & 0x8000) >> 14) as u8;
            background_color = sprite_bg_color;
        }

        if self.cycle < 257
            && self.has_sprite[self.cycle as usize]
            && self.cycle > self.minimum_draw_sprite_cycle
        {
            for i in 0..self.sprite_count as usize {
                let sprite = self.sprite_tiles[i];
                let shift = self.cycle as i32 - sprite.sprite_x as i32 - 1;
                if (0..8).contains(&shift) {
                    let shift = shift as u32;
                    let sprite_color = if sprite.horizontal_mirror {
                        ((sprite.low_byte >> shift) & 0x01)
                            | (((sprite.high_byte >> shift) & 0x01) << 1)
                    } else {
                        ((sprite.low_byte << shift) & 0x80) >> 7
                            | (((sprite.high_byte << shift) & 0x80) >> 6)
                    };

                    if sprite_color != 0 {
                        if i == 0
                            && sprite_bg_color != 0
                            && self.sprite0_visible
                            && self.cycle != 256
                            && self.flags.background_enabled
                            && !self.status_flags.sprite0_hit
                            && self.cycle > self.minimum_draw_sprite_standard_cycle
                        {
                            // Sprite 0 hit: sprite 0 in range, both output
                            // units non-zero, neither left column clipped.
                            self.status_flags.sprite0_hit = true;
                        }

                        if background_color == 0 || !sprite.background_priority {
                            self.hd_record_sprite(&sprite, shift as u32, self.cycle - 1);
                            return sprite.palette_offset + sprite_color as u16;
                        }
                        break;
                    }
                }
            }
        }

        // Background tile shared by this dot: same selection rule drives both
        // the pixel color below and the HD record, so they can never disagree.
        let x = self.cycle - 1;
        let (tile, col) = crate::hd::bg_span(
            self.previous_tile,
            self.current_tile,
            x,
            self.x_scroll as u32,
        );
        self.hd_record_bg(&tile, col, x);
        tile.palette_offset + background_color as u16
    }

    /// VENDORED-DELTA (HD recording): log one winning background pixel.
    #[inline]
    fn hd_record_bg(&mut self, tile: &TileInfo, col: u32, x: u32) {
        if !self.hd_active || self.hd_is_ram {
            return;
        }
        let p = tile.palette_offset as usize;
        let pal = crate::hd::bg_palette_key(
            self.palette_ram[0],
            self.palette_ram[p + 1],
            self.palette_ram[p + 2],
            self.palette_ram[p + 3],
        );
        self.hd_pixels.push(crate::hd::HdPixel {
            x: x as u16,
            y: self.scanline as u16,
            chr: crate::hd::chr_index(tile.tile_addr),
            pal,
            col: col as u8,
            row: (tile.offset_y & 7) as u8,
            sprite: false,
            ram: false,
            sprite_flags: 0,
        });
    }

    /// VENDORED-DELTA (HD recording): log one winning sprite pixel.
    /// `shift` is `cycle - sprite_x - 1`, matching `get_pixel_color`.
    #[inline]
    fn hd_record_sprite(&mut self, sprite: &SpriteInfo, shift: u32, x: u32) {
        if !self.hd_active || self.hd_is_ram {
            return;
        }
        let o = sprite.palette_offset as usize;
        let pal = crate::hd::sprite_palette_key(
            self.palette_ram[o + 1],
            self.palette_ram[o + 2],
            self.palette_ram[o + 3],
        );
        self.hd_pixels.push(crate::hd::HdPixel {
            x: x as u16,
            y: self.scanline as u16,
            chr: crate::hd::chr_index(sprite.tile_addr),
            pal,
            col: crate::hd::sprite_column(shift, sprite.horizontal_mirror) as u8,
            row: (sprite.offset_y & 7) as u8,
            sprite: true,
            ram: false,
            sprite_flags: (((sprite.palette_offset.saturating_sub(0x10) >> 2) & 3) as u8)
                | if sprite.horizontal_mirror { 0x40 } else { 0 }
                | if sprite.vertical_mirror { 0x80 } else { 0 },
        });
    }

    fn draw_pixel(&mut self) {
        let index = (self.scanline as usize) * SCREEN_WIDTH + (self.cycle as usize - 1);
        if index >= SCREEN_PIXELS {
            return;
        }

        if self.rendering_enabled || (self.video_ram_addr & 0x3F00) != 0x3F00 {
            let color = self.get_pixel_color();
            let palette_index = if color & 0x03 != 0 { color } else { 0 } as usize;
            self.output_buffer[index] = self.palette_ram[palette_index & 0x1F] as u16;
        } else {
            // Forced blanking with v in $3F00-$3FFF shows that palette entry.
            self.output_buffer[index] =
                self.palette_ram[(self.video_ram_addr & 0x1F) as usize] as u16;
        }
    }

    fn update_grayscale_and_intensify_bits(&mut self) {
        if self.scanline < 0 || self.scanline > self.nmi_scanline {
            return;
        }

        let pixel_number: i32 = if self.scanline >= 240 {
            (SCREEN_PIXELS - 1) as i32
        } else if self.cycle < 3 {
            ((self.scanline as i32) << 8) - 1
        } else if self.cycle <= 258 {
            ((self.scanline as i32) << 8) + self.cycle as i32 - 3
        } else {
            ((self.scanline as i32) << 8) + 255
        };

        if self.palette_ram_mask == 0x3F && self.intensify_color_bits == 0 {
            self.last_updated_pixel = pixel_number;
            return;
        }

        if self.last_updated_pixel < pixel_number {
            let mut index = (self.last_updated_pixel + 1).max(0) as usize;
            while self.last_updated_pixel < pixel_number && index < self.output_buffer.len() {
                self.output_buffer[index] =
                    (self.output_buffer[index] & self.palette_ram_mask as u16)
                        | self.intensify_color_bits;
                index += 1;
                self.last_updated_pixel += 1;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Sprite evaluation / fetching
    // -----------------------------------------------------------------------

    fn process_sprite_evaluation(&mut self) {
        if !(self.rendering_enabled
            || (self.region == Region::Pal && self.scanline >= self.pal_sprite_eval_scanline))
        {
            return;
        }

        if self.cycle < 65 {
            // Clear secondary OAM between cycles 1 and 64.
            self.oam_copybuffer = 0xFF;
            self.secondary_sprite_ram[((self.cycle - 1) >> 1) as usize] = 0xFF;
        } else {
            if self.cycle == 65 {
                self.sprite0_added = false;
                self.sprite_in_range = false;
                self.secondary_oam_addr = 0;
                self.overflow_bug_counter = 0;
                self.oam_copy_done = false;
                self.sprite_addr_h = ((self.sprite_ram_addr >> 2) & 0x3F) as u8;
                self.sprite_addr_l = (self.sprite_ram_addr & 0x03) as u8;
                self.first_visible_sprite_addr = self.sprite_addr_h.wrapping_mul(4);
                self.last_visible_sprite_addr = self.first_visible_sprite_addr;
            } else if self.cycle == 256 {
                self.sprite0_visible = self.sprite0_added;
                self.sprite_count = self.secondary_oam_addr >> 2;
            }

            if self.cycle & 0x01 != 0 {
                // Odd cycles read a byte from primary OAM.
                let addr = self.sprite_ram_addr as u8;
                self.oam_copybuffer = self.read_sprite_ram(addr);
            } else {
                if self.oam_copy_done {
                    self.sprite_addr_h = (self.sprite_addr_h + 1) & 0x3F;
                    if self.secondary_oam_addr >= 0x20 {
                        self.oam_copybuffer = self
                            .secondary_sprite_ram[(self.secondary_oam_addr & 0x1F) as usize];
                    }
                } else {
                    let sprite_height = if self.flags.large_sprites { 16i16 } else { 8i16 };
                    if !self.sprite_in_range
                        && self.scanline >= self.oam_copybuffer as i16
                        && self.scanline < self.oam_copybuffer as i16 + sprite_height
                    {
                        self.sprite_in_range = true;
                    }

                    if self.secondary_oam_addr < 0x20 {
                        // Copy one byte to secondary OAM.
                        self.secondary_sprite_ram[(self.secondary_oam_addr & 0x1F) as usize] =
                            self.oam_copybuffer;

                        if self.sprite_in_range {
                            self.sprite_addr_l = self.sprite_addr_l.wrapping_add(1);
                            self.secondary_oam_addr += 1;

                            if self.sprite_addr_h == 0 {
                                self.sprite0_added = true;
                            }

                            if (self.secondary_oam_addr & 0x03) == 0 {
                                self.sprite_in_range = false;
                                self.sprite_addr_l = 0;
                                self.last_visible_sprite_addr =
                                    self.sprite_addr_h.wrapping_mul(4);
                                self.sprite_addr_h = (self.sprite_addr_h + 1) & 0x3F;
                                if self.sprite_addr_h == 0 {
                                    self.oam_copy_done = true;
                                }
                            }
                        } else {
                            // Nothing to copy, skip to next sprite.
                            self.sprite_addr_h = (self.sprite_addr_h + 1) & 0x3F;
                            if self.sprite_addr_h == 0 {
                                self.oam_copy_done = true;
                            }
                        }
                    } else {
                        // 8 sprites found; check the next one for overflow and
                        // emulate the PPU's overflow bug.
                        self.oam_copybuffer = self
                            .secondary_sprite_ram[(self.secondary_oam_addr & 0x1F) as usize];

                        if self.sprite_in_range {
                            self.status_flags.sprite_overflow = true;
                            self.sprite_addr_l = self.sprite_addr_l.wrapping_add(1);
                            if self.sprite_addr_l == 4 {
                                self.sprite_addr_h = (self.sprite_addr_h + 1) & 0x3F;
                                self.sprite_addr_l = 0;
                            }

                            if self.overflow_bug_counter == 0 {
                                self.overflow_bug_counter = 3;
                            } else if self.overflow_bug_counter > 0 {
                                self.overflow_bug_counter -= 1;
                                if self.overflow_bug_counter == 0 {
                                    self.oam_copy_done = true;
                                    self.sprite_addr_l = 0;
                                }
                            }
                        } else {
                            // Bug: increment H and L simultaneously.
                            self.sprite_addr_h = (self.sprite_addr_h + 1) & 0x3F;
                            self.sprite_addr_l = (self.sprite_addr_l + 1) & 0x03;
                            if self.sprite_addr_h == 0 {
                                self.oam_copy_done = true;
                            }
                        }
                    }
                }

                self.sprite_ram_addr =
                    ((self.sprite_addr_l & 0x03) | (self.sprite_addr_h << 2)) as u16;
            }
        }
    }

    fn load_sprite_tile_info(&mut self, cart: &mut Cartridge) {
        let base = (self.sprite_index as usize) * 4;
        if base + 3 >= self.secondary_sprite_ram.len() {
            return;
        }
        let sprite_y = self.secondary_sprite_ram[base];
        let tile_index = self.secondary_sprite_ram[base + 1];
        let attributes = self.secondary_sprite_ram[base + 2];
        let sprite_x = self.secondary_sprite_ram[base + 3];
        self.load_sprite(cart, sprite_y, tile_index, attributes, sprite_x, false);
    }

    #[allow(clippy::too_many_arguments)]
    fn load_sprite(
        &mut self,
        cart: &mut Cartridge,
        sprite_y: u8,
        tile_index: u8,
        attributes: u8,
        sprite_x: u8,
        extra_sprite: bool,
    ) {
        let background_priority = (attributes & 0x20) == 0x20;
        let horizontal_mirror = (attributes & 0x40) == 0x40;
        let vertical_mirror = (attributes & 0x80) == 0x80;

        let mut line_offset: u8 = if vertical_mirror {
            let max = if self.flags.large_sprites { 15i16 } else { 7i16 };
            (max - (self.scanline - sprite_y as i16)) as u8
        } else {
            (self.scanline - sprite_y as i16) as u8
        };

        let tile_addr = |line_offset: u8| -> u16 {
            if self.flags.large_sprites {
                let base = (if tile_index & 0x01 != 0 { 0x1000 } else { 0 })
                    | ((tile_index as u16 & !0x01) << 4);
                base + if line_offset >= 8 {
                    line_offset as u16 + 8
                } else {
                    line_offset as u16
                }
            } else {
                ((tile_index as u16) << 4 | self.flags.sprite_pattern_addr) + line_offset as u16
            }
        };

        let mut fetch_last_sprite = true;
        if (self.sprite_index < self.sprite_count || extra_sprite) && sprite_y < 240 {
            let addr = tile_addr(line_offset);
            let info = SpriteInfo {
                background_priority,
                horizontal_mirror,
                vertical_mirror,
                palette_offset: (((attributes & 0x03) as u16) << 2) | 0x10,
                low_byte: 0,
                high_byte: 0,
                tile_addr: addr,
                offset_y: line_offset,
                sprite_x,
            };

            if self.sprite_index < self.sprite_tiles.len() as u32 {
                self.sprite_tiles[self.sprite_index as usize] = info;
            }

            if extra_sprite {
                // Extra sprites (RemoveSpriteLimit) are not supported; this
                // path only exists to keep the upstream structure.
                let low = cart.ppu_read(addr).unwrap_or(addr as u8);
                let high = cart
                    .ppu_read(addr.wrapping_add(8))
                    .unwrap_or(addr.wrapping_add(8) as u8);
                if self.sprite_index < self.sprite_tiles.len() as u32 {
                    self.sprite_tiles[self.sprite_index as usize].low_byte = low;
                    self.sprite_tiles[self.sprite_index as usize].high_byte = high;
                }
            } else {
                fetch_last_sprite = false;
                let low = self.read_vram(cart, addr);
                let high = self.read_vram(cart, addr.wrapping_add(8));
                if self.sprite_index < self.sprite_tiles.len() as u32 {
                    self.sprite_tiles[self.sprite_index as usize].low_byte = low;
                    self.sprite_tiles[self.sprite_index as usize].high_byte = high;
                }
            }

            if self.scanline >= 0 {
                // Sprites fetched on the pre-render line are not shown on
                // scanline 0.
                let x = sprite_x as usize;
                for i in 0..8 {
                    let idx = x + i + 1;
                    if idx < self.has_sprite.len() {
                        self.has_sprite[idx] = true;
                    }
                }
            }
        }

        if fetch_last_sprite {
            // Dummy $FF fetch for remaining slots, observed by MMC3's A12 IRQ.
            line_offset = 0;
            let dummy_index = 0xFF;
            let addr = if self.flags.large_sprites {
                let base = (if dummy_index & 0x01 != 0 { 0x1000 } else { 0 })
                    | ((dummy_index as u16 & !0x01) << 4);
                base + if line_offset >= 8 {
                    line_offset as u16 + 8
                } else {
                    line_offset as u16
                }
            } else {
                ((dummy_index as u16) << 4 | self.flags.sprite_pattern_addr)
                    + line_offset as u16
            };
            self.read_vram(cart, addr);
            self.read_vram(cart, addr.wrapping_add(8));
        }

        self.sprite_index = self.sprite_index.wrapping_add(1);
    }

    // -----------------------------------------------------------------------
    // Deferred register state updates (Mesen's UpdateState)
    // -----------------------------------------------------------------------

    fn update_state(&mut self, cart: &mut Cartridge) {
        self.need_state_update = false;

        // Rendering enable/disable takes effect one PPU cycle later.
        if self.prev_rendering_enabled != self.rendering_enabled {
            self.prev_rendering_enabled = self.rendering_enabled;
            if self.scanline < 240 {
                if self.prev_rendering_enabled {
                    // Rendering just enabled; pending OAM row corruption would
                    // be applied here (optional emulation flag, off by default).
                } else {
                    // Rendering just disabled mid-screen.
                    self.set_bus_address(cart, self.video_ram_addr & 0x3FFF);

                    if self.cycle >= 65 && self.cycle <= 256 {
                        // Disabling during OAM evaluation bumps the address.
                        self.sprite_ram_addr = (self.sprite_ram_addr + 1) & 0xFF;
                        self.sprite_addr_h = ((self.sprite_ram_addr >> 2) & 0x3F) as u8;
                        self.sprite_addr_l = (self.sprite_ram_addr & 0x03) as u8;
                    }
                }
            }
        }

        if self.rendering_enabled
            != (self.flags.background_enabled | self.flags.sprites_enabled)
        {
            self.rendering_enabled =
                self.flags.background_enabled | self.flags.sprites_enabled;
            self.need_state_update = true;
        }

        if self.update_vram_addr_delay > 0 {
            self.update_vram_addr_delay -= 1;
            if self.update_vram_addr_delay == 0 {
                self.video_ram_addr = self.update_vram_addr;
                self.tmp_video_ram_addr = self.video_ram_addr;

                if self.scanline >= 240 || !self.rendering_enabled {
                    self.set_bus_address(cart, self.video_ram_addr & 0x3FFF);
                }
            } else {
                self.need_state_update = true;
            }
        }

        if self.ignore_vram_read > 0 {
            self.ignore_vram_read -= 1;
            if self.ignore_vram_read > 0 {
                self.need_state_update = true;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Save states
    // -----------------------------------------------------------------------

    pub fn save_state(&self, w: &mut StateWriter) {
        let region = match self.region {
            Region::Ntsc => 0u8,
            Region::Pal => 1,
            Region::Dendy => 2,
        };
        w.u8(region);
        w.i16(self.scanline);
        w.u32(self.cycle);
        w.u64(self.frame_count);
        w.u64(self.total_dots);
        w.bool(self.frame_ready);
        w.bool(self.nmi_flag);
        w.bool(self.status_flags.sprite_overflow);
        w.bool(self.status_flags.sprite0_hit);
        w.bool(self.status_flags.vertical_blank);

        w.u8(self.control);
        w.u8(self.mask);
        w.u8(self.status);
        w.u16(self.video_ram_addr);
        w.u16(self.tmp_video_ram_addr);
        w.u8(self.x_scroll);
        w.bool(self.write_toggle);
        w.u16(self.sprite_ram_addr);
        w.u16(self.ppu_bus_address);
        w.u16(self.low_bit_shift);
        w.u16(self.high_bit_shift);
        w.u8(self.memory_read_buffer);
        w.u8(self.open_bus);

        w.fixed_bytes(&self.palette_ram);
        w.fixed_bytes(&self.sprite_ram);
        w.fixed_bytes(&self.secondary_sprite_ram);
        for &flag in &self.has_sprite {
            w.bool(flag);
        }

        for tile in [&self.previous_tile, &self.current_tile, &self.next_tile] {
            w.u8(tile.low_byte);
            w.u8(tile.high_byte);
            w.u16(tile.palette_offset);
            w.u16(tile.tile_addr);
            w.u8(tile.offset_y);
        }
        for sprite in &self.sprite_tiles {
            w.u8(sprite.low_byte);
            w.u8(sprite.high_byte);
            w.u16(sprite.palette_offset);
            w.u16(sprite.tile_addr);
            w.u8(sprite.offset_y);
            w.u8(sprite.sprite_x);
            w.bool(sprite.horizontal_mirror);
            w.bool(sprite.vertical_mirror);
            w.bool(sprite.background_priority);
        }

        w.u32(self.sprite_count);
        w.u32(self.secondary_oam_addr);
        w.u32(self.sprite_index);
        w.bool(self.sprite0_visible);
        w.bool(self.oam_copy_done);
        w.bool(self.sprite_in_range);
        w.bool(self.sprite0_added);
        w.u8(self.sprite_addr_h);
        w.u8(self.sprite_addr_l);
        w.u8(self.overflow_bug_counter);
        w.u8(self.oam_copybuffer);
        w.u8(self.first_visible_sprite_addr);
        w.u8(self.last_visible_sprite_addr);

        w.bool(self.need_state_update);
        w.bool(self.rendering_enabled);
        w.bool(self.prev_rendering_enabled);
        w.bool(self.prevent_vbl_flag);
        w.u16(self.update_vram_addr);
        w.u8(self.update_vram_addr_delay);
        w.u8(self.ignore_vram_read.min(0xFF) as u8);
        w.u16(self.intensify_color_bits);
        w.u8(self.palette_ram_mask);
        w.i32(self.last_updated_pixel);

        w.bool(self.enable_oam_decay);
        for &cycles in &self.oam_decay_cycles {
            w.u64(cycles);
        }
        for &stamp in &self.open_bus_decay_stamp {
            w.i32(stamp);
        }

        let mut pixels = Vec::with_capacity(SCREEN_PIXELS * 2);
        for &pixel in &self.output_buffer {
            pixels.extend_from_slice(&pixel.to_le_bytes());
        }
        w.fixed_bytes(&pixels);

        for &pixel in &self.framebuffer {
            w.u32(pixel);
        }
    }

    pub fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        let region = match r.u8()? {
            1 => Region::Pal,
            2 => Region::Dendy,
            _ => Region::Ntsc,
        };
        self.configure_region(region);

        self.scanline = r.i16()?;
        self.cycle = r.u32()?;
        self.frame_count = r.u64()?;
        self.total_dots = r.u64()?;
        self.frame_ready = r.bool()?;
        self.nmi_flag = r.bool()?;
        self.status_flags.sprite_overflow = r.bool()?;
        self.status_flags.sprite0_hit = r.bool()?;
        self.status_flags.vertical_blank = r.bool()?;

        self.control = r.u8()?;
        self.mask = r.u8()?;
        self.status = r.u8()?;
        self.video_ram_addr = r.u16()?;
        self.tmp_video_ram_addr = r.u16()?;
        self.x_scroll = r.u8()?;
        self.write_toggle = r.bool()?;
        self.sprite_ram_addr = r.u16()?;
        self.ppu_bus_address = r.u16()?;
        self.low_bit_shift = r.u16()?;
        self.high_bit_shift = r.u16()?;
        self.memory_read_buffer = r.u8()?;
        self.open_bus = r.u8()?;

        let palette = r.fixed_bytes(0x20)?;
        self.palette_ram.copy_from_slice(&palette);
        let oam = r.fixed_bytes(0x100)?;
        self.sprite_ram.copy_from_slice(&oam);
        let secondary = r.fixed_bytes(0x20)?;
        self.secondary_sprite_ram.copy_from_slice(&secondary);
        for flag in &mut self.has_sprite {
            *flag = r.bool()?;
        }

        for tile in [
            &mut self.previous_tile,
            &mut self.current_tile,
            &mut self.next_tile,
        ] {
            tile.low_byte = r.u8()?;
            tile.high_byte = r.u8()?;
            tile.palette_offset = r.u16()?;
            tile.tile_addr = r.u16()?;
            tile.offset_y = r.u8()?;
        }
        for sprite in &mut self.sprite_tiles {
            sprite.low_byte = r.u8()?;
            sprite.high_byte = r.u8()?;
            sprite.palette_offset = r.u16()?;
            sprite.tile_addr = r.u16()?;
            sprite.offset_y = r.u8()?;
            sprite.sprite_x = r.u8()?;
            sprite.horizontal_mirror = r.bool()?;
            sprite.vertical_mirror = r.bool()?;
            sprite.background_priority = r.bool()?;
        }

        self.sprite_count = r.u32()?;
        self.secondary_oam_addr = r.u32()?;
        self.sprite_index = r.u32()?;
        self.sprite0_visible = r.bool()?;
        self.oam_copy_done = r.bool()?;
        self.sprite_in_range = r.bool()?;
        self.sprite0_added = r.bool()?;
        self.sprite_addr_h = r.u8()?;
        self.sprite_addr_l = r.u8()?;
        self.overflow_bug_counter = r.u8()?;
        self.oam_copybuffer = r.u8()?;
        self.first_visible_sprite_addr = r.u8()?;
        self.last_visible_sprite_addr = r.u8()?;

        self.need_state_update = r.bool()?;
        self.rendering_enabled = r.bool()?;
        self.prev_rendering_enabled = r.bool()?;
        self.prevent_vbl_flag = r.bool()?;
        self.update_vram_addr = r.u16()?;
        self.update_vram_addr_delay = r.u8()?;
        self.ignore_vram_read = r.u8()? as u32;
        self.intensify_color_bits = r.u16()?;
        self.palette_ram_mask = r.u8()?;
        self.last_updated_pixel = r.i32()?;

        self.enable_oam_decay = r.bool()?;
        for cycles in &mut self.oam_decay_cycles {
            *cycles = r.u64()?;
        }
        for stamp in &mut self.open_bus_decay_stamp {
            *stamp = r.i32()?;
        }

        let pixels = r.fixed_bytes(SCREEN_PIXELS * 2)?;
        for (i, chunk) in pixels.chunks_exact(2).enumerate() {
            if i < self.output_buffer.len() {
                self.output_buffer[i] = u16::from_le_bytes([chunk[0], chunk[1]]);
            }
        }
        for pixel in &mut self.framebuffer {
            *pixel = r.u32()?;
        }

        // Rebuild derived state that is not serialized.
        self.rebuild_control_flags();
        self.rebuild_mask_flags();
        self.update_minimum_draw_cycles();
        // VENDORED-DELTA (HD recording): the tile log is transient and must
        // never leak records across a save-state load.
        self.hd_pixels.clear();

        Ok(())
    }
}

impl Default for Ppu {
    fn default() -> Self {
        Self::new(Region::Ntsc)
    }
}

/// Convert Mesen's 16-bit PPU output (color index + emphasis bits) to XRGB.
///
/// Emphasis follows Mesen's `GenerateFullColorPalette`: the selected channel is
/// scaled by 1.1 and the other two by 0.9.
fn emphasize_pixel(pixel: u16) -> u32 {
    let base = palette_color((pixel & 0x3F) as u8);
    let emphasis = (pixel >> 6) & 0x07;
    if emphasis == 0 {
        return base;
    }

    let mut r = ((base >> 16) & 0xFF) as f64;
    let mut g = ((base >> 8) & 0xFF) as f64;
    let mut b = (base & 0xFF) as f64;

    if emphasis & 0x01 != 0 {
        // Intensify red.
        r *= 1.1;
        g *= 0.9;
        b *= 0.9;
    }
    if emphasis & 0x02 != 0 {
        // Intensify green.
        g *= 1.1;
        r *= 0.9;
        b *= 0.9;
    }
    if emphasis & 0x04 != 0 {
        // Intensify blue.
        b *= 1.1;
        r *= 0.9;
        g *= 0.9;
    }

    let r = r.min(255.0) as u32;
    let g = g.min(255.0) as u32;
    let b = b.min(255.0) as u32;
    (r << 16) | (g << 8) | b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Mirroring, RomData, RomInfo, RomSystem};

    fn nrom_cart() -> Cartridge {
        nrom_cart_with_mirroring(Mirroring::Horizontal)
    }

    fn nrom_cart_with_mirroring(mirroring: Mirroring) -> Cartridge {
        let info = RomInfo {
            mapper: 0,
            mirroring,
            system: RomSystem::Nes,
            prg_rom_size: 0x8000,
            chr_rom_size: 0,
            chr_ram_size: 0x2000,
            prg_ram_size: 0x2000,
            ..Default::default()
        };
        let rom = RomData::new(info, vec![0xEA; 0x8000], Vec::new());
        Cartridge::from_rom_data(rom).expect("NROM cartridge")
    }

    fn tick_until(ppu: &mut Ppu, cart: &mut Cartridge, scanline: i16, cycle: u32) {
        for _ in 0..(400 * DOTS_PER_LINE) {
            ppu.tick(cart);
            if ppu.scanline() == scanline && ppu.cycle() == cycle {
                return;
            }
        }
        panic!(
            "PPU did not reach line {} cycle {} (at {}:{})",
            scanline,
            cycle,
            ppu.scanline(),
            ppu.cycle()
        );
    }

    fn run_frame(ppu: &mut Ppu, cart: &mut Cartridge) {
        ppu.take_frame_ready();
        for _ in 0..(400 * DOTS_PER_LINE) {
            ppu.tick(cart);
            if ppu.take_frame_ready() {
                return;
            }
        }
        panic!("PPU did not finish a frame");
    }

    #[test]
    fn frame_timing_and_ready_flag() {
        let mut cart = nrom_cart();
        let mut ppu = Ppu::new(Region::Ntsc);
        let mut ticks = 0u32;
        loop {
            ppu.tick(&mut cart);
            ticks += 1;
            if ppu.take_frame_ready() {
                break;
            }
            assert!(ticks < 1_000_000);
        }
        // First frame starts at scanline 0, no pre-render line; the frame is
        // signalled at the start of line 240.
        assert_eq!(ticks, 240 * DOTS_PER_LINE + 1);
        assert_eq!(ppu.scanline(), 240);
        assert_eq!(ppu.cycle(), 0);
        assert_eq!(ppu.frame_count(), 1);
    }

    #[test]
    fn status_and_nmi() {
        let mut cart = nrom_cart();
        let mut ppu = Ppu::new(Region::Ntsc);
        ppu.write_register(0x2000, 0x80, &mut cart);

        tick_until(&mut ppu, &mut cart, 241, 1);
        assert!(ppu.status_flags.vertical_blank);
        assert!(ppu.nmi_flag);
        assert!(ppu.take_nmi());
        assert!(!ppu.take_nmi());

        // Writing $2000 with NMI already enabled does not create a new edge.
        ppu.write_register(0x2000, 0x80, &mut cart);
        assert!(!ppu.nmi_flag);

        // Toggling NMI output while vblank is still set generates another NMI.
        ppu.write_register(0x2000, 0x00, &mut cart);
        assert!(!ppu.nmi_flag);
        ppu.write_register(0x2000, 0x80, &mut cart);
        assert!(ppu.take_nmi());

        // Reading $2002 returns the vblank bit, clears it and suppresses NMI.
        let status = ppu.read_register(0x2002, &mut cart);
        assert_eq!(status & 0x80, 0x80);
        assert!(!ppu.status_flags.vertical_blank);
        assert!(!ppu.take_nmi());

        // With the vblank flag cleared, enabling NMI does not trigger one
        // (Mesen keeps this tied to the status flag).
        ppu.write_register(0x2000, 0x80, &mut cart);
        assert!(!ppu.nmi_flag);
    }

    #[test]
    fn reading_status_before_vblank_suppresses_flag_and_nmi() {
        let mut cart = nrom_cart();
        let mut ppu = Ppu::new(Region::Ntsc);
        ppu.write_register(0x2000, 0x80, &mut cart);

        tick_until(&mut ppu, &mut cart, 241, 0);
        // Read one PPU clock before vblank starts.
        let status = ppu.read_register(0x2002, &mut cart);
        assert_eq!(status & 0x80, 0);
        ppu.tick(&mut cart); // now at 241:1
        assert!(!ppu.status_flags.vertical_blank);
        assert!(!ppu.nmi_flag);
    }

    #[test]
    fn disabling_nmi_before_vblank_prevents_nmi() {
        let mut cart = nrom_cart();
        let mut ppu = Ppu::new(Region::Ntsc);
        ppu.write_register(0x2000, 0x80, &mut cart);

        // Disable NMI on the dot before vblank starts.
        tick_until(&mut ppu, &mut cart, 241, 0);
        ppu.write_register(0x2000, 0x00, &mut cart);
        ppu.tick(&mut cart); // 241:1
        assert!(ppu.status_flags.vertical_blank);
        assert!(!ppu.nmi_flag);

        // Re-enabling while vblank is still set creates a new NMI edge.
        ppu.write_register(0x2000, 0x80, &mut cart);
        assert!(ppu.take_nmi());
        // ...but writing the same value again does not.
        ppu.write_register(0x2000, 0x80, &mut cart);
        assert!(!ppu.nmi_flag);
    }

    #[test]
    fn oam_dma_copies_256_bytes() {
        let mut cart = nrom_cart();
        let mut ppu = Ppu::new(Region::Ntsc);
        let data: Vec<u8> = (0..=255u8).collect();
        ppu.oam_dma(&data);
        assert_eq!(&ppu.sprite_ram[..], &data[..]);

        ppu.write_register(0x2003, 0x00, &mut cart);
        let value = ppu.read_register(0x2004, &mut cart);
        assert_eq!(value, 0x00);
        ppu.write_register(0x2003, 0x7F, &mut cart);
        let value = ppu.read_register(0x2004, &mut cart);
        assert_eq!(value, 0x7F);

        // Hardware starts the transfer at OAMADDR and wraps around.
        ppu.write_register(0x2003, 0x80, &mut cart);
        ppu.oam_dma(&data);
        assert_eq!(ppu.sprite_ram[0x80], 0x00);
        assert_eq!(ppu.sprite_ram[0x00], 0x80);
        assert_eq!(ppu.sprite_ram[0x7F], 0xFF);
        assert_eq!(ppu.sprite_ram_addr, 0x80);
    }

    #[test]
    fn palette_read_fills_buffer_from_nametable() {
        let mut cart = nrom_cart();
        let mut ppu = Ppu::new(Region::Ntsc);

        // The palette read buffer is loaded with the nametable byte at
        // $2F00-$2FFF (mirror of the palette address).
        cart.ppu_write(0x2F00, 0x5A);

        ppu.write_register(0x2006, 0x3F, &mut cart);
        ppu.write_register(0x2006, 0x00, &mut cart);
        for _ in 0..8 {
            ppu.tick(&mut cart);
        }
        let palette_value = ppu.read_register(0x2007, &mut cart);
        assert_eq!(palette_value & 0x3F, ppu.palette_ram[0]);

        // Next non-palette read returns the buffer filled by the palette read.
        ppu.write_register(0x2006, 0x20, &mut cart);
        ppu.write_register(0x2006, 0x00, &mut cart);
        for _ in 0..8 {
            ppu.tick(&mut cart);
        }
        let buffered = ppu.read_register(0x2007, &mut cart);
        assert_eq!(buffered, 0x5A);
    }

    #[test]
    fn palette_mirroring() {
        let mut cart = nrom_cart();
        let mut ppu = Ppu::new(Region::Ntsc);

        // Write 0x2A to $3F10 (mirrors $3F00).
        ppu.write_register(0x2006, 0x3F, &mut cart);
        ppu.write_register(0x2006, 0x10, &mut cart);
        for _ in 0..4 {
            ppu.tick(&mut cart);
        }
        ppu.write_register(0x2007, 0x2A, &mut cart);
        assert_eq!(ppu.palette_ram[0x00], 0x2A);
        assert_eq!(ppu.palette_ram[0x10], 0x2A);

        // Palette reads are immediate (not buffered) and mirror $10/$14/... .
        ppu.write_register(0x2006, 0x3F, &mut cart);
        ppu.write_register(0x2006, 0x00, &mut cart);
        for _ in 0..4 {
            ppu.tick(&mut cart);
        }
        let value = ppu.read_register(0x2007, &mut cart);
        assert_eq!(value & 0x3F, 0x2A);

        ppu.write_register(0x2006, 0x3F, &mut cart);
        ppu.write_register(0x2006, 0x14, &mut cart);
        for _ in 0..8 {
            ppu.tick(&mut cart);
        }
        let value = ppu.read_register(0x2007, &mut cart);
        assert_eq!(value & 0x3F, ppu.palette_ram[0x04]);
    }

    #[test]
    fn scroll_and_address_registers() {
        let mut cart = nrom_cart();
        let mut ppu = Ppu::new(Region::Ntsc);

        ppu.write_register(0x2000, 0x03, &mut cart);
        assert_eq!(ppu.tmp_video_ram_addr & 0x0C00, 0x0C00);

        // $2005 first write: X scroll + coarse X.
        ppu.write_register(0x2005, 0x7D, &mut cart);
        assert_eq!(ppu.x_scroll, 5);
        assert_eq!(ppu.tmp_video_ram_addr & 0x1F, 0x0F);
        assert!(ppu.write_toggle);
        // $2005 second write: fine Y + coarse Y.
        ppu.write_register(0x2005, 0x5E, &mut cart);
        assert_eq!((ppu.tmp_video_ram_addr & 0x7000) >> 12, 6);
        assert_eq!((ppu.tmp_video_ram_addr >> 5) & 0x1F, 0x0B);
        assert!(!ppu.write_toggle);

        // $2006 writes t, then v after a 3-dot delay.
        ppu.write_register(0x2006, 0x21, &mut cart);
        assert_eq!(ppu.tmp_video_ram_addr & 0x3F00, 0x2100);
        ppu.write_register(0x2006, 0x08, &mut cart);
        assert_eq!(ppu.tmp_video_ram_addr & 0xFF, 0x08);
        assert!(ppu.need_state_update);
        for _ in 0..3 {
            ppu.tick(&mut cart);
        }
        assert_eq!(ppu.video_ram_addr, ppu.tmp_video_ram_addr);
        assert_eq!(ppu.video_ram_addr & 0x3FFF, 0x2108);

        // Horizontal increment wraps coarse X and toggles the nametable.
        ppu.video_ram_addr = 0x041F;
        ppu.inc_horizontal_scrolling();
        assert_eq!(ppu.video_ram_addr, 0x0000);

        // Vertical increment at coarse Y=29/fine Y=7 wraps Y and toggles the
        // vertical nametable bit.
        ppu.video_ram_addr = 0x73A0;
        ppu.inc_vertical_scrolling();
        assert_eq!(ppu.video_ram_addr, 0x0800);

        // Ordinary fine-Y increment.
        ppu.video_ram_addr = 0x0200;
        ppu.inc_vertical_scrolling();
        assert_eq!(ppu.video_ram_addr, 0x1200);
    }

    #[test]
    fn background_rendering_and_left_column_clipping() {
        let mut cart = nrom_cart();
        let mut ppu = Ppu::new(Region::Ntsc);

        // Tile 0 is solid color 1 (low plane all ones).
        for i in 0..8 {
            cart.ppu_write(i, 0xFF);
            cart.ppu_write(8 + i, 0x00);
        }
        for i in 0..0x400 {
            cart.ppu_write(0x2000 + i, 0x00);
        }

        // Palette: backdrop 0x0F, color 1 = 0x01.
        ppu.write_register(0x2006, 0x3F, &mut cart);
        ppu.write_register(0x2006, 0x00, &mut cart);
        for _ in 0..4 {
            ppu.tick(&mut cart);
        }
        for value in [0x0Fu8, 0x01, 0x02, 0x03] {
            ppu.write_register(0x2007, value, &mut cart);
        }

        // Put v/t back to 0 and enable background with left column.
        ppu.write_register(0x2006, 0x00, &mut cart);
        ppu.write_register(0x2006, 0x00, &mut cart);
        ppu.write_register(0x2005, 0x00, &mut cart);
        ppu.write_register(0x2005, 0x00, &mut cart);
        ppu.write_register(0x2001, 0x0A, &mut cart);

        // First frame has no pre-render line; run two to settle the pipeline.
        run_frame(&mut ppu, &mut cart);
        run_frame(&mut ppu, &mut cart);
        assert_eq!(ppu.framebuffer()[0], palette_color(0x01));
        assert_eq!(
            ppu.framebuffer()[8 * SCREEN_WIDTH + 8],
            palette_color(0x01)
        );

        // Disable the left column: x = 0..7 should show the backdrop.
        ppu.write_register(0x2001, 0x08, &mut cart);
        run_frame(&mut ppu, &mut cart);
        run_frame(&mut ppu, &mut cart);
        assert_eq!(ppu.framebuffer()[0], palette_color(0x0F));
        assert_eq!(ppu.framebuffer()[7], palette_color(0x0F));
        assert_eq!(ppu.framebuffer()[8], palette_color(0x01));
    }

    #[test]
    fn sprite_zero_hit() {
        let mut cart = nrom_cart();
        let mut ppu = Ppu::new(Region::Ntsc);

        // Solid background tile 0 (color 1).
        for i in 0..8 {
            cart.ppu_write(i, 0xFF);
            cart.ppu_write(8 + i, 0x00);
        }
        for i in 0..0x400 {
            cart.ppu_write(0x2000 + i, 0x00);
        }

        // Palette entry 1 for both BG palette 0 and sprite palette 0.
        ppu.write_register(0x2006, 0x3F, &mut cart);
        ppu.write_register(0x2006, 0x00, &mut cart);
        for _ in 0..4 {
            ppu.tick(&mut cart);
        }
        for value in [0x0Fu8, 0x01, 0x02, 0x03, 0x0F, 0x01, 0x02, 0x03] {
            ppu.write_register(0x2007, value, &mut cart);
        }

        // Sprite 0: Y=0 (top-1 => displayed on line 1), tile 0, no flips, X=0.
        ppu.write_register(0x2003, 0x00, &mut cart);
        for value in [0x00u8, 0x00, 0x00, 0x00] {
            ppu.write_register(0x2004, value, &mut cart);
        }
        // Sprite evaluation starts at OAMADDR; point it back at sprite 0.
        ppu.write_register(0x2003, 0x00, &mut cart);

        ppu.write_register(0x2006, 0x00, &mut cart);
        ppu.write_register(0x2006, 0x00, &mut cart);
        ppu.write_register(0x2005, 0x00, &mut cart);
        ppu.write_register(0x2005, 0x00, &mut cart);
        // Background + sprites, both left columns visible.
        ppu.write_register(0x2001, 0x1E, &mut cart);

        tick_until(&mut ppu, &mut cart, 1, 2);
        assert!(
            ppu.status_flags.sprite0_hit,
            "sprite 0 should hit on scanline 1 at x=0"
        );
        let status = ppu.read_register(0x2002, &mut cart);
        assert_eq!(status & 0x40, 0x40);
    }

    #[test]
    fn sprite_zero_hit_is_masked_in_left_column() {
        let mut cart = nrom_cart();
        let mut ppu = Ppu::new(Region::Ntsc);

        for i in 0..8 {
            cart.ppu_write(i, 0xFF);
            cart.ppu_write(8 + i, 0x00);
        }
        for i in 0..0x400 {
            cart.ppu_write(0x2000 + i, 0x00);
        }
        ppu.write_register(0x2006, 0x3F, &mut cart);
        ppu.write_register(0x2006, 0x00, &mut cart);
        for _ in 0..8 {
            ppu.tick(&mut cart);
        }
        for value in [0x0Fu8, 0x01, 0x02, 0x03, 0x0F, 0x01, 0x02, 0x03] {
            ppu.write_register(0x2007, value, &mut cart);
        }

        ppu.write_register(0x2003, 0x00, &mut cart);
        for value in [0x00u8, 0x00, 0x00, 0x00] {
            ppu.write_register(0x2004, value, &mut cart);
        }
        ppu.write_register(0x2003, 0x00, &mut cart);

        ppu.write_register(0x2006, 0x00, &mut cart);
        ppu.write_register(0x2006, 0x00, &mut cart);
        ppu.write_register(0x2005, 0x00, &mut cart);
        ppu.write_register(0x2005, 0x00, &mut cart);
        // Sprites + background on, both left columns clipped.
        ppu.write_register(0x2001, 0x18, &mut cart);

        tick_until(&mut ppu, &mut cart, 1, 2);
        assert!(
            !ppu.status_flags.sprite0_hit,
            "sprite 0 hit must not occur in the clipped left column"
        );

        // Sprites are also not drawn there; line 1 pixel 0 is the backdrop.
        assert_eq!(ppu.output_buffer[SCREEN_WIDTH], 0x0F);
    }

    #[test]
    fn pal_frame_length_and_odd_frame_skip() {
        // NTSC: the second frame includes the skipped pre-render dot. Rendering
        // must be enabled for the skip to happen.
        let mut cart = nrom_cart();
        let mut ppu = Ppu::new(Region::Ntsc);
        ppu.write_register(0x2001, 0x08, &mut cart);
        let count_ticks = |ppu: &mut Ppu, cart: &mut Cartridge| {
            let mut ticks = 0u32;
            loop {
                ppu.tick(cart);
                ticks += 1;
                if ppu.take_frame_ready() {
                    return ticks;
                }
                assert!(ticks < 1_000_000);
            }
        };
        let first = count_ticks(&mut ppu, &mut cart);
        assert_eq!(first, 240 * DOTS_PER_LINE + 1);
        let second = count_ticks(&mut ppu, &mut cart);
        assert_eq!(second, 262 * DOTS_PER_LINE - 1);
        let third = count_ticks(&mut ppu, &mut cart);
        assert_eq!(third, 262 * DOTS_PER_LINE);

        // PAL: 312 lines, and no odd-frame dot skip.
        let mut ppu = Ppu::new(Region::Pal);
        ppu.write_register(0x2001, 0x08, &mut cart);
        let _ = count_ticks(&mut ppu, &mut cart);
        let pal_frame = count_ticks(&mut ppu, &mut cart);
        let pal_frame2 = count_ticks(&mut ppu, &mut cart);
        assert_eq!(pal_frame, 312 * DOTS_PER_LINE);
        assert_eq!(pal_frame2, 312 * DOTS_PER_LINE);
    }

    #[test]
    fn grayscale_and_emphasis_output() {
        let mut cart = nrom_cart();
        let mut ppu = Ppu::new(Region::Ntsc);

        for i in 0..8 {
            cart.ppu_write(i, 0xFF);
            cart.ppu_write(8 + i, 0x00);
        }
        for i in 0..0x400 {
            cart.ppu_write(0x2000 + i, 0x00);
        }

        ppu.write_register(0x2006, 0x3F, &mut cart);
        ppu.write_register(0x2006, 0x00, &mut cart);
        for _ in 0..8 {
            ppu.tick(&mut cart);
        }
        // Backdrop 0x21, background color 1 = 0x16.
        for value in [0x21u8, 0x16] {
            ppu.write_register(0x2007, value, &mut cart);
        }
        ppu.write_register(0x2006, 0x00, &mut cart);
        ppu.write_register(0x2006, 0x00, &mut cart);
        ppu.write_register(0x2005, 0x00, &mut cart);
        ppu.write_register(0x2005, 0x00, &mut cart);
        for _ in 0..4 {
            ppu.tick(&mut cart);
        }
        // Grayscale + background + left column.
        ppu.write_register(0x2001, 0x0B, &mut cart);

        run_frame(&mut ppu, &mut cart);
        run_frame(&mut ppu, &mut cart);
        // 0x16 & 0x30 = 0x10.
        assert_eq!(ppu.framebuffer()[0], palette_color(0x10));
        assert_eq!(ppu.framebuffer()[8], palette_color(0x10));

        // Red emphasis (no grayscale): 0x16 -> (r*1.1, g*0.9, b*0.9).
        ppu.write_register(0x2001, 0x2A, &mut cart);
        run_frame(&mut ppu, &mut cart);
        run_frame(&mut ppu, &mut cart);
        assert_eq!(ppu.framebuffer()[0], 0x00_C72C1C);
        assert_eq!(ppu.framebuffer()[8], 0x00_C72C1C);
    }

    #[test]
    fn sprite_overflow_evaluation_bug() {
        let mut cart = nrom_cart();
        let mut ppu = Ppu::new(Region::Ntsc);

        // 64 sprites at Y=0 (displayed on line 1) => the 9th in-range sprite
        // sets the overflow flag during line 0's evaluation.
        ppu.oam_dma(&[0u8; 0x100]);
        ppu.write_register(0x2001, 0x10, &mut cart); // sprites enabled

        tick_until(&mut ppu, &mut cart, 0, 257);
        assert!(ppu.status_flags.sprite_overflow);
        assert_eq!(ppu.sprite_count, 8);
        let status = ppu.read_register(0x2002, &mut cart);
        assert_eq!(status & 0x20, 0x20);
    }

    #[test]
    fn mid_scanline_vram_address_change() {
        let mut cart = nrom_cart_with_mirroring(Mirroring::Vertical);
        let mut ppu = Ppu::new(Region::Ntsc);

        // Tile 0 = color 1, tile 1 = color 2.
        for i in 0..8 {
            cart.ppu_write(i, 0xFF);
            cart.ppu_write(8 + i, 0x00);
            cart.ppu_write(16 + i, 0x00);
            cart.ppu_write(24 + i, 0xFF);
        }
        // Nametable 0 (physical bank 0) = tile 0; nametable 1 (bank 1) = tile 1.
        for i in 0..0x400 {
            cart.ppu_write(0x2000 + i, 0x00);
            cart.ppu_write(0x2400 + i, 0x01);
        }

        ppu.write_register(0x2006, 0x3F, &mut cart);
        ppu.write_register(0x2006, 0x00, &mut cart);
        for _ in 0..8 {
            ppu.tick(&mut cart);
        }
        for value in [0x0Fu8, 0x01, 0x02, 0x03] {
            ppu.write_register(0x2007, value, &mut cart);
        }
        ppu.write_register(0x2006, 0x00, &mut cart);
        ppu.write_register(0x2006, 0x00, &mut cart);
        ppu.write_register(0x2005, 0x00, &mut cart);
        ppu.write_register(0x2005, 0x00, &mut cart);
        for _ in 0..4 {
            ppu.tick(&mut cart);
        }
        ppu.write_register(0x2001, 0x0A, &mut cart);

        // Settle the pipeline, then change v to nametable 1 in the middle of
        // scanline 100 of the next frame.
        run_frame(&mut ppu, &mut cart);
        run_frame(&mut ppu, &mut cart);
        tick_until(&mut ppu, &mut cart, 100, 100);
        ppu.write_register(0x2006, 0x24, &mut cart);
        ppu.write_register(0x2006, 0x00, &mut cart);
        run_frame(&mut ppu, &mut cart);

        // Before the write the line uses nametable 0 (color 1); afterwards it
        // switches to nametable 1 (color 2).
        assert_eq!(
            ppu.framebuffer()[100 * SCREEN_WIDTH + 50],
            palette_color(0x01)
        );
        assert_eq!(
            ppu.framebuffer()[100 * SCREEN_WIDTH + 200],
            palette_color(0x02)
        );
        assert_eq!(
            ppu.framebuffer()[99 * SCREEN_WIDTH + 200],
            palette_color(0x01)
        );
    }

    #[test]
    fn save_state_round_trip() {
        let mut cart = nrom_cart();
        let mut ppu = Ppu::new(Region::Ntsc);
        ppu.write_register(0x2000, 0xA0, &mut cart);
        ppu.write_register(0x2001, 0x1E, &mut cart);
        ppu.write_register(0x2005, 0x55, &mut cart);
        ppu.write_register(0x2005, 0xAA, &mut cart);
        ppu.oam_dma(&(0..=255u8).collect::<Vec<_>>());
        for _ in 0..1000 {
            ppu.tick(&mut cart);
        }

        let mut writer = StateWriter::new();
        ppu.save_state(&mut writer);
        let data = writer.into_inner();

        let mut restored = Ppu::new(Region::Ntsc);
        let mut reader = StateReader::new(&data);
        restored.load_state(&mut reader).expect("load state");

        assert_eq!(restored.scanline, ppu.scanline);
        assert_eq!(restored.cycle, ppu.cycle);
        assert_eq!(restored.frame_count, ppu.frame_count);
        assert_eq!(restored.video_ram_addr, ppu.video_ram_addr);
        assert_eq!(restored.tmp_video_ram_addr, ppu.tmp_video_ram_addr);
        assert_eq!(restored.x_scroll, ppu.x_scroll);
        assert_eq!(restored.write_toggle, ppu.write_toggle);
        assert_eq!(restored.control, ppu.control);
        assert_eq!(restored.mask, ppu.mask);
        assert_eq!(restored.palette_ram, ppu.palette_ram);
        assert_eq!(restored.sprite_ram, ppu.sprite_ram);
        assert_eq!(restored.framebuffer, ppu.framebuffer);
        assert_eq!(restored.nmi_flag, ppu.nmi_flag);
        assert_eq!(
            restored.status_flags.vertical_blank,
            ppu.status_flags.vertical_blank
        );
        assert_eq!(
            restored.status_flags.sprite0_hit,
            ppu.status_flags.sprite0_hit
        );
        assert_eq!(
            restored.status_flags.sprite_overflow,
            ppu.status_flags.sprite_overflow
        );

        // Re-serializing must produce identical bytes (stable field order).
        let mut writer2 = StateWriter::new();
        restored.save_state(&mut writer2);
        assert_eq!(writer2.into_inner(), data);
    }
}
