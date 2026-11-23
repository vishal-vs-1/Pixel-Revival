//! Cartridge mapper infrastructure.
//!
//! The design mirrors Mesen's `BaseMapper`: a common bank-mapped PRG/CHR
//! skeleton with per-mapper register handling. Mappers own their memories, so
//! the bus can hand out `&mut` access without awkward shared ownership.

use crate::state::{StateReader, StateResult, StateWriter};
use crate::types::{Mirroring, RomData, RomInfo};
use std::any::Any;

pub mod capcom;
pub mod capcom_extra;
pub mod discrete;
pub mod konami;
pub mod misc;
pub mod special;
pub mod vendors;

/// Mutable state shared by nearly every mapper.
pub struct MapperCommon {
    pub info: RomInfo,

    pub prg_rom: Vec<u8>,
    /// CHR ROM, or CHR RAM when `chr_is_ram` is true.
    pub chr: Vec<u8>,
    pub chr_is_ram: bool,
    /// PRG RAM mapped at $6000-$7FFF (battery-backed or work RAM).
    pub prg_ram: Vec<u8>,
    pub nametables: Box<[u8; 0x1000]>,

    /// Byte offsets of the four 8KB PRG windows at $8000/$A000/$C000/$E000.
    pub prg_map: [usize; 4],
    /// Byte offsets of the eight 1KB CHR windows at $0000-$1FFF.
    pub chr_map: [usize; 8],

    pub mirroring: Mirroring,
    pub irq_pending: bool,
}

impl MapperCommon {
    pub fn new(rom: RomData) -> Self {
        let info = rom.info;
        let mut prg_rom = rom.prg_rom;
        if prg_rom.is_empty() {
            prg_rom.resize(0x8000, 0xFF);
        }
        if prg_rom.len() < 0x2000 {
            prg_rom.resize(0x2000, 0xFF);
        }

        let (chr, chr_is_ram) = if rom.chr_rom.is_empty() {
            let size = info.chr_ram_size.max(0x2000);
            (vec![0u8; size], true)
        } else {
            (rom.chr_rom, false)
        };

        let prg_ram_size = info
            .prg_ram_size
            .max(info.prg_nvram_size)
            .max(0x2000);
        let mut prg_ram = vec![0u8; prg_ram_size];
        if let Some(battery) = rom.battery {
            let n = battery.len().min(prg_ram.len());
            prg_ram[..n].copy_from_slice(&battery[..n]);
        }

        let mut common = Self {
            mirroring: info.mirroring,
            info,
            prg_rom,
            chr,
            chr_is_ram,
            prg_ram,
            nametables: Box::new([0u8; 0x1000]),
            prg_map: [0; 4],
            chr_map: [0; 8],
            irq_pending: false,
        };
        common.init_default_maps();
        common
    }

    /// NROM/UNROM-style power-on mapping: last 32KB (or mirrored last 16KB) of
    /// PRG at $8000, first 8KB of CHR at $0000.
    pub fn init_default_maps(&mut self) {
        let prg_len = self.prg_rom.len();
        self.prg_map = if prg_len >= 0x8000 {
            let base = prg_len - 0x8000;
            [base, base + 0x2000, base + 0x4000, base + 0x6000]
        } else if prg_len >= 0x4000 {
            [0, 0x2000, 0, 0x2000]
        } else {
            [0, 0, 0, 0]
        };

        let chr_len = self.chr.len().max(1);
        let base = chr_len.saturating_sub(0x2000);
        for (i, slot) in self.chr_map.iter_mut().enumerate() {
            *slot = (base + i * 0x400) % chr_len;
        }
        self.irq_pending = false;
    }

    #[inline]
    pub fn map_prg_8k(&mut self, slot: usize, offset: usize) {
        self.prg_map[slot & 3] = offset % self.prg_rom.len().max(1);
    }

    #[inline]
    pub fn map_chr_1k(&mut self, slot: usize, offset: usize) {
        self.chr_map[slot & 7] = offset % self.chr.len().max(1);
    }

    #[inline]
    pub fn map_chr_1k_safe(&mut self, slot: usize, page: usize, page_size: usize) {
        self.map_chr_1k(slot, page * page_size);
    }

    #[inline]
    pub fn read_prg_rom(&self, addr: u16) -> u8 {
        let slot = ((addr as usize - 0x8000) >> 13) & 3;
        let offset = self.prg_map[slot] + (addr as usize & 0x1FFF);
        self.prg_rom[offset % self.prg_rom.len()]
    }

    #[inline]
    pub fn read_prg_ram(&self, addr: u16) -> u8 {
        let idx = (addr as usize - 0x6000) % self.prg_ram.len();
        self.prg_ram[idx]
    }

    #[inline]
    pub fn write_prg_ram(&mut self, addr: u16, value: u8) {
        let idx = (addr as usize - 0x6000) % self.prg_ram.len();
        self.prg_ram[idx] = value;
    }

    #[inline]
    pub fn read_chr(&self, addr: u16) -> u8 {
        let slot = (addr as usize & 0x1FFF) >> 10;
        let offset = self.chr_map[slot] + (addr as usize & 0x3FF);
        self.chr[offset % self.chr.len()]
    }

    #[inline]
    pub fn write_chr(&mut self, addr: u16, value: u8) {
        if self.chr_is_ram {
            let slot = (addr as usize & 0x1FFF) >> 10;
            let offset = self.chr_map[slot] + (addr as usize & 0x3FF);
            let len = self.chr.len();
            self.chr[offset % len] = value;
        }
    }

    /// Translate a PPU $2000-$3EFF address to an index in `nametables`.
    pub fn nametable_index(&self, addr: u16) -> usize {
        let addr = (addr & 0x3FFF) as usize;
        // $3000-$3EFF mirrors $2000-$2EFF: mask to the four 1KB slots first.
        let slot = ((addr - 0x2000) >> 10) & 3;
        let offset = addr & 0x3FF;
        let bank = match self.mirroring {
            Mirroring::Vertical => slot & 1,
            Mirroring::Horizontal => slot >> 1,
            Mirroring::ScreenAOnly => 0,
            Mirroring::ScreenBOnly => 1,
            Mirroring::FourScreens => slot & 3,
        };
        (bank * 0x400 + offset) & 0x0FFF
    }

    pub fn read_nametable(&self, addr: u16) -> u8 {
        self.nametables[self.nametable_index(addr)]
    }

    pub fn write_nametable(&mut self, addr: u16, value: u8) {
        let idx = self.nametable_index(addr);
        self.nametables[idx] = value;
    }

    pub fn save_state(&self, w: &mut StateWriter) {
        w.u32(self.info.mapper as u32);
        w.u8(self.mirroring as u8);
        w.bool(self.irq_pending);
        for bank in self.prg_map {
            w.u32(bank as u32);
        }
        for bank in self.chr_map {
            w.u32(bank as u32);
        }
        w.bytes(&self.prg_ram);
        w.fixed_bytes(&self.nametables[..]);
        if self.chr_is_ram {
            w.bytes(&self.chr);
        }
    }

    pub fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        let mapper = r.u32()?;
        if mapper != self.info.mapper as u32 {
            return Err(format!(
                "mapper mismatch: state {} != cart {}",
                mapper, self.info.mapper
            )
            .into());
        }
        self.mirroring = match r.u8()? {
            0 => Mirroring::Horizontal,
            1 => Mirroring::Vertical,
            2 => Mirroring::ScreenAOnly,
            3 => Mirroring::ScreenBOnly,
            4 => Mirroring::FourScreens,
            _ => Mirroring::Horizontal,
        };
        self.irq_pending = r.bool()?;
        for bank in &mut self.prg_map {
            *bank = r.u32()? as usize;
        }
        for bank in &mut self.chr_map {
            *bank = r.u32()? as usize;
        }
        let prg_ram = r.bytes()?;
        if prg_ram.len() != self.prg_ram.len() {
            return Err("PRG RAM size mismatch in save state".into());
        }
        self.prg_ram.copy_from_slice(&prg_ram);
        let nametables = r.fixed_bytes(0x1000)?;
        self.nametables.copy_from_slice(&nametables);
        if self.chr_is_ram {
            let chr = r.bytes()?;
            if chr.len() != self.chr.len() {
                return Err("CHR RAM size mismatch in save state".into());
            }
            self.chr.copy_from_slice(&chr);
        }
        Ok(())
    }
}

/// Common mapper interface. Everything except `common`/`common_mut` has a
/// useful default, so simple mappers are only a few dozen lines.
pub trait Mapper: Send {
    fn common(&self) -> &MapperCommon;
    fn common_mut(&mut self) -> &mut MapperCommon;

    fn mapper_id(&self) -> u16 {
        self.common().info.mapper
    }

    fn reset(&mut self) {
        self.common_mut().init_default_maps();
    }

    fn cpu_read(&mut self, addr: u16) -> Option<u8> {
        let c = self.common();
        match addr {
            0x6000..=0x7FFF => Some(c.read_prg_ram(addr)),
            0x8000..=0xFFFF => Some(c.read_prg_rom(addr)),
            _ => None,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        if (0x6000..=0x7FFF).contains(&addr) {
            self.common_mut().write_prg_ram(addr, value);
        }
    }

    fn ppu_read(&mut self, addr: u16) -> Option<u8> {
        let c = self.common();
        match addr & 0x3FFF {
            0x0000..=0x1FFF => Some(c.read_chr(addr)),
            0x2000..=0x3EFF => Some(c.read_nametable(addr)),
            _ => None,
        }
    }

    fn ppu_write(&mut self, addr: u16, value: u8) {
        match addr & 0x3FFF {
            0x0000..=0x1FFF => self.common_mut().write_chr(addr, value),
            0x2000..=0x3EFF => self.common_mut().write_nametable(addr, value),
            _ => {}
        }
    }

    /// Called when the PPU's VRAM address bus changes (Mesen's
    /// `BaseMapper::NotifyVRAMAddressChange`). Mappers that observe A12
    /// (MMC3, MMC5, ...) should override this.
    fn ppu_bus_address_changed(&mut self, _addr: u16) {}

    /// Called for writes to fully decoded PPU registers (`$2000-$2007`).
    /// `addr` is canonicalized to `$2000 | (addr & 7)`.
    fn ppu_register_write(&mut self, _addr: u16, _value: u8) {}

    /// Called once at the start of every visible AND vblank scanline.
    fn scanline(&mut self, _scanline: i16) {}

    /// Called once per CPU cycle.
    fn cpu_cycle(&mut self) {}

    fn irq_pending(&self) -> bool {
        false
    }

    /// Instantaneous expansion-audio output in the range -1.0..=1.0.
    fn expansion_audio_sample(&self) -> f32 {
        0.0
    }

    /// Battery-backed PRG RAM, if the cartridge has one.
    fn battery(&self) -> Option<&[u8]> {
        None
    }

    fn set_battery(&mut self, _data: &[u8]) {}

    /// Optional input-port merge for boards that share $4016/$4017 with the
    /// controller ports (VS System DIP switches, for example). The controller
    /// shift-register value is supplied so the mapper can merge its bits.
    fn read_input_port(&mut self, _addr: u16, _controller_bits: u8) -> Option<u8> {
        None
    }

    /// Optional handling for writes to $4016 (VS System PRG/CHR latch).
    fn write_input_port(&mut self, _value: u8) {}

    /// Downcasting for frontend device controls (FDS disk, NSF track select).
    fn as_any(&self) -> Option<&dyn Any> {
        None
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn Any> {
        None
    }

    /// FDS only: true when no BIOS image was supplied.
    fn bios_missing(&self) -> bool {
        false
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common().save_state(w);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common_mut().load_state(r)
    }
}

/// Build a mapper for the given ROM. Every mapper group gets a chance to claim
/// the ID; group functions return `None` for IDs they do not implement.
pub fn create_mapper(rom: RomData) -> Option<Box<dyn Mapper>> {
    let id = rom.info.mapper;

    if let Some(m) = special::create(id, rom.clone()) {
        return Some(m);
    }
    if let Some(m) = discrete::create(id, rom.clone()) {
        return Some(m);
    }
    if let Some(m) = capcom::create(id, rom.clone()) {
        return Some(m);
    }
    if let Some(m) = capcom_extra::create(id, rom.clone()) {
        return Some(m);
    }
    if let Some(m) = konami::create(id, rom.clone()) {
        return Some(m);
    }
    if let Some(m) = vendors::create(id, rom.clone()) {
        return Some(m);
    }
    if let Some(m) = misc::create(id, rom.clone()) {
        return Some(m);
    }

    None
}

/// IDs that the Rust port currently claims to support (used by the CLI and
/// tests to report unhandled ROMs clearly).
pub fn is_supported(id: u16) -> bool {
    special::supports(id)
        || discrete::supports(id)
        || capcom::supports(id)
        || capcom_extra::supports(id)
        || konami::supports(id)
        || vendors::supports(id)
        || misc::supports(id)
}
