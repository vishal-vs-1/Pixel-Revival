//! Namco, Sunsoft, Jaleco, Irem, Taito and Bandai mappers.
//!
//! Direct translation of Mesen 0.9.9's vendor mapper implementations
//! (`Namco163.h`, `Namco163Audio.h`, `Sunsoft*.h`, `Jaleco*.h`, `Irem*.h`,
//! `Taito*.h`, `Bandai*.h`).  GPL-3.0-or-later.
//!
//! OWNER: mapper-vendors agent.

use super::{Mapper, MapperCommon};
use crate::state::{StateReader, StateResult, StateWriter};
use crate::types::{Mirroring, RomData};

// ---------------------------------------------------------------------------
// ID dispatch
// ---------------------------------------------------------------------------

/// Mapper IDs implemented by this module.
///
/// Note: upstream Mesen maps 150 to `Sachen74LS374N` and 164 to `Waixing164`;
/// 88 is Namcot 108/118 (not Konami).  We implement those boards here because
/// they were requested, even though their vendor labels differ.
pub fn supports(id: u16) -> bool {
    matches!(
        id,
        16 | 18 | 19 | 32 | 33 | 48 | 65 | 67 | 68 | 69 | 70 | 72 | 82 | 86 | 87 | 88 | 89 | 92
            | 93 | 140 | 150 | 159 | 164 | 184 | 210
    )
}

pub fn create(id: u16, rom: RomData) -> Option<Box<dyn Mapper>> {
    Some(match id {
        16 | 159 => Box::new(BandaiFcg::new(rom)),
        18 => Box::new(JalecoSs88006::new(rom)),
        19 | 210 => Box::new(Namco163Mapper::new(rom)),
        32 => Box::new(IremG101::new(rom)),
        33 => Box::new(TaitoTc0190::new(rom)),
        48 => Box::new(TaitoTc0690::new(rom)),
        65 => Box::new(IremH3001::new(rom)),
        67 => Box::new(Sunsoft3::new(rom)),
        68 => Box::new(Sunsoft4::new(rom)),
        69 => Box::new(SunsoftFme7::new(rom)),
        70 => Box::new(Bandai74161::new(rom)),
        72 => Box::new(JalecoJf17_19::new(rom, false)),
        82 => Box::new(TaitoX1017::new(rom)),
        86 => Box::new(JalecoJf13::new(rom)),
        87 => Box::new(JalecoJfxx::new(rom, false)),
        88 => Box::new(Namco108_88::new(rom)),
        89 => Box::new(Sunsoft89::new(rom)),
        92 => Box::new(JalecoJf17_19::new(rom, true)),
        93 => Box::new(Sunsoft93::new(rom)),
        140 => Box::new(JalecoJf11_14::new(rom)),
        150 => Box::new(Sachen74LS374N::new(rom, 150)),
        164 => Box::new(Waixing164::new(rom)),
        184 => Box::new(Sunsoft184::new(rom)),
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Small shared helpers
// ---------------------------------------------------------------------------

/// Offset of a (possibly negative) page, matching `BaseMapper::Select*Page`.
fn page_offset(page: i32, page_size: usize, mem_len: usize) -> usize {
    let len = mem_len.max(1);
    let count = (len / page_size).max(1);
    let p = if page < 0 {
        (count as i32 + page).max(0) as usize
    } else {
        page as usize
    };
    (p % count) * page_size
}

/// Map one PRG page of `page_size` bytes starting at 8KB slot `slot`.
fn map_prg_page(c: &mut MapperCommon, slot: usize, page: i32, page_size: usize) {
    let base = page_offset(page, page_size, c.prg_rom.len());
    let slots = (page_size / 0x2000).max(1);
    for i in 0..slots {
        c.map_prg_8k(slot + i, base + i * 0x2000);
    }
}

/// Map one CHR page of `page_size` bytes starting at 1KB slot `slot`.
fn map_chr_page(c: &mut MapperCommon, slot: usize, page: i32, page_size: usize) {
    let base = page_offset(page, page_size, c.chr.len());
    let slots = (page_size / 0x400).max(1);
    for i in 0..slots {
        c.map_chr_1k(slot + i, base + i * 0x400);
    }
}

/// Correct 4KB nametable index: $3000-$3FFF mirrors the same 1KB windows as
/// $2000-$2FFF (Mesen maps both ranges to the same nametable pages).
fn fixed_nametable_index(c: &MapperCommon, addr: u16) -> usize {
    let a = (addr & 0x3FFF) as usize;
    let slot = ((a - 0x2000) >> 10) & 3;
    let offset = a & 0x3FF;
    let bank = match c.mirroring {
        Mirroring::Vertical => slot & 1,
        Mirroring::Horizontal => slot >> 1,
        Mirroring::ScreenAOnly => 0,
        Mirroring::ScreenBOnly => 1,
        Mirroring::FourScreens => slot,
    };
    (bank * 0x400 + offset) & 0x0FFF
}

fn default_ppu_read(c: &MapperCommon, addr: u16) -> Option<u8> {
    match addr & 0x3FFF {
        0x0000..=0x1FFF => Some(c.read_chr(addr)),
        0x2000..=0x3EFF => Some(c.nametables[fixed_nametable_index(c, addr)]),
        _ => None,
    }
}

fn default_ppu_write(c: &mut MapperCommon, addr: u16, value: u8) {
    match addr & 0x3FFF {
        0x0000..=0x1FFF => c.write_chr(addr, value),
        0x2000..=0x3EFF => {
            let idx = fixed_nametable_index(c, addr);
            c.nametables[idx] = value;
        }
        _ => {}
    }
}

/// Generates the standard CHR+nametable PPU handlers for a mapper that does
/// not need custom PPU behavior.  This also avoids the $3000 mirroring bug in
/// `MapperCommon::nametable_index` until the integration owner fixes it.
macro_rules! default_ppu_methods {
    () => {
        fn ppu_read(&mut self, addr: u16) -> Option<u8> {
            default_ppu_read(self.common(), addr)
        }
        fn ppu_write(&mut self, addr: u16, value: u8) {
            default_ppu_write(self.common_mut(), addr, value)
        }
    };
}

fn mirror_from_bits(c: &mut MapperCommon, value: u8) {
    c.mirroring = match value & 0x03 {
        0 => Mirroring::Vertical,
        1 => Mirroring::Horizontal,
        2 => Mirroring::ScreenAOnly,
        _ => Mirroring::ScreenBOnly,
    };
}

/// Default battery view used by boards whose battery is their PRG save RAM.
fn default_battery(c: &MapperCommon) -> Option<&[u8]> {
    if c.info.has_battery {
        Some(&c.prg_ram)
    } else {
        None
    }
}

fn default_set_battery(c: &mut MapperCommon, data: &[u8]) {
    let n = data.len().min(c.prg_ram.len());
    c.prg_ram[..n].copy_from_slice(&data[..n]);
}

/// Small A12 rise filter, equivalent in spirit to Mesen's `A12Watcher`.
/// Mesen measures in PPU cycles; we use CPU cycles (3x coarser) and require
/// at least 4 CPU cycles (~12 dots) of A12 low before accepting a rise.
#[derive(Clone, Copy, Default)]
struct A12Watcher {
    high: bool,
    low_cycles: u32,
}

impl A12Watcher {
    fn update(&mut self, addr: u16) -> bool {
        let high = (addr & 0x1000) != 0;
        let mut rise = false;
        if high {
            if !self.high && self.low_cycles >= 4 {
                rise = true;
            }
            self.high = true;
            self.low_cycles = 0;
        } else {
            self.high = false;
            if self.low_cycles == 0 {
                self.low_cycles = 1;
            }
        }
        rise
    }

    fn tick(&mut self) {
        if !self.high {
            self.low_cycles = self.low_cycles.saturating_add(1);
        }
    }

    fn save_state(&self, w: &mut StateWriter) {
        w.bool(self.high);
        w.u32(self.low_cycles);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.high = r.bool()?;
        self.low_cycles = r.u32()?;
        Ok(())
    }
}

// ===========================================================================
// Namco 163 (mapper 19) / Namco 175 / Namco 340 (mapper 210)
// ===========================================================================

const NAMCO_AUDIO_SCALE: f32 = 1.0 / 512.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum NamcoVariant {
    Namco163,
    Namco175,
    Namco340,
    Unknown,
}

impl NamcoVariant {
    fn to_u8(self) -> u8 {
        match self {
            NamcoVariant::Namco163 => 0,
            NamcoVariant::Namco175 => 1,
            NamcoVariant::Namco340 => 2,
            NamcoVariant::Unknown => 3,
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            0 => NamcoVariant::Namco163,
            1 => NamcoVariant::Namco175,
            2 => NamcoVariant::Namco340,
            _ => NamcoVariant::Unknown,
        }
    }
}

#[derive(Clone)]
struct Namco163Audio {
    ram: [u8; 0x80],
    channel_output: [i16; 8],
    ram_position: u8,
    auto_increment: bool,
    update_counter: u8,
    current_channel: i8,
    last_output: i16,
    disable_sound: bool,
}

impl Namco163Audio {
    fn new() -> Self {
        Self {
            ram: [0; 0x80],
            channel_output: [0; 8],
            ram_position: 0,
            auto_increment: false,
            update_counter: 0,
            current_channel: 7,
            last_output: 0,
            disable_sound: false,
        }
    }

    fn frequency(&self, channel: usize) -> u32 {
        let base = 0x40 + channel * 8;
        ((self.ram[base + 4] as u32 & 0x03) << 16)
            | ((self.ram[base + 2] as u32) << 8)
            | self.ram[base] as u32
    }

    fn phase(&self, channel: usize) -> u32 {
        let base = 0x40 + channel * 8;
        ((self.ram[base + 5] as u32) << 16)
            | ((self.ram[base + 3] as u32) << 8)
            | self.ram[base + 1] as u32
    }

    fn set_phase(&mut self, channel: usize, phase: u32) {
        let base = 0x40 + channel * 8;
        self.ram[base + 5] = (phase >> 16) as u8;
        self.ram[base + 3] = (phase >> 8) as u8;
        self.ram[base + 1] = phase as u8;
    }

    fn wave_address(&self, channel: usize) -> u8 {
        self.ram[0x40 + channel * 8 + 6]
    }

    fn wave_length(&self, channel: usize) -> u8 {
        256u16.wrapping_sub(self.ram[0x40 + channel * 8 + 4] as u16 & 0xFC) as u8
    }

    fn volume(&self, channel: usize) -> u8 {
        self.ram[0x40 + channel * 8 + 7] & 0x0F
    }

    fn channel_count(&self) -> u8 {
        (self.ram[0x7F] >> 4) & 0x07
    }

    fn update_channel(&mut self, channel: usize) {
        let mut phase = self.phase(channel);
        let freq = self.frequency(channel);
        let length = self.wave_length(channel);
        let offset = self.wave_address(channel);
        let volume = self.volume(channel);

        if length == 0 {
            phase = 0;
        } else {
            phase = (phase + freq) % ((length as u32) << 16);
        }

        let sample_position = (((phase >> 16) as u8).wrapping_add(offset)) as usize;
        let sample = if sample_position & 1 != 0 {
            (self.ram[(sample_position / 2) & 0x7F] >> 4) as i16
        } else {
            (self.ram[(sample_position / 2) & 0x7F] & 0x0F) as i16
        };

        self.channel_output[channel] = (sample - 8) * volume as i16;
        self.update_output_level();
        self.set_phase(channel, phase);
    }

    fn update_output_level(&mut self) {
        let count = self.channel_count();
        let mut summed: i16 = 0;
        let min = 7i16 - count as i16;
        for i in (min..=7).rev() {
            summed += self.channel_output[i as usize];
        }
        summed /= count as i16 + 1;
        self.last_output = summed;
    }

    fn clock(&mut self) {
        if !self.disable_sound {
            self.update_counter = self.update_counter.wrapping_add(1);
            if self.update_counter == 15 {
                let ch = self.current_channel;
                if (0..8).contains(&ch) {
                    self.update_channel(ch as usize);
                }
                self.update_counter = 0;
                self.current_channel -= 1;
                let min = 7 - self.channel_count() as i8;
                if self.current_channel < min {
                    self.current_channel = 7;
                }
            }
        }
    }

    fn write_register(&mut self, addr: u16, value: u8) {
        match addr & 0xF800 {
            0x4800 => {
                self.ram[self.ram_position as usize] = value;
                if self.auto_increment {
                    self.ram_position = (self.ram_position + 1) & 0x7F;
                }
            }
            0xE000 => self.disable_sound = (value & 0x40) == 0x40,
            0xF800 => {
                self.ram_position = value & 0x7F;
                self.auto_increment = (value & 0x80) == 0x80;
            }
            _ => {}
        }
    }

    fn read_register(&mut self, addr: u16) -> u8 {
        let mut value = 0;
        if addr & 0xF800 == 0x4800 {
            value = self.ram[self.ram_position as usize];
            if self.auto_increment {
                self.ram_position = (self.ram_position + 1) & 0x7F;
            }
        }
        value
    }

    fn save_state(&self, w: &mut StateWriter) {
        w.fixed_bytes(&self.ram);
        for out in self.channel_output {
            w.i16(out);
        }
        w.u8(self.ram_position);
        w.bool(self.auto_increment);
        w.u8(self.update_counter);
        w.u8(self.current_channel as u8);
        w.i16(self.last_output);
        w.bool(self.disable_sound);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        let ram = r.fixed_bytes(0x80)?;
        self.ram.copy_from_slice(&ram);
        for out in &mut self.channel_output {
            *out = r.i16()?;
        }
        self.ram_position = r.u8()?;
        self.auto_increment = r.bool()?;
        self.update_counter = r.u8()?;
        self.current_channel = r.u8()? as i8;
        self.last_output = r.i16()?;
        self.disable_sound = r.bool()?;
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq)]
enum NtSel {
    Default,
    Nametable(u8),
    Chr(usize),
}

struct Namco163Mapper {
    common: MapperCommon,
    audio: Namco163Audio,
    variant: NamcoVariant,
    not_namco340: bool,
    auto_detect_variant: bool,
    write_protect: u8,
    low_chr_nt_mode: bool,
    high_chr_nt_mode: bool,
    irq_counter: u16,
    /// Set when a pattern-table 1KB window is mapped to nametable RAM page.
    pattern_nt: [Option<u8>; 8],
    /// $2000-$2FFF 1KB windows (registers $C000-$D800).
    nt_sel: [NtSel; 4],
    battery_image: Vec<u8>,
}

impl Namco163Mapper {
    fn new(rom: RomData) -> Self {
        let battery = rom.battery.clone();
        let mut common = MapperCommon::new(rom);
        // Upstream always exposes exactly 8KB of save/work RAM.
        common.prg_ram.resize(0x2000, 0);

        let (variant, auto_detect) = if common.info.mapper == 210 {
            match common.info.submapper {
                0 => (NamcoVariant::Unknown, true),
                1 => (NamcoVariant::Namco175, false),
                2 => (NamcoVariant::Namco340, false),
                _ => (NamcoVariant::Unknown, true),
            }
        } else {
            (NamcoVariant::Namco163, true)
        };

        let mut audio = Namco163Audio::new();
        if let Some(data) = &battery {
            if data.len() >= 0x2000 + 0x80 {
                audio.ram.copy_from_slice(&data[0x2000..0x2000 + 0x80]);
            }
        }

        let mut mapper = Self {
            common,
            audio,
            variant,
            not_namco340: false,
            auto_detect_variant: auto_detect,
            write_protect: 0,
            low_chr_nt_mode: false,
            high_chr_nt_mode: false,
            irq_counter: 0,
            pattern_nt: [None; 8],
            nt_sel: [NtSel::Default; 4],
            battery_image: Vec::new(),
        };

        // Power-on mapping: PRG slot 3 is the last 8KB bank; other slots are
        // unset in hardware.  We use bank 0 (games program these before use).
        for slot in 0..3 {
            map_prg_page(&mut mapper.common, slot, 0, 0x2000);
        }
        map_prg_page(&mut mapper.common, 3, -1, 0x2000);
        for slot in 0..8 {
            map_chr_page(&mut mapper.common, slot, slot as i32, 0x400);
        }
        mapper.sync_battery();
        mapper
    }

    fn set_variant(&mut self, variant: NamcoVariant) {
        if self.auto_detect_variant {
            if !self.not_namco340 || variant != NamcoVariant::Namco340 {
                self.variant = variant;
            }
        }
    }

    /// $6000-$7FFF is readable on the 163/175 (write access is separately
    /// gated); Namco 340/unknown boards do not expose it at all.
    fn save_ram_read_allowed(&self) -> bool {
        matches!(
            self.variant,
            NamcoVariant::Namco163 | NamcoVariant::Namco175
        )
    }

    fn save_ram_write_allowed(&self, addr: u16) -> bool {
        match self.variant {
            NamcoVariant::Namco163 => {
                let bit = 1u8 << ((addr as usize - 0x6000) / 0x800);
                (self.write_protect & 0x40) == 0x40 && (self.write_protect & bit) == 0
            }
            NamcoVariant::Namco175 => (self.write_protect & 0x01) == 0x01,
            _ => false,
        }
    }

    fn sync_battery(&mut self) {
        let n = self.common.prg_ram.len();
        if self.battery_image.len() != n + 0x80 {
            self.battery_image = vec![0; n + 0x80];
        }
        self.battery_image[..n].copy_from_slice(&self.common.prg_ram);
        self.battery_image[n..].copy_from_slice(&self.audio.ram);
    }

    fn set_chr_pattern_reg(&mut self, slot: usize, value: u8, nt_allowed: bool) {
        if nt_allowed && value >= 0xE0 && self.variant == NamcoVariant::Namco163 {
            self.pattern_nt[slot] = Some(value & 0x01);
        } else {
            self.pattern_nt[slot] = None;
            map_chr_page(&mut self.common, slot, value as i32, 0x400);
        }
    }

    fn set_nt_reg(&mut self, slot: usize, value: u8) {
        if value >= 0xE0 {
            self.nt_sel[slot] = NtSel::Nametable(value & 0x01);
        } else {
            let off = page_offset(value as i32, 0x400, self.common.chr.len());
            self.nt_sel[slot] = NtSel::Chr(off);
        }
    }

    fn read_nt_page(&self, page: u8, off: usize) -> u8 {
        let idx = (page as usize * 0x400 + off) & 0x0FFF;
        self.common.nametables[idx]
    }

    fn write_nt_page(&mut self, page: u8, off: usize, value: u8) {
        let idx = (page as usize * 0x400 + off) & 0x0FFF;
        self.common.nametables[idx] = value;
    }

    fn write_register(&mut self, addr: u16, value: u8) {
        let addr = addr & 0xF800;
        match addr {
            0x4800 => {
                self.set_variant(NamcoVariant::Namco163);
                self.audio.write_register(addr, value);
                self.sync_battery();
            }
            0x5000 => {
                self.set_variant(NamcoVariant::Namco163);
                self.irq_counter = (self.irq_counter & 0xFF00) | value as u16;
                self.common.irq_pending = false;
            }
            0x5800 => {
                self.set_variant(NamcoVariant::Namco163);
                self.irq_counter = (self.irq_counter & 0x00FF) | ((value as u16) << 8);
                self.common.irq_pending = false;
            }
            0x8000 | 0x8800 | 0x9000 | 0x9800 => {
                let slot = ((addr - 0x8000) >> 11) as usize;
                self.set_chr_pattern_reg(slot, value, !self.low_chr_nt_mode);
            }
            0xA000 | 0xA800 | 0xB000 | 0xB800 => {
                let slot = (((addr - 0xA000) >> 11) + 4) as usize;
                self.set_chr_pattern_reg(slot, value, !self.high_chr_nt_mode);
            }
            0xC000 | 0xC800 | 0xD000 | 0xD800 => {
                if addr >= 0xC800 {
                    self.set_variant(NamcoVariant::Namco163);
                } else if self.variant != NamcoVariant::Namco163 {
                    self.set_variant(NamcoVariant::Namco175);
                }

                if self.variant == NamcoVariant::Namco175 {
                    self.write_protect = value;
                } else {
                    let slot = ((addr - 0xC000) >> 11) as usize;
                    self.set_nt_reg(slot, value);
                }
            }
            0xE000 => {
                if value & 0x80 == 0x80 {
                    self.set_variant(NamcoVariant::Namco340);
                } else if value & 0x40 == 0x40 && self.variant != NamcoVariant::Namco163 {
                    self.set_variant(NamcoVariant::Namco340);
                }

                map_prg_page(&mut self.common, 0, (value & 0x3F) as i32, 0x2000);

                if self.variant == NamcoVariant::Namco340 {
                    self.common.mirroring = match (value & 0xC0) >> 6 {
                        0 => Mirroring::ScreenAOnly,
                        1 => Mirroring::Vertical,
                        2 => Mirroring::Horizontal,
                        _ => Mirroring::ScreenBOnly,
                    };
                    // SetMirroringType() remaps all four $2000-$2FFF windows.
                    self.nt_sel = [NtSel::Default; 4];
                } else if self.variant == NamcoVariant::Namco163 {
                    self.audio.write_register(addr, value);
                }
            }
            0xE800 => {
                map_prg_page(&mut self.common, 1, (value & 0x3F) as i32, 0x2000);
                if self.variant == NamcoVariant::Namco163 {
                    self.low_chr_nt_mode = (value & 0x40) == 0x40;
                    self.high_chr_nt_mode = (value & 0x80) == 0x80;
                }
            }
            0xF000 => {
                map_prg_page(&mut self.common, 2, (value & 0x3F) as i32, 0x2000);
            }
            0xF800 => {
                self.set_variant(NamcoVariant::Namco163);
                if self.variant == NamcoVariant::Namco163 {
                    self.write_protect = value;
                    self.audio.write_register(addr, value);
                }
            }
            _ => {}
        }
    }
}

impl Mapper for Namco163Mapper {
    fn common(&self) -> &MapperCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }

    fn reset(&mut self) {
        self.common.irq_pending = false;
    }

    fn cpu_read(&mut self, addr: u16) -> Option<u8> {
        match addr {
            0x4800..=0x5FFF => Some(match addr & 0xF800 {
                0x4800 => self.audio.read_register(addr),
                0x5000 => self.irq_counter as u8,
                0x5800 => (self.irq_counter >> 8) as u8,
                _ => 0,
            }),
            0x6000..=0x7FFF => {
                if self.save_ram_read_allowed() {
                    Some(self.common.read_prg_ram(addr))
                } else {
                    None
                }
            }
            0x8000..=0xFFFF => Some(self.common.read_prg_rom(addr)),
            _ => None,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x4800..=0x5FFF => self.write_register(addr, value),
            0x6000..=0x7FFF => {
                self.not_namco340 = true;
                if self.variant == NamcoVariant::Namco340 {
                    self.set_variant(NamcoVariant::Unknown);
                }
                if self.save_ram_write_allowed(addr) {
                    self.common.write_prg_ram(addr, value);
                    self.sync_battery();
                }
            }
            0x8000..=0xFFFF => self.write_register(addr, value),
            _ => {}
        }
    }

    fn ppu_read(&mut self, addr: u16) -> Option<u8> {
        let a = addr & 0x3FFF;
        match a {
            0x0000..=0x1FFF => {
                let slot = ((a as usize) >> 10) & 7;
                if let Some(page) = self.pattern_nt[slot] {
                    Some(self.read_nt_page(page, a as usize & 0x3FF))
                } else {
                    Some(self.common.read_chr(addr))
                }
            }
            0x2000..=0x2FFF => {
                let slot = ((a as usize - 0x2000) >> 10) & 3;
                let off = a as usize & 0x3FF;
                match self.nt_sel[slot] {
                    NtSel::Default => Some(self.common.read_nametable(addr)),
                    NtSel::Nametable(page) => Some(self.read_nt_page(page, off)),
                    NtSel::Chr(base) => {
                        let idx = (base + off) % self.common.chr.len().max(1);
                        Some(self.common.chr[idx])
                    }
                }
            }
            0x3000..=0x3EFF => Some(self.common.read_nametable(addr & 0x2FFF)),
            _ => None,
        }
    }

    fn ppu_write(&mut self, addr: u16, value: u8) {
        let a = addr & 0x3FFF;
        match a {
            0x0000..=0x1FFF => {
                let slot = ((a as usize) >> 10) & 7;
                if let Some(page) = self.pattern_nt[slot] {
                    self.write_nt_page(page, a as usize & 0x3FF, value);
                } else {
                    self.common.write_chr(addr, value);
                }
            }
            0x2000..=0x2FFF => {
                let slot = ((a as usize - 0x2000) >> 10) & 3;
                let off = a as usize & 0x3FF;
                match self.nt_sel[slot] {
                    NtSel::Default => self.common.write_nametable(addr, value),
                    NtSel::Nametable(page) => self.write_nt_page(page, off, value),
                    NtSel::Chr(base) => {
                        if self.common.chr_is_ram {
                            let idx = (base + off) % self.common.chr.len().max(1);
                            self.common.chr[idx] = value;
                        }
                    }
                }
            }
            0x3000..=0x3EFF => self.common.write_nametable(addr & 0x2FFF, value),
            _ => {}
        }
    }

    fn cpu_cycle(&mut self) {
        if self.irq_counter & 0x8000 != 0 && (self.irq_counter & 0x7FFF) != 0x7FFF {
            self.irq_counter = self.irq_counter.wrapping_add(1);
            if (self.irq_counter & 0x7FFF) == 0x7FFF {
                self.common.irq_pending = true;
            }
        }
        if self.variant == NamcoVariant::Namco163 {
            self.audio.clock();
        }
    }

    fn irq_pending(&self) -> bool {
        self.common.irq_pending
    }

    fn expansion_audio_sample(&self) -> f32 {
        (self.audio.last_output as f32 * NAMCO_AUDIO_SCALE).clamp(-1.0, 1.0)
    }

    fn battery(&self) -> Option<&[u8]> {
        if self.common.info.has_battery {
            Some(&self.battery_image)
        } else {
            None
        }
    }

    fn set_battery(&mut self, data: &[u8]) {
        let n = self.common.prg_ram.len();
        let copy = data.len().min(n);
        self.common.prg_ram[..copy].copy_from_slice(&data[..copy]);
        if data.len() >= n + 0x80 {
            self.audio.ram.copy_from_slice(&data[n..n + 0x80]);
        }
        self.sync_battery();
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);
        w.u8(self.variant.to_u8());
        w.bool(self.not_namco340);
        w.bool(self.auto_detect_variant);
        w.u8(self.write_protect);
        w.bool(self.low_chr_nt_mode);
        w.bool(self.high_chr_nt_mode);
        w.u16(self.irq_counter);
        for nt in self.pattern_nt {
            match nt {
                Some(page) => {
                    w.bool(true);
                    w.u8(page);
                }
                None => w.bool(false),
            }
        }
        for sel in self.nt_sel {
            match sel {
                NtSel::Default => w.u8(0),
                NtSel::Nametable(page) => {
                    w.u8(1);
                    w.u8(page);
                }
                NtSel::Chr(off) => {
                    w.u8(2);
                    w.u32(off as u32);
                }
            }
        }
        self.audio.save_state(w);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        self.variant = NamcoVariant::from_u8(r.u8()?);
        self.not_namco340 = r.bool()?;
        self.auto_detect_variant = r.bool()?;
        self.write_protect = r.u8()?;
        self.low_chr_nt_mode = r.bool()?;
        self.high_chr_nt_mode = r.bool()?;
        self.irq_counter = r.u16()?;
        for i in 0..8 {
            self.pattern_nt[i] = if r.bool()? { Some(r.u8()?) } else { None };
        }
        for i in 0..4 {
            self.nt_sel[i] = match r.u8()? {
                0 => NtSel::Default,
                1 => NtSel::Nametable(r.u8()?),
                _ => NtSel::Chr(r.u32()? as usize),
            };
        }
        self.audio.load_state(r)?;
        self.sync_battery();
        Ok(())
    }
}

// ===========================================================================
// Sunsoft 3 (mapper 67)
// ===========================================================================

struct Sunsoft3 {
    common: MapperCommon,
    irq_latch: bool,
    irq_enabled: bool,
    irq_counter: u16,
}

impl Sunsoft3 {
    fn new(rom: RomData) -> Self {
        let mut common = MapperCommon::new(rom);
        map_prg_page(&mut common, 0, 0, 0x4000);
        map_prg_page(&mut common, 2, -1, 0x4000);
        for i in 0..8 {
            common.map_chr_1k(i, i * 0x400);
        }
        let mut m = Self {
            common,
            irq_latch: false,
            irq_enabled: false,
            irq_counter: 0,
        };
        m.common.irq_pending = false;
        m
    }
}

impl Mapper for Sunsoft3 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }
    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }
    fn reset(&mut self) {
        self.common.irq_pending = false;
    }
    default_ppu_methods!();

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr & 0xF800 {
            0x8800 => map_chr_page(&mut self.common, 0, value as i32, 0x800),
            0x9800 => map_chr_page(&mut self.common, 2, value as i32, 0x800),
            0xA800 => map_chr_page(&mut self.common, 4, value as i32, 0x800),
            0xB800 => map_chr_page(&mut self.common, 6, value as i32, 0x800),
            0xC800 => {
                self.irq_counter = if self.irq_latch {
                    (self.irq_counter & 0xFF00) | value as u16
                } else {
                    (self.irq_counter & 0x00FF) | ((value as u16) << 8)
                };
                self.irq_latch = !self.irq_latch;
            }
            0xD800 => {
                self.irq_enabled = (value & 0x10) == 0x10;
                self.irq_latch = false;
                self.common.irq_pending = false;
            }
            0xE800 => match value & 0x03 {
                0 => self.common.mirroring = Mirroring::Vertical,
                1 => self.common.mirroring = Mirroring::Horizontal,
                2 => self.common.mirroring = Mirroring::ScreenAOnly,
                _ => self.common.mirroring = Mirroring::ScreenBOnly,
            },
            0xF800 => map_prg_page(&mut self.common, 0, value as i32, 0x4000),
            _ => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    self.common.write_prg_ram(addr, value);
                }
            }
        }
    }

    fn cpu_cycle(&mut self) {
        if self.irq_enabled {
            self.irq_counter = self.irq_counter.wrapping_sub(1);
            if self.irq_counter == 0xFFFF {
                self.irq_enabled = false;
                self.common.irq_pending = true;
            }
        }
    }

    fn irq_pending(&self) -> bool {
        self.common.irq_pending
    }

    fn battery(&self) -> Option<&[u8]> {
        default_battery(&self.common)
    }

    fn set_battery(&mut self, data: &[u8]) {
        default_set_battery(&mut self.common, data);
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);
        w.bool(self.irq_latch);
        w.bool(self.irq_enabled);
        w.u16(self.irq_counter);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        self.irq_latch = r.bool()?;
        self.irq_enabled = r.bool()?;
        self.irq_counter = r.u16()?;
        Ok(())
    }
}

// ===========================================================================
// Sunsoft 4 (mapper 68)
// ===========================================================================

struct Sunsoft4 {
    common: MapperCommon,
    nt_regs: [u8; 2],
    use_chr_for_nametables: bool,
    prg_ram_enabled: bool,
    licensing_timer: u32,
    using_external_rom: bool,
    external_page: u8,
    unmapped_8000_bfff: bool,
}

impl Sunsoft4 {
    fn new(rom: RomData) -> Self {
        let mut common = MapperCommon::new(rom);
        for i in 0..8 {
            common.map_chr_1k(i, i * 0x400);
        }
        let mut m = Self {
            common,
            nt_regs: [0; 2],
            use_chr_for_nametables: false,
            prg_ram_enabled: false,
            licensing_timer: 0,
            using_external_rom: false,
            external_page: 0,
            unmapped_8000_bfff: false,
        };
        map_prg_page(&mut m.common, 0, 0, 0x4000);
        map_prg_page(&mut m.common, 2, 7, 0x4000);
        m.update_state();
        m
    }

    fn update_state(&mut self) {
        // Upstream maps work/save RAM at $6000-$7FFF unconditionally (the
        // computed access type is not used by that call).
        self.common.prg_ram.resize(self.common.prg_ram.len().max(0x2000), 0);
        if self.using_external_rom {
            if self.licensing_timer == 0 {
                self.unmapped_8000_bfff = true;
            } else {
                self.unmapped_8000_bfff = false;
                map_prg_page(&mut self.common, 0, self.external_page as i32, 0x4000);
            }
        } else {
            self.unmapped_8000_bfff = false;
        }
    }

    fn nt_reg_for_slot(&self, slot: usize) -> u8 {
        match self.common.mirroring {
            Mirroring::Vertical => (slot & 1) as u8,
            Mirroring::Horizontal => ((slot & 2) >> 1) as u8,
            Mirroring::ScreenAOnly => 0,
            Mirroring::ScreenBOnly => 1,
            // Upstream leaves reg at 0 for four-screen (unsupported).
            Mirroring::FourScreens => 0,
        }
    }
}

impl Mapper for Sunsoft4 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }
    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }
    fn reset(&mut self) {
        self.common.irq_pending = false;
    }

    fn cpu_read(&mut self, addr: u16) -> Option<u8> {
        match addr {
            0x6000..=0x7FFF => Some(self.common.read_prg_ram(addr)),
            0x8000..=0xBFFF if self.unmapped_8000_bfff => None,
            0x8000..=0xFFFF => Some(self.common.read_prg_rom(addr)),
            _ => None,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x6000..=0x7FFF => {
                self.licensing_timer = 1024 * 105;
                self.update_state();
                self.common.write_prg_ram(addr, value);
            }
            0x8000..=0xFFFF => {
                match addr & 0xF000 {
                    0x8000 => map_chr_page(&mut self.common, 0, value as i32, 0x800),
                    0x9000 => map_chr_page(&mut self.common, 2, value as i32, 0x800),
                    0xA000 => map_chr_page(&mut self.common, 4, value as i32, 0x800),
                    0xB000 => map_chr_page(&mut self.common, 6, value as i32, 0x800),
                    0xC000 => self.nt_regs[0] = value | 0x80,
                    0xD000 => self.nt_regs[1] = value | 0x80,
                    0xE000 => {
                        mirror_from_bits(&mut self.common, value);
                        self.use_chr_for_nametables = (value & 0x10) == 0x10;
                    }
                    0xF000 => {
                        let external_prg = (value & 0x08) == 0;
                        let page_count = (self.common.prg_rom.len() / 0x4000).max(1);
                        if external_prg && page_count > 8 {
                            self.using_external_rom = true;
                            let span = page_count - 0x08;
                            self.external_page =
                                0x08 | (((value & 0x07) as usize % span) as u8);
                            map_prg_page(
                                &mut self.common,
                                0,
                                self.external_page as i32,
                                0x4000,
                            );
                        } else {
                            self.using_external_rom = false;
                            map_prg_page(
                                &mut self.common,
                                0,
                                (value & 0x07) as i32,
                                0x4000,
                            );
                        }
                        self.prg_ram_enabled = (value & 0x10) == 0x10;
                        self.update_state();
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn ppu_read(&mut self, addr: u16) -> Option<u8> {
        let a = addr & 0x3FFF;
        match a {
            0x0000..=0x1FFF => Some(self.common.read_chr(addr)),
            0x2000..=0x2FFF if self.use_chr_for_nametables => {
                let slot = ((a as usize - 0x2000) >> 10) & 3;
                let reg = self.nt_reg_for_slot(slot) as usize;
                let off = (self.nt_regs[reg] as usize) * 0x400 + (a as usize & 0x3FF);
                let idx = off % self.common.chr.len().max(1);
                Some(self.common.chr[idx])
            }
            0x2000..=0x3EFF => {
                Some(self.common.read_nametable(addr & 0x2FFF))
            }
            _ => None,
        }
    }

    fn ppu_write(&mut self, addr: u16, value: u8) {
        let a = addr & 0x3FFF;
        match a {
            0x0000..=0x1FFF => self.common.write_chr(addr, value),
            0x2000..=0x2FFF if self.use_chr_for_nametables => {
                if self.common.chr_is_ram {
                    let slot = ((a as usize - 0x2000) >> 10) & 3;
                    let reg = self.nt_reg_for_slot(slot) as usize;
                    let off =
                        (self.nt_regs[reg] as usize) * 0x400 + (a as usize & 0x3FF);
                    let idx = off % self.common.chr.len().max(1);
                    self.common.chr[idx] = value;
                }
            }
            0x2000..=0x3EFF => {
                self.common.write_nametable(addr & 0x2FFF, value)
            }
            _ => {}
        }
    }

    fn cpu_cycle(&mut self) {
        if self.licensing_timer != 0 {
            self.licensing_timer -= 1;
            if self.licensing_timer == 0 {
                self.update_state();
            }
        }
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);
        w.u8(self.nt_regs[0]);
        w.u8(self.nt_regs[1]);
        w.bool(self.use_chr_for_nametables);
        w.bool(self.prg_ram_enabled);
        w.u32(self.licensing_timer);
        w.bool(self.using_external_rom);
        w.u8(self.external_page);
        w.bool(self.unmapped_8000_bfff);
    }

    fn battery(&self) -> Option<&[u8]> {
        default_battery(&self.common)
    }

    fn set_battery(&mut self, data: &[u8]) {
        default_set_battery(&mut self.common, data);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        self.nt_regs[0] = r.u8()?;
        self.nt_regs[1] = r.u8()?;
        self.use_chr_for_nametables = r.bool()?;
        self.prg_ram_enabled = r.bool()?;
        self.licensing_timer = r.u32()?;
        self.using_external_rom = r.bool()?;
        self.external_page = r.u8()?;
        self.unmapped_8000_bfff = r.bool()?;
        Ok(())
    }
}

// ===========================================================================
// Sunsoft 5B audio
// ===========================================================================

const SUNSOFT_AUDIO_SCALE: f32 = 1.0 / 512.0;

#[derive(Clone)]
struct Sunsoft5bAudio {
    volume_lut: [u8; 0x10],
    current_register: u8,
    registers: [u8; 0x10],
    last_output: i16,
    timer: [i16; 3],
    tone_step: [u8; 3],
    process_tick: bool,
}

impl Sunsoft5bAudio {
    fn new() -> Self {
        let mut volume_lut = [0u8; 0x10];
        let mut output = 1.0f64;
        for lut in volume_lut.iter_mut().skip(1) {
            output *= 1.1885022274370184377301224648922;
            output *= 1.1885022274370184377301224648922;
            *lut = output as u8;
        }
        Self {
            volume_lut,
            current_register: 0,
            registers: [0; 0x10],
            last_output: 0,
            timer: [0; 3],
            tone_step: [0; 3],
            process_tick: false,
        }
    }

    fn period(&self, channel: usize) -> u16 {
        self.registers[channel * 2] as u16
            | ((self.registers[channel * 2 + 1] as u16) << 8)
    }

    fn volume(&self, channel: usize) -> u8 {
        self.volume_lut[(self.registers[8 + channel] & 0x0F) as usize]
    }

    fn is_tone_enabled(&self, channel: usize) -> bool {
        ((self.registers[7] >> channel) & 0x01) == 0
    }

    fn update_channel(&mut self, channel: usize) {
        self.timer[channel] = self.timer[channel].wrapping_sub(1);
        if self.timer[channel] <= 0 {
            self.timer[channel] = self.period(channel) as i16;
            self.tone_step[channel] = (self.tone_step[channel] + 1) & 0x0F;
        }
    }

    fn update_output_level(&mut self) {
        let mut summed: i16 = 0;
        for i in 0..3 {
            if self.is_tone_enabled(i) && self.tone_step[i] < 0x08 {
                summed += self.volume(i) as i16;
            }
        }
        self.last_output = summed;
    }

    fn clock(&mut self) {
        if self.process_tick {
            for i in 0..3 {
                self.update_channel(i);
            }
            self.update_output_level();
        }
        self.process_tick = !self.process_tick;
    }

    fn write_register(&mut self, addr: u16, value: u8) {
        match addr & 0xE000 {
            0xC000 => self.current_register = value & 0x0F,
            0xE000 => self.registers[self.current_register as usize] = value,
            _ => {}
        }
    }

    fn save_state(&self, w: &mut StateWriter) {
        for t in self.timer {
            w.i16(t);
        }
        w.fixed_bytes(&self.registers);
        w.fixed_bytes(&self.tone_step);
        w.u8(self.current_register);
        w.i16(self.last_output);
        w.bool(self.process_tick);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        for t in &mut self.timer {
            *t = r.i16()?;
        }
        let regs = r.fixed_bytes(0x10)?;
        self.registers.copy_from_slice(&regs);
        let steps = r.fixed_bytes(3)?;
        self.tone_step.copy_from_slice(&steps);
        self.current_register = r.u8()?;
        self.last_output = r.i16()?;
        self.process_tick = r.bool()?;
        Ok(())
    }
}

// ===========================================================================
// Sunsoft FME-7 (mapper 69)
// ===========================================================================

struct SunsoftFme7 {
    common: MapperCommon,
    command: u8,
    work_ram_value: u8,
    irq_enabled: bool,
    irq_counter_enabled: bool,
    irq_counter: u16,
    audio: Sunsoft5bAudio,
    ram_at_6000: bool,
    ram_page: usize,
    ram_accessible: bool,
}

impl SunsoftFme7 {
    fn new(rom: RomData) -> Self {
        let mut common = MapperCommon::new(rom);
        common.prg_ram.resize(0x8000, 0);
        for i in 0..8 {
            common.map_chr_1k(i, i * 0x400);
        }
        map_prg_page(&mut common, 0, 0, 0x2000);
        map_prg_page(&mut common, 1, 1, 0x2000);
        map_prg_page(&mut common, 2, 2, 0x2000);
        map_prg_page(&mut common, 3, -1, 0x2000);
        let mut m = Self {
            common,
            command: 0,
            work_ram_value: 0,
            irq_enabled: false,
            irq_counter_enabled: false,
            irq_counter: 0,
            audio: Sunsoft5bAudio::new(),
            ram_at_6000: false,
            ram_page: 0,
            ram_accessible: false,
        };
        m.update_work_ram();
        m
    }

    fn update_work_ram(&mut self) {
        if self.work_ram_value & 0x40 != 0 {
            self.ram_at_6000 = true;
            self.ram_page = (self.work_ram_value & 0x3F) as usize;
            self.ram_accessible = (self.work_ram_value & 0x80) != 0;
        } else {
            self.ram_at_6000 = false;
            self.ram_page = (self.work_ram_value & 0x3F) as usize;
            self.ram_accessible = true;
        }
    }
}

impl Mapper for SunsoftFme7 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }
    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }
    fn reset(&mut self) {
        self.common.irq_pending = false;
    }
    default_ppu_methods!();

    fn cpu_read(&mut self, addr: u16) -> Option<u8> {
        match addr {
            0x6000..=0x7FFF => {
                if !self.ram_accessible {
                    return None;
                }
                let off = self.ram_page * 0x2000 + (addr as usize - 0x6000);
                if self.ram_at_6000 {
                    Some(self.common.prg_ram[off % self.common.prg_ram.len()])
                } else {
                    Some(self.common.prg_rom[off % self.common.prg_rom.len()])
                }
            }
            0x8000..=0xFFFF => Some(self.common.read_prg_rom(addr)),
            _ => None,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x6000..=0x7FFF => {
                if self.ram_at_6000 && self.ram_accessible {
                    let off = self.ram_page * 0x2000 + (addr as usize - 0x6000);
                    let len = self.common.prg_ram.len();
                    self.common.prg_ram[off % len] = value;
                }
            }
            0x8000..=0xBFFF => match addr & 0xE000 {
                0x8000 => self.command = value,
                0xA000 => match self.command {
                    0..=7 => map_chr_page(
                        &mut self.common,
                        self.command as usize,
                        value as i32,
                        0x400,
                    ),
                    8 => {
                        self.work_ram_value = value;
                        self.update_work_ram();
                    }
                    9..=0xB => {
                        let slot = (self.command - 9) as usize;
                        map_prg_page(&mut self.common, slot, (value & 0x3F) as i32, 0x2000);
                    }
                    0xC => mirror_from_bits(&mut self.common, value),
                    0xD => {
                        self.irq_enabled = (value & 0x01) == 0x01;
                        self.irq_counter_enabled = (value & 0x80) == 0x80;
                        self.common.irq_pending = false;
                    }
                    0xE => {
                        self.irq_counter =
                            (self.irq_counter & 0xFF00) | value as u16;
                    }
                    0xF => {
                        self.irq_counter =
                            (self.irq_counter & 0x00FF) | ((value as u16) << 8);
                    }
                    _ => {}
                },
                _ => {}
            },
            0xC000..=0xFFFF => self.audio.write_register(addr, value),
            _ => {}
        }
    }

    fn cpu_cycle(&mut self) {
        if self.irq_counter_enabled {
            self.irq_counter = self.irq_counter.wrapping_sub(1);
            if self.irq_counter == 0xFFFF && self.irq_enabled {
                self.common.irq_pending = true;
            }
        }
        self.audio.clock();
    }

    fn irq_pending(&self) -> bool {
        self.common.irq_pending
    }

    fn expansion_audio_sample(&self) -> f32 {
        (self.audio.last_output as f32 * SUNSOFT_AUDIO_SCALE).clamp(-1.0, 1.0)
    }

    fn battery(&self) -> Option<&[u8]> {
        if self.common.info.has_battery {
            Some(&self.common.prg_ram)
        } else {
            None
        }
    }

    fn set_battery(&mut self, data: &[u8]) {
        let n = data.len().min(self.common.prg_ram.len());
        self.common.prg_ram[..n].copy_from_slice(&data[..n]);
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);
        w.u8(self.command);
        w.u8(self.work_ram_value);
        w.bool(self.irq_enabled);
        w.bool(self.irq_counter_enabled);
        w.u16(self.irq_counter);
        self.audio.save_state(w);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        self.command = r.u8()?;
        self.work_ram_value = r.u8()?;
        self.irq_enabled = r.bool()?;
        self.irq_counter_enabled = r.bool()?;
        self.irq_counter = r.u16()?;
        self.audio.load_state(r)?;
        self.update_work_ram();
        Ok(())
    }
}

// ===========================================================================
// Sunsoft 89 / 93 / 184
// ===========================================================================

struct Sunsoft89 {
    common: MapperCommon,
}

impl Sunsoft89 {
    fn new(rom: RomData) -> Self {
        let mut common = MapperCommon::new(rom);
        map_prg_page(&mut common, 0, 0, 0x4000);
        map_prg_page(&mut common, 2, -1, 0x4000);
        map_chr_page(&mut common, 0, 0, 0x2000);
        Self { common }
    }
}

impl Mapper for Sunsoft89 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }
    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }
    fn reset(&mut self) {
        self.common.irq_pending = false;
    }
    default_ppu_methods!();
    fn cpu_write(&mut self, addr: u16, value: u8) {
        if addr >= 0x8000 {
            map_prg_page(&mut self.common, 0, ((value >> 4) & 0x07) as i32, 0x4000);
            map_chr_page(
                &mut self.common,
                0,
                ((value & 0x07) | ((value & 0x80) >> 4)) as i32,
                0x2000,
            );
            self.common.mirroring = if value & 0x08 != 0 {
                Mirroring::ScreenBOnly
            } else {
                Mirroring::ScreenAOnly
            };
        } else if (0x6000..=0x7FFF).contains(&addr) {
            self.common.write_prg_ram(addr, value);
        }
    }

    fn battery(&self) -> Option<&[u8]> {
        default_battery(&self.common)
    }

    fn set_battery(&mut self, data: &[u8]) {
        default_set_battery(&mut self.common, data);
    }
}

struct Sunsoft93 {
    common: MapperCommon,
    chr_enabled: bool,
}

impl Sunsoft93 {
    fn new(rom: RomData) -> Self {
        let mut common = MapperCommon::new(rom);
        map_prg_page(&mut common, 0, 0, 0x4000);
        map_prg_page(&mut common, 2, -1, 0x4000);
        map_chr_page(&mut common, 0, 0, 0x2000);
        Self {
            common,
            chr_enabled: true,
        }
    }
}

impl Mapper for Sunsoft93 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }
    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }
    fn reset(&mut self) {
        self.common.irq_pending = false;
    }
    fn cpu_write(&mut self, addr: u16, value: u8) {
        if addr >= 0x8000 {
            map_prg_page(&mut self.common, 0, ((value >> 4) & 0x07) as i32, 0x4000);
            self.chr_enabled = value & 0x01 != 0;
            if self.chr_enabled {
                map_chr_page(&mut self.common, 0, 0, 0x2000);
            }
        } else if (0x6000..=0x7FFF).contains(&addr) {
            self.common.write_prg_ram(addr, value);
        }
    }

    fn ppu_read(&mut self, addr: u16) -> Option<u8> {
        let a = addr & 0x3FFF;
        match a {
            0x0000..=0x1FFF => {
                if self.chr_enabled {
                    Some(self.common.read_chr(addr))
                } else {
                    None
                }
            }
            0x2000..=0x3EFF => Some(self.common.read_nametable(addr & 0x2FFF)),
            _ => None,
        }
    }

    fn ppu_write(&mut self, addr: u16, value: u8) {
        let a = addr & 0x3FFF;
        match a {
            0x0000..=0x1FFF => {
                if self.chr_enabled {
                    self.common.write_chr(addr, value);
                }
            }
            0x2000..=0x3EFF => {
                self.common.write_nametable(addr & 0x2FFF, value)
            }
            _ => {}
        }
    }

    fn battery(&self) -> Option<&[u8]> {
        default_battery(&self.common)
    }

    fn set_battery(&mut self, data: &[u8]) {
        default_set_battery(&mut self.common, data);
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);
        w.bool(self.chr_enabled);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        self.chr_enabled = r.bool()?;
        Ok(())
    }
}

struct Sunsoft184 {
    common: MapperCommon,
}

impl Sunsoft184 {
    fn new(rom: RomData) -> Self {
        let mut common = MapperCommon::new(rom);
        map_prg_page(&mut common, 0, 0, 0x8000);
        map_chr_page(&mut common, 0, 0, 0x1000);
        map_chr_page(&mut common, 4, 0x80, 0x1000);
        Self { common }
    }
}

impl Mapper for Sunsoft184 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }
    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }
    fn reset(&mut self) {
        self.common.irq_pending = false;
    }
    default_ppu_methods!();
    fn cpu_write(&mut self, addr: u16, value: u8) {
        if (0x6000..=0x7FFF).contains(&addr) {
            map_chr_page(&mut self.common, 0, (value & 0x07) as i32, 0x1000);
            map_chr_page(
                &mut self.common,
                4,
                (0x80 | ((value >> 4) & 0x07) as u32) as i32,
                0x1000,
            );
        }
    }

    fn battery(&self) -> Option<&[u8]> {
        default_battery(&self.common)
    }

    fn set_battery(&mut self, data: &[u8]) {
        default_set_battery(&mut self.common, data);
    }
}

// ===========================================================================
// Jaleco SS88006 (mapper 18)
// ===========================================================================

struct JalecoSs88006 {
    common: MapperCommon,
    prg_banks: [u8; 3],
    chr_banks: [u8; 8],
    irq_reload: [u8; 4],
    irq_counter: u16,
    irq_counter_size: u8,
    irq_enabled: bool,
}

impl JalecoSs88006 {
    fn new(rom: RomData) -> Self {
        let mut common = MapperCommon::new(rom);
        for slot in 0..3 {
            map_prg_page(&mut common, slot, 0, 0x2000);
        }
        map_prg_page(&mut common, 3, -1, 0x2000);
        for i in 0..8 {
            common.map_chr_1k(i, i * 0x400);
        }
        Self {
            common,
            prg_banks: [0; 3],
            chr_banks: [0; 8],
            irq_reload: [0; 4],
            irq_counter: 0,
            irq_counter_size: 0,
            irq_enabled: false,
        }
    }

    fn update_prg_bank(&mut self, bank: usize, value: u8, upper: bool) {
        if upper {
            self.prg_banks[bank] = (self.prg_banks[bank] & 0x0F) | (value << 4);
        } else {
            self.prg_banks[bank] = (self.prg_banks[bank] & 0xF0) | value;
        }
        map_prg_page(
            &mut self.common,
            bank,
            self.prg_banks[bank] as i32,
            0x2000,
        );
    }

    fn update_chr_bank(&mut self, bank: usize, value: u8, upper: bool) {
        if upper {
            self.chr_banks[bank] = (self.chr_banks[bank] & 0x0F) | (value << 4);
        } else {
            self.chr_banks[bank] = (self.chr_banks[bank] & 0xF0) | value;
        }
        map_chr_page(
            &mut self.common,
            bank,
            self.chr_banks[bank] as i32,
            0x400,
        );
    }

    fn reload_irq_counter(&mut self) {
        self.irq_counter = self.irq_reload[0] as u16
            | ((self.irq_reload[1] as u16) << 4)
            | ((self.irq_reload[2] as u16) << 8)
            | ((self.irq_reload[3] as u16) << 12);
    }

    fn clock_irq_counter(&mut self) {
        if !self.irq_enabled {
            return;
        }
        let mask: u16 = match self.irq_counter_size {
            1 => 0x0FFF,
            2 => 0x00FF,
            3 => 0x000F,
            _ => 0xFFFF,
        };
        let mut counter = self.irq_counter & mask;
        counter = counter.wrapping_sub(1);
        if counter == 0 {
            self.common.irq_pending = true;
        }
        self.irq_counter = (self.irq_counter & !mask) | (counter & mask);
    }
}

impl Mapper for JalecoSs88006 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }
    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }
    fn reset(&mut self) {
        self.common.irq_pending = false;
    }
    default_ppu_methods!();

    fn cpu_write(&mut self, addr: u16, value: u8) {
        if addr < 0x8000 {
            if (0x6000..=0x7FFF).contains(&addr) {
                self.common.write_prg_ram(addr, value);
            }
            return;
        }
        let upper = (addr & 0x01) == 0x01;
        let value = value & 0x0F;
        match addr & 0xF003 {
            0x8000 | 0x8001 => self.update_prg_bank(0, value, upper),
            0x8002 | 0x8003 => self.update_prg_bank(1, value, upper),
            0x9000 | 0x9001 => self.update_prg_bank(2, value, upper),
            0xA000 | 0xA001 => self.update_chr_bank(0, value, upper),
            0xA002 | 0xA003 => self.update_chr_bank(1, value, upper),
            0xB000 | 0xB001 => self.update_chr_bank(2, value, upper),
            0xB002 | 0xB003 => self.update_chr_bank(3, value, upper),
            0xC000 | 0xC001 => self.update_chr_bank(4, value, upper),
            0xC002 | 0xC003 => self.update_chr_bank(5, value, upper),
            0xD000 | 0xD001 => self.update_chr_bank(6, value, upper),
            0xD002 | 0xD003 => self.update_chr_bank(7, value, upper),
            0xE000..=0xE003 => self.irq_reload[(addr & 0x03) as usize] = value,
            0xF000 => {
                self.common.irq_pending = false;
                self.reload_irq_counter();
            }
            0xF001 => {
                self.common.irq_pending = false;
                self.irq_enabled = (value & 0x01) == 0x01;
                if value & 0x08 != 0 {
                    self.irq_counter_size = 3;
                } else if value & 0x04 != 0 {
                    self.irq_counter_size = 2;
                } else if value & 0x02 != 0 {
                    self.irq_counter_size = 1;
                } else {
                    self.irq_counter_size = 0;
                }
            }
            0xF002 => {
                self.common.mirroring = match value & 0x03 {
                    0 => Mirroring::Horizontal,
                    1 => Mirroring::Vertical,
                    2 => Mirroring::ScreenAOnly,
                    _ => Mirroring::ScreenBOnly,
                };
            }
            0xF003 => {}
            _ => {}
        }
    }

    fn cpu_cycle(&mut self) {
        self.clock_irq_counter();
    }

    fn irq_pending(&self) -> bool {
        self.common.irq_pending
    }

    fn battery(&self) -> Option<&[u8]> {
        if self.common.info.has_battery {
            Some(&self.common.prg_ram)
        } else {
            None
        }
    }

    fn set_battery(&mut self, data: &[u8]) {
        let n = data.len().min(self.common.prg_ram.len());
        self.common.prg_ram[..n].copy_from_slice(&data[..n]);
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);
        w.fixed_bytes(&self.prg_banks);
        w.fixed_bytes(&self.chr_banks);
        w.fixed_bytes(&self.irq_reload);
        w.u16(self.irq_counter);
        w.u8(self.irq_counter_size);
        w.bool(self.irq_enabled);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        let prg = r.fixed_bytes(3)?;
        self.prg_banks.copy_from_slice(&prg);
        let chr = r.fixed_bytes(8)?;
        self.chr_banks.copy_from_slice(&chr);
        let reload = r.fixed_bytes(4)?;
        self.irq_reload.copy_from_slice(&reload);
        self.irq_counter = r.u16()?;
        self.irq_counter_size = r.u8()?;
        self.irq_enabled = r.bool()?;
        Ok(())
    }
}

// ===========================================================================
// Jaleco JF-17 / JF-19 (mappers 72 and 92)
// ===========================================================================

struct JalecoJf17_19 {
    common: MapperCommon,
    is_jf19: bool,
    prg_flag: bool,
    chr_flag: bool,
}

impl JalecoJf17_19 {
    fn new(rom: RomData, is_jf19: bool) -> Self {
        let mut common = MapperCommon::new(rom);
        map_prg_page(&mut common, 0, 0, 0x4000);
        map_prg_page(&mut common, 2, -1, 0x4000);
        map_chr_page(&mut common, 0, 0, 0x2000);
        Self {
            common,
            is_jf19,
            prg_flag: false,
            chr_flag: false,
        }
    }
}

impl Mapper for JalecoJf17_19 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }
    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }
    fn reset(&mut self) {
        self.common.irq_pending = false;
    }
    default_ppu_methods!();

    fn cpu_write(&mut self, addr: u16, value: u8) {
        if addr < 0x8000 {
            if (0x6000..=0x7FFF).contains(&addr) {
                self.common.write_prg_ram(addr, value);
            }
            return;
        }
        // Bus conflicts: the ROM value at the write address is ANDed in.
        let value = value & self.common.read_prg_rom(addr);

        if !self.prg_flag && value & 0x80 != 0 {
            if self.is_jf19 {
                map_prg_page(&mut self.common, 2, (value & 0x0F) as i32, 0x4000);
            } else {
                map_prg_page(&mut self.common, 0, (value & 0x07) as i32, 0x4000);
            }
        }
        if !self.chr_flag && value & 0x40 != 0 {
            map_chr_page(&mut self.common, 0, (value & 0x0F) as i32, 0x2000);
        }
        self.prg_flag = (value & 0x80) == 0x80;
        self.chr_flag = (value & 0x40) == 0x40;
    }

    fn battery(&self) -> Option<&[u8]> {
        default_battery(&self.common)
    }

    fn set_battery(&mut self, data: &[u8]) {
        default_set_battery(&mut self.common, data);
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);
        w.bool(self.prg_flag);
        w.bool(self.chr_flag);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        self.prg_flag = r.bool()?;
        self.chr_flag = r.bool()?;
        Ok(())
    }
}

// ===========================================================================
// Jaleco JF-13 (mapper 86), JF-xx (87), JF-11/14 (140)
// ===========================================================================

struct JalecoJf13 {
    common: MapperCommon,
}

impl JalecoJf13 {
    fn new(rom: RomData) -> Self {
        let mut common = MapperCommon::new(rom);
        map_prg_page(&mut common, 0, 0, 0x8000);
        map_chr_page(&mut common, 0, 0, 0x2000);
        Self { common }
    }
}

impl Mapper for JalecoJf13 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }
    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }
    fn reset(&mut self) {
        self.common.irq_pending = false;
    }
    default_ppu_methods!();
    fn cpu_write(&mut self, addr: u16, value: u8) {
        if !(0x6000..=0x7FFF).contains(&addr) {
            return;
        }
        if addr & 0x7000 == 0x6000 {
            map_prg_page(
                &mut self.common,
                0,
                ((value & 0x30) >> 4) as i32,
                0x8000,
            );
            map_chr_page(
                &mut self.common,
                0,
                ((value & 0x03) | ((value >> 4) & 0x04)) as i32,
                0x2000,
            );
        }
        // $7000 is the (unimplemented upstream) expansion audio register.
    }

    fn battery(&self) -> Option<&[u8]> {
        default_battery(&self.common)
    }

    fn set_battery(&mut self, data: &[u8]) {
        default_set_battery(&mut self.common, data);
    }
}

struct JalecoJfxx {
    common: MapperCommon,
    ordered_bits: bool,
}

impl JalecoJfxx {
    fn new(rom: RomData, ordered_bits: bool) -> Self {
        let mut common = MapperCommon::new(rom);
        map_prg_page(&mut common, 0, 0, 0x8000);
        map_chr_page(&mut common, 0, 0, 0x2000);
        Self { common, ordered_bits }
    }
}

impl Mapper for JalecoJfxx {
    fn common(&self) -> &MapperCommon {
        &self.common
    }
    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }
    fn reset(&mut self) {
        self.common.irq_pending = false;
    }
    default_ppu_methods!();
    fn cpu_write(&mut self, addr: u16, value: u8) {
        if !(0x6000..=0x7FFF).contains(&addr) {
            return;
        }
        let page = if self.ordered_bits {
            value as i32
        } else {
            (((value & 0x01) << 1) | ((value & 0x02) >> 1)) as i32
        };
        map_chr_page(&mut self.common, 0, page, 0x2000);
    }

    fn battery(&self) -> Option<&[u8]> {
        default_battery(&self.common)
    }

    fn set_battery(&mut self, data: &[u8]) {
        default_set_battery(&mut self.common, data);
    }
}

struct JalecoJf11_14 {
    common: MapperCommon,
}

impl JalecoJf11_14 {
    fn new(rom: RomData) -> Self {
        let mut common = MapperCommon::new(rom);
        map_prg_page(&mut common, 0, 0, 0x8000);
        map_chr_page(&mut common, 0, 0, 0x2000);
        Self { common }
    }
}

impl Mapper for JalecoJf11_14 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }
    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }
    fn reset(&mut self) {
        self.common.irq_pending = false;
    }
    default_ppu_methods!();
    fn cpu_write(&mut self, addr: u16, value: u8) {
        if !(0x6000..=0x7FFF).contains(&addr) {
            return;
        }
        map_prg_page(
            &mut self.common,
            0,
            ((value >> 4) & 0x03) as i32,
            0x8000,
        );
        map_chr_page(&mut self.common, 0, (value & 0x0F) as i32, 0x2000);
    }

    fn battery(&self) -> Option<&[u8]> {
        default_battery(&self.common)
    }

    fn set_battery(&mut self, data: &[u8]) {
        default_set_battery(&mut self.common, data);
    }
}

// ===========================================================================
// Irem G-101 (mapper 32)
// ===========================================================================

struct IremG101 {
    common: MapperCommon,
    prg_regs: [u8; 2],
    prg_mode: u8,
    submapper: u8,
}

impl IremG101 {
    fn new(rom: RomData) -> Self {
        let submapper = rom.info.submapper;
        let mut common = MapperCommon::new(rom);
        for i in 0..8 {
            common.map_chr_1k(i, i * 0x400);
        }
        map_prg_page(&mut common, 0, 0, 0x2000);
        map_prg_page(&mut common, 1, 0, 0x2000);
        map_prg_page(&mut common, 2, -2, 0x2000);
        map_prg_page(&mut common, 3, -1, 0x2000);
        if submapper == 1 {
            common.mirroring = Mirroring::ScreenAOnly;
        }
        Self {
            common,
            prg_regs: [0; 2],
            prg_mode: 0,
            submapper,
        }
    }

    fn update_prg_mode(&mut self) {
        if self.prg_mode == 0 {
            map_prg_page(&mut self.common, 0, self.prg_regs[0] as i32, 0x2000);
            map_prg_page(&mut self.common, 1, self.prg_regs[1] as i32, 0x2000);
            map_prg_page(&mut self.common, 2, -2, 0x2000);
            map_prg_page(&mut self.common, 3, -1, 0x2000);
        } else {
            map_prg_page(&mut self.common, 0, -2, 0x2000);
            map_prg_page(&mut self.common, 1, self.prg_regs[1] as i32, 0x2000);
            map_prg_page(&mut self.common, 2, self.prg_regs[0] as i32, 0x2000);
            map_prg_page(&mut self.common, 3, -1, 0x2000);
        }
    }
}

impl Mapper for IremG101 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }
    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }
    fn reset(&mut self) {
        self.common.irq_pending = false;
    }
    default_ppu_methods!();

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr & 0xF000 {
            0x8000 => {
                self.prg_regs[0] = value & 0x1F;
                let slot = if self.prg_mode == 0 { 0 } else { 2 };
                map_prg_page(
                    &mut self.common,
                    slot,
                    self.prg_regs[0] as i32,
                    0x2000,
                );
            }
            0x9000 => {
                self.prg_mode = (value & 0x02) >> 1;
                if self.submapper == 1 {
                    self.prg_mode = 0;
                }
                self.update_prg_mode();
                self.common.mirroring = if value & 0x01 != 0 {
                    Mirroring::Horizontal
                } else {
                    Mirroring::Vertical
                };
            }
            0xA000 => {
                self.prg_regs[1] = value & 0x1F;
                map_prg_page(
                    &mut self.common,
                    1,
                    self.prg_regs[1] as i32,
                    0x2000,
                );
            }
            0xB000 => {
                map_chr_page(
                    &mut self.common,
                    (addr & 0x07) as usize,
                    value as i32,
                    0x400,
                );
            }
            _ => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    self.common.write_prg_ram(addr, value);
                }
            }
        }
    }

    fn battery(&self) -> Option<&[u8]> {
        default_battery(&self.common)
    }

    fn set_battery(&mut self, data: &[u8]) {
        default_set_battery(&mut self.common, data);
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);
        w.u8(self.prg_regs[0]);
        w.u8(self.prg_regs[1]);
        w.u8(self.prg_mode);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        self.prg_regs[0] = r.u8()?;
        self.prg_regs[1] = r.u8()?;
        self.prg_mode = r.u8()?;
        Ok(())
    }
}

// ===========================================================================
// Irem H-3001 (mapper 65)
// ===========================================================================

struct IremH3001 {
    common: MapperCommon,
    irq_enabled: bool,
    irq_counter: u16,
    irq_reload_value: u16,
}

impl IremH3001 {
    fn new(rom: RomData) -> Self {
        let mut common = MapperCommon::new(rom);
        for i in 0..8 {
            common.map_chr_1k(i, i * 0x400);
        }
        map_prg_page(&mut common, 0, 0, 0x2000);
        map_prg_page(&mut common, 1, 1, 0x2000);
        map_prg_page(&mut common, 2, 0xFE, 0x2000);
        map_prg_page(&mut common, 3, -1, 0x2000);
        Self {
            common,
            irq_enabled: false,
            irq_counter: 0,
            irq_reload_value: 0,
        }
    }
}

impl Mapper for IremH3001 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }
    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }
    fn reset(&mut self) {
        self.common.irq_pending = false;
    }
    default_ppu_methods!();

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x8000 => map_prg_page(&mut self.common, 0, value as i32, 0x2000),
            0x9001 => {
                self.common.mirroring = if value & 0x80 != 0 {
                    Mirroring::Horizontal
                } else {
                    Mirroring::Vertical
                };
            }
            0x9003 => {
                self.irq_enabled = (value & 0x80) == 0x80;
                self.common.irq_pending = false;
            }
            0x9004 => {
                self.irq_counter = self.irq_reload_value;
                self.common.irq_pending = false;
            }
            0x9005 => {
                self.irq_reload_value =
                    (self.irq_reload_value & 0x00FF) | ((value as u16) << 8);
            }
            0x9006 => {
                self.irq_reload_value =
                    (self.irq_reload_value & 0xFF00) | value as u16;
            }
            0xA000 => map_prg_page(&mut self.common, 1, value as i32, 0x2000),
            0xB000..=0xB007 => {
                map_chr_page(
                    &mut self.common,
                    (addr & 0x07) as usize,
                    value as i32,
                    0x400,
                );
            }
            0xC000 => map_prg_page(&mut self.common, 2, value as i32, 0x2000),
            _ => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    self.common.write_prg_ram(addr, value);
                }
            }
        }
    }

    fn cpu_cycle(&mut self) {
        if self.irq_enabled {
            self.irq_counter = self.irq_counter.wrapping_sub(1);
            if self.irq_counter == 0 {
                self.irq_enabled = false;
                self.common.irq_pending = true;
            }
        }
    }

    fn irq_pending(&self) -> bool {
        self.common.irq_pending
    }

    fn battery(&self) -> Option<&[u8]> {
        default_battery(&self.common)
    }

    fn set_battery(&mut self, data: &[u8]) {
        default_set_battery(&mut self.common, data);
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);
        w.bool(self.irq_enabled);
        w.u16(self.irq_counter);
        w.u16(self.irq_reload_value);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        self.irq_enabled = r.bool()?;
        self.irq_counter = r.u16()?;
        self.irq_reload_value = r.u16()?;
        Ok(())
    }
}

// ===========================================================================
// Taito TC0190 (mapper 33)
// ===========================================================================

struct TaitoTc0190 {
    common: MapperCommon,
}

impl TaitoTc0190 {
    fn new(rom: RomData) -> Self {
        let mut common = MapperCommon::new(rom);
        for i in 0..8 {
            common.map_chr_1k(i, i * 0x400);
        }
        map_prg_page(&mut common, 0, 0, 0x2000);
        map_prg_page(&mut common, 1, 0, 0x2000);
        map_prg_page(&mut common, 2, -2, 0x2000);
        map_prg_page(&mut common, 3, -1, 0x2000);
        Self { common }
    }
}

impl Mapper for TaitoTc0190 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }
    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }
    fn reset(&mut self) {
        self.common.irq_pending = false;
    }
    default_ppu_methods!();

    fn cpu_write(&mut self, addr: u16, value: u8) {
        if addr < 0x8000 {
            if (0x6000..=0x7FFF).contains(&addr) {
                self.common.write_prg_ram(addr, value);
            }
            return;
        }
        match addr & 0xA003 {
            0x8000 => {
                map_prg_page(
                    &mut self.common,
                    0,
                    (value & 0x3F) as i32,
                    0x2000,
                );
                self.common.mirroring = if value & 0x40 != 0 {
                    Mirroring::Horizontal
                } else {
                    Mirroring::Vertical
                };
            }
            0x8001 => {
                map_prg_page(
                    &mut self.common,
                    1,
                    (value & 0x3F) as i32,
                    0x2000,
                );
            }
            0x8002 => {
                map_chr_page(&mut self.common, 0, value as i32 * 2, 0x400);
                map_chr_page(&mut self.common, 1, value as i32 * 2 + 1, 0x400);
            }
            0x8003 => {
                map_chr_page(&mut self.common, 2, value as i32 * 2, 0x400);
                map_chr_page(&mut self.common, 3, value as i32 * 2 + 1, 0x400);
            }
            0xA000..=0xA003 => {
                let slot = 4 + (addr & 0x03) as usize;
                map_chr_page(&mut self.common, slot, value as i32, 0x400);
            }
            _ => {}
        }
    }

    fn battery(&self) -> Option<&[u8]> {
        default_battery(&self.common)
    }

    fn set_battery(&mut self, data: &[u8]) {
        default_set_battery(&mut self.common, data);
    }
}

// ===========================================================================
// Taito TC0690 (mapper 48)
// ===========================================================================

struct TaitoTc0690 {
    common: MapperCommon,
    irq_reload_value: u8,
    irq_counter: u8,
    irq_reload: bool,
    irq_enabled: bool,
    irq_delay: u8,
    is_flintstones: bool,
    a12: A12Watcher,
}

impl TaitoTc0690 {
    fn new(rom: RomData) -> Self {
        let is_flintstones = rom.info.submapper == 255;
        let mut common = MapperCommon::new(rom);
        for i in 0..8 {
            common.map_chr_1k(i, i * 0x400);
        }
        map_prg_page(&mut common, 0, 0, 0x2000);
        map_prg_page(&mut common, 1, 0, 0x2000);
        map_prg_page(&mut common, 2, -2, 0x2000);
        map_prg_page(&mut common, 3, -1, 0x2000);
        Self {
            common,
            irq_reload_value: 0,
            irq_counter: 0,
            irq_reload: false,
            irq_enabled: false,
            irq_delay: 0,
            is_flintstones,
            a12: A12Watcher::default(),
        }
    }

    fn clock_irq_counter(&mut self) {
        if self.irq_counter == 0 || self.irq_reload {
            self.irq_counter = self.irq_reload_value;
        } else {
            self.irq_counter -= 1;
        }
        if self.irq_counter == 0 && self.irq_enabled {
            self.irq_delay = if self.is_flintstones { 19 } else { 6 };
        }
        self.irq_reload = false;
    }
}

impl Mapper for TaitoTc0690 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }
    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }
    fn reset(&mut self) {
        self.common.irq_pending = false;
        self.irq_delay = 0;
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        if addr < 0x8000 {
            if (0x6000..=0x7FFF).contains(&addr) {
                self.common.write_prg_ram(addr, value);
            }
            return;
        }
        match addr & 0xE003 {
            0x8000 => map_prg_page(
                &mut self.common,
                0,
                (value & 0x3F) as i32,
                0x2000,
            ),
            0x8001 => map_prg_page(
                &mut self.common,
                1,
                (value & 0x3F) as i32,
                0x2000,
            ),
            0x8002 => {
                map_chr_page(&mut self.common, 0, value as i32 * 2, 0x400);
                map_chr_page(&mut self.common, 1, value as i32 * 2 + 1, 0x400);
            }
            0x8003 => {
                map_chr_page(&mut self.common, 2, value as i32 * 2, 0x400);
                map_chr_page(&mut self.common, 3, value as i32 * 2 + 1, 0x400);
            }
            0xA000..=0xA003 => {
                let slot = 4 + (addr & 0x03) as usize;
                map_chr_page(&mut self.common, slot, value as i32, 0x400);
            }
            0xC000 => {
                self.common.irq_pending = false;
                self.irq_reload_value = if self.is_flintstones {
                    !value
                } else {
                    value.wrapping_neg()
                };
            }
            0xC001 => {
                self.common.irq_pending = false;
                self.irq_counter = 0;
                self.irq_reload = true;
            }
            0xC002 => self.irq_enabled = true,
            0xC003 => {
                self.irq_enabled = false;
                self.common.irq_pending = false;
            }
            0xE000 => {
                self.common.mirroring = if value & 0x40 != 0 {
                    Mirroring::Horizontal
                } else {
                    Mirroring::Vertical
                };
            }
            _ => {}
        }
    }

    fn ppu_read(&mut self, addr: u16) -> Option<u8> {
        if self.a12.update(addr) {
            self.clock_irq_counter();
        }
        let a = addr & 0x3FFF;
        match a {
            0x0000..=0x1FFF => Some(self.common.read_chr(addr)),
            0x2000..=0x3EFF => Some(self.common.read_nametable(addr & 0x2FFF)),
            _ => None,
        }
    }

    fn ppu_write(&mut self, addr: u16, value: u8) {
        if self.a12.update(addr) {
            self.clock_irq_counter();
        }
        match addr & 0x3FFF {
            0x0000..=0x1FFF => self.common.write_chr(addr, value),
            0x2000..=0x3EFF => {
                self.common.write_nametable(addr & 0x2FFF, value)
            }
            _ => {}
        }
    }

    fn cpu_cycle(&mut self) {
        self.a12.tick();
        if self.irq_delay != 0 {
            self.irq_delay -= 1;
            if self.irq_delay == 0 {
                self.common.irq_pending = true;
            }
        }
    }

    fn irq_pending(&self) -> bool {
        self.common.irq_pending
    }

    fn battery(&self) -> Option<&[u8]> {
        default_battery(&self.common)
    }

    fn set_battery(&mut self, data: &[u8]) {
        default_set_battery(&mut self.common, data);
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);
        w.u8(self.irq_reload_value);
        w.u8(self.irq_counter);
        w.bool(self.irq_reload);
        w.bool(self.irq_enabled);
        w.u8(self.irq_delay);
        self.a12.save_state(w);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        self.irq_reload_value = r.u8()?;
        self.irq_counter = r.u8()?;
        self.irq_reload = r.bool()?;
        self.irq_enabled = r.bool()?;
        self.irq_delay = r.u8()?;
        self.a12.load_state(r)?;
        Ok(())
    }
}

// ===========================================================================
// Taito X1-017 (mapper 82)
// ===========================================================================

struct TaitoX1017 {
    common: MapperCommon,
    chr_mode: u8,
    chr_regs: [u8; 6],
    ram_permission: [u8; 3],
}

impl TaitoX1017 {
    fn new(rom: RomData) -> Self {
        let mut common = MapperCommon::new(rom);
        common.prg_ram.resize(common.prg_ram.len().max(0x1400), 0);
        for i in 0..8 {
            common.map_chr_1k(i, i * 0x400);
        }
        map_prg_page(&mut common, 0, 0, 0x2000);
        map_prg_page(&mut common, 1, 0, 0x2000);
        map_prg_page(&mut common, 2, 0, 0x2000);
        map_prg_page(&mut common, 3, -1, 0x2000);
        Self {
            common,
            chr_mode: 0,
            chr_regs: [0; 6],
            ram_permission: [0; 3],
        }
    }

    fn update_chr_banking(&mut self) {
        if self.chr_mode == 0 {
            map_chr_page(
                &mut self.common,
                0,
                (self.chr_regs[0] & 0xFE) as i32,
                0x400,
            );
            map_chr_page(
                &mut self.common,
                1,
                ((self.chr_regs[0] & 0xFE) + 1) as i32,
                0x400,
            );
            map_chr_page(
                &mut self.common,
                2,
                (self.chr_regs[1] & 0xFE) as i32,
                0x400,
            );
            map_chr_page(
                &mut self.common,
                3,
                ((self.chr_regs[1] & 0xFE) + 1) as i32,
                0x400,
            );
            for i in 0..4 {
                map_chr_page(
                    &mut self.common,
                    4 + i,
                    self.chr_regs[2 + i] as i32,
                    0x400,
                );
            }
        } else {
            for i in 0..4 {
                map_chr_page(
                    &mut self.common,
                    i,
                    self.chr_regs[2 + i] as i32,
                    0x400,
                );
            }
            map_chr_page(
                &mut self.common,
                4,
                (self.chr_regs[0] & 0xFE) as i32,
                0x400,
            );
            map_chr_page(
                &mut self.common,
                5,
                ((self.chr_regs[0] & 0xFE) + 1) as i32,
                0x400,
            );
            map_chr_page(
                &mut self.common,
                6,
                (self.chr_regs[1] & 0xFE) as i32,
                0x400,
            );
            map_chr_page(
                &mut self.common,
                7,
                ((self.chr_regs[1] & 0xFE) + 1) as i32,
                0x400,
            );
        }
    }

    fn ram_offset(&self, addr: u16) -> Option<usize> {
        let page = match addr {
            0x6000..=0x67FF if self.ram_permission[0] == 0xCA => {
                Some((addr as usize - 0x6000) / 0x400)
            }
            0x6800..=0x6FFF if self.ram_permission[1] == 0x69 => {
                Some(2 + (addr as usize - 0x6800) / 0x400)
            }
            0x7000..=0x73FF if self.ram_permission[2] == 0x84 => {
                Some(4 + (addr as usize - 0x7000) / 0x400)
            }
            _ => None,
        };
        page.map(|p| p * 0x400 + (addr as usize & 0x3FF))
    }
}

impl Mapper for TaitoX1017 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }
    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }
    fn reset(&mut self) {
        self.common.irq_pending = false;
    }
    default_ppu_methods!();

    fn cpu_read(&mut self, addr: u16) -> Option<u8> {
        match addr {
            0x6000..=0x7FFF => self
                .ram_offset(addr)
                .map(|off| self.common.prg_ram[off % self.common.prg_ram.len()]),
            0x8000..=0xFFFF => Some(self.common.read_prg_rom(addr)),
            _ => None,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x7EF0..=0x7EFF => match addr {
                0x7EF0..=0x7EF5 => {
                    self.chr_regs[(addr & 0x0F) as usize] = value;
                    self.update_chr_banking();
                }
                0x7EF6 => {
                    self.common.mirroring = if value & 0x01 != 0 {
                        Mirroring::Vertical
                    } else {
                        Mirroring::Horizontal
                    };
                    self.chr_mode = (value & 0x02) >> 1;
                    self.update_chr_banking();
                }
                0x7EF7..=0x7EF9 => {
                    self.ram_permission[(addr & 0x0F) as usize - 7] = value;
                }
                0x7EFA => map_prg_page(
                    &mut self.common,
                    0,
                    (value >> 2) as i32,
                    0x2000,
                ),
                0x7EFB => map_prg_page(
                    &mut self.common,
                    1,
                    (value >> 2) as i32,
                    0x2000,
                ),
                0x7EFC => map_prg_page(
                    &mut self.common,
                    2,
                    (value >> 2) as i32,
                    0x2000,
                ),
                _ => {}
            },
            0x6000..=0x7FFF => {
                if let Some(off) = self.ram_offset(addr) {
                    let len = self.common.prg_ram.len();
                    self.common.prg_ram[off % len] = value;
                }
            }
            0x8000..=0xFFFF => {}
            _ => {}
        }
    }

    fn battery(&self) -> Option<&[u8]> {
        if self.common.info.has_battery {
            Some(&self.common.prg_ram[..0x1400.min(self.common.prg_ram.len())])
        } else {
            None
        }
    }

    fn set_battery(&mut self, data: &[u8]) {
        let n = data.len().min(self.common.prg_ram.len());
        self.common.prg_ram[..n].copy_from_slice(&data[..n]);
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);
        for perm in self.ram_permission {
            w.u8(perm);
        }
        for reg in self.chr_regs {
            w.u8(reg);
        }
        w.u8(self.chr_mode);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        for perm in &mut self.ram_permission {
            *perm = r.u8()?;
        }
        for reg in &mut self.chr_regs {
            *reg = r.u8()?;
        }
        self.chr_mode = r.u8()?;
        self.update_chr_banking();
        Ok(())
    }
}

// ===========================================================================
// Bandai FCG / LZ93D50 (mappers 16 and 159)
// ===========================================================================

#[derive(Clone, Copy, PartialEq, Eq)]
enum EepromMode {
    Idle,
    Address,
    Read,
    Write,
    SendAck,
    WaitAck,
    ChipAddress,
}

impl EepromMode {
    fn to_u8(self) -> u8 {
        match self {
            EepromMode::Idle => 0,
            EepromMode::Address => 1,
            EepromMode::Read => 2,
            EepromMode::Write => 3,
            EepromMode::SendAck => 4,
            EepromMode::WaitAck => 5,
            EepromMode::ChipAddress => 6,
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            0 => EepromMode::Idle,
            1 => EepromMode::Address,
            2 => EepromMode::Read,
            3 => EepromMode::Write,
            4 => EepromMode::SendAck,
            5 => EepromMode::WaitAck,
            _ => EepromMode::ChipAddress,
        }
    }
}

/// 24C01 / 24C02 serial EEPROM, ported from `Eeprom24C01.h` / `Eeprom24C02.h`.
#[derive(Clone)]
struct Eeprom24C0X {
    is_24c02: bool,
    mode: EepromMode,
    next_mode: EepromMode,
    chip_address: u8,
    address: u8,
    data: u8,
    counter: u8,
    output: u8,
    prev_scl: u8,
    prev_sda: u8,
    rom: Vec<u8>,
}

impl Eeprom24C0X {
    fn new(size: usize, initial: Option<&[u8]>) -> Self {
        let mut rom = vec![0u8; size];
        if let Some(data) = initial {
            let n = data.len().min(size);
            rom[..n].copy_from_slice(&data[..n]);
        }
        Self {
            is_24c02: size == 256,
            mode: EepromMode::Idle,
            next_mode: EepromMode::Idle,
            chip_address: 0,
            address: 0,
            data: 0,
            counter: 0,
            output: 0,
            prev_scl: 0,
            prev_sda: 0,
            rom,
        }
    }

    /// LSB-first bit writer used by the 24C01.
    fn write_bit_lsb(dest: &mut u8, counter: &mut u8, value: u8) {
        if *counter < 8 {
            let mask = !(1u8 << *counter);
            *dest = (*dest & mask) | ((value & 1) << *counter);
            *counter += 1;
        }
    }

    /// MSB-first bit writer used by the 24C02.
    fn write_bit_msb(dest: &mut u8, counter: &mut u8, value: u8) {
        if *counter < 8 {
            let shift = 7 - *counter;
            let mask = !(1u8 << shift);
            *dest = (*dest & mask) | ((value & 1) << shift);
            *counter += 1;
        }
    }

    fn read_bit_lsb(data: u8, counter: &mut u8, current: u8) -> u8 {
        if *counter < 8 {
            let value = (data >> *counter) & 1;
            *counter += 1;
            value
        } else {
            current
        }
    }

    fn read_bit_msb(data: u8, counter: &mut u8, current: u8) -> u8 {
        if *counter < 8 {
            let value = (data >> (7 - *counter)) & 1;
            *counter += 1;
            value
        } else {
            current
        }
    }

    fn write(&mut self, scl: u8, sda: u8) {
        if self.prev_scl != 0 && scl != 0 && sda < self.prev_sda {
            // START
            if self.is_24c02 {
                self.mode = EepromMode::ChipAddress;
            } else {
                self.mode = EepromMode::Address;
                self.address = 0;
            }
            self.counter = 0;
            self.output = 1;
        } else if self.prev_scl != 0 && scl != 0 && sda > self.prev_sda {
            // STOP
            self.mode = EepromMode::Idle;
            self.output = 1;
        } else if scl > self.prev_scl {
            if self.is_24c02 {
                match self.mode {
                    EepromMode::ChipAddress => {
                        Self::write_bit_msb(&mut self.chip_address, &mut self.counter, sda)
                    }
                    EepromMode::Address => {
                        Self::write_bit_msb(&mut self.address, &mut self.counter, sda)
                    }
                    EepromMode::Read => {
                        self.output =
                            Self::read_bit_msb(self.data, &mut self.counter, self.output)
                    }
                    EepromMode::Write => {
                        Self::write_bit_msb(&mut self.data, &mut self.counter, sda)
                    }
                    EepromMode::SendAck => self.output = 0,
                    EepromMode::WaitAck => {
                        if sda == 0 {
                            self.next_mode = EepromMode::Read;
                            self.data = self.rom[self.address as usize];
                        }
                    }
                    EepromMode::Idle => {}
                }
            } else {
                match self.mode {
                    EepromMode::Address => {
                        if self.counter < 7 {
                            Self::write_bit_lsb(&mut self.address, &mut self.counter, sda);
                        } else if self.counter == 7 {
                            self.counter = 8;
                            if sda != 0 {
                                self.next_mode = EepromMode::Read;
                                self.data = self.rom[(self.address & 0x7F) as usize];
                            } else {
                                self.next_mode = EepromMode::Write;
                            }
                        }
                    }
                    EepromMode::SendAck => self.output = 0,
                    EepromMode::Read => {
                        self.output =
                            Self::read_bit_lsb(self.data, &mut self.counter, self.output)
                    }
                    EepromMode::Write => {
                        Self::write_bit_lsb(&mut self.data, &mut self.counter, sda)
                    }
                    EepromMode::WaitAck => {
                        if sda == 0 {
                            self.next_mode = EepromMode::Idle;
                        }
                    }
                    _ => {}
                }
            }
        } else if scl < self.prev_scl {
            if self.is_24c02 {
                match self.mode {
                    EepromMode::ChipAddress => {
                        if self.counter == 8 {
                            if (self.chip_address & 0xA0) == 0xA0 {
                                self.mode = EepromMode::SendAck;
                                self.counter = 0;
                                self.output = 1;
                                if self.chip_address & 0x01 != 0 {
                                    self.next_mode = EepromMode::Read;
                                    self.data = self.rom[self.address as usize];
                                } else {
                                    self.next_mode = EepromMode::Address;
                                }
                            } else {
                                self.mode = EepromMode::Idle;
                                self.counter = 0;
                                self.output = 1;
                            }
                        }
                    }
                    EepromMode::Address => {
                        if self.counter == 8 {
                            self.counter = 0;
                            self.mode = EepromMode::SendAck;
                            self.next_mode = EepromMode::Write;
                            self.output = 1;
                        }
                    }
                    EepromMode::Read => {
                        if self.counter == 8 {
                            self.mode = EepromMode::WaitAck;
                            self.address = self.address.wrapping_add(1);
                        }
                    }
                    EepromMode::Write => {
                        if self.counter == 8 {
                            self.counter = 0;
                            self.mode = EepromMode::SendAck;
                            self.next_mode = EepromMode::Write;
                            self.rom[self.address as usize] = self.data;
                            self.address = self.address.wrapping_add(1);
                        }
                    }
                    EepromMode::SendAck | EepromMode::WaitAck => {
                        self.mode = self.next_mode;
                        self.counter = 0;
                        self.output = 1;
                    }
                    EepromMode::Idle => {}
                }
            } else {
                match self.mode {
                    EepromMode::Address => {
                        if self.counter == 8 {
                            self.mode = EepromMode::SendAck;
                            self.output = 1;
                        }
                    }
                    EepromMode::SendAck => {
                        self.mode = self.next_mode;
                        self.counter = 0;
                        self.output = 1;
                    }
                    EepromMode::Read => {
                        if self.counter == 8 {
                            self.mode = EepromMode::WaitAck;
                            self.address = (self.address + 1) & 0x7F;
                        }
                    }
                    EepromMode::Write => {
                        if self.counter == 8 {
                            self.mode = EepromMode::SendAck;
                            self.next_mode = EepromMode::Idle;
                            self.rom[(self.address & 0x7F) as usize] = self.data;
                            self.address = (self.address + 1) & 0x7F;
                        }
                    }
                    _ => {}
                }
            }
        }
        self.prev_scl = scl;
        self.prev_sda = sda;
    }

    fn read(&self) -> u8 {
        self.output
    }

    fn save_state(&self, w: &mut StateWriter) {
        w.u8(self.mode.to_u8());
        w.u8(self.next_mode.to_u8());
        w.u8(self.chip_address);
        w.u8(self.address);
        w.u8(self.data);
        w.u8(self.counter);
        w.u8(self.output);
        w.u8(self.prev_scl);
        w.u8(self.prev_sda);
        w.bytes(&self.rom);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.mode = EepromMode::from_u8(r.u8()?);
        self.next_mode = EepromMode::from_u8(r.u8()?);
        self.chip_address = r.u8()?;
        self.address = r.u8()?;
        self.data = r.u8()?;
        self.counter = r.u8()?;
        self.output = r.u8()?;
        self.prev_scl = r.u8()?;
        self.prev_sda = r.u8()?;
        let rom = r.bytes()?;
        if rom.len() != self.rom.len() {
            return Err("EEPROM size mismatch in save state".into());
        }
        self.rom.copy_from_slice(&rom);
        Ok(())
    }
}

struct BandaiFcg {
    common: MapperCommon,
    irq_enabled: bool,
    irq_counter: u16,
    irq_reload: u16,
    prg_page: u8,
    prg_bank_select: u8,
    chr_regs: [u8; 8],
    eeprom: Option<Eeprom24C0X>,
    mapper_id: u16,
    submapper: u8,
    is_nes2: bool,
}

impl BandaiFcg {
    fn new(rom: RomData) -> Self {
        let mapper_id = rom.info.mapper;
        let submapper = rom.info.submapper;
        let is_nes2 = rom.info.is_nes2;
        // Mesen's `GetSaveRamSize()` uses the NES 2.0 PRG-NVRAM field.
        let nvram = if rom.info.prg_nvram_size != 0 {
            rom.info.prg_nvram_size
        } else {
            rom.info.prg_ram_size
        };
        let battery = rom.battery.clone();
        let mut common = MapperCommon::new(rom);
        for i in 0..8 {
            common.map_chr_1k(i, i * 0x400);
        }
        map_prg_page(&mut common, 0, 0, 0x4000);
        map_prg_page(&mut common, 2, 0x0F, 0x4000);

        let eeprom = if mapper_id == 159 {
            Some(Eeprom24C0X::new(
                128,
                battery.as_deref().filter(|d| d.len() == 128),
            ))
        } else if mapper_id == 16
            && (!is_nes2 || (submapper == 5 && nvram == 256))
        {
            Some(Eeprom24C0X::new(
                256,
                battery.as_deref().filter(|d| d.len() == 256),
            ))
        } else {
            None
        };

        Self {
            common,
            irq_enabled: false,
            irq_counter: 0,
            irq_reload: 0,
            prg_page: 0,
            prg_bank_select: 0,
            chr_regs: [0; 8],
            eeprom,
            mapper_id,
            submapper,
            is_nes2,
        }
    }

    fn update_prg(&mut self) {
        let page = (self.prg_page | self.prg_bank_select) as i32;
        map_prg_page(&mut self.common, 0, page, 0x4000);
        map_prg_page(
            &mut self.common,
            2,
            (0x0F | self.prg_bank_select) as i32,
            0x4000,
        );
    }

    fn register_write_range(&self, addr: u16) -> bool {
        if self.mapper_id == 16 {
            match self.submapper {
                4 => (0x6000..=0x7FFF).contains(&addr),
                5 => addr >= 0x8000,
                _ => addr >= 0x6000,
            }
        } else {
            addr >= 0x8000
        }
    }

    fn write_register(&mut self, addr: u16, value: u8) {
        match addr & 0x0F {
            reg @ 0x00..=0x07 => {
                self.chr_regs[reg as usize] = value;
                let page_count = (self.common.prg_rom.len() / 0x4000).max(1);
                if self.mapper_id == 153 || page_count >= 0x20 {
                    self.prg_bank_select = 0;
                    for i in 0..8 {
                        self.prg_bank_select |= (self.chr_regs[i] & 0x01) << 4;
                    }
                    self.update_prg();
                } else if !self.common.chr_is_ram && self.mapper_id != 157 {
                    map_chr_page(
                        &mut self.common,
                        (addr & 0x07) as usize,
                        value as i32,
                        0x400,
                    );
                }
            }
            0x08 => {
                self.prg_page = value & 0x0F;
                self.update_prg();
            }
            0x09 => match value & 0x03 {
                0 => self.common.mirroring = Mirroring::Vertical,
                1 => self.common.mirroring = Mirroring::Horizontal,
                2 => self.common.mirroring = Mirroring::ScreenAOnly,
                _ => self.common.mirroring = Mirroring::ScreenBOnly,
            },
            0x0A => {
                self.irq_enabled = (value & 0x01) == 0x01;
                if self.mapper_id != 16 || !self.is_nes2 || self.submapper == 5 {
                    self.irq_counter = self.irq_reload;
                }
                self.common.irq_pending = false;
            }
            0x0B => {
                let fcg_sub4 =
                    self.mapper_id == 16 && self.is_nes2 && self.submapper == 4;
                if fcg_sub4 {
                    self.irq_counter =
                        (self.irq_counter & 0xFF00) | value as u16;
                } else {
                    self.irq_reload = (self.irq_reload & 0xFF00) | value as u16;
                }
            }
            0x0C => {
                let fcg_sub4 =
                    self.mapper_id == 16 && self.is_nes2 && self.submapper == 4;
                if fcg_sub4 {
                    self.irq_counter =
                        (self.irq_counter & 0x00FF) | ((value as u16) << 8);
                } else {
                    self.irq_reload =
                        (self.irq_reload & 0x00FF) | ((value as u16) << 8);
                }
            }
            0x0D => {
                if self.mapper_id != 153 {
                    let scl = (value & 0x20) >> 5;
                    let sda = (value & 0x40) >> 6;
                    if let Some(eeprom) = &mut self.eeprom {
                        eeprom.write(scl, sda);
                    }
                }
            }
            _ => {}
        }
    }
}

impl Mapper for BandaiFcg {
    fn common(&self) -> &MapperCommon {
        &self.common
    }
    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }
    fn reset(&mut self) {
        self.common.irq_pending = false;
    }
    default_ppu_methods!();

    fn cpu_read(&mut self, addr: u16) -> Option<u8> {
        match addr {
            0x6000..=0x7FFF => {
                if let Some(eeprom) = &self.eeprom {
                    Some(0xE0 | (eeprom.read() << 4))
                } else {
                    None
                }
            }
            0x8000..=0xFFFF => Some(self.common.read_prg_rom(addr)),
            _ => None,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        if self.register_write_range(addr) {
            self.write_register(addr, value);
        } else if (0x6000..=0x7FFF).contains(&addr) {
            self.common.write_prg_ram(addr, value);
        }
    }

    fn cpu_cycle(&mut self) {
        if self.irq_enabled {
            if self.irq_counter == 0 {
                self.common.irq_pending = true;
            }
            self.irq_counter = self.irq_counter.wrapping_sub(1);
        }
    }

    fn irq_pending(&self) -> bool {
        self.common.irq_pending
    }

    fn battery(&self) -> Option<&[u8]> {
        self.eeprom.as_ref().map(|e| e.rom.as_slice())
    }

    fn set_battery(&mut self, data: &[u8]) {
        if let Some(eeprom) = &mut self.eeprom {
            let n = data.len().min(eeprom.rom.len());
            eeprom.rom[..n].copy_from_slice(&data[..n]);
        }
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);
        w.bool(self.irq_enabled);
        w.u16(self.irq_counter);
        w.u16(self.irq_reload);
        w.u8(self.prg_page);
        w.u8(self.prg_bank_select);
        w.fixed_bytes(&self.chr_regs);
        w.bool(self.eeprom.is_some());
        if let Some(eeprom) = &self.eeprom {
            eeprom.save_state(w);
        }
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        self.irq_enabled = r.bool()?;
        self.irq_counter = r.u16()?;
        self.irq_reload = r.u16()?;
        self.prg_page = r.u8()?;
        self.prg_bank_select = r.u8()?;
        let regs = r.fixed_bytes(8)?;
        self.chr_regs.copy_from_slice(&regs);
        let has_eeprom = r.bool()?;
        if has_eeprom {
            if let Some(eeprom) = &mut self.eeprom {
                eeprom.load_state(r)?;
            } else {
                return Err("save state has EEPROM but mapper does not".into());
            }
        }
        Ok(())
    }
}

// ===========================================================================
// Bandai 74161/7432 (mapper 70)
// ===========================================================================

struct Bandai74161 {
    common: MapperCommon,
    enable_mirroring_control: bool,
}

impl Bandai74161 {
    fn new(rom: RomData) -> Self {
        let mut common = MapperCommon::new(rom);
        map_prg_page(&mut common, 0, 0, 0x4000);
        map_prg_page(&mut common, 2, -1, 0x4000);
        map_chr_page(&mut common, 0, 0, 0x2000);
        // Upstream forces vertical mirroring as a compatibility hack.
        common.mirroring = Mirroring::Vertical;
        Self {
            common,
            enable_mirroring_control: false,
        }
    }
}

impl Mapper for Bandai74161 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }
    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }
    fn reset(&mut self) {
        self.common.irq_pending = false;
    }
    default_ppu_methods!();

    fn cpu_write(&mut self, addr: u16, value: u8) {
        if addr < 0x8000 {
            if (0x6000..=0x7FFF).contains(&addr) {
                self.common.write_prg_ram(addr, value);
            }
            return;
        }
        let mirroring_bit = (value & 0x80) == 0x80;
        if mirroring_bit {
            self.enable_mirroring_control = true;
        }
        if self.enable_mirroring_control {
            self.common.mirroring = if mirroring_bit {
                Mirroring::ScreenBOnly
            } else {
                Mirroring::ScreenAOnly
            };
        }
        map_prg_page(
            &mut self.common,
            0,
            ((value >> 4) & 0x07) as i32,
            0x4000,
        );
        map_chr_page(&mut self.common, 0, (value & 0x0F) as i32, 0x2000);
    }

    fn battery(&self) -> Option<&[u8]> {
        default_battery(&self.common)
    }

    fn set_battery(&mut self, data: &[u8]) {
        default_set_battery(&mut self.common, data);
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);
        w.bool(self.enable_mirroring_control);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        self.enable_mirroring_control = r.bool()?;
        Ok(())
    }
}

// ===========================================================================
// Namcot 108 / 118 (mapper 88)
// ===========================================================================

struct Namco108_88 {
    common: MapperCommon,
    registers: [u8; 8],
    current_register: u8,
}

impl Namco108_88 {
    fn new(rom: RomData) -> Self {
        let common = MapperCommon::new(rom);
        let mut m = Self {
            common,
            registers: [0, 2, 4, 5, 6, 7, 0, 1],
            current_register: 0,
        };
        m.update_prg();
        m.update_chr();
        m
    }

    fn update_prg(&mut self) {
        map_prg_page(&mut self.common, 0, self.registers[6] as i32, 0x2000);
        map_prg_page(&mut self.common, 1, self.registers[7] as i32, 0x2000);
        map_prg_page(&mut self.common, 2, -2, 0x2000);
        map_prg_page(&mut self.common, 3, -1, 0x2000);
    }

    fn update_chr(&mut self) {
        let r0 = (self.registers[0] & 0x3F) as i32;
        let r1 = (self.registers[1] & 0x3F) as i32;
        let r2 = (self.registers[2] | 0x40) as i32;
        let r3 = (self.registers[3] | 0x40) as i32;
        let r4 = (self.registers[4] | 0x40) as i32;
        let r5 = (self.registers[5] | 0x40) as i32;

        map_chr_page(&mut self.common, 0, r0 & !1, 0x400);
        map_chr_page(&mut self.common, 1, (r0 & !1) + 1, 0x400);
        map_chr_page(&mut self.common, 2, r1 & !1, 0x400);
        map_chr_page(&mut self.common, 3, (r1 & !1) + 1, 0x400);
        map_chr_page(&mut self.common, 4, r2, 0x400);
        map_chr_page(&mut self.common, 5, r3, 0x400);
        map_chr_page(&mut self.common, 6, r4, 0x400);
        map_chr_page(&mut self.common, 7, r5, 0x400);
    }
}

impl Mapper for Namco108_88 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }
    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }
    fn reset(&mut self) {
        self.common.irq_pending = false;
    }
    default_ppu_methods!();

    fn cpu_write(&mut self, addr: u16, value: u8) {
        if addr < 0x8000 {
            if (0x6000..=0x7FFF).contains(&addr) {
                self.common.write_prg_ram(addr, value);
            }
            return;
        }
        if addr & 0x01 == 0 {
            // Namco 108 only exposes registers $8000/$8001; all other writes
            // are redirected by masking with 0x8001.
            let value = value & 0x3F;
            self.current_register = value & 0x07;
        } else {
            let mut value = value;
            if self.current_register <= 1 {
                value &= !0x01;
            }
            self.registers[self.current_register as usize] = value;
            self.update_prg();
            self.update_chr();
        }
    }

    fn battery(&self) -> Option<&[u8]> {
        default_battery(&self.common)
    }

    fn set_battery(&mut self, data: &[u8]) {
        default_set_battery(&mut self.common, data);
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);
        w.fixed_bytes(&self.registers);
        w.u8(self.current_register);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        let regs = r.fixed_bytes(8)?;
        self.registers.copy_from_slice(&regs);
        self.current_register = r.u8()?;
        Ok(())
    }
}

// ===========================================================================
// Sachen 74LS374N (mapper 150)
// ===========================================================================

struct Sachen74LS374N {
    common: MapperCommon,
    current_register: u8,
    regs: [u8; 8],
    nt_pages: [u8; 4],
}

impl Sachen74LS374N {
    fn new(rom: RomData, _mapper_id: u16) -> Self {
        let mut common = MapperCommon::new(rom);
        map_prg_page(&mut common, 0, 0, 0x8000);
        map_chr_page(&mut common, 0, 0, 0x2000);
        let mut m = Self {
            common,
            current_register: 0,
            regs: [0; 8],
            nt_pages: [0, 0, 0, 1],
        };
        m.update_state();
        m
    }

    fn update_state(&mut self) {
        let chr_page =
            (((self.regs[4] & 0x01) << 2) | (self.regs[6] & 0x03)) as i32;
        map_chr_page(&mut self.common, 0, chr_page, 0x2000);
        map_prg_page(
            &mut self.common,
            0,
            (self.regs[5] & 0x03) as i32,
            0x8000,
        );
        self.nt_pages = match (self.regs[7] >> 1) & 0x03 {
            0 => [0, 0, 0, 1],
            1 => [0, 0, 1, 1],
            2 => [0, 1, 0, 1],
            _ => [0, 0, 0, 0],
        };
    }
}

impl Mapper for Sachen74LS374N {
    fn common(&self) -> &MapperCommon {
        &self.common
    }
    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }
    fn reset(&mut self) {
        self.common.irq_pending = false;
    }

    fn cpu_read(&mut self, addr: u16) -> Option<u8> {
        match addr {
            0x4100..=0x7FFF => {
                if addr & 0xC101 == 0x4101 {
                    Some(0xE0 | (self.regs[self.current_register as usize] & 0x07))
                } else {
                    None
                }
            }
            0x8000..=0xFFFF => Some(self.common.read_prg_rom(addr)),
            _ => None,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        if !(0x4100..=0x7FFF).contains(&addr) {
            return;
        }
        match addr & 0xC101 {
            0x4100 => self.current_register = value & 0x07,
            0x4101 => {
                self.regs[self.current_register as usize] = value & 0x07;
                self.update_state();
            }
            _ => {}
        }
    }

    fn ppu_read(&mut self, addr: u16) -> Option<u8> {
        let a = addr & 0x3FFF;
        match a {
            0x0000..=0x1FFF => Some(self.common.read_chr(addr)),
            0x2000..=0x3EFF => {
                let slot = ((a as usize - 0x2000) >> 10) & 3;
                let page = self.nt_pages[slot] as usize;
                let idx = (page * 0x400 + (a as usize & 0x3FF)) & 0x0FFF;
                Some(self.common.nametables[idx])
            }
            _ => None,
        }
    }

    fn ppu_write(&mut self, addr: u16, value: u8) {
        let a = addr & 0x3FFF;
        match a {
            0x0000..=0x1FFF => self.common.write_chr(addr, value),
            0x2000..=0x3EFF => {
                let slot = ((a as usize - 0x2000) >> 10) & 3;
                let page = self.nt_pages[slot] as usize;
                let idx = (page * 0x400 + (a as usize & 0x3FF)) & 0x0FFF;
                self.common.nametables[idx] = value;
            }
            _ => {}
        }
    }

    fn battery(&self) -> Option<&[u8]> {
        default_battery(&self.common)
    }

    fn set_battery(&mut self, data: &[u8]) {
        default_set_battery(&mut self.common, data);
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);
        w.u8(self.current_register);
        w.fixed_bytes(&self.regs);
        w.fixed_bytes(&self.nt_pages);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        self.current_register = r.u8()?;
        let regs = r.fixed_bytes(8)?;
        self.regs.copy_from_slice(&regs);
        let pages = r.fixed_bytes(4)?;
        self.nt_pages.copy_from_slice(&pages);
        Ok(())
    }
}

// ===========================================================================
// Waixing FS304 (mapper 164)
// ===========================================================================

struct Waixing164 {
    common: MapperCommon,
    prg_bank: u8,
}

impl Waixing164 {
    fn new(rom: RomData) -> Self {
        let mut common = MapperCommon::new(rom);
        map_prg_page(&mut common, 0, 0x0F, 0x8000);
        map_chr_page(&mut common, 0, 0, 0x2000);
        Self {
            common,
            prg_bank: 0x0F,
        }
    }
}

impl Mapper for Waixing164 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }
    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }
    fn reset(&mut self) {
        self.common.irq_pending = false;
    }
    default_ppu_methods!();

    fn cpu_write(&mut self, addr: u16, value: u8) {
        if !(0x5000..=0x5FFF).contains(&addr) {
            if (0x6000..=0x7FFF).contains(&addr) {
                self.common.write_prg_ram(addr, value);
            }
            return;
        }
        match addr & 0x7300 {
            0x5000 => {
                self.prg_bank = (self.prg_bank & 0xF0) | (value & 0x0F);
                map_prg_page(&mut self.common, 0, self.prg_bank as i32, 0x8000);
            }
            0x5100 => {
                self.prg_bank =
                    (self.prg_bank & 0x0F) | ((value & 0x0F) << 4);
                map_prg_page(&mut self.common, 0, self.prg_bank as i32, 0x8000);
            }
            _ => {}
        }
    }

    fn battery(&self) -> Option<&[u8]> {
        default_battery(&self.common)
    }

    fn set_battery(&mut self, data: &[u8]) {
        default_set_battery(&mut self.common, data);
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);
        w.u8(self.prg_bank);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        self.prg_bank = r.u8()?;
        Ok(())
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapper::Mapper;
    use crate::state::StateWriter;
    use crate::types::RomInfo;

    fn make_rom(mapper: u16, prg_kb: usize, chr_kb: usize) -> RomData {
        let mut info = RomInfo::default();
        info.mapper = mapper;
        info.prg_rom_size = prg_kb * 1024;
        info.chr_rom_size = chr_kb * 1024;
        let prg: Vec<u8> = (0..prg_kb * 1024).map(|i| (i / 0x2000) as u8).collect();
        let chr: Vec<u8> = (0..chr_kb * 1024).map(|i| (i / 0x400) as u8).collect();
        RomData::new(info, prg, chr)
    }

    fn roundtrip<M: Mapper>(m: &mut M) -> Vec<u8> {
        let mut w = StateWriter::new();
        m.save_state(&mut w);
        let saved = w.into_inner();
        let mut r = StateReader::new(&saved);
        m.load_state(&mut r).expect("load_state");
        assert_eq!(r.remaining(), 0, "state load did not consume all bytes");
        let mut w2 = StateWriter::new();
        m.save_state(&mut w2);
        assert_eq!(saved, w2.into_inner(), "state is not deterministic");
        saved
    }

    #[test]
    fn supports_expected_ids() {
        for id in [
            16u16, 18, 19, 32, 33, 48, 65, 67, 68, 69, 70, 72, 82, 86, 87, 88, 89, 92, 93, 140,
            150, 159, 164, 184, 210,
        ] {
            assert!(supports(id), "mapper {} should be supported", id);
            let rom = make_rom(id, 128, 128);
            assert!(create(id, rom).is_some(), "create({}) failed", id);
            // Also construct with CHR RAM and a small PRG ROM.
            let rom_ram = make_rom(id, 32, 0);
            assert!(
                create(id, rom_ram).is_some(),
                "create({}) with CHR RAM failed",
                id
            );
        }
        assert!(!supports(0));
        assert!(!supports(4));
    }

    #[test]
    fn namco163_prg_chr_and_irq() {
        let rom = make_rom(19, 256, 128);
        let mut m = Namco163Mapper::new(rom);

        // $E000/$E800/$F000 bank the first three 8KB windows; slot 3 is fixed.
        m.cpu_write(0xE000, 5);
        m.cpu_write(0xE800, 6);
        m.cpu_write(0xF000, 7);
        assert_eq!(m.common.prg_map[0], 5 * 0x2000);
        assert_eq!(m.common.prg_map[1], 6 * 0x2000);
        assert_eq!(m.common.prg_map[2], 7 * 0x2000);
        assert_eq!(m.common.prg_map[3], (256 * 1024 / 0x2000 - 1) * 0x2000);

        // CHR banking (1KB windows).
        m.cpu_write(0x8000, 3);
        m.cpu_write(0x9800, 4);
        m.cpu_write(0xA800, 5);
        m.cpu_write(0xB000, 6);
        assert_eq!(m.common.chr_map[0], 3 * 0x400);
        assert_eq!(m.common.chr_map[3], 4 * 0x400);
        assert_eq!(m.common.chr_map[5], 5 * 0x400);
        assert_eq!(m.common.chr_map[6], 6 * 0x400);

        // Nametable CHR mode: value >= $E0 in $C000-$D800 maps $2000-$2FFF
        // windows to nametable pages.
        m.ppu_write(0x2500, 0x5A);
        m.cpu_write(0xC000, 0xE0);
        assert!(matches!(m.nt_sel[0], NtSel::Nametable(0)));
        assert_eq!(m.ppu_read(0x2500), Some(0x5A));

        // $6000-$7FFF is always readable, but write-protected until the
        // global enable bit and per-region bits allow writes.
        assert_eq!(m.cpu_read(0x6000), Some(0));
        m.cpu_write(0x6000, 0x12);
        assert_eq!(m.cpu_read(0x6000), Some(0));
        m.cpu_write(0xF800, 0x42); // global write enable, region 1 protected
        m.cpu_write(0x6000, 0x12);
        assert_eq!(m.cpu_read(0x6000), Some(0x12));
        m.cpu_write(0x6800, 0x34);
        assert_eq!(m.cpu_read(0x6800), Some(0));

        // IRQ increments once per CPU cycle when bit 15 is set.
        m.cpu_write(0x5000, 0x00);
        m.cpu_write(0x5800, 0x80);
        assert!(!m.irq_pending());
        let mut cycles = 0;
        while !m.irq_pending() && cycles < 0x8000 {
            m.cpu_cycle();
            cycles += 1;
        }
        assert!(m.irq_pending(), "Namco 163 IRQ did not fire");
        assert_eq!(m.irq_counter & 0x7FFF, 0x7FFF);
        m.cpu_write(0x5000, 0);
        assert!(!m.irq_pending(), "IRQ write should acknowledge");

        let saved = roundtrip(&mut m);
        assert!(!saved.is_empty());
    }

    #[test]
    fn namco163_audio_registers_and_clock() {
        let rom = make_rom(19, 256, 128);
        let mut m = Namco163Mapper::new(rom);

        // Wave RAM nibble 9 at address 0.
        m.cpu_write(0xF800, 0x00);
        m.cpu_write(0x4800, 0x99);

        // Channel 7 registers: freq = $10, phase = 0, wave length 256,
        // wave address 0, volume 15 (also channel count = 0).  Bit 7 enables
        // the auto-incrementing RAM pointer.
        m.cpu_write(0xF800, 0xF8);
        for value in [0x10u8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0F] {
            m.cpu_write(0x4800, value);
        }
        assert_eq!(m.expansion_audio_sample(), 0.0);
        for _ in 0..15 {
            m.cpu_cycle();
        }
        assert!(
            m.expansion_audio_sample() > 0.0,
            "Namco 163 audio should output after a channel update"
        );

        // Auto-increment read/write pointer.
        m.cpu_write(0xF800, 0x80);
        m.cpu_write(0x4800, 0x42);
        m.cpu_write(0xF800, 0x80);
        assert_eq!(m.cpu_read(0x4800), Some(0x42));
        assert_eq!(m.audio.ram_position, 0x01);

        roundtrip(&mut m);
    }

    #[test]
    fn sunsoft3_banking_and_irq() {
        let rom = make_rom(67, 128, 128);
        let mut m = Sunsoft3::new(rom);
        m.cpu_write(0xF800, 2);
        assert_eq!(m.common.prg_map[0], 2 * 0x4000);
        m.cpu_write(0x8800, 3);
        m.cpu_write(0x9800, 5);
        assert_eq!(m.common.chr_map[0], 3 * 0x800);
        assert_eq!(m.common.chr_map[2], 5 * 0x800);
        m.cpu_write(0xE800, 1);
        assert_eq!(m.common.mirroring, Mirroring::Horizontal);
        // $3000-$3FFF mirrors the same 1KB nametable windows as $2000-$2FFF.
        m.ppu_write(0x2400, 0x5A);
        assert_eq!(m.ppu_read(0x3400), Some(0x5A));
        m.ppu_write(0x2800, 0xA5);
        assert_eq!(m.ppu_read(0x3800), Some(0xA5));

        m.cpu_write(0xC800, 0);
        m.cpu_write(0xC800, 0);
        m.cpu_write(0xD800, 0x10);
        m.cpu_cycle();
        assert!(m.irq_pending(), "Sunsoft 3 IRQ underflow");
        assert!(!m.irq_enabled);
        roundtrip(&mut m);
    }

    #[test]
    fn sunsoft4_chr_nametables() {
        let rom = make_rom(68, 128, 256);
        let mut m = Sunsoft4::new(rom);
        m.cpu_write(0x8000, 3);
        assert_eq!(m.common.chr_map[0], 3 * 0x800);
        m.cpu_write(0xC000, 0x02);
        assert_eq!(m.nt_regs[0], 0x82);
        m.cpu_write(0xE000, 0x10); // CHR nametables, vertical mirroring
        assert_eq!(
            m.ppu_read(0x2000),
            Some(m.common.chr[0x82 * 0x400])
        );
        // Vertical mirroring: $2800 uses nametable register 0 again.
        assert_eq!(m.ppu_read(0x2800), Some(m.common.chr[0x82 * 0x400]));
        m.cpu_write(0xF000, 0x0C); // internal PRG, page 4, RAM enabled
        assert_eq!(m.common.prg_map[0], 4 * 0x4000);
        m.cpu_write(0x6000, 0x77);
        assert_eq!(m.cpu_read(0x6000), Some(0x77));
        roundtrip(&mut m);
    }

    #[test]
    fn sunsoft_fme7_banking_ram_irq_audio() {
        let rom = make_rom(69, 256, 128);
        let mut m = SunsoftFme7::new(rom);

        m.cpu_write(0x8000, 9);
        m.cpu_write(0xA000, 3);
        assert_eq!(m.common.prg_map[0], 3 * 0x2000);
        m.cpu_write(0x8000, 0x0A);
        m.cpu_write(0xA000, 4);
        assert_eq!(m.common.prg_map[1], 4 * 0x2000);
        m.cpu_write(0x8000, 0x0B);
        m.cpu_write(0xA000, 5);
        assert_eq!(m.common.prg_map[2], 5 * 0x2000);
        assert_eq!(
            m.common.prg_map[3],
            (256 * 1024 / 0x2000 - 1) * 0x2000
        );

        // Command 8: work RAM at $6000, page 1, enabled.
        m.cpu_write(0x8000, 8);
        m.cpu_write(0xA000, 0x40 | 0x80 | 1);
        m.cpu_write(0x6000, 0x33);
        assert_eq!(m.cpu_read(0x6000), Some(0x33));
        assert_eq!(m.common.prg_ram[0x2000], 0x33);

        // IRQ: enable + counter enabled, counter 0 -> wrap to $FFFF.
        m.cpu_write(0x8000, 0x0D);
        m.cpu_write(0xA000, 0x81);
        m.cpu_write(0x8000, 0x0E);
        m.cpu_write(0xA000, 0);
        m.cpu_write(0x8000, 0x0F);
        m.cpu_write(0xA000, 0);
        m.cpu_cycle();
        assert!(m.irq_pending(), "FME-7 IRQ did not fire");
        m.cpu_write(0x8000, 0x0D);
        m.cpu_write(0xA000, 0x80);
        assert!(!m.irq_pending());

        // 5B audio: max volume on channel 0, tone enabled.
        m.cpu_write(0xC000, 8);
        m.cpu_write(0xE000, 0x0F);
        for _ in 0..2 {
            m.cpu_cycle();
        }
        assert!(m.expansion_audio_sample() > 0.0);

        roundtrip(&mut m);
    }

    #[test]
    fn sunsoft89_93_184_banking() {
        let mut m89 = Sunsoft89::new(make_rom(89, 128, 128));
        m89.cpu_write(0x8000, 0xD3);
        assert_eq!(m89.common.prg_map[0], 5 * 0x4000);
        assert_eq!(m89.common.chr_map[0], (3 | 8) * 0x2000);
        assert_eq!(m89.common.mirroring, Mirroring::ScreenAOnly);
        roundtrip(&mut m89);

        let mut m93 = Sunsoft93::new(make_rom(93, 128, 128));
        m93.cpu_write(0x8000, 0x31);
        assert_eq!(m93.common.prg_map[0], 3 * 0x4000);
        assert!(m93.chr_enabled);
        m93.cpu_write(0x8000, 0x20);
        assert!(!m93.chr_enabled);
        assert_eq!(m93.ppu_read(0x0000), None);
        roundtrip(&mut m93);

        let mut m184 = Sunsoft184::new(make_rom(184, 128, 1024));
        m184.cpu_write(0x6000, 0x25);
        assert_eq!(m184.common.chr_map[0], 5 * 0x1000);
        assert_eq!(m184.common.chr_map[4], 0x82 * 0x1000);
        roundtrip(&mut m184);
    }

    #[test]
    fn jaleco_mappers() {
        // Mapper 18: nibble banking + IRQ.
        let mut m18 = JalecoSs88006::new(make_rom(18, 256, 128));
        m18.cpu_write(0x8000, 0x05);
        assert_eq!(m18.common.prg_map[0], 5 * 0x2000);
        m18.cpu_write(0x8001, 0x01);
        assert_eq!(m18.common.prg_map[0], 0x15 * 0x2000);
        m18.cpu_write(0xA000, 0x0C);
        m18.cpu_write(0xA001, 0x01);
        assert_eq!(m18.common.chr_map[0], 0x1C * 0x400);
        m18.cpu_write(0xE000, 1);
        m18.cpu_write(0xE001, 0);
        m18.cpu_write(0xE002, 0);
        m18.cpu_write(0xE003, 0);
        m18.cpu_write(0xF001, 0x01);
        m18.cpu_write(0xF000, 0);
        m18.cpu_cycle();
        assert!(m18.irq_pending());
        roundtrip(&mut m18);

        // Mapper 72: bus conflicts and latch behavior.
        let mut m72 = JalecoJf17_19::new(make_rom(72, 128, 128), false);
        // ROM byte at $8000 is 0 (bank 0), so a write of $82 is masked to 0.
        m72.cpu_write(0x8000, 0x80 | 2);
        assert_eq!(m72.common.prg_map[0], 0);
        // All-ones ROM byte in bank 7 allows the write.
        m72.common.prg_map[0] = 7 * 0x4000;
        m72.common.prg_rom[7 * 0x4000] = 0xFF;
        m72.cpu_write(0x8000, 0x80 | 2);
        assert_eq!(m72.common.prg_map[0], 2 * 0x4000);
        roundtrip(&mut m72);

        // Mapper 86/87/140 register writes.
        let mut m86 = JalecoJf13::new(make_rom(86, 128, 128));
        m86.cpu_write(0x6000, 0x64);
        assert_eq!(m86.common.prg_map[0], 2 * 0x8000);
        assert_eq!(m86.common.chr_map[0], 4 * 0x2000);
        roundtrip(&mut m86);

        let mut m87 = JalecoJfxx::new(make_rom(87, 128, 128), false);
        m87.cpu_write(0x6000, 0x03);
        assert_eq!(m87.common.chr_map[0], 3 * 0x2000);
        roundtrip(&mut m87);

        let mut m140 = JalecoJf11_14::new(make_rom(140, 128, 128));
        m140.cpu_write(0x6000, 0x33);
        assert_eq!(m140.common.prg_map[0], 3 * 0x8000);
        assert_eq!(m140.common.chr_map[0], 3 * 0x2000);
        roundtrip(&mut m140);
    }

    #[test]
    fn irem_mappers() {
        let mut m32 = IremG101::new(make_rom(32, 128, 128));
        m32.cpu_write(0x8000, 0x03);
        m32.cpu_write(0xA000, 0x04);
        assert_eq!(m32.common.prg_map[0], 3 * 0x2000);
        assert_eq!(m32.common.prg_map[1], 4 * 0x2000);
        m32.cpu_write(0x9000, 0x02);
        assert_eq!(m32.common.prg_map[0], (128 * 1024 / 0x2000 - 2) * 0x2000);
        assert_eq!(m32.common.prg_map[2], 3 * 0x2000);
        m32.cpu_write(0xB003, 0x09);
        assert_eq!(m32.common.chr_map[3], 9 * 0x400);
        roundtrip(&mut m32);

        let mut m65 = IremH3001::new(make_rom(65, 128, 128));
        m65.cpu_write(0x8000, 2);
        m65.cpu_write(0xA000, 3);
        m65.cpu_write(0xC000, 4);
        assert_eq!(m65.common.prg_map[0], 2 * 0x2000);
        assert_eq!(m65.common.prg_map[1], 3 * 0x2000);
        assert_eq!(m65.common.prg_map[2], 4 * 0x2000);
        m65.cpu_write(0xB001, 7);
        assert_eq!(m65.common.chr_map[1], 7 * 0x400);
        m65.cpu_write(0x9005, 0);
        m65.cpu_write(0x9006, 1);
        m65.cpu_write(0x9004, 0);
        m65.cpu_write(0x9003, 0x80);
        m65.cpu_cycle();
        assert!(m65.irq_pending());
        assert!(!m65.irq_enabled);
        roundtrip(&mut m65);
    }

    #[test]
    fn taito_mappers() {
        // Mapper 33.
        let mut m33 = TaitoTc0190::new(make_rom(33, 128, 128));
        m33.cpu_write(0x8000, 0x43);
        assert_eq!(m33.common.prg_map[0], 3 * 0x2000);
        assert_eq!(m33.common.mirroring, Mirroring::Horizontal);
        m33.cpu_write(0x8002, 5);
        assert_eq!(m33.common.chr_map[0], 10 * 0x400);
        assert_eq!(m33.common.chr_map[1], 11 * 0x400);
        m33.cpu_write(0xA003, 9);
        assert_eq!(m33.common.chr_map[7], 9 * 0x400);
        roundtrip(&mut m33);

        // Mapper 48: A12-clocked IRQ with a 6-cycle delay.
        let mut m48 = TaitoTc0690::new(make_rom(48, 128, 128));
        m48.cpu_write(0x8000, 3);
        m48.cpu_write(0x8002, 5);
        assert_eq!(m48.common.prg_map[0], 3 * 0x2000);
        assert_eq!(m48.common.chr_map[0], 10 * 0x400);
        m48.cpu_write(0xC000, 0); // reload value 0
        m48.cpu_write(0xC001, 0); // reload counter
        m48.cpu_write(0xC002, 0); // enable
        assert_eq!(
            m48.ppu_read(0x0000),
            Some(m48.common.chr[10 * 0x400])
        );
        for _ in 0..4 {
            m48.cpu_cycle();
        }
        assert_eq!(m48.ppu_read(0x1000), Some(m48.common.chr[0x1000]));
        assert!(!m48.irq_pending());
        for _ in 0..6 {
            m48.cpu_cycle();
        }
        assert!(m48.irq_pending(), "Taito 48 delayed IRQ");
        roundtrip(&mut m48);

        // Mapper 82: CHR modes and RAM permissions.
        let mut m82 = TaitoX1017::new(make_rom(82, 128, 128));
        m82.cpu_write(0x7EF0, 2);
        m82.cpu_write(0x7EF1, 4);
        m82.cpu_write(0x7EF2, 8);
        m82.cpu_write(0x7EF3, 9);
        m82.cpu_write(0x7EF4, 10);
        m82.cpu_write(0x7EF5, 11);
        assert_eq!(m82.common.chr_map[0], 2 * 0x400);
        assert_eq!(m82.common.chr_map[1], 3 * 0x400);
        assert_eq!(m82.common.chr_map[4], 8 * 0x400);
        m82.cpu_write(0x7EF6, 0x02); // CHR mode 1
        assert_eq!(m82.common.chr_map[0], 8 * 0x400);
        assert_eq!(m82.common.chr_map[4], 2 * 0x400);
        assert_eq!(m82.cpu_read(0x6000), None);
        m82.cpu_write(0x7EF7, 0xCA);
        m82.cpu_write(0x6000, 0x5A);
        assert_eq!(m82.cpu_read(0x6000), Some(0x5A));
        m82.cpu_write(0x7EF8, 0x69);
        m82.cpu_write(0x6800, 0xA5);
        assert_eq!(m82.cpu_read(0x6800), Some(0xA5));
        roundtrip(&mut m82);
    }

    #[test]
    fn bandai_mappers() {
        // Mapper 16 (FCG).
        let mut m16 = BandaiFcg::new(make_rom(16, 256, 128));
        assert_eq!(m16.common.prg_map[2], 0x0F * 0x4000);
        m16.cpu_write(0x8008, 2);
        assert_eq!(m16.common.prg_map[0], 2 * 0x4000);
        m16.cpu_write(0x8001, 3);
        assert_eq!(m16.common.chr_map[1], 3 * 0x400);
        m16.cpu_write(0x8009, 0);
        assert_eq!(m16.common.mirroring, Mirroring::Vertical);
        m16.cpu_write(0x800B, 0x10);
        m16.cpu_write(0x800C, 0);
        m16.cpu_write(0x800A, 1); // enabled, copy latch
        for _ in 0..17 {
            m16.cpu_cycle();
        }
        assert!(m16.irq_pending(), "Bandai FCG IRQ");
        m16.cpu_write(0x800A, 0);
        assert!(!m16.irq_pending());
        assert!(m16.cpu_read(0x6000).is_some());
        roundtrip(&mut m16);

        // Mapper 70.
        let mut m70 = Bandai74161::new(make_rom(70, 128, 128));
        m70.cpu_write(0x8000, 0x25);
        assert_eq!(m70.common.prg_map[0], 2 * 0x4000);
        assert_eq!(m70.common.chr_map[0], 5 * 0x2000);
        assert_eq!(m70.common.mirroring, Mirroring::Vertical);
        m70.cpu_write(0x8000, 0xA5);
        assert_eq!(m70.common.mirroring, Mirroring::ScreenBOnly);
        roundtrip(&mut m70);
    }

    #[test]
    fn namco108_88_banking() {
        let rom = make_rom(88, 128, 128);
        let mut m = Namco108_88::new(rom);
        assert_eq!(m.common.prg_map[0], 0);
        assert_eq!(m.common.prg_map[1], 1 * 0x2000);
        assert_eq!(m.common.prg_map[2], 14 * 0x2000);
        assert_eq!(m.common.prg_map[3], 15 * 0x2000);
        // Power-on CHR: regs 0/1 = 0/2, regs 2..5 |= $40.
        assert_eq!(m.common.chr_map[0], 0);
        assert_eq!(m.common.chr_map[2], 2 * 0x400);
        assert_eq!(m.common.chr_map[4], 0x44 * 0x400);

        m.cpu_write(0x8000, 6);
        m.cpu_write(0x8001, 3);
        assert_eq!(m.common.prg_map[0], 3 * 0x2000);
        m.cpu_write(0x8000, 0);
        m.cpu_write(0x8001, 0x0A);
        assert_eq!(m.common.chr_map[0], 0x0A * 0x400);
        assert_eq!(m.common.chr_map[1], 0x0B * 0x400);
        m.cpu_write(0x8000, 2);
        m.cpu_write(0x8001, 0x05);
        assert_eq!(m.common.chr_map[4], 0x45 * 0x400);
        // Odd addresses redirect to $8001; even to $8000.
        m.cpu_write(0xA000, 4);
        m.cpu_write(0xA001, 6);
        assert_eq!(m.common.chr_map[6], 0x46 * 0x400);
        roundtrip(&mut m);
    }

    #[test]
    fn bandai_eeprom_write() {
        fn bit(e: &mut Eeprom24C0X, value: u8) {
            e.write(0, value);
            e.write(1, value);
            e.write(0, value);
        }
        fn byte(e: &mut Eeprom24C0X, value: u8) {
            for i in (0..8).rev() {
                bit(e, (value >> i) & 1);
            }
        }
        fn byte_lsb(e: &mut Eeprom24C0X, value: u8) {
            for i in 0..8 {
                bit(e, (value >> i) & 1);
            }
        }

        let mut e = Eeprom24C0X::new(256, None);
        // START
        e.write(1, 1);
        e.write(1, 0);
        byte(&mut e, 0xA0); // chip address + write
        bit(&mut e, 0); // ACK
        byte(&mut e, 0x05); // word address
        bit(&mut e, 0); // ACK
        byte(&mut e, 0x42); // data
        bit(&mut e, 0); // ACK
        // STOP
        e.write(1, 0);
        e.write(1, 1);
        assert_eq!(e.rom[5], 0x42);

        // 24C01 uses LSB-first addressing/data.
        let mut e1 = Eeprom24C0X::new(128, None);
        e1.write(1, 1);
        e1.write(1, 0);
        byte_lsb(&mut e1, 0x55); // 7-bit address + write bit (LSB first)
        bit(&mut e1, 0); // ACK
        byte_lsb(&mut e1, 0x81); // data (LSB first)
        bit(&mut e1, 0); // ACK
        e1.write(1, 0);
        e1.write(1, 1);
        assert_eq!(e1.rom[0x55 & 0x7F], 0x81);

        let mut w = StateWriter::new();
        e.save_state(&mut w);
        let saved = w.into_inner();
        let mut r = StateReader::new(&saved);
        let mut e2 = Eeprom24C0X::new(256, None);
        e2.load_state(&mut r).unwrap();
        assert_eq!(e2.rom[5], 0x42);
    }

    #[test]
    fn sachen_and_waixing() {
        let mut m150 = Sachen74LS374N::new(make_rom(150, 128, 128), 150);
        m150.cpu_write(0x4100, 5);
        m150.cpu_write(0x4101, 1);
        assert_eq!(m150.common.prg_map[0], 1 * 0x8000);
        m150.cpu_write(0x4100, 6);
        m150.cpu_write(0x4101, 3);
        assert_eq!(m150.common.chr_map[0], 3 * 0x2000);
        m150.cpu_write(0x4100, 7);
        m150.cpu_write(0x4101, 2);
        assert_eq!(m150.nt_pages, [0, 0, 1, 1]);
        m150.ppu_write(0x2C00, 0x12);
        assert_eq!(m150.ppu_read(0x2C00), Some(0x12));
        roundtrip(&mut m150);

        let mut m164 = Waixing164::new(make_rom(164, 128, 128));
        m164.cpu_write(0x5000, 0x02);
        assert_eq!(m164.common.prg_map[0], 2 * 0x8000);
        m164.cpu_write(0x5100, 0x01);
        assert_eq!(m164.common.prg_map[0], (0x12 % 4) * 0x8000);
        roundtrip(&mut m164);
    }
}
