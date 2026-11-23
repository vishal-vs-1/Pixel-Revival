//! Remaining licensed/unlicensed mappers and multicarts.
//!
//! OWNER: mapper-misc agent.
//!
//! Direct translation of the corresponding Mesen 0.9.9 mapper classes
//! (`Core/Mapper*.h`, `Core/Sachen*.h`, `Core/Bmc*.h`, `Core/Unl*.h`,
//! `Core/Kaiser*.h`, `Core/Txc*.h`, `Core/JyCompany.h`, ...).
//! Mesen is GPL-3.0-or-later; this file keeps the same license.

use super::{Mapper, MapperCommon};
use crate::state::{StateReader, StateResult, StateWriter};
use crate::types::{Mirroring, RomData};

const SZ_1K: usize = 0x400;
const SZ_2K: usize = 0x800;
const SZ_4K: usize = 0x1000;
const SZ_8K: usize = 0x2000;
const SZ_16K: usize = 0x4000;
const SZ_32K: usize = 0x8000;

// ---------------------------------------------------------------------------
// Generic bank helpers
// ---------------------------------------------------------------------------

fn page_index(page: i32, count: usize) -> usize {
    if count <= 1 {
        return 0;
    }
    page.rem_euclid(count as i32) as usize
}

/// `SelectPRGPage(slot, page)` equivalent. `slot` is in units of `size`
/// starting at $8000.
fn map_prg(c: &mut MapperCommon, slot: usize, page: i32, size: usize) {
    let size = size.max(SZ_8K);
    let count = (c.prg_rom.len() / size).max(1);
    let p = page_index(page, count);
    let off = p * size;
    let first = slot * size / SZ_8K;
    let n = size / SZ_8K;
    let len = c.prg_rom.len().max(1);
    for i in 0..n {
        let s = first + i;
        if s < 4 {
            c.prg_map[s] = (off + i * SZ_8K) % len;
        }
    }
}

fn map_prg_2x(c: &mut MapperCommon, slot: usize, page: i32, size: usize) {
    map_prg(c, slot * 2, page, size);
    map_prg(c, slot * 2 + 1, page + 1, size);
}

fn map_prg_4x(c: &mut MapperCommon, slot: usize, page: i32, size: usize) {
    map_prg_2x(c, slot * 2, page, size);
    map_prg_2x(c, slot * 2 + 1, page + 2, size);
}

/// `SelectCHRPage(slot, page)` equivalent. `slot` is the PPU address in units
/// of `size`.
fn map_chr(c: &mut MapperCommon, slot: usize, page: i32, size: usize) {
    let size = size.max(SZ_1K);
    let count = (c.chr.len() / size).max(1);
    let p = page_index(page, count);
    let off = p * size;
    let first = slot * size / SZ_1K;
    let n = size / SZ_1K;
    let len = c.chr.len().max(1);
    for i in 0..n {
        let s = first + i;
        if s < 8 {
            c.chr_map[s] = (off + i * SZ_1K) % len;
        }
    }
}

fn map_chr_2x(c: &mut MapperCommon, slot: usize, page: i32, size: usize) {
    map_chr(c, slot * 2, page, size);
    map_chr(c, slot * 2 + 1, page + 1, size);
}

fn map_chr_4x(c: &mut MapperCommon, slot: usize, page: i32, size: usize) {
    map_chr_2x(c, slot * 2, page, size);
    map_chr_2x(c, slot * 2 + 1, page + 2, size);
}

fn prg_rom_byte(c: &MapperCommon, addr: u16) -> u8 {
    c.prg_rom[c.prg_map[((addr as usize - 0x8000) >> 13) & 3] + (addr as usize & 0x1FFF)]
}

fn standard_nt_map(m: Mirroring) -> [u8; 4] {
    match m {
        Mirroring::Horizontal => [0, 0, 1, 1],
        Mirroring::Vertical => [0, 1, 0, 1],
        Mirroring::ScreenAOnly => [0, 0, 0, 0],
        Mirroring::ScreenBOnly => [1, 1, 1, 1],
        Mirroring::FourScreens => [0, 1, 2, 3],
    }
}

fn nt_index(map: &[u8; 4], addr: u16) -> usize {
    let slot = (((addr as usize) & 0x2FFF).saturating_sub(0x2000)) >> 10;
    ((map[slot & 3] as usize & 3) * 0x400 + (addr as usize & 0x3FF)) & 0x0FFF
}

fn read_nt_map(c: &MapperCommon, map: &[u8; 4], addr: u16) -> u8 {
    c.nametables[nt_index(map, addr)]
}

fn write_nt_map(c: &mut MapperCommon, map: &[u8; 4], addr: u16, value: u8) {
    let idx = nt_index(map, addr);
    c.nametables[idx] = value;
}

// ---------------------------------------------------------------------------
// A12 watcher (Core/A12Watcher.h)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default)]
struct A12Watcher {
    last_cycle: u32,
    cycles_down: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum A12Change {
    None,
    Rise,
    Fall,
}

impl A12Watcher {
    fn update(&mut self, addr: u16, frame_cycle: u32, min_delay: u32) -> A12Change {
        let mut result = A12Change::None;
        if self.cycles_down > 0 {
            if self.last_cycle > frame_cycle {
                self.cycles_down += (89342u32.wrapping_sub(self.last_cycle)).wrapping_add(frame_cycle);
            } else {
                self.cycles_down += frame_cycle - self.last_cycle;
            }
        }
        if addr & 0x1000 == 0 {
            if self.cycles_down == 0 {
                self.cycles_down = 1;
                result = A12Change::Fall;
            }
        } else {
            if self.cycles_down > min_delay {
                result = A12Change::Rise;
            }
            self.cycles_down = 0;
        }
        self.last_cycle = frame_cycle;
        result
    }
}

// ---------------------------------------------------------------------------
// TXC 22211 chip (Core/TxcChip.h)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default)]
struct TxcChip {
    accumulator: u8,
    inverter: u8,
    staging: u8,
    output: u8,
    increase: bool,
    y_flag: bool,
    invert: bool,
    is_jv001: bool,
}

impl TxcChip {
    fn new(is_jv001: bool) -> Self {
        Self {
            invert: is_jv001,
            is_jv001,
            ..Default::default()
        }
    }

    fn mask(&self) -> u8 {
        if self.is_jv001 {
            0x0F
        } else {
            0x07
        }
    }

    fn read(&mut self) -> u8 {
        let mask = self.mask();
        let value = (self.accumulator & mask)
            | ((self.inverter ^ (if self.invert { 0xFF } else { 0 })) & !mask);
        self.y_flag = !self.invert || (value & 0x10) != 0;
        value
    }

    fn write(&mut self, addr: u16, value: u8) {
        let mask = self.mask();
        if addr < 0x8000 {
            match addr & 0xE103 {
                0x4100 => {
                    if self.increase {
                        self.accumulator = self.accumulator.wrapping_add(1);
                    } else {
                        self.accumulator = ((self.accumulator & !mask) | (self.staging & mask))
                            ^ (if self.invert { 0xFF } else { 0 });
                    }
                }
                0x4101 => self.invert = value & 0x01 != 0,
                0x4102 => {
                    self.staging = value & mask;
                    self.inverter = value & !mask;
                }
                0x4103 => self.increase = value & 0x01 != 0,
                _ => {}
            }
        } else {
            self.output = if self.is_jv001 {
                (self.accumulator & 0x0F) | (self.inverter & 0xF0)
            } else {
                (self.accumulator & 0x0F) | ((self.inverter & 0x08) << 1)
            };
        }
        self.y_flag = !self.invert || (value & 0x10) != 0;
    }

    fn save(&self, w: &mut StateWriter) {
        w.u8(self.accumulator);
        w.u8(self.inverter);
        w.u8(self.staging);
        w.u8(self.output);
        w.bool(self.increase);
        w.bool(self.y_flag);
        w.bool(self.invert);
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.accumulator = r.u8()?;
        self.inverter = r.u8()?;
        self.staging = r.u8()?;
        self.output = r.u8()?;
        self.increase = r.bool()?;
        self.y_flag = r.bool()?;
        self.invert = r.bool()?;
        Ok(())
    }
}

fn convert_txc_b(v: u8) -> u8 {
    ((v & 0x01) << 5)
        | ((v & 0x02) << 3)
        | ((v & 0x04) << 1)
        | ((v & 0x08) >> 1)
        | ((v & 0x10) >> 3)
        | ((v & 0x20) >> 5)
}

// ---------------------------------------------------------------------------
// MMC3 subset (Core/MMC3.h) used by mappers 91, 95 and 206
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct Mmc3Core {
    reg8000: u8,
    reg_a000: u8,
    reg_a001: u8,
    current: u8,
    chr_mode: u8,
    prg_mode: u8,
    registers: [u8; 8],
    irq_reload_value: u8,
    irq_counter: u8,
    irq_reload: bool,
    irq_enabled: bool,
    a12: A12Watcher,
}

impl Default for Mmc3Core {
    fn default() -> Self {
        Self {
            reg8000: 0,
            reg_a000: 0,
            reg_a001: 0,
            current: 0,
            chr_mode: 0,
            prg_mode: 0,
            registers: [0, 2, 4, 5, 6, 7, 0, 1],
            irq_reload_value: 0,
            irq_counter: 0,
            irq_reload: false,
            irq_enabled: false,
            a12: A12Watcher::default(),
        }
    }
}

impl Mmc3Core {
    fn write(&mut self, c: &mut MapperCommon, addr: u16, value: u8) {
        match addr & 0xE001 {
            0x8000 => {
                self.current = value & 0x07;
                self.chr_mode = (value & 0x80) >> 7;
                self.prg_mode = (value & 0x40) >> 6;
            }
            0x8001 => {
                if self.current <= 1 {
                    self.registers[self.current as usize] = value & !0x01;
                } else {
                    self.registers[self.current as usize] = value;
                }
            }
            0xA000 => self.reg_a000 = value,
            0xA001 => self.reg_a001 = value,
            0xC000 => self.irq_reload_value = value,
            0xC001 => {
                self.irq_counter = 0;
                self.irq_reload = true;
            }
            0xE000 => {
                self.irq_enabled = false;
                c.irq_pending = false;
            }
            0xE001 => self.irq_enabled = true,
            _ => {}
        }
    }

    fn update(&self, c: &mut MapperCommon) {
        // PRG
        if self.prg_mode == 0 {
            map_prg(c, 0, self.registers[6] as i32, SZ_8K);
            map_prg(c, 1, self.registers[7] as i32, SZ_8K);
            map_prg(c, 2, -2, SZ_8K);
            map_prg(c, 3, -1, SZ_8K);
        } else {
            map_prg(c, 0, -2, SZ_8K);
            map_prg(c, 1, self.registers[7] as i32, SZ_8K);
            map_prg(c, 2, self.registers[6] as i32, SZ_8K);
            map_prg(c, 3, -1, SZ_8K);
        }

        // CHR
        if self.chr_mode == 0 {
            map_chr(c, 0, (self.registers[0] & 0xFE) as i32, SZ_1K);
            map_chr(c, 1, (self.registers[0] | 0x01) as i32, SZ_1K);
            map_chr(c, 2, (self.registers[1] & 0xFE) as i32, SZ_1K);
            map_chr(c, 3, (self.registers[1] | 0x01) as i32, SZ_1K);
            map_chr(c, 4, self.registers[2] as i32, SZ_1K);
            map_chr(c, 5, self.registers[3] as i32, SZ_1K);
            map_chr(c, 6, self.registers[4] as i32, SZ_1K);
            map_chr(c, 7, self.registers[5] as i32, SZ_1K);
        } else {
            map_chr(c, 0, self.registers[2] as i32, SZ_1K);
            map_chr(c, 1, self.registers[3] as i32, SZ_1K);
            map_chr(c, 2, self.registers[4] as i32, SZ_1K);
            map_chr(c, 3, self.registers[5] as i32, SZ_1K);
            map_chr(c, 4, (self.registers[0] & 0xFE) as i32, SZ_1K);
            map_chr(c, 5, (self.registers[0] | 0x01) as i32, SZ_1K);
            map_chr(c, 6, (self.registers[1] & 0xFE) as i32, SZ_1K);
            map_chr(c, 7, (self.registers[1] | 0x01) as i32, SZ_1K);
        }
    }

    fn notify_a12(&mut self, c: &mut MapperCommon, addr: u16, frame_cycle: u32) {
        if self.a12.update(addr, frame_cycle, 10) == A12Change::Rise {
            if self.irq_counter == 0 || self.irq_reload {
                self.irq_counter = self.irq_reload_value;
            } else {
                self.irq_counter -= 1;
            }
            if self.irq_counter == 0 && self.irq_enabled {
                c.irq_pending = true;
            }
            self.irq_reload = false;
        }
    }

    fn save(&self, w: &mut StateWriter) {
        w.u8(self.reg8000);
        w.u8(self.reg_a000);
        w.u8(self.reg_a001);
        w.u8(self.current);
        w.u8(self.chr_mode);
        w.u8(self.prg_mode);
        w.bytes(&self.registers);
        w.u8(self.irq_reload_value);
        w.u8(self.irq_counter);
        w.bool(self.irq_reload);
        w.bool(self.irq_enabled);
        w.u32(self.a12.last_cycle);
        w.u32(self.a12.cycles_down);
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.reg8000 = r.u8()?;
        self.reg_a000 = r.u8()?;
        self.reg_a001 = r.u8()?;
        self.current = r.u8()?;
        self.chr_mode = r.u8()?;
        self.prg_mode = r.u8()?;
        let regs = r.bytes()?;
        if regs.len() != 8 {
            return Err("MMC3 register size mismatch".into());
        }
        self.registers.copy_from_slice(&regs);
        self.irq_reload_value = r.u8()?;
        self.irq_counter = r.u8()?;
        self.irq_reload = r.bool()?;
        self.irq_enabled = r.bool()?;
        self.a12.last_cycle = r.u32()?;
        self.a12.cycles_down = r.u32()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// MiscBoard: one state container shared by all misc mappers
// ---------------------------------------------------------------------------

pub struct MiscBoard {
    common: MapperCommon,
    id: u16,
    has_chr_rom: bool,

    regs: [u8; 48],
    fk_regs: [u8; 12],
    prg_base_bits: u16,
    current: u8,
    cpu_write_flag: bool,

    irq_reload: u16,
    irq_counter: u16,
    irq_prescaler: i16,
    irq_delay: u8,
    irq_mode: u8,
    irq_dir: u8,
    irq_xor: u8,

    irq_enabled: bool,
    irq_alt: bool,
    irq_reload_flag: bool,
    need_irq: bool,
    irq_small: bool,

    open_bus: bool,
    enable_mirroring_control: bool,
    prg_ram_disabled: bool,
    auto_chr: bool,
    toggle: bool,
    contra_mode: bool,
    reset_toggle: bool,

    use_outer_bank: bool,
    select_chr_ram: bool,
    mmc3_chr_mode: bool,
    cnrom_chr_mode: bool,
    extended_mmc3: bool,
    ram_in_first_chr_bank: bool,
    allow_single_screen_mirroring: bool,
    fk23_regs_enabled: bool,
    wram_config_enabled: bool,
    wram_enabled: bool,
    wram_write_protected: bool,
    invert_prg_a14: bool,
    invert_chr_a12: bool,

    nt_map: [u8; 4],
    custom_nt: bool,

    prg_2k: [usize; 16],
    chr_pages: [u16; 8],
    chr_ram: Vec<u8>,
    chr_ram_mask: u8,

    last_ppu_addr: u16,
    clock: u32,
    scanline: i16,
    jy_irq_source: u8,

    a12: A12Watcher,
    mmc3: Mmc3Core,
    txc: TxcChip,
}

impl MiscBoard {
    pub fn new(id: u16, rom: RomData) -> Self {
        let has_chr_rom = !rom.chr_rom.is_empty();
        let mut common = MapperCommon::new(rom);
        let mut chr_ram: Vec<u8> = Vec::new();

        match id {
            103 => common.prg_ram.resize(0x4000, 0),
            176 => {
                common.prg_ram.resize(0x8000, 0);
                if !has_chr_rom {
                    common.chr = vec![0u8; 0x40000];
                    common.chr_is_ram = true;
                } else {
                    chr_ram = vec![0u8; 0x40000];
                }
            }
            178 => common.prg_ram.resize(0x8000, 0),
            168 => {
                if !has_chr_rom {
                    common.chr = vec![0u8; 0x10000];
                    common.chr_is_ram = true;
                }
            }
            253 => chr_ram = vec![0u8; 0x800],
            _ => {}
        }

        let mut b = Self {
            common,
            id,
            has_chr_rom,
            regs: [0; 48],
            fk_regs: [0; 12],
            prg_base_bits: 0,
            current: 0,
            cpu_write_flag: false,
            irq_reload: 0,
            irq_counter: 0,
            irq_prescaler: 0,
            irq_delay: 0,
            irq_mode: 0,
            irq_dir: 0,
            irq_xor: 0,
            irq_enabled: false,
            irq_alt: false,
            irq_reload_flag: false,
            need_irq: false,
            irq_small: false,
            open_bus: false,
            enable_mirroring_control: false,
            prg_ram_disabled: false,
            auto_chr: false,
            toggle: false,
            contra_mode: false,
            reset_toggle: false,
            use_outer_bank: false,
            select_chr_ram: false,
            mmc3_chr_mode: false,
            cnrom_chr_mode: false,
            extended_mmc3: false,
            ram_in_first_chr_bank: false,
            allow_single_screen_mirroring: false,
            fk23_regs_enabled: false,
            wram_config_enabled: false,
            wram_enabled: false,
            wram_write_protected: false,
            invert_prg_a14: false,
            invert_chr_a12: false,
            nt_map: [0, 1, 0, 1],
            custom_nt: false,
            prg_2k: [0; 16],
            chr_pages: [0; 8],
            chr_ram,
            chr_ram_mask: 0,
            last_ppu_addr: 0,
            clock: 0,
            scanline: 0,
            jy_irq_source: 0,
            a12: A12Watcher::default(),
            mmc3: Mmc3Core::default(),
            txc: TxcChip::new(false),
        };
        if id == 168 {
            // CHR RAM defaults to the first 8KB (BaseMapper::Initialize).
            for i in 0..8 {
                b.common.chr_map[i] = i * SZ_1K;
            }
        }
        b.init();
        b
    }

    fn set_nt(&mut self, map: [u8; 4]) {
        self.nt_map = map;
        self.custom_nt = true;
    }

    fn has_bus_conflicts(&self) -> bool {
        matches!(self.id, 78 | 96 | 144 | 148 | 185 | 234)
    }

    fn reset_irq(&mut self) {
        self.common.irq_pending = false;
    }

    fn frame_cycle(&self) -> u32 {
        self.clock.wrapping_mul(3)
    }

    fn init(&mut self) {
        // Reset the generic register file, then let each board apply its
        // power-on state. ROM-independent.
        self.regs = [0; 48];
        self.fk_regs = [0; 12];
        self.prg_base_bits = 0;
        self.current = 0;
        self.cpu_write_flag = false;
        self.irq_reload = 0;
        self.irq_counter = 0;
        self.irq_prescaler = 0;
        self.irq_delay = 0;
        self.irq_mode = 0;
        self.irq_dir = 0;
        self.irq_xor = 0;
        self.irq_enabled = false;
        self.irq_alt = false;
        self.irq_reload_flag = false;
        self.need_irq = false;
        self.irq_small = false;
        self.open_bus = false;
        self.enable_mirroring_control = false;
        self.prg_ram_disabled = false;
        self.auto_chr = false;
        self.toggle = true;
        self.contra_mode = false;
        self.reset_toggle = false;
        self.use_outer_bank = false;
        self.select_chr_ram = false;
        self.mmc3_chr_mode = true;
        self.cnrom_chr_mode = false;
        self.extended_mmc3 = false;
        self.ram_in_first_chr_bank = false;
        self.allow_single_screen_mirroring = false;
        self.fk23_regs_enabled = false;
        self.wram_config_enabled = false;
        self.wram_enabled = false;
        self.wram_write_protected = false;
        self.invert_prg_a14 = false;
        self.invert_chr_a12 = false;
        self.nt_map = standard_nt_map(self.common.info.mirroring);
        self.custom_nt = false;
        self.prg_2k = [0; 16];
        self.chr_pages = [0; 8];
        self.chr_ram_mask = 0;
        self.last_ppu_addr = 0;
        self.clock = 0;
        self.scanline = 0;
        self.jy_irq_source = 0;
        self.a12 = A12Watcher::default();
        self.mmc3 = Mmc3Core::default();
        self.txc = TxcChip::new(false);
        self.common.irq_pending = false;
        self.common.mirroring = self.common.info.mirroring;

        // Start from a known mapping; each board overrides what it needs.
        let prg_len = self.common.prg_rom.len().max(1);
        if prg_len >= SZ_32K {
            for i in 0..4 {
                self.common.prg_map[i] = prg_len - SZ_32K + i * SZ_8K;
            }
        } else {
            for i in 0..4 {
                self.common.prg_map[i] = (i * SZ_8K) % prg_len;
            }
        }
        for i in 0..8 {
            self.common.chr_map[i] = (i * SZ_1K) % self.common.chr.len().max(1);
        }

        match self.id {
            34 => {
                let nina = match self.common.info.submapper {
                    1 => true,
                    2 => false,
                    _ => self.has_chr_rom,
                };
                if nina {
                    map_prg(&mut self.common, 0, 0, SZ_32K);
                    map_chr(&mut self.common, 0, 0, SZ_4K);
                    map_chr(&mut self.common, 1, 0, SZ_4K);
                } else {
                    map_prg(&mut self.common, 0, 0, SZ_32K);
                    map_chr(&mut self.common, 0, 0, SZ_8K);
                }
            }
            35 => {
                map_prg(&mut self.common, 3, -1, SZ_8K);
                for i in 0..4 {
                    map_prg(&mut self.common, i, i as i32, SZ_8K);
                }
                for i in 0..8 {
                    map_chr(&mut self.common, i, i as i32, SZ_1K);
                }
            }
            39 => {
                map_prg(&mut self.common, 0, 0, SZ_32K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            40 => {
                for i in 0..4 {
                    map_prg(&mut self.common, i, [4, 5, 0, 7][i], SZ_8K);
                }
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            42 => {
                map_prg(&mut self.common, 0, -4, SZ_8K);
                map_prg(&mut self.common, 1, -3, SZ_8K);
                map_prg(&mut self.common, 2, -2, SZ_8K);
                map_prg(&mut self.common, 3, -1, SZ_8K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            43 => {
                self.regs[0] = 0; // _reg
                self.regs[1] = 0; // swap
                self.recalc();
                map_prg(&mut self.common, 0, 1, SZ_8K);
                map_prg(&mut self.common, 1, 0, SZ_8K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            46 => {
                self.regs[0] = 0;
                self.regs[1] = 0;
                self.recalc();
            }
            50 => {
                map_prg(&mut self.common, 0, 8, SZ_8K);
                map_prg(&mut self.common, 1, 9, SZ_8K);
                map_prg(&mut self.common, 2, 0, SZ_8K);
                map_prg(&mut self.common, 3, 11, SZ_8K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
                self.regs[0] = 0x0F; // $6000 PRG bank
            }
            51 => {
                self.regs[0] = 0; // bank
                self.regs[1] = 1; // mode
                self.recalc();
            }
            54 | 201 => {
                map_prg(&mut self.common, 0, 0, SZ_32K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            56 | 142 => {
                map_prg(&mut self.common, 3, -1, SZ_8K);
                self.regs[23] = 0xFF; // no $6000 ROM mapping (-1 selected)
            }
            64 | 158 => {
                for i in 0..16 {
                    self.regs[i] = 0;
                }
                map_prg(&mut self.common, 3, -1, SZ_8K);
                for i in 0..8 {
                    map_chr(&mut self.common, i, i as i32, SZ_1K);
                }
            }
            80 | 207 => {
                self.regs[0] = 0; // ram permission
                map_prg(&mut self.common, 3, -1, SZ_8K);
                for i in 0..8 {
                    map_chr(&mut self.common, i, i as i32, SZ_1K);
                }
            }
            90 | 209 | 211 => {
                for i in 0..4 {
                    self.regs[i] = 0;
                }
                for i in 0..8 {
                    self.regs[4 + i] = 0;
                    self.regs[12 + i] = 0;
                }
                self.regs[20] = 0; // CHR latch 0
                self.regs[21] = 4; // CHR latch 1
                self.current = 0;
                self.recalc();
            }
            91 => {
                map_prg(&mut self.common, 0, 0, SZ_8K);
                map_prg(&mut self.common, 1, 0, SZ_8K);
                map_prg(&mut self.common, 2, -2, SZ_8K);
                map_prg(&mut self.common, 3, -1, SZ_8K);
                for i in 0..4 {
                    map_chr(&mut self.common, i, i as i32, SZ_2K);
                }
            }
            95 | 206 => {
                self.mmc3 = Mmc3Core::default();
                self.mmc3.reg_a000 = if self.common.info.mirroring == Mirroring::Horizontal {
                    1
                } else {
                    0
                };
                self.mmc3.update(&mut self.common);
            }
            96 => {
                self.regs[0] = 0; // outer CHR bank
                self.regs[1] = 0; // inner CHR bank
                self.last_ppu_addr = 0;
                map_prg(&mut self.common, 0, 0, SZ_32K);
                map_chr(&mut self.common, 0, 0, SZ_4K);
                map_chr(&mut self.common, 1, 3, SZ_4K);
            }
            97 => {
                map_prg(&mut self.common, 0, -1, SZ_16K);
                map_prg(&mut self.common, 1, -1, SZ_16K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            101 => {
                map_prg(&mut self.common, 0, 0, SZ_32K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            103 => {
                self.regs[0] = 0;
                self.prg_ram_disabled = false;
                self.recalc();
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            107 => {
                map_prg(&mut self.common, 0, 0, SZ_32K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            108 => {
                self.regs[0] = 0xFF; // PRG reg at $6000
                self.regs[1] = 0; // CHR reg
                map_prg_4x(&mut self.common, 0, -4, SZ_8K);
                self.recalc();
            }
            112 => {
                for i in 0..8 {
                    self.regs[i] = 0;
                }
                self.current = 0;
                self.regs[8] = 0; // outer CHR bank
                self.common.mirroring = Mirroring::Vertical;
                self.recalc();
                map_prg(&mut self.common, 2, -2, SZ_8K);
                map_prg(&mut self.common, 3, -1, SZ_8K);
            }
            113 | 146 => {
                map_prg(&mut self.common, 0, 0, SZ_32K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            117 => {
                self.irq_reload = 0;
                self.irq_counter = 0;
                self.irq_enabled = false;
                self.irq_alt = false;
                map_prg_4x(&mut self.common, 0, -4, SZ_8K);
                for i in 0..8 {
                    map_chr(&mut self.common, i, i as i32, SZ_1K);
                }
            }
            120 => {
                self.regs[0] = 0;
                map_prg_4x(&mut self.common, 0, 8, SZ_8K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
                self.recalc();
            }
            132 | 172 | 173 => {
                self.txc = TxcChip::new(self.id == 172);
                map_prg(&mut self.common, 0, 0, SZ_32K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            133 => {
                map_prg(&mut self.common, 0, 0, SZ_32K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            136 | 147 => {
                self.txc = TxcChip::new(true);
                map_prg(&mut self.common, 0, 0, SZ_32K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
                self.recalc();
            }
            137 | 138 | 139 | 141 => {
                self.current = 0;
                for i in 0..8 {
                    self.regs[i] = 0;
                }
                map_prg(&mut self.common, 0, 0, SZ_32K);
                self.recalc();
            }
            143 => {
                map_prg(&mut self.common, 0, 0, SZ_32K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            145 | 149 => {
                map_prg(&mut self.common, 0, 0, SZ_32K);
                self.recalc();
            }
            148 => {
                map_prg(&mut self.common, 0, 0, SZ_32K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            152 => {
                self.enable_mirroring_control = true;
                map_prg(&mut self.common, 0, 0, SZ_16K);
                map_prg(&mut self.common, 1, -1, SZ_16K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
                self.common.mirroring = Mirroring::Vertical;
            }
            163 => {
                for i in 0..5 {
                    self.regs[i] = 0;
                }
                self.toggle = true;
                self.auto_chr = false;
                map_prg(&mut self.common, 0, 0, SZ_32K);
                map_chr(&mut self.common, 0, 0, SZ_4K);
                map_chr(&mut self.common, 1, 0, SZ_4K);
            }
            166 | 167 => {
                for i in 0..4 {
                    self.regs[i] = 0;
                }
                self.recalc();
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            168 => {
                self.irq_counter = 0;
                map_prg(&mut self.common, 1, -1, SZ_16K);
                map_chr(&mut self.common, 0, 0, SZ_4K);
                map_chr(&mut self.common, 1, 0, SZ_4K);
            }
            170 => {
                map_prg(&mut self.common, 0, 0, SZ_32K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            171 => {
                map_prg(&mut self.common, 0, 0, SZ_32K);
                for i in 0..2 {
                    map_chr(&mut self.common, i, i as i32, SZ_4K);
                }
            }
            174 => {
                self.write_reg(0x8000, 0);
            }
            175 => {
                self.regs[0] = 0;
                self.recalc();
            }
            176 => {
                self.init_fk23c();
            }
            178 => {
                for i in 0..4 {
                    self.regs[i] = 0;
                }
                self.recalc();
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            179 => {
                map_prg(&mut self.common, 0, 0, SZ_32K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            183 => {
                for i in 0..8 {
                    self.regs[i] = 0;
                }
                self.regs[8] = 0;
                self.recalc();
            }
            185 => {
                map_prg(&mut self.common, 0, 0, SZ_32K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            190 => {
                map_prg(&mut self.common, 0, 0, SZ_16K);
                map_prg(&mut self.common, 1, 0, SZ_16K);
                map_chr_4x(&mut self.common, 0, 0, SZ_2K);
                self.common.mirroring = Mirroring::Vertical;
            }
            193 => {
                map_prg(&mut self.common, 0, 0, SZ_8K);
                map_prg(&mut self.common, 1, -3, SZ_8K);
                map_prg(&mut self.common, 2, -2, SZ_8K);
                map_prg(&mut self.common, 3, -1, SZ_8K);
                for i in 0..8 {
                    map_chr(&mut self.common, i, (i / 2) as i32, SZ_2K);
                }
            }
            200 | 202 | 203 | 204 => {
                map_prg(&mut self.common, 0, 0, SZ_16K);
                map_prg(&mut self.common, 1, 0, SZ_16K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
                if self.id == 204 {
                    self.write_reg(0x8000, 0);
                }
            }
            212 => {
                self.regs[0] = 0;
                self.recalc();
            }
            213 => {
                self.write_reg(0x8000, 0);
            }
            214 => {
                self.write_reg(0x8000, 0);
            }
            216 => {
                self.write_reg(0x8000, 0);
            }
            218 => {
                map_prg(&mut self.common, 0, 0, SZ_32K);
                self.common.mirroring =
                    if self.common.info.mirroring == Mirroring::FourScreens {
                        Mirroring::ScreenAOnly
                    } else {
                        self.common.info.mirroring
                    };
            }
            220 => {
                for i in 0..8 {
                    self.regs[i] = 0;
                }
                map_chr(&mut self.common, 0, 0, SZ_8K);
                self.recalc();
            }
            221 => {
                self.regs[0] = 0; // prg reg
                self.regs[1] = 0; // mode low
                self.regs[2] = 0; // mode high
                map_chr(&mut self.common, 0, 0, SZ_8K);
                self.recalc();
            }
            222 => {
                self.irq_counter = 0;
                map_prg(&mut self.common, 0, 0, SZ_8K);
                map_prg(&mut self.common, 1, 0, SZ_8K);
                map_prg_2x(&mut self.common, 1, -2, SZ_8K);
                for i in 0..8 {
                    map_chr(&mut self.common, i, i as i32, SZ_1K);
                }
            }
            225 => {
                map_prg(&mut self.common, 0, 0, SZ_16K);
                map_prg(&mut self.common, 1, 1, SZ_16K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            226 | 233 => {
                self.regs[0] = 0;
                self.regs[1] = 0;
                map_prg(&mut self.common, 0, 0, SZ_16K);
                map_prg(&mut self.common, 1, 1, SZ_16K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            227 => {
                self.write_reg(0x8000, 0);
            }
            228 => {
                self.write_reg(0x8000, 0);
            }
            229 => {
                self.write_reg(0x8000, 0);
            }
            230 => {
                map_chr(&mut self.common, 0, 0, SZ_8K);
                self.contra_mode = true;
                map_prg(&mut self.common, 0, 0, SZ_16K);
                map_prg(&mut self.common, 1, 7, SZ_16K);
                self.common.mirroring = Mirroring::Vertical;
            }
            231 => {
                map_prg(&mut self.common, 0, 0, SZ_16K);
                map_prg(&mut self.common, 1, 0, SZ_16K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            234 => {
                self.regs[0] = 0;
                self.regs[1] = 0;
                self.recalc();
            }
            235 => {
                map_prg_2x(&mut self.common, 0, 0, SZ_16K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            236 => {
                self.use_outer_bank = !self.has_chr_rom;
                self.regs[0] = 0; // bank mode
                self.regs[1] = 0; // outer bank
                self.regs[2] = 0; // prg reg
                self.regs[3] = 0; // chr reg
                map_chr(&mut self.common, 0, 0, SZ_8K);
                self.recalc();
            }
            240 => {
                map_prg(&mut self.common, 0, 0, SZ_32K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            241 => {
                map_prg(&mut self.common, 0, 0, SZ_32K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            242 => {
                map_prg(&mut self.common, 0, 0, SZ_32K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
                self.common.mirroring = Mirroring::Vertical;
            }
            243 => {
                self.current = 0;
                for i in 0..8 {
                    self.regs[i] = 0;
                }
                self.set_nt([0, 0, 0, 1]);
                self.recalc();
            }
            244 => {
                map_prg(&mut self.common, 0, 0, SZ_32K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            246 => {
                map_prg(&mut self.common, 3, 0xFF, SZ_8K);
                for i in 0..4 {
                    map_prg(&mut self.common, i, i as i32, SZ_8K);
                }
                map_chr_4x(&mut self.common, 0, 0, SZ_2K);
            }
            252 => {
                for i in 0..8 {
                    self.regs[i] = 0;
                }
                self.irq_prescaler = 0;
                self.irq_reload = 0;
                self.irq_counter = 0;
                self.irq_enabled = false;
                self.irq_alt = false;
                map_prg(&mut self.common, 2, -2, SZ_8K);
                map_prg(&mut self.common, 3, -1, SZ_8K);
                self.recalc();
            }
            253 => {
                for i in 0..8 {
                    self.regs[i] = 0;
                    self.regs[8 + i] = 0;
                }
                self.regs[16] = 0; // reload
                self.regs[17] = 0; // counter
                self.irq_enabled = false;
                self.irq_prescaler = 0;
                map_prg(&mut self.common, 2, -2, SZ_8K);
                map_prg(&mut self.common, 3, -1, SZ_8K);
                self.recalc();
            }
            255 => {
                self.write_reg(0x8000, 0);
            }
            36 => {
                self.regs[0] = 0;
                self.txc = TxcChip::new(false);
                map_prg(&mut self.common, 0, 0, SZ_32K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            38 => {
                self.regs[0] = 0;
                map_prg(&mut self.common, 0, 0, SZ_32K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            41 => {
                self.regs[0] = 0;
                self.regs[1] = 0;
                self.regs[2] = 0;
                self.write_reg(0x6000, 0);
                self.write_reg(0x8000, 0);
            }
            28 => {
                self.regs[0] = 0;
                self.regs[1] = 0;
                self.regs[2] = 0;
                self.regs[3] = 0;
                self.current = 0;
                map_prg(&mut self.common, 1, -1, SZ_16K);
                self.recalc();
            }
            57 => {
                self.regs[0] = 0;
                self.regs[1] = 0;
                self.recalc();
            }
            58 => {
                self.regs[0] = 0;
                self.regs[1] = 0;
                map_prg(&mut self.common, 0, 0, SZ_16K);
                map_prg(&mut self.common, 1, 1, SZ_16K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
                self.common.mirroring = Mirroring::Vertical;
            }
            61 => {
                self.regs[0] = 0;
                self.regs[1] = 0;
                map_prg(&mut self.common, 0, 0, SZ_16K);
                map_prg(&mut self.common, 1, 1, SZ_16K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
                self.common.mirroring = Mirroring::Vertical;
            }
            62 => {
                self.regs[0] = 0;
                self.regs[1] = 0;
                self.regs[2] = 0;
                map_prg(&mut self.common, 0, 0, SZ_16K);
                map_prg(&mut self.common, 1, 1, SZ_16K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
                self.common.mirroring = Mirroring::Vertical;
            }
            63 => {
                self.regs[0] = 0;
                self.regs[1] = 0;
                self.open_bus = false;
                self.recalc();
            }
            76 | 154 => {
                self.mmc3 = Mmc3Core::default();
                self.mmc3.update(&mut self.common);
                if self.id == 76 {
                    for i in 0..4 {
                        map_chr(&mut self.common, i, self.mmc3.registers[2 + i] as i32, SZ_2K);
                    }
                }
            }
            78 => {
                self.regs[0] = 0;
                map_prg(&mut self.common, 0, 0, SZ_16K);
                map_prg(&mut self.common, 1, -1, SZ_16K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            83 => {
                self.regs = [0; 48];
                self.irq_counter = 0;
                self.irq_enabled = false;
                self.recalc();
            }
            94 => {
                map_prg(&mut self.common, 0, 0, SZ_16K);
                map_prg(&mut self.common, 1, -1, SZ_16K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            104 => {
                self.regs[0] = 0;
                map_prg(&mut self.common, 1, 0x0F, SZ_16K);
            }
            106 => {
                self.regs[0..4].fill(0xFF);
                map_prg(&mut self.common, 0, -1, SZ_8K);
                map_prg(&mut self.common, 1, -1, SZ_8K);
                map_prg(&mut self.common, 2, -1, SZ_8K);
                map_prg(&mut self.common, 3, -1, SZ_8K);
                self.irq_counter = 0;
                self.irq_enabled = false;
            }
            125 => {
                self.regs[0] = 0;
                map_chr(&mut self.common, 0, 0, SZ_8K);
                map_prg(&mut self.common, 0, -4, SZ_8K);
                map_prg(&mut self.common, 1, -3, SZ_8K);
                map_prg(&mut self.common, 3, -1, SZ_8K);
                self.prg_2k[0] = 0;
            }
            144 => {
                self.regs[0] = 0;
                map_prg(&mut self.common, 0, 0, SZ_32K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            156 => {
                self.regs[0..16].fill(0);
                map_prg(&mut self.common, 1, -1, SZ_16K);
                self.recalc();
                self.common.mirroring = Mirroring::ScreenAOnly;
            }
            162 => {
                self.regs[0] = 3;
                self.regs[1] = 0;
                self.regs[2] = 0;
                self.regs[3] = 7;
                map_chr(&mut self.common, 0, 0, SZ_8K);
                self.recalc();
            }
            177 => {
                map_prg(&mut self.common, 0, 0, SZ_32K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            _ => {}
        }
    }

    fn recalc(&mut self) {
        match self.id {
            43 => {
                let swap = self.regs[1] & 1 != 0;
                map_prg(&mut self.common, 2, 0, SZ_8K);
                map_prg(&mut self.common, 3, if swap { 8 } else { 9 }, SZ_8K);
                map_prg(&mut self.common, 2, self.regs[0] as i32, SZ_8K);
                let page6000 = if swap { 0 } else { 2 };
                self.prg_2k[0] = page_index(page6000, (self.common.prg_rom.len() / SZ_8K).max(1)) * SZ_8K;
            }
            46 => {
                let prg = ((self.regs[0] & 0x0F) as i32) << 1 | (self.regs[1] & 1) as i32;
                let chr = ((self.regs[0] & 0xF0) >> 1) | ((self.regs[1] & 0x70) >> 4);
                map_prg(&mut self.common, 0, prg, SZ_32K);
                map_chr(&mut self.common, 0, chr as i32, SZ_8K);
            }
            51 => {
                let bank = self.regs[0] as i32;
                let mode = self.regs[1];
                if mode & 1 != 0 {
                    map_prg_4x(&mut self.common, 0, bank << 2, SZ_8K);
                    self.prg_2k[0] = page_index(0x23 | (bank << 2), (self.common.prg_rom.len() / SZ_8K).max(1)) * SZ_8K;
                } else {
                    map_prg_2x(&mut self.common, 0, (bank << 2) | mode as i32, SZ_8K);
                    map_prg_2x(&mut self.common, 1, (bank << 2) | 0x0E, SZ_8K);
                    self.prg_2k[0] = page_index(0x2F | (bank << 2), (self.common.prg_rom.len() / SZ_8K).max(1)) * SZ_8K;
                }
                self.common.mirroring = if mode == 0x03 {
                    Mirroring::Horizontal
                } else {
                    Mirroring::Vertical
                };
            }
            56 | 142 => {
                map_prg(&mut self.common, 0, self.regs[0] as i32, SZ_8K);
                map_prg(&mut self.common, 1, self.regs[1] as i32, SZ_8K);
                map_prg(&mut self.common, 2, self.regs[2] as i32, SZ_8K);
                map_prg(&mut self.common, 3, -1, SZ_8K);
                if self.regs[23] != 0xFF {
                    self.prg_2k[0] = page_index(
                        self.regs[23] as i32,
                        (self.common.prg_rom.len() / SZ_8K).max(1),
                    ) * SZ_8K;
                }
            }
            90 | 209 | 211 => self.recalc_jy(),
            96 => {
                let outer = self.regs[0] as i32;
                let inner = self.regs[1] as i32;
                map_chr(&mut self.common, 0, outer | inner, SZ_4K);
                map_chr(&mut self.common, 1, outer | 0x03, SZ_4K);
            }
            103 => {
                map_prg_4x(&mut self.common, 0, -4, SZ_8K);
                self.prg_2k[0] = page_index(
                    self.regs[0] as i32,
                    (self.common.prg_rom.len() / SZ_8K).max(1),
                ) * SZ_8K;
            }
            112 => self.recalc_112(),
            120 => {
                let page = self.regs[0];
                self.prg_2k[0] =
                    page_index(page as i32, (self.common.prg_rom.len() / SZ_8K).max(1)) * SZ_8K;
            }
            133 => {
                let page = ((self.regs[0] >> 2) & 0x01) as i32;
                map_prg(&mut self.common, 0, page, SZ_32K);
                map_chr(&mut self.common, 0, (self.regs[0] & 0x03) as i32, SZ_8K);
            }
            132 => {
                let out = self.txc.output;
                map_prg(&mut self.common, 0, ((out >> 2) & 1) as i32, SZ_32K);
                map_chr(&mut self.common, 0, (out & 0x03) as i32, SZ_8K);
            }
            136 => {
                map_chr(&mut self.common, 0, self.txc.output as i32, SZ_8K);
            }
            147 => {
                let out = self.txc.output;
                map_prg(
                    &mut self.common,
                    0,
                    (((out & 0x20) >> 4) | (out & 0x01)) as i32,
                    SZ_32K,
                );
                map_chr(&mut self.common, 0, ((out & 0x1E) >> 1) as i32, SZ_8K);
            }
            172 => {
                map_chr(&mut self.common, 0, self.txc.output as i32, SZ_8K);
                self.common.mirroring = if self.txc.invert {
                    Mirroring::Vertical
                } else {
                    Mirroring::Horizontal
                };
            }
            173 => {
                map_prg(&mut self.common, 0, 0, SZ_32K);
                if self.has_chr_rom && self.common.chr.len() > SZ_8K {
                    let page = (self.txc.output & 0x01) as i32
                        | if self.txc.y_flag { 0x02 } else { 0 }
                        | (((self.txc.output & 0x02) as i32) << 1);
                    map_chr(&mut self.common, 0, page, SZ_8K);
                } else if self.txc.y_flag {
                    map_chr(&mut self.common, 0, 0, SZ_8K);
                }
            }
            137 | 138 | 139 | 141 => self.recalc_sachen8259(),
            145 => {
                map_chr(&mut self.common, 0, (self.regs[0] >> 7) as i32, SZ_8K);
            }
            146 => {
                map_prg(&mut self.common, 0, ((self.regs[0] >> 3) & 1) as i32, SZ_32K);
                map_chr(&mut self.common, 0, (self.regs[0] & 0x07) as i32, SZ_8K);
            }
            148 => {
                map_prg(&mut self.common, 0, ((self.regs[0] >> 3) & 1) as i32, SZ_32K);
                map_chr(&mut self.common, 0, (self.regs[0] & 0x07) as i32, SZ_8K);
            }
            149 => {
                map_chr(&mut self.common, 0, (self.regs[0] >> 7) as i32, SZ_8K);
            }
            152 => {
                let value = self.regs[0];
                map_prg(&mut self.common, 0, ((value >> 4) & 0x07) as i32, SZ_16K);
                map_chr(&mut self.common, 0, (value & 0x0F) as i32, SZ_8K);
                if self.enable_mirroring_control {
                    self.common.mirroring = if value & 0x80 != 0 {
                        Mirroring::ScreenBOnly
                    } else {
                        Mirroring::ScreenAOnly
                    };
                }
            }
            163 => {
                let page = (self.regs[0] & 0x0F) as i32 | (((self.regs[2] & 0x0F) as i32) << 4);
                self.auto_chr = self.regs[0] & 0x80 != 0;
                map_prg(&mut self.common, 0, page, SZ_32K);
            }
            166 | 167 => self.recalc_166(),
            170 => {
                map_prg(&mut self.common, 0, 0, SZ_32K);
                map_chr(&mut self.common, 0, 0, SZ_8K);
            }
            171 => {
                map_prg(&mut self.common, 0, 0, SZ_32K);
                map_chr(&mut self.common, 0, self.regs[0] as i32, SZ_4K);
                map_chr(&mut self.common, 1, self.regs[1] as i32, SZ_4K);
            }
            174 => self.recalc_174(),
            175 => {
                self.regs[0] = self.regs[0]; // quiet
                let page = self.regs[0] as i32;
                map_chr(&mut self.common, 0, page, SZ_8K);
                map_prg(&mut self.common, 0, page, SZ_16K);
                map_prg(&mut self.common, 1, page, SZ_16K);
            }
            176 => self.recalc_fk23c(),
            178 => self.recalc_178(),
            179 => {
                map_prg(&mut self.common, 0, self.regs[0] as i32 >> 1, SZ_32K);
            }
            183 => self.recalc_183(),
            185 => {
                map_prg(&mut self.common, 0, 0, SZ_32K);
                map_chr(&mut self.common, 0, self.regs[0] as i32, SZ_8K);
            }
            190 => {
                map_prg(&mut self.common, 0, self.regs[0] as i32, SZ_16K);
                map_prg(&mut self.common, 1, 0, SZ_16K);
            }
            200 => self.recalc_200(),
            202 => self.recalc_202(),
            203 => {
                map_prg(&mut self.common, 0, (self.regs[0] >> 2) as i32, SZ_16K);
                map_prg(&mut self.common, 1, (self.regs[0] >> 2) as i32, SZ_16K);
                map_chr(&mut self.common, 0, (self.regs[0] & 0x03) as i32, SZ_8K);
            }
            204 => self.recalc_204(),
            212 => self.recalc_212(),
            213 => {
                map_chr(&mut self.common, 0, (self.regs[0] >> 3) as i32 & 7, SZ_8K);
                map_prg(
                    &mut self.common,
                    0,
                    (self.regs[0] >> 1) as i32 & 0x03,
                    SZ_32K,
                );
            }
            214 => {
                map_chr(&mut self.common, 0, (self.regs[0] & 3) as i32, SZ_8K);
                map_prg(&mut self.common, 0, ((self.regs[0] >> 2) & 3) as i32, SZ_16K);
                map_prg(&mut self.common, 1, ((self.regs[0] >> 2) & 3) as i32, SZ_16K);
            }
            216 => {
                map_prg(&mut self.common, 0, (self.regs[0] & 1) as i32, SZ_32K);
                map_chr(&mut self.common, 0, ((self.regs[0] & 0x0E) >> 1) as i32, SZ_8K);
            }
            220 => self.recalc_220(),
            221 => self.recalc_221(),
            225 => self.recalc_225(),
            226 | 233 => self.recalc_226(),
            227 => self.recalc_227(),
            228 => self.recalc_228(),
            229 => self.recalc_229(),
            231 => self.recalc_231(),
            234 => self.recalc_234(),
            235 => self.recalc_235(),
            236 => self.recalc_236(),
            240 => {
                map_prg(&mut self.common, 0, ((self.regs[0] >> 4) & 0x0F) as i32, SZ_32K);
                map_chr(&mut self.common, 0, (self.regs[0] & 0x0F) as i32, SZ_8K);
            }
            241 => {
                map_prg(&mut self.common, 0, self.regs[0] as i32, SZ_32K);
            }
            242 => {
                self.common.mirroring = if self.regs[0] & 2 != 0 {
                    Mirroring::Horizontal
                } else {
                    Mirroring::Vertical
                };
                map_prg(&mut self.common, 0, ((self.regs[0] >> 3) & 0x0F) as i32, SZ_32K);
            }
            243 => self.recalc_243(),
            244 => self.recalc_244(),
            252 => self.recalc_252(),
            253 => self.recalc_253(),
            255 => self.recalc_255(),
            28 => self.recalc_28(),
            36 => self.recalc_36(),
            38 => self.recalc_38(),
            41 => self.recalc_41(),
            57 => self.recalc_57(),
            58 => self.recalc_58(),
            61 => self.recalc_61(),
            62 => self.recalc_62(),
            63 => self.recalc_63(),
            76 | 154 => self.recalc_namco_variant(),
            78 => self.recalc_78(),
            83 => self.recalc_83(),
            94 => self.recalc_94(),
            104 => self.recalc_104(),
            106 => self.recalc_106(),
            125 => {
                self.prg_2k[0] = page_index(
                    self.regs[0] as i32,
                    (self.common.prg_rom.len() / SZ_8K).max(1),
                ) * SZ_8K;
            }
            144 => self.recalc_144(),
            156 => self.recalc_156(),
            162 => self.recalc_162(),
            177 => self.recalc_177(),
            _ => {}
        }
    }

    // -- per-board recalculation helpers -----------------------------------

    fn recalc_112(&mut self) {
        let r = self.regs;
        let outer = self.regs[8];
        map_prg(&mut self.common, 0, r[0] as i32, SZ_8K);
        map_prg(&mut self.common, 1, r[1] as i32, SZ_8K);
        map_chr(&mut self.common, 0, r[2] as i32, SZ_1K);
        map_chr(&mut self.common, 1, r[2] as i32 + 1, SZ_1K);
        map_chr(&mut self.common, 2, r[3] as i32, SZ_1K);
        map_chr(&mut self.common, 3, r[3] as i32 + 1, SZ_1K);
        map_chr(
            &mut self.common,
            4,
            (r[4] as i32) | (((outer & 0x10) as i32) << 4),
            SZ_1K,
        );
        map_chr(
            &mut self.common,
            5,
            (r[5] as i32) | (((outer & 0x20) as i32) << 3),
            SZ_1K,
        );
        map_chr(
            &mut self.common,
            6,
            (r[6] as i32) | (((outer & 0x40) as i32) << 2),
            SZ_1K,
        );
        map_chr(
            &mut self.common,
            7,
            (r[7] as i32) | (((outer & 0x80) as i32) << 1),
            SZ_1K,
        );
    }

    fn recalc_sachen8259(&mut self) {
        let variant_d = self.id == 137;
        let simple = self.regs[7] & 1 != 0;
        match (self.regs[7] >> 1) & 3 {
            0 => {
                self.common.mirroring = if variant_d {
                    Mirroring::Horizontal
                } else {
                    Mirroring::Vertical
                }
            }
            1 => {
                self.common.mirroring = if variant_d {
                    Mirroring::Vertical
                } else {
                    Mirroring::Horizontal
                }
            }
            2 => self.set_nt([0, 1, 1, 1]),
            3 => self.common.mirroring = Mirroring::ScreenAOnly,
            _ => {}
        }
        if variant_d && simple {
            self.common.mirroring = Mirroring::Horizontal;
        }
        map_prg(&mut self.common, 0, self.regs[5] as i32, SZ_32K);

        if variant_d {
            map_chr(&mut self.common, 0, self.regs[0] as i32, SZ_1K);
            map_chr(
                &mut self.common,
                1,
                (((self.regs[4] & 0x01) << 4) | self.regs[if simple { 0 } else { 1 }]) as i32,
                SZ_1K,
            );
            map_chr(
                &mut self.common,
                2,
                (((self.regs[4] & 0x02) << 3) | self.regs[if simple { 0 } else { 2 }]) as i32,
                SZ_1K,
            );
            map_chr(
                &mut self.common,
                3,
                (((self.regs[4] & 0x04) << 2)
                    | ((self.regs[6] & 0x01) << 3)
                    | self.regs[if simple { 0 } else { 3 }]) as i32,
                SZ_1K,
            );
            map_chr_4x(&mut self.common, 1, -4, SZ_1K);
        } else if !self.common.chr_is_ram {
            let (shift, chr_or) = match self.id {
                141 => (1u8, [1u8, 0, 1]),
                138 => (0u8, [0u8, 0, 0]),
                139 => (2u8, [1u8, 2, 3]),
                _ => (0u8, [0u8, 0, 0]),
            };
            let chr_high = self.regs[4] << 3;
            map_chr(
                &mut self.common,
                0,
                ((chr_high | self.regs[0]) as i32) << shift,
                SZ_2K,
            );
            map_chr(
                &mut self.common,
                1,
                ((chr_high | self.regs[if simple { 0 } else { 1 }]) as i32) << shift
                    | chr_or[0] as i32,
                SZ_2K,
            );
            map_chr(
                &mut self.common,
                2,
                ((chr_high | self.regs[if simple { 0 } else { 2 }]) as i32) << shift
                    | chr_or[1] as i32,
                SZ_2K,
            );
            map_chr(
                &mut self.common,
                3,
                ((chr_high | self.regs[if simple { 0 } else { 3 }]) as i32) << shift
                    | chr_or[2] as i32,
                SZ_2K,
            );
        }
    }

    fn recalc_jy(&mut self) {
        let prg_mode = self.regs[24];
        let prg0 = self.regs[0];
        let prg1 = self.regs[1];
        let prg2 = self.regs[2];
        let prg3 = self.regs[3];
        let invert = prg_mode & 0x03 == 0x03;
        let ip = |v: u8| -> u8 {
            if invert {
                ((v & 0x01) << 6)
                    | ((v & 0x02) << 4)
                    | ((v & 0x04) << 2)
                    | ((v & 0x10) >> 2)
                    | ((v & 0x20) >> 4)
                    | ((v & 0x40) >> 6)
            } else {
                v
            }
        };
        let p = [ip(prg0), ip(prg1), ip(prg2), ip(prg3)];
        match prg_mode & 0x03 {
            0 => {
                let page = if prg_mode & 0x04 != 0 {
                    p[3] as i32
                } else {
                    0x3C
                };
                map_prg_4x(&mut self.common, 0, page, SZ_8K);
                if self.regs[27] & 0x04 != 0 {
                    self.prg_2k[0] = page_index(
                        p[3] as i32 * 4 + 3,
                        (self.common.prg_rom.len() / SZ_8K).max(1),
                    ) * SZ_8K;
                }
            }
            1 => {
                map_prg_2x(&mut self.common, 0, (p[1] as i32) << 1, SZ_8K);
                let page = if prg_mode & 0x04 != 0 {
                    p[3] as i32
                } else {
                    0x3E
                };
                map_prg_2x(&mut self.common, 1, page, SZ_8K);
                if self.regs[27] & 0x04 != 0 {
                    self.prg_2k[0] = page_index(
                        p[3] as i32 * 2 + 1,
                        (self.common.prg_rom.len() / SZ_8K).max(1),
                    ) * SZ_8K;
                }
            }
            _ => {
                map_prg(&mut self.common, 0, p[0] as i32, SZ_8K);
                map_prg(&mut self.common, 1, p[1] as i32, SZ_8K);
                map_prg(&mut self.common, 2, p[2] as i32, SZ_8K);
                let page = if prg_mode & 0x04 != 0 {
                    p[3] as i32
                } else {
                    0x3F
                };
                map_prg(&mut self.common, 3, page, SZ_8K);
                if self.regs[27] & 0x04 != 0 {
                    self.prg_2k[0] = page_index(
                        p[3] as i32,
                        (self.common.prg_rom.len() / SZ_8K).max(1),
                    ) * SZ_8K;
                }
            }
        }

        // CHR
        let chr_mode = self.regs[25] & 0x03;
        let chr_block_mode = self.regs[26] & 0x20 == 0;
        let chr_block = ((self.regs[26] & 0x18) >> 2) | (self.regs[26] & 0x01);
        let mirror_chr = self.regs[26] & 0x80 != 0;
        let chr_latch = [self.regs[20], self.regs[21]];
        let get_chr = |this: &Self, index_in: usize| -> i32 {
            let mut index = index_in;
            if chr_mode >= 2 && mirror_chr && (index == 2 || index == 3) {
                index -= 2;
            }
            if chr_block_mode {
                let (mask, shift) = match chr_mode {
                    0 => (0x1Fu8, 5u8),
                    1 => (0x3Fu8, 6u8),
                    2 => (0x7Fu8, 7u8),
                    _ => (0xFFu8, 8u8),
                };
                ((this.regs[4 + index] & mask) as i32) | ((chr_block as i32) << shift)
            } else {
                (this.regs[4 + index] as i32) | ((this.regs[12 + index] as i32) << 8)
            }
        };
        let cr = [
            get_chr(self, 0),
            get_chr(self, 1),
            get_chr(self, 2),
            get_chr(self, 3),
            get_chr(self, 4),
            get_chr(self, 5),
            get_chr(self, 6),
            get_chr(self, 7),
        ];
        match chr_mode {
            0 => map_chr(&mut self.common, 0, cr[0] << 3, SZ_1K),
            1 => {
                map_chr_4x(
                    &mut self.common,
                    0,
                    cr[chr_latch[0] as usize & 7] << 2,
                    SZ_1K,
                );
                map_chr_4x(
                    &mut self.common,
                    1,
                    cr[chr_latch[1] as usize & 7] << 2,
                    SZ_1K,
                );
            }
            2 => {
                map_chr_2x(&mut self.common, 0, cr[0] << 1, SZ_1K);
                map_chr_2x(&mut self.common, 1, cr[2] << 1, SZ_1K);
                map_chr_2x(&mut self.common, 2, cr[4] << 1, SZ_1K);
                map_chr_2x(&mut self.common, 3, cr[6] << 1, SZ_1K);
            }
            _ => {
                for i in 0..8 {
                    map_chr(&mut self.common, i, cr[i], SZ_1K);
                }
            }
        }

        // Mirroring
        let advanced = self.regs[27] & 0x01 != 0 || self.id == 211;
        if advanced && self.id != 90 {            let map = [
                self.regs[28] & 1,
                self.regs[29] & 1,
                self.regs[30] & 1,
                self.regs[31] & 1,
            ];
            self.set_nt(map);
        } else {
            match self.regs[22] & 3 {
                0 => self.common.mirroring = Mirroring::Vertical,
                1 => self.common.mirroring = Mirroring::Horizontal,
                2 => self.common.mirroring = Mirroring::ScreenAOnly,
                _ => self.common.mirroring = Mirroring::ScreenBOnly,
            }
        }
    }

    fn recalc_166(&mut self) {
        let outer = (((self.regs[0] ^ self.regs[1]) & 0x10) as u16) << 1;
        let inner = self.regs[2] ^ self.regs[3];
        let alt = self.id == 167;
        if self.regs[1] & 0x08 != 0 {
            let bank = ((outer | inner as u16) & 0xFE) as i32;
            map_prg(&mut self.common, 0, if alt { bank + 1 } else { bank }, SZ_16K);
            map_prg(&mut self.common, 1, if alt { bank } else { bank + 1 }, SZ_16K);
        } else if self.regs[1] & 0x04 != 0 {
            map_prg(&mut self.common, 0, 0x1F, SZ_16K);
            map_prg(&mut self.common, 1, (outer | inner as u16) as i32, SZ_16K);
        } else {
            map_prg(&mut self.common, 0, (outer | inner as u16) as i32, SZ_16K);
            map_prg(&mut self.common, 1, if alt { 0x20 } else { 0x07 }, SZ_16K);
        }
    }

    fn recalc_174(&mut self) {
        let a = self.regs[0] as u16;
        // Stored register is the last write's address.
        let prg_bank = ((a >> 4) & 0x07) as i32;
        if a & 0x80 != 0 {
            map_prg_2x(&mut self.common, 0, prg_bank & 0xFE, SZ_16K);
        } else {
            map_prg(&mut self.common, 0, prg_bank, SZ_16K);
            map_prg(&mut self.common, 1, prg_bank, SZ_16K);
        }
        map_chr(&mut self.common, 0, ((a >> 1) & 0x07) as i32, SZ_8K);
        self.common.mirroring = if a & 1 != 0 {
            Mirroring::Horizontal
        } else {
            Mirroring::Vertical
        };
    }

    fn recalc_178(&mut self) {
        let sbank = (self.regs[1] & 0x07) as u16;
        let bbank = self.regs[2] as u16;
        if self.regs[0] & 0x02 != 0 {
            map_prg(&mut self.common, 0, ((bbank << 3) | sbank) as i32, SZ_16K);
            if self.regs[0] & 0x04 != 0 {
                map_prg(
                    &mut self.common,
                    1,
                    ((bbank << 3) | 0x06 | ((self.regs[1] & 0x01) as u16)) as i32,
                    SZ_16K,
                );
            } else {
                map_prg(&mut self.common, 1, ((bbank << 3) | 0x07) as i32, SZ_16K);
            }
        } else {
            let bank = (bbank << 3) | sbank;
            if self.regs[0] & 0x04 != 0 {
                map_prg(&mut self.common, 0, bank as i32, SZ_16K);
                map_prg(&mut self.common, 1, bank as i32, SZ_16K);
            } else {
                map_prg_2x(&mut self.common, 0, bank as i32, SZ_16K);
            }
        }
        self.common.mirroring = if self.regs[0] & 0x01 != 0 {
            Mirroring::Horizontal
        } else {
            Mirroring::Vertical
        };
        self.prg_2k[0] = ((self.regs[3] & 0x03) as usize) * SZ_8K;
    }

    fn recalc_183(&mut self) {
        self.prg_2k[0] = page_index(
            self.regs[8] as i32,
            (self.common.prg_rom.len() / SZ_8K).max(1),
        ) * SZ_8K;
        map_prg(&mut self.common, 3, -1, SZ_8K);
        for i in 0..8 {
            map_chr(&mut self.common, i, self.regs[i] as i32, SZ_1K);
        }
    }

    fn recalc_200(&mut self) {
        let a = self.regs[0] as u16;
        let bank = (a & 0x07) as i32;
        map_prg(&mut self.common, 0, bank, SZ_16K);
        map_prg(&mut self.common, 1, bank, SZ_16K);
        map_chr(&mut self.common, 0, bank, SZ_8K);
        self.common.mirroring = if a & 0x08 != 0 {
            Mirroring::Vertical
        } else {
            Mirroring::Horizontal
        };
    }

    fn recalc_202(&mut self) {
        let a = self.regs[0] as u16;
        let mode1 = (a & 0x09) == 0x09;
        let bank = ((a >> 1) & 0x07) as i32;
        map_chr(&mut self.common, 0, bank, SZ_8K);
        map_prg(&mut self.common, 0, bank, SZ_16K);
        map_prg(&mut self.common, 1, if mode1 { bank + 1 } else { bank }, SZ_16K);
        self.common.mirroring = if a & 0x01 != 0 {
            Mirroring::Horizontal
        } else {
            Mirroring::Vertical
        };
    }

    fn recalc_204(&mut self) {
        let a = self.regs[0] as u16;
        let bit_mask = (a & 0x06) as i32;
        let page = bit_mask + if bit_mask == 0x06 { 0 } else { (a & 1) as i32 };
        map_prg(&mut self.common, 0, page, SZ_16K);
        map_prg(
            &mut self.common,
            1,
            bit_mask + if bit_mask == 0x06 { 1 } else { (a & 1) as i32 },
            SZ_16K,
        );
        map_chr(&mut self.common, 0, page, SZ_8K);
        self.common.mirroring = if a & 0x10 != 0 {
            Mirroring::Horizontal
        } else {
            Mirroring::Vertical
        };
    }

    fn recalc_212(&mut self) {
        let a = self.regs[0] as u16 | ((self.regs[1] as u16) << 8);
        if a & 0x4000 != 0 {
            map_prg_2x(&mut self.common, 0, (a & 0x06) as i32, SZ_16K);
        } else {
            map_prg(&mut self.common, 0, (a & 0x07) as i32, SZ_16K);
            map_prg(&mut self.common, 1, (a & 0x07) as i32, SZ_16K);
        }
        map_chr(&mut self.common, 0, (a & 0x07) as i32, SZ_8K);
        self.common.mirroring = if a & 0x08 != 0 {
            Mirroring::Horizontal
        } else {
            Mirroring::Vertical
        };
    }

    fn recalc_220(&mut self) {
        let count = (self.common.prg_rom.len() / SZ_2K).max(1);
        for i in 0..4 {
            self.prg_2k[i] = page_index(self.regs[i] as i32, count) * SZ_2K;
            self.prg_2k[4 + i] = page_index(0x34 + i as i32, count) * SZ_2K;
            self.prg_2k[8 + i] = page_index(0x38 + i as i32, count) * SZ_2K;
            self.prg_2k[12 + i] = page_index(0x3C + i as i32, count) * SZ_2K;
        }
    }

    fn recalc_221(&mut self) {
        let mode = self.regs[0] as u16 | ((self.regs[1] as u16) << 8);
        let prg_reg = self.regs[2] as i32;
        let outer = ((mode & 0xFC) >> 2) as i32;
        if mode & 0x02 != 0 {
            if mode & 0x0100 != 0 {
                map_prg(&mut self.common, 0, outer | prg_reg, SZ_16K);
                map_prg(&mut self.common, 1, outer | 0x07, SZ_16K);
            } else {
                map_prg_2x(&mut self.common, 0, outer | (prg_reg & 0x06), SZ_16K);
            }
        } else {
            map_prg(&mut self.common, 0, outer | prg_reg, SZ_16K);
            map_prg(&mut self.common, 1, outer | prg_reg, SZ_16K);
        }
        self.common.mirroring = if mode & 0x01 != 0 {
            Mirroring::Horizontal
        } else {
            Mirroring::Vertical
        };
    }

    fn recalc_225(&mut self) {
        let a = self.regs[0] as u16;
        let high = ((a >> 8) & 0x40) as u8;
        let prg = (((a >> 6) & 0x3F) as u8 | high) as i32;
        if a & 0x1000 != 0 {
            map_prg(&mut self.common, 0, prg, SZ_16K);
            map_prg(&mut self.common, 1, prg, SZ_16K);
        } else {
            map_prg(&mut self.common, 0, prg & 0xFE, SZ_16K);
            map_prg(&mut self.common, 1, (prg & 0xFE) + 1, SZ_16K);
        }
        map_chr(
            &mut self.common,
            0,
            ((a & 0x3F) as u8 | high) as i32,
            SZ_8K,
        );
        self.common.mirroring = if a & 0x2000 != 0 {
            Mirroring::Horizontal
        } else {
            Mirroring::Vertical
        };
    }

    fn recalc_226(&mut self) {
        let mut prg = (self.regs[0] & 0x1F) as i32
            | (((self.regs[0] & 0x80) >> 2) as i32)
            | (((self.regs[1] & 0x01) as i32) << 6);
        if self.id == 233 {
            prg |= (self.reset_toggle as i32) << 5;
        }
        if self.regs[0] & 0x20 != 0 {
            map_prg(&mut self.common, 0, prg, SZ_16K);
            map_prg(&mut self.common, 1, prg, SZ_16K);
        } else {
            map_prg(&mut self.common, 0, prg & 0xFE, SZ_16K);
            map_prg(&mut self.common, 1, (prg & 0xFE) + 1, SZ_16K);
        }
        self.common.mirroring = if self.regs[0] & 0x40 != 0 {
            Mirroring::Vertical
        } else {
            Mirroring::Horizontal
        };
    }

    fn recalc_227(&mut self) {
        let a = self.regs[0] as u16 | ((self.regs[1] as u16) << 8);
        let prg_bank = (((a >> 2) & 0x1F) | ((a & 0x100) >> 3)) as i32;
        let s_flag = a & 1 != 0;
        let l_flag = (a >> 9) & 1 != 0;
        let prg_mode = (a >> 7) & 1 != 0;
        if prg_mode {
            if s_flag {
                map_prg_2x(&mut self.common, 0, prg_bank & 0xFE, SZ_16K);
            } else {
                map_prg(&mut self.common, 0, prg_bank, SZ_16K);
                map_prg(&mut self.common, 1, prg_bank, SZ_16K);
            }
        } else if s_flag {
            if l_flag {
                map_prg(&mut self.common, 0, prg_bank & 0x3E, SZ_16K);
                map_prg(&mut self.common, 1, prg_bank | 0x07, SZ_16K);
            } else {
                map_prg(&mut self.common, 0, prg_bank & 0x3E, SZ_16K);
                map_prg(&mut self.common, 1, prg_bank & 0x38, SZ_16K);
            }
        } else if l_flag {
            map_prg(&mut self.common, 0, prg_bank, SZ_16K);
            map_prg(&mut self.common, 1, prg_bank | 0x07, SZ_16K);
        } else {
            map_prg(&mut self.common, 0, prg_bank, SZ_16K);
            map_prg(&mut self.common, 1, prg_bank & 0x38, SZ_16K);
        }
        self.common.mirroring = if a & 0x02 != 0 {
            Mirroring::Horizontal
        } else {
            Mirroring::Vertical
        };
    }

    fn recalc_228(&mut self) {
        let a = self.regs[0] as u16 | ((self.regs[1] as u16) << 8);
        let value = self.regs[2];
        let mut chip_select = ((a >> 11) & 0x03) as u8;
        if chip_select == 3 {
            chip_select = 2;
        }
        let prg = ((((a >> 6) & 0x1F) as u8) | (chip_select << 5)) as i32;
        if a & 0x20 != 0 {
            map_prg(&mut self.common, 0, prg, SZ_16K);
            map_prg(&mut self.common, 1, prg, SZ_16K);
        } else {
            map_prg(&mut self.common, 0, prg & 0xFE, SZ_16K);
            map_prg(&mut self.common, 1, (prg & 0xFE) + 1, SZ_16K);
        }
        map_chr(
            &mut self.common,
            0,
            (((a & 0x0F) << 2) | (value as u16 & 0x03)) as i32,
            SZ_8K,
        );
        self.common.mirroring = if a & 0x2000 != 0 {
            Mirroring::Horizontal
        } else {
            Mirroring::Vertical
        };
    }

    fn recalc_229(&mut self) {
        let a = self.regs[0] as u16;
        map_chr(&mut self.common, 0, (a & 0xFF) as i32, SZ_8K);
        if a & 0x1E == 0 {
            map_prg_2x(&mut self.common, 0, 0, SZ_16K);
        } else {
            map_prg(&mut self.common, 0, (a & 0x1F) as i32, SZ_16K);
            map_prg(&mut self.common, 1, (a & 0x1F) as i32, SZ_16K);
        }
        self.common.mirroring = if a & 0x20 != 0 {
            Mirroring::Horizontal
        } else {
            Mirroring::Vertical
        };
    }

    fn recalc_231(&mut self) {
        let a = self.regs[0] as u16;
        let prg_bank = ((((a >> 5) & 0x01) | (a & 0x1E)) as i32) & 0x1E;
        map_prg(&mut self.common, 0, prg_bank, SZ_16K);
        map_prg(
            &mut self.common,
            1,
            (((a >> 5) & 0x01) | (a & 0x1E)) as i32,
            SZ_16K,
        );
        self.common.mirroring = if a & 0x80 != 0 {
            Mirroring::Horizontal
        } else {
            Mirroring::Vertical
        };
    }

    fn recalc_234(&mut self) {
        if self.regs[0] & 0x40 != 0 {
            map_prg(
                &mut self.common,
                0,
                ((self.regs[0] & 0x0E) | (self.regs[1] & 0x01)) as i32,
                SZ_32K,
            );
            map_chr(
                &mut self.common,
                0,
                ((((self.regs[0] as u16) << 2) & 0x38) | (((self.regs[1] >> 4) as u16) & 0x07))
                    as i32,
                SZ_8K,
            );
        } else {
            map_prg(&mut self.common, 0, (self.regs[0] & 0x0F) as i32, SZ_32K);
            map_chr(
                &mut self.common,
                0,
                ((((self.regs[0] as u16) << 2) & 0x3C) | (((self.regs[1] >> 4) as u16) & 0x03))
                    as i32,
                SZ_8K,
            );
        }
        self.common.mirroring = if self.regs[0] & 0x80 != 0 {
            Mirroring::Horizontal
        } else {
            Mirroring::Vertical
        };
    }

    fn recalc_235(&mut self) {
        let a = self.regs[0] as u16 | ((self.regs[1] as u16) << 8);
        self.recalc_235_inner(a);
    }

    fn recalc_235_inner(&mut self, addr: u16) {
        self.common.mirroring = if addr & 0x0400 != 0 {
            Mirroring::ScreenAOnly
        } else if addr & 0x2000 != 0 {
            Mirroring::Horizontal
        } else {
            Mirroring::Vertical
        };
        let config: [[[u8; 2]; 4]; 4] = [
            [[0x00, 0], [0x00, 1], [0x00, 1], [0x00, 1]],
            [[0x00, 0], [0x00, 1], [0x20, 0], [0x00, 1]],
            [[0x00, 0], [0x00, 1], [0x20, 0], [0x40, 0]],
            [[0x00, 0], [0x20, 0], [0x40, 0], [0x60, 0]],
        ];
        let pages = (self.common.prg_rom.len() / SZ_16K).max(1);
        let mode = match pages {
            64 => 0,
            128 => 1,
            256 => 2,
            _ => 3,
        };
        let idx = ((addr >> 8) & 0x03) as usize;
        let mut bank = config[mode][idx][0] | (addr & 0x1F) as u8;
        self.open_bus = false;
        if config[mode][idx][1] != 0 {
            self.open_bus = true;
        } else if addr & 0x0800 != 0 {
            bank = (bank << 1) | (((addr >> 12) & 0x01) as u8);
            map_prg(&mut self.common, 0, bank as i32, SZ_16K);
            map_prg(&mut self.common, 1, bank as i32, SZ_16K);
        } else {
            map_prg_2x(&mut self.common, 0, (bank as i32) << 1, SZ_16K);
        }
    }

    fn recalc_236(&mut self) {
        let mode = self.regs[0] & 0x30;
        let outer = self.regs[1] as i32;
        let prg = self.regs[2] as i32;
        match mode {
            0x00 | 0x10 => {
                map_prg(&mut self.common, 0, outer | prg, SZ_16K);
                map_prg(&mut self.common, 1, outer | 7, SZ_16K);
            }
            0x20 => {
                map_prg_2x(&mut self.common, 0, (outer | prg) & 0xFE, SZ_16K);
            }
            0x30 => {
                map_prg(&mut self.common, 0, outer | prg, SZ_16K);
                map_prg(&mut self.common, 1, outer | prg, SZ_16K);
            }
            _ => {}
        }
        if !self.use_outer_bank {
            map_chr(&mut self.common, 0, self.regs[3] as i32, SZ_8K);
        }
    }

    fn recalc_243(&mut self) {
        let chr_page = ((self.regs[2] & 0x01) as u16)
            | (((self.regs[4] & 0x01) as u16) << 1)
            | (((self.regs[6] & 0x03) as u16) << 2);
        map_chr(&mut self.common, 0, chr_page as i32, SZ_8K);
        map_prg(&mut self.common, 0, (self.regs[5] & 0x03) as i32, SZ_32K);
        let map = match (self.regs[7] >> 1) & 0x03 {
            0 => [0, 0, 0, 1],
            1 => standard_nt_map(Mirroring::Horizontal),
            2 => standard_nt_map(Mirroring::Vertical),
            _ => standard_nt_map(Mirroring::ScreenAOnly),
        };
        self.set_nt(map);
    }

    fn recalc_244(&mut self) {
        const LUT_PRG: [[u8; 4]; 4] = [[0, 1, 2, 3], [3, 2, 1, 0], [0, 2, 1, 3], [3, 1, 2, 0]];
        const LUT_CHR: [[u8; 8]; 8] = [
            [0, 1, 2, 3, 4, 5, 6, 7],
            [0, 2, 1, 3, 4, 6, 5, 7],
            [0, 1, 4, 5, 2, 3, 6, 7],
            [0, 4, 1, 5, 2, 6, 3, 7],
            [0, 4, 2, 6, 1, 5, 3, 7],
            [0, 2, 4, 6, 1, 3, 5, 7],
            [7, 6, 5, 4, 3, 2, 1, 0],
            [7, 6, 5, 4, 3, 2, 1, 0],
        ];
        let value = self.regs[0];
        if value & 0x08 != 0 {
            map_chr(
                &mut self.common,
                0,
                LUT_CHR[((value >> 4) & 0x07) as usize][(value & 0x07) as usize] as i32,
                SZ_8K,
            );
        } else {
            map_prg(
                &mut self.common,
                0,
                LUT_PRG[((value >> 4) & 0x03) as usize][(value & 0x03) as usize] as i32,
                SZ_32K,
            );
        }
    }

    fn recalc_252(&mut self) {
        for i in 0..8 {
            map_chr(&mut self.common, i, self.regs[i] as i32, SZ_1K);
        }
    }

    fn recalc_253(&mut self) {
        self.chr_ram_mask = 0;
        for i in 0..8 {
            let page = (self.regs[i] as u16) | ((self.regs[8 + i] as u16) << 8);
            if (self.regs[i] == 4 || self.regs[i] == 5) && !self.force_chr_rom() {
                self.chr_ram_mask |= 1 << i;
                self.chr_pages[i] = page;
            } else {
                map_chr(&mut self.common, i, page as i32, SZ_1K);
            }
        }
    }

    fn force_chr_rom(&self) -> bool {
        self.regs[19] & 1 != 0
    }

    fn recalc_255(&mut self) {
        let a = self.regs[0] as u16 | ((self.regs[1] as u16) << 8);
        let prg_bit = if a & 0x1000 != 0 { 0 } else { 1 };
        let bank = (((a >> 8) & 0x40) | ((a >> 6) & 0x3F)) as i32;
        map_prg(&mut self.common, 0, bank & !prg_bit, SZ_16K);
        map_prg(&mut self.common, 1, bank | prg_bit, SZ_16K);
        map_chr(
            &mut self.common,
            0,
            (((a >> 8) & 0x40) | (a & 0x3F)) as i32,
            SZ_8K,
        );
        self.common.mirroring = if a & 0x2000 != 0 {
            Mirroring::Horizontal
        } else {
            Mirroring::Vertical
        };
    }

    fn recalc_28(&mut self) {
        let mirroring = if self.regs[2] & 0x02 == 0 {
            self.regs[20] & 1
        } else {
            self.regs[2] & 0x03
        };
        self.common.mirroring = match mirroring {
            0 => Mirroring::ScreenAOnly,
            1 => Mirroring::ScreenBOnly,
            2 => Mirroring::Vertical,
            _ => Mirroring::Horizontal,
        };
        let game_size = (self.regs[2] & 0x30) >> 4;
        let prg_size = (self.regs[2] & 0x08) >> 3;
        let slot_select = (self.regs[2] & 0x04) >> 2;
        let chr_select = self.regs[0] & 0x03;
        let mut prg_select = self.regs[1] & 0x0F;
        let outer = (self.regs[3] as u16) << 1;
        map_chr(&mut self.common, 0, chr_select as i32, SZ_8K);
        if prg_size != 0 {
            let bank = if slot_select != 0 { 0usize } else { 1usize };
            let page = match game_size {
                0 => (outer & 0x1FE) | (prg_select & 0x01) as u16,
                1 => (outer & 0x1FC) | (prg_select & 0x03) as u16,
                2 => (outer & 0x1F8) | (prg_select & 0x07) as u16,
                _ => (outer & 0x1F0) | (prg_select & 0x0F) as u16,
            };
            map_prg(&mut self.common, bank, page as i32, SZ_16K);
            map_prg(
                &mut self.common,
                if slot_select != 0 { 1 } else { 0 },
                ((outer & 0x1FE) | slot_select as u16) as i32,
                SZ_16K,
            );
        } else {
            prg_select <<= 1;
            let outer_and = [0x1FEu16, 0x1FC, 0x1F8, 0x1F0];
            let inner_and = [0x01u8, 0x03, 0x07, 0x0F];
            let gs = game_size as usize & 3;
            map_prg(
                &mut self.common,
                0,
                ((outer & outer_and[gs]) | (prg_select as u16 & inner_and[gs] as u16)) as i32,
                SZ_16K,
            );
            map_prg(
                &mut self.common,
                1,
                ((outer & outer_and[gs])
                    | (((prg_select | 0x01) as u16) & inner_and[gs] as u16)) as i32,
                SZ_16K,
            );
        }
    }

    fn recalc_36(&mut self) {
        map_prg(&mut self.common, 0, (self.txc.output & 0x03) as i32, SZ_32K);
        map_chr(&mut self.common, 0, self.regs[0] as i32, SZ_8K);
    }

    fn recalc_38(&mut self) {
        let value = self.regs[0];
        map_prg(&mut self.common, 0, (value & 0x03) as i32, SZ_32K);
        map_chr(&mut self.common, 0, ((value >> 2) & 0x03) as i32, SZ_8K);
    }

    fn recalc_41(&mut self) {
        map_prg(&mut self.common, 0, (self.regs[0] & 0x07) as i32, SZ_32K);
        map_chr(&mut self.common, 0, (self.regs[1] & 0x0F) as i32, SZ_8K);
        self.common.mirroring = if self.regs[2] & 0x20 != 0 {
            Mirroring::Horizontal
        } else {
            Mirroring::Vertical
        };
    }

    fn recalc_57(&mut self) {
        let r0 = self.regs[0];
        let r1 = self.regs[1];
        self.common.mirroring = if r1 & 0x08 != 0 {
            Mirroring::Horizontal
        } else {
            Mirroring::Vertical
        };
        map_chr(
            &mut self.common,
            0,
            (((r0 & 0x40) >> 3) | ((r0 | r1) & 0x07)) as i32,
            SZ_8K,
        );
        if r1 & 0x10 != 0 {
            let page = ((r1 >> 5) & 0x06) as i32;
            map_prg(&mut self.common, 0, page, SZ_16K);
            map_prg(&mut self.common, 1, page + 1, SZ_16K);
        } else {
            let page = ((r1 >> 5) & 0x07) as i32;
            map_prg(&mut self.common, 0, page, SZ_16K);
            map_prg(&mut self.common, 1, page, SZ_16K);
        }
    }

    fn recalc_58(&mut self) {
        let addr = self.regs[0] as u16 | ((self.regs[1] as u16) << 8);
        let prg_bank = (addr & 0x07) as i32;
        if addr & 0x40 != 0 {
            map_prg(&mut self.common, 0, prg_bank, SZ_16K);
            map_prg(&mut self.common, 1, prg_bank, SZ_16K);
        } else {
            map_prg_2x(&mut self.common, 0, prg_bank & 0x06, SZ_16K);
        }
        map_chr(&mut self.common, 0, ((addr >> 3) & 0x07) as i32, SZ_8K);
        self.common.mirroring = if addr & 0x80 != 0 {
            Mirroring::Horizontal
        } else {
            Mirroring::Vertical
        };
    }

    fn recalc_61(&mut self) {
        let addr = self.regs[0] as u16 | ((self.regs[1] as u16) << 8);
        let prg_page = (((addr & 0x0F) << 1) | ((addr >> 5) & 0x01)) as i32;
        if addr & 0x10 != 0 {
            map_prg(&mut self.common, 0, prg_page, SZ_16K);
            map_prg(&mut self.common, 1, prg_page, SZ_16K);
        } else {
            map_prg(&mut self.common, 0, prg_page & 0xFE, SZ_16K);
            map_prg(&mut self.common, 1, (prg_page & 0xFE) + 1, SZ_16K);
        }
        self.common.mirroring = if addr & 0x80 != 0 {
            Mirroring::Horizontal
        } else {
            Mirroring::Vertical
        };
    }

    fn recalc_62(&mut self) {
        let addr = self.regs[0] as u16 | ((self.regs[1] as u16) << 8);
        let value = self.regs[2];
        let prg_page = (((addr & 0x3F00) >> 8) | (addr & 0x40)) as i32;
        let chr_page = ((((addr & 0x1F) << 2) | (value & 0x03) as u16) & 0x7F) as i32;
        if addr & 0x20 != 0 {
            map_prg(&mut self.common, 0, prg_page, SZ_16K);
            map_prg(&mut self.common, 1, prg_page, SZ_16K);
        } else {
            map_prg(&mut self.common, 0, prg_page & 0xFE, SZ_16K);
            map_prg(&mut self.common, 1, (prg_page & 0xFE) + 1, SZ_16K);
        }
        map_chr(&mut self.common, 0, chr_page, SZ_8K);
        self.common.mirroring = if addr & 0x80 != 0 {
            Mirroring::Horizontal
        } else {
            Mirroring::Vertical
        };
    }

    fn recalc_63(&mut self) {
        let addr = self.regs[0] as u16 | ((self.regs[1] as u16) << 8);
        self.open_bus = (addr & 0x0300) == 0x0300;
        if !self.open_bus {
            let p0 = ((addr >> 1) & 0x1FC)
                | if addr & 0x02 != 0 {
                    0
                } else {
                    ((addr >> 1) & 0x02) | 0
                };
            let p1 = ((addr >> 1) & 0x1FC)
                | if addr & 0x02 != 0 {
                    1
                } else {
                    ((addr >> 1) & 0x02) | 1
                };
            let p3 = if addr & 0x800 != 0 {
                (addr & 0x07C) | if addr & 0x06 != 0 { 0x03 } else { 0x01 }
            } else {
                ((addr >> 1) & 0x01FC)
                    | if addr & 0x02 != 0 {
                        0x03
                    } else {
                        ((addr >> 1) & 0x02) | 0x01
                    }
            };
            map_prg(&mut self.common, 0, p0 as i32, SZ_8K);
            map_prg(&mut self.common, 1, p1 as i32, SZ_8K);
            map_prg(&mut self.common, 3, p3 as i32, SZ_8K);
        }
        let p2 = ((addr >> 1) & 0x1FC)
            | if addr & 0x02 != 0 {
                2
            } else {
                ((addr >> 1) & 0x02) | 0
            };
        map_prg(&mut self.common, 2, p2 as i32, SZ_8K);
        self.common.mirroring = if addr & 0x01 != 0 {
            Mirroring::Horizontal
        } else {
            Mirroring::Vertical
        };
    }

    fn recalc_namco_variant(&mut self) {
        if self.id == 76 {
            for i in 0..4 {
                map_chr(&mut self.common, i, self.mmc3.registers[2 + i] as i32, SZ_2K);
            }
        } else {
            self.mmc3.registers[0] &= 0x3F;
            self.mmc3.registers[1] &= 0x3F;
            for i in 2..6 {
                self.mmc3.registers[i] |= 0x40;
            }
            self.mmc3.update(&mut self.common);
        }
    }

    fn recalc_78(&mut self) {
        let value = self.regs[0];
        map_prg(&mut self.common, 0, (value & 0x07) as i32, SZ_16K);
        map_prg(&mut self.common, 1, -1, SZ_16K);
        map_chr(&mut self.common, 0, ((value >> 4) & 0x0F) as i32, SZ_8K);
        let bit = value & 0x08 != 0;
        self.common.mirroring = if self.common.info.submapper == 3 {
            if bit {
                Mirroring::Vertical
            } else {
                Mirroring::Horizontal
            }
        } else if bit {
            Mirroring::ScreenBOnly
        } else {
            Mirroring::ScreenAOnly
        };
    }

    fn recalc_83(&mut self) {
        let mode = self.regs[19];
        self.common.mirroring = match mode & 0x03 {
            0 => Mirroring::Vertical,
            1 => Mirroring::Horizontal,
            2 => Mirroring::ScreenAOnly,
            _ => Mirroring::ScreenBOnly,
        };
        let is_2k = self.regs[17] != 0;
        let is_not_2k = self.regs[18] != 0;
        let bank = self.regs[20] as i32;
        if is_2k && !is_not_2k {
            map_chr_2x(&mut self.common, 0, (self.regs[0] as i32) << 1, SZ_1K);
            map_chr_2x(&mut self.common, 1, (self.regs[1] as i32) << 1, SZ_1K);
            map_chr_2x(&mut self.common, 2, (self.regs[6] as i32) << 1, SZ_1K);
            map_chr_2x(&mut self.common, 3, (self.regs[7] as i32) << 1, SZ_1K);
        } else {
            for i in 0..8 {
                let page = self.regs[i] as i32 | ((bank & 0x30) << 4);
                map_chr(&mut self.common, i, page, SZ_1K);
            }
        }
        if mode & 0x40 != 0 {
            map_prg_2x(&mut self.common, 0, (bank & 0x3F) << 1, SZ_8K);
            map_prg_2x(&mut self.common, 1, ((bank & 0x30) | 0x0F) << 1, SZ_8K);
        } else {
            map_prg(&mut self.common, 0, self.regs[8] as i32, SZ_8K);
            map_prg(&mut self.common, 1, self.regs[9] as i32, SZ_8K);
            map_prg(&mut self.common, 2, self.regs[10] as i32, SZ_8K);
            map_prg(&mut self.common, 3, -1, SZ_8K);
        }
    }

    fn recalc_94(&mut self) {
        map_prg(&mut self.common, 0, (self.regs[0] >> 2 & 0x07) as i32, SZ_16K);
        map_prg(&mut self.common, 1, -1, SZ_16K);
    }

    fn recalc_104(&mut self) {
        map_prg(&mut self.common, 0, self.regs[0] as i32, SZ_16K);
        map_prg(
            &mut self.common,
            1,
            ((self.regs[0] & 0x70) | 0x0F) as i32,
            SZ_16K,
        );
    }

    fn recalc_106(&mut self) {
        map_chr(&mut self.common, 0, (self.regs[24] & 0xFE) as i32, SZ_1K);
        map_chr(&mut self.common, 1, (self.regs[24] | 0x01) as i32, SZ_1K);
        map_chr(&mut self.common, 2, (self.regs[25] & 0xFE) as i32, SZ_1K);
        map_chr(&mut self.common, 3, (self.regs[25] | 0x01) as i32, SZ_1K);
        for i in 0..4 {
            map_chr(&mut self.common, 4 + i, self.regs[26 + i] as i32, SZ_1K);
        }
        for i in 0..4 {
            map_prg(&mut self.common, i, self.regs[i] as i32, SZ_8K);
        }
    }

    fn recalc_144(&mut self) {
        let value = self.regs[0];
        map_prg(&mut self.common, 0, (value & 0x0F) as i32, SZ_32K);
        map_chr(&mut self.common, 0, ((value >> 4) & 0x0F) as i32, SZ_8K);
    }

    fn recalc_156(&mut self) {
        for i in 0..8 {
            let page = (self.regs[i] as i32) | ((self.regs[8 + i] as i32) << 8);
            map_chr(&mut self.common, i, page, SZ_1K);
        }
        map_prg(&mut self.common, 0, self.regs[16] as i32, SZ_16K);
        map_prg(&mut self.common, 1, -1, SZ_16K);
    }

    fn recalc_162(&mut self) {
        let r = self.regs;
        let page = match r[3] & 0x05 {
            0 => (r[0] & 0x0C) | (r[1] & 0x02) | ((r[2] & 0x0F) << 4),
            1 => (r[0] & 0x0C) | ((r[2] & 0x0F) << 4),
            4 => (r[0] & 0x0E) | ((r[1] >> 1) & 0x01) | ((r[2] & 0x0F) << 4),
            _ => (r[0] & 0x0F) | ((r[2] & 0x0F) << 4),
        };
        map_prg(&mut self.common, 0, page as i32, SZ_32K);
    }

    fn recalc_177(&mut self) {
        let value = self.regs[0];
        map_prg(&mut self.common, 0, value as i32, SZ_32K);
        self.common.mirroring = if value & 0x20 != 0 {
            Mirroring::Horizontal
        } else {
            Mirroring::Vertical
        };
    }

    fn init_fk23c(&mut self) {        self.regs = [0; 48];
        self.regs[0] = 0; // prg banking mode
        self.regs[1] = 0; // outer chr bank size
        self.select_chr_ram = false;
        self.mmc3_chr_mode = true;
        self.chr_ram_mask = 0;
        self.prg_base_bits = if self.common.prg_rom.len() == 0x100000
            && self.common.chr.len() == 0x100000
            && self.has_chr_rom
        {
            0x20
        } else {
            0
        };
        self.regs[2] = 0; // chr base bits
        self.extended_mmc3 = false;
        self.cnrom_chr_mode = false;
        self.regs[3] = 0; // wram bank select
        self.ram_in_first_chr_bank = false;
        self.allow_single_screen_mirroring = false;
        self.wram_config_enabled = false;
        self.fk23_regs_enabled = false;
        self.wram_enabled = false;
        self.wram_write_protected = false;
        self.current = 0;
        self.regs[4] = 0; // cnrom chr reg
        self.regs[5] = 0; // mirroring reg
        self.fk_regs = [0, 2, 4, 5, 6, 7, 0, 1, 0xFE, 0xFF, 0xFF, 0xFF];
        self.irq_reload = 0;
        self.irq_counter = 0;
        self.irq_reload_flag = false;
        self.irq_enabled = false;
        self.irq_delay = 0;
        self.a12 = A12Watcher::default();
        self.recalc();
    }

    fn recalc_fk23c(&mut self) {
        // Mirroring
        let mask = if self.allow_single_screen_mirroring {
            0x03
        } else {
            0x01
        };
        match self.regs[5] & mask {
            0 => self.common.mirroring = Mirroring::Vertical,
            1 => self.common.mirroring = Mirroring::Horizontal,
            2 => self.common.mirroring = Mirroring::ScreenAOnly,
            _ => self.common.mirroring = Mirroring::ScreenBOnly,
        }

        // PRG
        let mode = self.regs[0] & 0x07;
        match mode {
            0..=2 => {
                let swap = if self.invert_prg_a14 { 2 } else { 0 };
                let inner_mask: u16 = (0x3Fu16) >> mode;
                if self.extended_mmc3 {
                    let outer = self.prg_base_bits << 1;
                    let pages = [
                        self.fk_regs[6] as u16 | outer,
                        self.fk_regs[7] as u16 | outer,
                        self.fk_regs[8] as u16 | outer,
                        self.fk_regs[9] as u16 | outer,
                    ];
                    map_prg(&mut self.common, swap, pages[0] as i32, SZ_8K);
                    map_prg(&mut self.common, 1, pages[1] as i32, SZ_8K);
                    map_prg(&mut self.common, swap ^ 2, pages[2] as i32, SZ_8K);
                    map_prg(&mut self.common, 3, pages[3] as i32, SZ_8K);
                } else {
                    let outer = (self.prg_base_bits << 1) & !inner_mask;
                    map_prg(
                        &mut self.common,
                        swap,
                        ((self.fk_regs[6] as u16 & inner_mask) | outer) as i32,
                        SZ_8K,
                    );
                    map_prg(
                        &mut self.common,
                        1,
                        ((self.fk_regs[7] as u16 & inner_mask) | outer) as i32,
                        SZ_8K,
                    );
                    map_prg(
                        &mut self.common,
                        swap ^ 2,
                        ((0xFE & inner_mask) | outer) as i32,
                        SZ_8K,
                    );
                    map_prg(
                        &mut self.common,
                        3,
                        ((0xFF & inner_mask) | outer) as i32,
                        SZ_8K,
                    );
                }
            }
            3 => {
                for i in 0..2 {
                    map_prg_2x(&mut self.common, i, (self.prg_base_bits << 1) as i32, SZ_8K);
                }
            }
            4 => {
                map_prg_4x(
                    &mut self.common,
                    0,
                    ((self.prg_base_bits & 0xFFE) << 1) as i32,
                    SZ_8K,
                );
            }
            _ => {}
        }

        // CHR
        self.chr_ram_mask = 0;
        if !self.mmc3_chr_mode {
            let inner_mask: u8 = if self.cnrom_chr_mode {
                if self.regs[1] & 0x10 != 0 {
                    0x01
                } else {
                    0x03
                }
            } else {
                0
            };
            let base = self.regs[2];
            for i in 0..8 {
                let page = ((((self.regs[4] & inner_mask) | base) as i32) << 3) + i as i32;
                self.map_chr_bank(i, page);
            }
        } else {
            let swap = if self.invert_chr_a12 { 0x04 } else { 0 };
            if self.extended_mmc3 {
                let outer = (self.regs[2] as u16) << 3;
                let order = [0usize, 1, 2, 3, 4, 5, 6, 7];
                let pages = [
                    self.fk_regs[0] as u16 | outer,
                    self.fk_regs[10] as u16 | outer,
                    self.fk_regs[1] as u16 | outer,
                    self.fk_regs[11] as u16 | outer,
                    self.fk_regs[2] as u16 | outer,
                    self.fk_regs[3] as u16 | outer,
                    self.fk_regs[4] as u16 | outer,
                    self.fk_regs[5] as u16 | outer,
                ];
                for i in 0..8 {
                    self.map_chr_bank((order[i] ^ swap) & 7, pages[i] as i32);
                }
            } else {
                let inner_mask: u16 = if self.regs[1] & 0x10 != 0 { 0x7F } else { 0xFF };
                let outer = ((self.regs[2] as u16) << 3) & !inner_mask;
                let r = self.fk_regs;
                let pages: [u16; 8] = [
                    ((r[0] & 0xFE) as u16 & inner_mask) | outer,
                    ((r[0] | 0x01) as u16 & inner_mask) | outer,
                    ((r[1] & 0xFE) as u16 & inner_mask) | outer,
                    ((r[1] | 0x01) as u16 & inner_mask) | outer,
                    (r[2] as u16 & inner_mask) | outer,
                    (r[3] as u16 & inner_mask) | outer,
                    (r[4] as u16 & inner_mask) | outer,
                    (r[5] as u16 & inner_mask) | outer,
                ];
                for i in 0..8 {
                    self.map_chr_bank((i ^ swap) & 7, pages[i] as i32);
                }
            }
        }
    }

    fn map_chr_bank(&mut self, slot: usize, page: i32) {
        let use_ram = self.common.chr_is_ram
            || (self.select_chr_ram && !self.chr_ram.is_empty())
            || (self.wram_config_enabled && self.ram_in_first_chr_bank && page <= 7);
        if use_ram && !self.common.chr_is_ram && !self.chr_ram.is_empty() {
            self.chr_ram_mask |= 1 << slot;
            self.chr_pages[slot] = page.max(0) as u16;
        } else {
            map_chr(&mut self.common, slot, page, SZ_1K);
        }
    }

    fn read_chr_special(&mut self, addr: u16) -> Option<u8> {
        let slot = (addr as usize & 0x1FFF) >> 10;
        if self.chr_ram_mask & (1 << slot) != 0 {
            let page = self.chr_pages[slot] as usize;
            if self.id == 253 {
                let off = (page & 1) * SZ_1K + (addr as usize & 0x3FF);
                return Some(self.chr_ram[off % self.chr_ram.len().max(1)]);
            }
            let off = page * SZ_1K + (addr as usize & 0x3FF);
            return Some(self.chr_ram[off % self.chr_ram.len().max(1)]);
        }
        None
    }

    fn write_chr_special(&mut self, addr: u16, value: u8) -> bool {
        let slot = (addr as usize & 0x1FFF) >> 10;
        if self.chr_ram_mask & (1 << slot) != 0 {
            let page = self.chr_pages[slot] as usize;
            let off = if self.id == 253 {
                (page & 1) * SZ_1K + (addr as usize & 0x3FF)
            } else {
                page * SZ_1K + (addr as usize & 0x3FF)
            };
            let len = self.chr_ram.len();
            self.chr_ram[off % len] = value;
            return true;
        }
        false
    }

    // -- register handling -------------------------------------------------

    fn write_reg(&mut self, addr: u16, value: u8) {
        match self.id {
            34 => match addr {
                0x7FFD => map_prg(&mut self.common, 0, (value & 1) as i32, SZ_32K),
                0x7FFE => map_chr(&mut self.common, 0, (value & 0x0F) as i32, SZ_4K),
                0x7FFF => map_chr(&mut self.common, 1, (value & 0x0F) as i32, SZ_4K),
                _ => {
                    if addr >= 0x8000 {
                        let nina = match self.common.info.submapper {
                            1 => true,
                            2 => false,
                            _ => self.has_chr_rom,
                        };
                        if !nina {
                            map_prg(&mut self.common, 0, value as i32, SZ_32K);
                        }
                    }
                }
            },
            35 => match addr & 0xF007 {
                0x8000..=0x8003 => {
                    map_prg(&mut self.common, (addr & 3) as usize, value as i32, SZ_8K)
                }
                0x9000..=0x9007 => {
                    map_chr(&mut self.common, (addr & 7) as usize, value as i32, SZ_1K)
                }
                0xC002 => {
                    self.irq_enabled = false;
                    self.reset_irq();
                }
                0xC003 => self.irq_enabled = true,
                0xC005 => self.irq_counter = value as u16,
                0xD001 => {
                    self.common.mirroring = if value & 1 != 0 {
                        Mirroring::Horizontal
                    } else {
                        Mirroring::Vertical
                    }
                }
                _ => {}
            },
            39 => {
                map_prg(&mut self.common, 0, value as i32, SZ_32K);
            }
            40 => match addr & 0xE000 {
                0x8000 => {
                    self.irq_counter = 0;
                    self.reset_irq();
                }
                0xA000 => self.irq_counter = 4096,
                0xE000 => map_prg(&mut self.common, 2, value as i32, SZ_8K),
                _ => {}
            },
            42 => match addr & 0xE003 {
                0x8000 => {
                    if self.has_chr_rom {
                        map_chr(&mut self.common, 0, (value & 0x0F) as i32, SZ_8K);
                    }
                }
                0xE000 => {
                    self.regs[0] = value & 0x0F;
                    self.prg_2k[0] = page_index(
                        self.regs[0] as i32,
                        (self.common.prg_rom.len() / SZ_8K).max(1),
                    ) * SZ_8K;
                }
                0xE001 => {
                    self.common.mirroring = if value & 0x08 != 0 {
                        Mirroring::Horizontal
                    } else {
                        Mirroring::Vertical
                    }
                }
                0xE002 => {
                    self.irq_enabled = value == 0x02;
                    if !self.irq_enabled {
                        self.reset_irq();
                        self.irq_counter = 0;
                    }
                }
                _ => {}
            },
            43 => {
                match addr & 0xF1FF {
                    0x4022 => {
                        let lut = [4u8, 3, 5, 3, 6, 3, 7, 3];
                        self.regs[0] = lut[(value & 7) as usize];
                        self.recalc();
                    }
                    0x4120 => {
                        self.regs[1] = value & 1;
                        self.recalc();
                    }
                    0x8122 | 0x4122 => {
                        self.irq_enabled = value & 1 != 0;
                        self.reset_irq();
                        self.irq_counter = 0;
                    }
                    _ => {}
                }
            }
            46 => {
                if addr < 0x8000 {
                    self.regs[0] = value;
                } else {
                    self.regs[1] = value;
                }
                self.recalc();
            }
            50 => match addr & 0x4120 {
                0x4020 => {
                    let prg = ((value & 0x08) | ((value & 0x01) << 2) | ((value & 0x06) >> 1)) as i32;
                    map_prg(&mut self.common, 2, prg, SZ_8K);
                }
                0x4120 => {
                    if value & 0x01 != 0 {
                        self.irq_enabled = true;
                    } else {
                        self.reset_irq();
                        self.irq_counter = 0;
                        self.irq_enabled = false;
                    }
                }
                _ => {}
            },
            51 => {
                if addr <= 0x7FFF {
                    self.regs[1] = ((value >> 3) & 0x02) | ((value >> 1) & 0x01);
                } else if (0xC000..=0xDFFF).contains(&addr) {
                    self.regs[0] = value & 0x0F;
                    self.regs[1] = ((value >> 3) & 0x02) | (self.regs[1] & 0x01);
                } else {
                    self.regs[0] = value & 0x0F;
                }
                self.recalc();
            }
            54 | 201 => {
                map_prg(&mut self.common, 0, (addr & 0x03) as i32, SZ_32K);
                map_chr(&mut self.common, 0, (addr & 0x07) as i32, SZ_8K);
            }
            56 | 142 => self.write_kaiser202(addr, value),
            64 | 158 => self.write_rambo1(addr, value),
            80 | 207 => self.write_taito_x1005(addr, value),
            90 | 209 | 211 => self.write_jy(addr, value),
            91 => self.write_mmc3_lite(addr, value),
            95 | 206 => self.write_namco108(addr, value),
            96 => {
                let value = if self.has_bus_conflicts() {
                    value & prg_rom_byte(&self.common, addr)
                } else {
                    value
                };
                map_prg(&mut self.common, 0, (value & 0x03) as i32, SZ_32K);
                self.regs[0] = value & 0x04;
                self.recalc();
            }
            97 => {
                map_prg(&mut self.common, 1, (value & 0x0F) as i32, SZ_16K);
                self.common.mirroring = match value >> 6 {
                    0 => Mirroring::ScreenAOnly,
                    1 => Mirroring::Horizontal,
                    2 => Mirroring::Vertical,
                    _ => Mirroring::ScreenBOnly,
                };
            }
            101 => map_chr(&mut self.common, 0, value as i32, SZ_8K),
            103 => match addr & 0xF000 {
                0x6000 | 0x7000 => self.common.prg_ram[(addr - 0x6000) as usize] = value,
                0x8000 => {
                    self.regs[0] = value & 0x0F;
                    self.recalc();
                }
                0xB000 | 0xC000 | 0xD000 => {
                    if (0xB800..0xD800).contains(&addr) {
                        self.common.prg_ram[0x2000 + (addr - 0xB800) as usize] = value;
                    }
                }
                0xE000 => {
                    self.common.mirroring = if value & 0x08 != 0 {
                        Mirroring::Horizontal
                    } else {
                        Mirroring::Vertical
                    }
                }
                0xF000 => {
                    self.prg_ram_disabled = value & 0x10 != 0;
                    self.recalc();
                }
                _ => {}
            },
            107 => {
                map_prg(&mut self.common, 0, (value & 0xFE) as i32, SZ_32K);
                map_chr(&mut self.common, 0, value as i32, SZ_8K);
            }
            108 => {
                if (addr & 0x9000) == 0x8000 || addr >= 0xF000 {
                    self.regs[0] = value;
                    self.regs[1] = value;
                } else {
                    self.regs[1] = value & 1;
                }
                self.recalc();
            }
            112 => {
                match addr & 0xE001 {
                    0x8000 => self.current = value & 0x07,
                    0xA000 => self.regs[(self.current & 7) as usize] = value,
                    0xC000 => self.regs[8] = value,
                    0xE000 => {
                        self.common.mirroring = if value & 1 != 0 {
                            Mirroring::Horizontal
                        } else {
                            Mirroring::Vertical
                        }
                    }
                    _ => {}
                }
                self.recalc();
            }
            113 | 146 => {
                if (addr & 0xE100) == 0x4100 {
                    self.regs[0] = value;
                    self.recalc();
                }
            }
            117 => self.write_117(addr, value),
            120 => {
                if addr == 0x41FF {
                    self.regs[0] = value;
                    self.recalc();
                }
            }
            132 => {
                self.txc.write(addr, value & 0x0F);
                self.recalc();
            }
            133 => {
                if (addr & 0x6100) == 0x4100 {
                    self.regs[0] = value;
                    self.recalc();
                }
            }
            136 => {
                self.txc.write(addr, value & 0x3F);
                self.recalc();
            }
            137 | 138 | 139 | 141 => {
                match addr & 0xC101 {
                    0x4100 => self.current = value & 0x07,
                    0x4101 => {
                        self.regs[(self.current & 7) as usize] = value & 0x07;
                        self.recalc();
                    }
                    _ => {}
                }
            }
            143 => {
                // Reads return ~addr; writes are ignored.
            }
            145 => {
                if (addr & 0x4100) == 0x4100 {
                    self.regs[0] = value;
                    self.recalc();
                }
            }
            147 => {
                self.txc.write(
                    addr,
                    ((value & 0xFC) >> 2) | ((value & 0x03) << 6),
                );
                if addr >= 0x8000 {
                    self.recalc();
                }
            }
            148 => {
                let value = value & prg_rom_byte(&self.common, addr);
                self.regs[0] = value;
                self.recalc();
            }
            149 => {
                self.regs[0] = value;
                self.recalc();
            }
            152 => {
                self.regs[0] = value;
                self.recalc();
            }
            163 => self.write_163(addr, value),
            166 | 167 => {
                match addr & 0xE000 {
                    0x8000 => self.regs[0] = value & 0x10,
                    0xA000 => self.regs[1] = value & 0x1C,
                    0xC000 => self.regs[2] = value & 0x1F,
                    0xE000 => self.regs[3] = value & 0x1F,
                    _ => {}
                }
                self.recalc();
            }
            168 => match addr & 0xC000 {
                0x8000 => {
                    map_prg(&mut self.common, 0, ((value >> 6) & 3) as i32, SZ_16K);
                    map_chr(&mut self.common, 1, (value & 0x0F) as i32, SZ_4K);
                }
                0xC000 => {
                    self.irq_counter = 1024;
                    self.reset_irq();
                }
                _ => {}
            },
            170 => {
                if addr == 0x6502 {
                    self.regs[0] = (value << 1) & 0x80;
                }
            }
            171 => match addr & 0xF080 {
                0xF000 => {
                    self.regs[0] = value;
                    self.recalc();
                }
                0xF080 => {
                    self.regs[1] = value;
                    self.recalc();
                }
                _ => {}
            },
            172 => {
                let v = convert_txc_b(value);
                self.txc.write(addr, v);
                if addr >= 0x8000 {
                    self.recalc();
                }
            }
            173 => {
                self.txc.write(addr, value & 0x0F);
                self.recalc();
            }
            174 => {
                self.regs[0] = (addr >> 4) as u8;
                // Keep full address bits used by recalc_174 (bit 7, bits 4-6, 1-3, 0).
                let a = addr as u8;
                self.regs[0] = a;
                self.recalc();
            }
            175 => {
                match addr {
                    0x8000 => {
                        self.common.mirroring = if value & 0x04 != 0 {
                            Mirroring::Horizontal
                        } else {
                            Mirroring::Vertical
                        }
                    }
                    0xA000 => {
                        self.regs[0] = value & 0x0F;
                    }
                    _ => {}
                }
            }
            176 => self.write_fk23c(addr, value),
            178 => {
                if (0x4800..=0x4FFF).contains(&addr) {
                    self.regs[(addr & 0x03) as usize] = value;
                    self.recalc();
                }
            }
            179 => {
                if addr >= 0x8000 {
                    self.common.mirroring = if value & 1 != 0 {
                        Mirroring::Horizontal
                    } else {
                        Mirroring::Vertical
                    };
                } else {
                    self.regs[0] = value;
                    self.recalc();
                }
            }
            183 => self.write_183(addr, value),
            185 => {
                let value = value & prg_rom_byte(&self.common, addr);
                self.regs[0] = value;
                self.recalc();
            }
            190 => {
                if (0x8000..=0x9FFF).contains(&addr) {
                    self.regs[0] = value & 0x07;
                    map_prg(&mut self.common, 0, (value & 0x07) as i32, SZ_16K);
                } else if (0xC000..=0xDFFF).contains(&addr) {
                    self.regs[0] = (value & 0x07) | 0x08;
                    map_prg(&mut self.common, 0, ((value & 0x07) | 0x08) as i32, SZ_16K);
                } else if (addr & 0xA000) == 0xA000 {
                    map_chr(&mut self.common, (addr & 0x03) as usize, value as i32, SZ_2K);
                }
            }
            193 => match addr & 0x03 {
                0 => {
                    map_chr(&mut self.common, 0, (value >> 1) as i32, SZ_2K);
                    map_chr(&mut self.common, 1, (value >> 1) as i32 + 1, SZ_2K);
                }
                1 => map_chr(&mut self.common, 2, (value >> 1) as i32, SZ_2K),
                2 => map_chr(&mut self.common, 3, (value >> 1) as i32, SZ_2K),
                _ => map_prg(&mut self.common, 0, value as i32, SZ_8K),
            },
            200 => {
                self.regs[0] = addr as u8;
                self.recalc();
            }
            202 => {
                self.regs[0] = addr as u8;
                self.recalc();
            }
            203 => {
                self.regs[0] = value;
                self.recalc();
            }
            204 => {
                self.regs[0] = addr as u8;
                self.recalc();
            }
            212 => {
                self.regs[0] = addr as u8;
                self.regs[1] = (addr >> 8) as u8;
                self.recalc();
            }
            213 => {
                self.regs[0] = addr as u8;
                self.recalc();
            }
            214 => {
                self.regs[0] = addr as u8;
                self.recalc();
            }
            216 => {
                self.regs[0] = addr as u8;
                self.recalc();
            }
            220 => {
                let low = addr & 1 == 0;
                let idx = match addr & 0xF002 {
                    0x8000 | 0x8002 | 0x9000 | 0x9002 => {
                        self.common.mirroring = if value & 1 != 0 {
                            Mirroring::Vertical
                        } else {
                            Mirroring::Horizontal
                        };
                        0xFF
                    }
                    0xB000 => 0,
                    0xB002 => 1,
                    0xC000 => 2,
                    0xC002 => 3,
                    0xD000 => 4,
                    0xD002 => 5,
                    0xE000 => 6,
                    0xE002 => 7,
                    _ => 0xFF,
                };
                if idx != 0xFF {
                    let i = idx as usize;
                    self.regs[i] = if low {
                        (self.regs[i] & 0xF0) | (value & 0x0F)
                    } else {
                        (self.regs[i] & 0x0F) | ((value << 4) & 0xF0)
                    };
                    self.recalc();
                }
            }
            221 => match addr & 0xC000 {
                0x8000 => {
                    self.regs[0] = (addr & 0xFF) as u8;
                    self.regs[1] = (addr >> 8) as u8;
                    self.recalc();
                }
                0xC000 => {
                    self.regs[2] = (addr & 0x07) as u8;
                    self.recalc();
                }
                _ => {}
            },
            222 => self.write_222(addr, value),
            225 => {
                self.regs[0] = addr as u8;
                self.regs[1] = (addr >> 8) as u8;
                self.recalc();
            }
            226 | 233 => {
                match addr & 0x8001 {
                    0x8000 => self.regs[0] = value,
                    0x8001 => self.regs[1] = value,
                    _ => {}
                }
                self.recalc();
            }
            227 => {
                self.regs[0] = addr as u8;
                self.regs[1] = (addr >> 8) as u8;
                let a = addr as u16;
                let prg_bank = (((a >> 2) & 0x1F) | ((a & 0x100) >> 3)) as i32;
                let s_flag = a & 1 != 0;
                let l_flag = (a >> 9) & 1 != 0;
                let prg_mode = (a >> 7) & 1 != 0;
                if prg_mode {
                    if s_flag {
                        map_prg_2x(&mut self.common, 0, prg_bank & 0xFE, SZ_16K);
                    } else {
                        map_prg(&mut self.common, 0, prg_bank, SZ_16K);
                        map_prg(&mut self.common, 1, prg_bank, SZ_16K);
                    }
                } else if s_flag {
                    if l_flag {
                        map_prg(&mut self.common, 0, prg_bank & 0x3E, SZ_16K);
                        map_prg(&mut self.common, 1, prg_bank | 0x07, SZ_16K);
                    } else {
                        map_prg(&mut self.common, 0, prg_bank & 0x3E, SZ_16K);
                        map_prg(&mut self.common, 1, prg_bank & 0x38, SZ_16K);
                    }
                } else if l_flag {
                    map_prg(&mut self.common, 0, prg_bank, SZ_16K);
                    map_prg(&mut self.common, 1, prg_bank | 0x07, SZ_16K);
                } else {
                    map_prg(&mut self.common, 0, prg_bank, SZ_16K);
                    map_prg(&mut self.common, 1, prg_bank & 0x38, SZ_16K);
                }
                self.common.mirroring = if a & 0x02 != 0 {
                    Mirroring::Horizontal
                } else {
                    Mirroring::Vertical
                };
            }
            228 => {
                self.regs[0] = addr as u8;
                self.regs[1] = (addr >> 8) as u8;
                self.regs[2] = value;
                self.recalc();
            }
            229 => {
                self.regs[0] = addr as u8;
                self.regs[1] = (addr >> 8) as u8;
                self.recalc();
            }
            230 => {
                if self.contra_mode {
                    map_prg(&mut self.common, 0, (value & 0x07) as i32, SZ_16K);
                } else if value & 0x20 != 0 {
                    map_prg(&mut self.common, 0, ((value & 0x1F) + 8) as i32, SZ_16K);
                    map_prg(&mut self.common, 1, ((value & 0x1F) + 8) as i32, SZ_16K);
                } else {
                    map_prg(&mut self.common, 0, ((value & 0x1E) + 8) as i32, SZ_16K);
                    map_prg(&mut self.common, 1, ((value & 0x1E) + 9) as i32, SZ_16K);
                }
                if !self.contra_mode {
                    self.common.mirroring = if value & 0x40 != 0 {
                        Mirroring::Vertical
                    } else {
                        Mirroring::Horizontal
                    };
                }
            }
            231 => {
                self.regs[0] = addr as u8;
                self.regs[1] = (addr >> 8) as u8;
                self.recalc();
            }
            234 => self.write_234(addr, value),
            235 => {
                self.regs[0] = addr as u8;
                self.regs[1] = (addr >> 8) as u8;
                self.recalc_235_inner(addr);
            }
            236 => {
                if addr & 0x4000 != 0 {
                    self.regs[0] = (addr & 0x30) as u8;
                    self.regs[2] = (addr & 0x07) as u8;
                } else {
                    self.common.mirroring = if addr & 0x20 != 0 {
                        Mirroring::Horizontal
                    } else {
                        Mirroring::Vertical
                    };
                    if self.use_outer_bank {
                        self.regs[1] = ((addr & 0x03) << 3) as u8;
                    } else {
                        self.regs[3] = (addr & 0x07) as u8;
                    }
                }
                self.recalc();
            }
            240 => {
                self.regs[0] = value;
                self.recalc();
            }
            241 => {
                self.regs[0] = value;
                self.recalc();
            }
            242 => {
                self.regs[0] = addr as u8;
                self.recalc();
            }
            243 => {
                match addr & 0xC101 {
                    0x4100 => self.current = value & 0x07,
                    0x4101 => {
                        self.regs[(self.current & 7) as usize] = value & 0x07;
                        self.recalc();
                    }
                    _ => {}
                }
            }
            244 => {
                self.regs[0] = value;
                self.recalc();
            }
            246 => {
                let idx = (addr & 0x07) as usize;
                if idx <= 3 {
                    map_prg(&mut self.common, idx, value as i32, SZ_8K);
                } else {
                    map_chr(&mut self.common, idx & 3, value as i32, SZ_2K);
                }
            }
            252 => self.write_252(addr, value),
            253 => self.write_253(addr, value),
            255 => {
                self.regs[0] = addr as u8;
                self.regs[1] = (addr >> 8) as u8;
                self.recalc();
            }
            28 => {
                if addr <= 0x5FFF {
                    self.current = ((value & 0x80) >> 6) | (value & 0x01);
                } else if addr >= 0x8000 {
                    if self.current <= 1 {
                        self.regs[20] = (value >> 4) & 0x01;
                    } else if self.current == 2 {
                        self.regs[20] = value & 0x01;
                    }
                    self.regs[(self.current & 3) as usize] = value;
                    self.recalc();
                }
            }
            36 => {
                if (addr & 0xF200) == 0x4200 {
                    self.regs[0] = value;
                }
                self.txc.write(addr, (value >> 4) & 0x03);
                self.recalc();
            }
            38 => {
                if (0x7000..=0x7FFF).contains(&addr) {
                    self.regs[0] = value;
                    self.recalc();
                }
            }
            41 => {
                if addr <= 0x67FF {
                    self.regs[0] = (addr & 0x07) as u8;
                    self.regs[1] = (self.regs[1] & 0x03) | ((addr >> 1) & 0x0C) as u8;
                    self.regs[2] = (addr & 0x20) as u8;
                    self.recalc();
                } else if addr >= 0x8000 {
                    if self.regs[0] >= 4 {
                        self.regs[1] = (self.regs[1] & 0x0C) | (value & 0x03);
                        self.recalc();
                    }
                }
            }
            57 => {
                match addr & 0x8800 {
                    0x8000 => self.regs[0] = value,
                    0x8800 => self.regs[1] = value,
                    _ => {}
                }
                self.recalc();
            }
            58 | 61 | 62 | 63 => {
                self.regs[0] = addr as u8;
                self.regs[1] = (addr >> 8) as u8;
                if self.id == 62 {
                    self.regs[2] = value;
                }
                self.recalc();
            }
            76 | 154 => self.write_namco_variant(addr, value),
            78 => {
                let value = value & prg_rom_byte(&self.common, addr);
                self.regs[0] = value;
                self.recalc();
            }
            83 => self.write_83(addr, value),
            94 => {
                self.regs[0] = value;
                self.recalc();
            }
            104 => {
                if addr >= 0xC000 {
                    self.regs[0] = (self.regs[0] & 0xF0) | (value & 0x0F);
                    self.recalc();
                } else if addr <= 0x9FFF && value & 0x08 != 0 {
                    self.regs[0] = (self.regs[0] & 0x0F) | ((value << 4) & 0x70);
                    self.recalc();
                }
            }
            106 => self.write_106(addr, value),
            125 => {
                if addr == 0x6000 {
                    self.regs[0] = value;
                    self.recalc();
                }
            }
            144 => {
                let prg = prg_rom_byte(&self.common, addr);
                let value = (value & prg) | (prg & 0x01);
                self.regs[0] = value;
                self.recalc();
            }
            156 => {
                match addr {
                    0xC000..=0xC00F => {
                        let bank = ((addr & 0x03) + if addr >= 0xC008 { 4 } else { 0 }) as usize;
                        if addr & 0x04 != 0 {
                            self.regs[8 + bank] = value;
                        } else {
                            self.regs[bank] = value;
                        }
                        self.recalc();
                    }
                    0xC010 => {
                        self.regs[16] = value;
                        self.recalc();
                    }
                    0xC014 => {
                        self.regs[17] = value;
                        self.recalc();
                        self.common.mirroring = if value & 1 != 0 {
                            Mirroring::Horizontal
                        } else {
                            Mirroring::Vertical
                        };
                    }
                    _ => {}
                }
            }
            162 => {
                if (0x5000..=0x5FFF).contains(&addr) {
                    self.regs[((addr >> 8) & 0x03) as usize] = value;
                    self.recalc();
                }
            }
            177 => {
                self.regs[0] = value;
                self.recalc();
            }
            _ => {}
        }
    }

    fn write_namco_variant(&mut self, addr: u16, value: u8) {
        if self.id == 154 {
            self.common.mirroring = if value & 0x40 != 0 {
                Mirroring::ScreenBOnly
            } else {
                Mirroring::ScreenAOnly
            };
        }
        let a = addr & 0x8001;
        let mut v = value;
        if a == 0x8000 {
            v &= 0x3F;
        }
        self.mmc3.write(&mut self.common, a, v);
        self.mmc3.update(&mut self.common);
        if self.id == 76 {
            for i in 0..4 {
                map_chr(&mut self.common, i, self.mmc3.registers[2 + i] as i32, SZ_2K);
            }
        } else {
            self.mmc3.registers[0] &= 0x3F;
            self.mmc3.registers[1] &= 0x3F;
            for i in 2..6 {
                self.mmc3.registers[i] |= 0x40;
            }
            self.mmc3.update(&mut self.common);
        }
    }

    fn write_83(&mut self, addr: u16, value: u8) {
        if addr < 0x8000 {
            if (0x5100..=0x5103).contains(&addr) {
                self.regs[12 + (addr & 0x03) as usize] = value;
            }
            return;
        }
        if (0x8300..=0x8302).contains(&addr) {
            self.regs[19] &= 0xBF;
            self.regs[8 + (addr - 0x8300) as usize] = value;
            self.recalc();
        } else if (0x8310..=0x8317).contains(&addr) {
            self.regs[(addr - 0x8310) as usize] = value;
            if (0x8312..=0x8315).contains(&addr) {
                self.regs[18] = 1;
            }
            self.recalc();
        } else {
            match addr {
                0x8000 => {
                    self.regs[17] = 1;
                    self.regs[20] = value;
                    self.regs[19] |= 0x40;
                    self.recalc();
                }
                0xB000 | 0xB0FF | 0xB1FF => {
                    self.regs[20] = value;
                    self.regs[19] |= 0x40;
                    self.recalc();
                }
                0x8100 => {
                    self.regs[19] = value | (self.regs[19] & 0x40);
                    self.recalc();
                }
                0x8200 => {
                    self.irq_counter = (self.irq_counter & 0xFF00) | value as u16;
                    self.reset_irq();
                }
                0x8201 => {
                    self.irq_enabled = self.regs[19] & 0x80 != 0;
                    self.irq_counter =
                        (self.irq_counter & 0x00FF) | ((value as u16) << 8);
                }
                _ => {}
            }
        }
    }

    fn write_106(&mut self, addr: u16, value: u8) {
        match addr & 0x0F {
            0 | 2 => {
                self.regs[24 + (addr & 0x07) as usize] = value & 0xFE;
                self.recalc();
            }
            1 | 3 => {
                self.regs[24 + (addr & 0x07) as usize] = value | 0x01;
                self.recalc();
            }
            4..=7 => {
                self.regs[24 + (addr & 0x07) as usize] = value;
                self.recalc();
            }
            8 | 0x0B => {
                self.regs[((addr & 0x0F) - 8) as usize] = (value & 0x0F) | 0x10;
                self.recalc();
            }
            9 | 0x0A => {
                self.regs[((addr & 0x0F) - 8) as usize] = value & 0x1F;
                self.recalc();
            }
            0x0D => {
                self.irq_enabled = false;
                self.irq_counter = 0;
                self.reset_irq();
            }
            0x0E => {
                self.irq_counter = (self.irq_counter & 0xFF00) | value as u16;
            }
            _ => {
                // 0x0F
                self.irq_counter = (self.irq_counter & 0x00FF) | ((value as u16) << 8);
                self.irq_enabled = true;
            }
        }
    }

    fn write_kaiser202(&mut self, addr: u16, value: u8) {        match addr & 0xF000 {
            0x8000 => self.irq_reload = (self.irq_reload & 0xFFF0) | (value & 0x0F) as u16,
            0x9000 => self.irq_reload = (self.irq_reload & 0xFF0F) | (((value & 0x0F) as u16) << 4),
            0xA000 => self.irq_reload = (self.irq_reload & 0xF0FF) | (((value & 0x0F) as u16) << 8),
            0xB000 => self.irq_reload = (self.irq_reload & 0x0FFF) | (((value & 0x0F) as u16) << 12),
            0xC000 => {
                self.irq_enabled = value != 0;
                if self.irq_enabled {
                    self.irq_counter = self.irq_reload;
                }
                self.reset_irq();
            }
            0xD000 => self.reset_irq(),
            0xE000 => {
                self.current = (value & 0x0F).wrapping_sub(1);
            }
            0xF000 => {
                let sel = self.current as usize;
                if sel < 3 {
                    self.regs[sel] = (self.regs[sel] & 0x10) | (value & 0x0F);
                } else if sel < 4 {
                    self.regs[23] = value;
                }
                match addr & 0xFC00 {
                    0xF000 => {
                        let bank = (addr & 0x03) as usize;
                        if bank < 3 {
                            self.regs[bank] = (value & 0x10) | (self.regs[bank] & 0x0F);
                        }
                    }
                    0xF800 => self.regs[22] = value,
                    _ => {
                        // 0xFC00: 1KB CHR bank
                        map_chr(&mut self.common, (addr & 0x07) as usize, value as i32, SZ_1K);
                    }
                }
                if addr & 0xFC00 == 0xF800 {
                    self.common.mirroring = if value & 1 != 0 {
                        Mirroring::Vertical
                    } else {
                        Mirroring::Horizontal
                    };
                }
                self.recalc();
            }
            _ => {}
        }
    }

    fn write_rambo1(&mut self, addr: u16, value: u8) {
        match addr & 0xE001 {
            0x8000 => self.current = value,
            0x8001 => {
                self.regs[(self.current & 0x0F) as usize] = value;
                self.recalc();
            }
            0xA000 => {
                if self.id != 158 {
                    self.common.mirroring = if value & 1 != 0 {
                        Mirroring::Horizontal
                    } else {
                        Mirroring::Vertical
                    };
                }
            }
            0xC000 => self.irq_reload = value as u16,
            0xC001 => {
                if self.irq_mode & 1 != 0 && value & 1 == 0 {
                    self.need_irq = true;
                }
                self.irq_mode = value & 1;
                if self.irq_mode & 1 != 0 {
                    self.irq_prescaler = 0;
                }
                self.irq_reload_flag = true;
            }
            0xE000 => {
                self.irq_enabled = false;
                self.reset_irq();
            }
            0xE001 => self.irq_enabled = true,
            _ => {}
        }
        if self.id == 158 && (addr & 0xE001) == 0x8001 {
            let nt = value >> 7;
            if self.current & 0x80 != 0 {
                match self.current & 0x07 {
                    2 => self.set_nt([nt, self.nt_map[1], self.nt_map[2], self.nt_map[3]]),
                    3 => self.set_nt([self.nt_map[0], nt, self.nt_map[2], self.nt_map[3]]),
                    4 => self.set_nt([self.nt_map[0], self.nt_map[1], nt, self.nt_map[3]]),
                    5 => self.set_nt([self.nt_map[0], self.nt_map[1], self.nt_map[2], nt]),
                    _ => {}
                }
            } else {
                match self.current & 0x07 {
                    0 => self.set_nt([nt, nt, self.nt_map[2], self.nt_map[3]]),
                    1 => self.set_nt([self.nt_map[0], self.nt_map[1], nt, nt]),
                    _ => {}
                }
            }
        }
        if self.id == 158 && (addr & 0xE001) == 0xA000 {
            return;
        }
        if (addr & 0xE001) == 0x8001 {
            self.recalc_rambo1();
        }
    }

    fn recalc_rambo1(&mut self) {
        let r = self.regs;
        if self.current & 0x40 != 0 {
            map_prg(&mut self.common, 0, r[15] as i32, SZ_8K);
            map_prg(&mut self.common, 1, r[6] as i32, SZ_8K);
            map_prg(&mut self.common, 2, r[7] as i32, SZ_8K);
        } else {
            map_prg(&mut self.common, 0, r[6] as i32, SZ_8K);
            map_prg(&mut self.common, 1, r[7] as i32, SZ_8K);
            map_prg(&mut self.common, 2, r[15] as i32, SZ_8K);
        }
        let inv = if self.current & 0x80 != 0 { 4 } else { 0 };
        map_chr(&mut self.common, 0 ^ inv, r[0] as i32, SZ_1K);
        map_chr(&mut self.common, 2 ^ inv, r[1] as i32, SZ_1K);
        map_chr(&mut self.common, 4 ^ inv, r[2] as i32, SZ_1K);
        map_chr(&mut self.common, 5 ^ inv, r[3] as i32, SZ_1K);
        map_chr(&mut self.common, 6 ^ inv, r[4] as i32, SZ_1K);
        map_chr(&mut self.common, 7 ^ inv, r[5] as i32, SZ_1K);
        if self.current & 0x20 != 0 {
            map_chr(&mut self.common, 1 ^ inv, r[8] as i32, SZ_1K);
            map_chr(&mut self.common, 3 ^ inv, r[9] as i32, SZ_1K);
        } else {
            map_chr(&mut self.common, 1 ^ inv, r[0] as i32 + 1, SZ_1K);
            map_chr(&mut self.common, 3 ^ inv, r[1] as i32 + 1, SZ_1K);
        }
    }

    fn write_taito_x1005(&mut self, addr: u16, value: u8) {
        let alternate = self.id == 207;
        match addr {
            0x7EF0 => {
                map_chr(&mut self.common, 0, value as i32, SZ_1K);
                map_chr(&mut self.common, 1, value as i32 + 1, SZ_1K);
                if alternate {
                    let nt = value >> 7;
                    self.set_nt([nt, nt, self.nt_map[2], self.nt_map[3]]);
                }
            }
            0x7EF1 => {
                map_chr(&mut self.common, 2, value as i32, SZ_1K);
                map_chr(&mut self.common, 3, value as i32 + 1, SZ_1K);
                if alternate {
                    let nt = value >> 7;
                    self.set_nt([self.nt_map[0], self.nt_map[1], nt, nt]);
                }
            }
            0x7EF2 => map_chr(&mut self.common, 4, value as i32, SZ_1K),
            0x7EF3 => map_chr(&mut self.common, 5, value as i32, SZ_1K),
            0x7EF4 => map_chr(&mut self.common, 6, value as i32, SZ_1K),
            0x7EF5 => map_chr(&mut self.common, 7, value as i32, SZ_1K),
            0x7EF6 | 0x7EF7 => {
                if !alternate {
                    self.common.mirroring = if value & 1 != 0 {
                        Mirroring::Vertical
                    } else {
                        Mirroring::Horizontal
                    };
                }
            }
            0x7EF8 | 0x7EF9 => self.regs[0] = value,
            0x7EFA | 0x7EFB => map_prg(&mut self.common, 0, value as i32, SZ_8K),
            0x7EFC | 0x7EFD => map_prg(&mut self.common, 1, value as i32, SZ_8K),
            0x7EFE | 0x7EFF => map_prg(&mut self.common, 2, value as i32, SZ_8K),
            _ => {}
        }
    }

    fn write_jy(&mut self, addr: u16, value: u8) {
        if addr < 0x8000 {
            match addr & 0xF803 {
                0x5800 => self.regs[44] = value,
                0x5801 => self.regs[45] = value,
                0x5803 => self.regs[46] = value,
                _ => {}
            }
            return;
        }
        match addr & 0xF007 {
            0x8000..=0x8007 => {
                self.regs[(addr & 3) as usize] = value & 0x7F;
                self.recalc();
            }
            0x9000..=0x9007 => {
                self.regs[4 + (addr & 7) as usize] = value;
                self.recalc();
            }
            0xA000..=0xA007 => {
                self.regs[12 + (addr & 7) as usize] = value;
                self.recalc();
            }
            0xB000..=0xB003 => {
                self.regs[28 + (addr & 3) as usize] = value;
                self.recalc();
            }
            0xB004..=0xB007 => {
                self.regs[32 + (addr & 3) as usize] = value;
                self.recalc();
            }
            0xC000 => {
                self.irq_enabled = value & 1 != 0;
                if !self.irq_enabled {
                    self.reset_irq();
                }
            }
            0xC001 => {
                self.irq_dir = (value >> 6) & 0x03;
                self.irq_mode = if value & 0x08 != 0 { 1 } else { 0 };
                self.irq_small = value & 0x04 != 0;
                self.jy_irq_source = value & 0x03;
            }
            0xC002 => {
                self.irq_enabled = false;
                self.reset_irq();
            }
            0xC003 => self.irq_enabled = true,
            0xC004 => self.irq_prescaler = (value ^ self.irq_xor) as i16,
            0xC005 => self.irq_counter = (value ^ self.irq_xor) as u16,
            0xC006 => self.irq_xor = value,
            0xC007 => self.irq_delay = value,
            0xD000 => {
                self.regs[24] = value & 0x07;
                self.regs[25] = (value >> 3) & 0x03;
                self.regs[27] = (self.regs[27] & 0x07)
                    | (if value & 0x20 != 0 { 0x01 } else { 0 })
                    | (if value & 0x40 != 0 { 0x02 } else { 0 })
                    | (if value & 0x80 != 0 { 0x04 } else { 0 });
                self.recalc();
            }
            0xD001 => {
                self.regs[22] = value & 0x03;
                self.recalc();
            }
            0xD002 => {
                self.regs[23] = value & 0x80;
                self.recalc();
            }
            0xD003 => {
                self.regs[26] = value;
                self.recalc();
            }
            _ => {}
        }
    }

    fn write_mmc3_lite(&mut self, addr: u16, value: u8) {
        match addr & 0x7003 {
            0x6000 => map_chr(&mut self.common, 0, value as i32, SZ_2K),
            0x6001 => map_chr(&mut self.common, 1, value as i32, SZ_2K),
            0x6002 => map_chr(&mut self.common, 2, value as i32, SZ_2K),
            0x6003 => map_chr(&mut self.common, 3, value as i32, SZ_2K),
            0x7000 => map_prg(&mut self.common, 0, (value & 0x0F) as i32, SZ_8K),
            0x7001 => map_prg(&mut self.common, 1, (value & 0x0F) as i32, SZ_8K),
            0x7002 => {
                // MMC3::WriteRegister(0xE000, value)
                self.mmc3.irq_enabled = false;
                self.common.irq_pending = false;
            }
            0x7003 => {
                self.mmc3.irq_reload_value = 0x07;
                self.mmc3.irq_counter = 0;
                self.mmc3.irq_reload = true;
                self.mmc3.irq_enabled = true;
            }
            _ => {}
        }
    }

    fn write_namco108(&mut self, addr: u16, value: u8) {
        let a = addr & 0x8001;
        let mut v = value;
        if a == 0x8000 {
            v &= 0x3F;
        }
        self.mmc3.write(&mut self.common, a, v);
        self.mmc3.update(&mut self.common);
        if self.id == 95 && a & 0x01 != 0 {
            let nt1 = (self.mmc3.registers[0] >> 5) & 1;
            let nt2 = (self.mmc3.registers[1] >> 5) & 1;
            self.set_nt([nt1, nt1, nt2, nt2]);
        }
        // Namco 108 has hardwired mirroring.
        self.common.mirroring = self.common.info.mirroring;
    }

    fn write_117(&mut self, addr: u16, value: u8) {
        match addr {
            0x8000..=0x8003 => {
                map_prg(&mut self.common, (addr & 3) as usize, value as i32, SZ_8K)
            }
            0xA000..=0xA007 => {
                map_chr(&mut self.common, (addr & 7) as usize, value as i32, SZ_1K)
            }
            0xC001 => self.irq_reload = value as u16,
            0xC002 => self.reset_irq(),
            0xC003 => {
                self.irq_counter = self.irq_reload;
                self.irq_alt = true;
            }
            0xD000 => {
                self.common.mirroring = if value & 1 != 0 {
                    Mirroring::Horizontal
                } else {
                    Mirroring::Vertical
                }
            }
            0xE000 => {
                self.irq_enabled = value & 1 != 0;
                self.reset_irq();
            }
            _ => {}
        }
    }

    fn write_163(&mut self, addr: u16, value: u8) {
        if !(0x5000..=0x5FFF).contains(&addr) {
            return;
        }
        if addr == 0x5101 {
            if self.regs[4] != 0 && value == 0 {
                self.toggle = !self.toggle;
            }
            self.regs[4] = value;
            return;
        }
        if addr == 0x5100 && value == 6 {
            map_prg(&mut self.common, 0, 3, SZ_32K);
            return;
        }
        match addr & 0x7300 {
            0x5000 => {
                self.regs[0] = value;
                if self.regs[0] & 0x80 == 0 && self.scanline < 128 {
                    map_chr(&mut self.common, 0, 0, SZ_4K);
                    map_chr(&mut self.common, 1, 1, SZ_4K);
                }
                self.recalc();
            }
            0x5100 => {
                self.regs[1] = value;
                if value == 6 {
                    map_prg(&mut self.common, 0, 3, SZ_32K);
                }
            }
            0x5200 => {
                self.regs[2] = value;
                self.recalc();
            }
            0x5300 => self.regs[3] = value,
            _ => {}
        }
    }

    fn write_183(&mut self, addr: u16, value: u8) {
        if (addr & 0xF800) == 0x6800 {
            self.regs[8] = (addr & 0x3F) as u8;
            self.recalc();
        } else if ((addr & 0xF80C) >= 0xB000) && ((addr & 0xF80C) <= 0xE00C) {
            let slot = ((((addr >> 11).wrapping_sub(6)) | (addr >> 3)) & 0x07) as usize;
            let shift = (addr & 0x04) as u32;
            let cur = self.regs[slot];
            self.regs[slot] = (cur & ((0xF0u16 >> shift) as u8))
                | (((value & 0x0F) as u8) << shift);
            map_chr(&mut self.common, slot, self.regs[slot] as i32, SZ_1K);
        } else {
            match addr & 0xF80C {
                0x8800 => map_prg(&mut self.common, 0, value as i32, SZ_8K),
                0xA800 => map_prg(&mut self.common, 1, value as i32, SZ_8K),
                0xA000 => map_prg(&mut self.common, 2, value as i32, SZ_8K),
                0x9800 => {
                    self.common.mirroring = match value & 0x03 {
                        0 => Mirroring::Vertical,
                        1 => Mirroring::Horizontal,
                        2 => Mirroring::ScreenAOnly,
                        _ => Mirroring::ScreenBOnly,
                    }
                }
                0xF000 => {
                    self.irq_counter = (self.irq_counter & 0xF0) | (value & 0x0F) as u16
                }
                0xF004 => {
                    self.irq_counter =
                        (self.irq_counter & 0x0F) | (((value & 0x0F) as u16) << 4)
                }
                0xF008 => {
                    self.irq_enabled = value > 0;
                    if !self.irq_enabled {
                        self.irq_prescaler = 0;
                    }
                    self.reset_irq();
                }
                _ => {}
            }
        }
    }

    fn write_222(&mut self, addr: u16, value: u8) {
        match addr & 0xF003 {
            0x8000 => map_prg(&mut self.common, 0, value as i32, SZ_8K),
            0x9000 => {
                self.common.mirroring = if value & 1 != 0 {
                    Mirroring::Horizontal
                } else {
                    Mirroring::Vertical
                }
            }
            0xA000 => map_prg(&mut self.common, 1, value as i32, SZ_8K),
            0xB000 => map_chr(&mut self.common, 0, value as i32, SZ_1K),
            0xB002 => map_chr(&mut self.common, 1, value as i32, SZ_1K),
            0xC000 => map_chr(&mut self.common, 2, value as i32, SZ_1K),
            0xC002 => map_chr(&mut self.common, 3, value as i32, SZ_1K),
            0xD000 => map_chr(&mut self.common, 4, value as i32, SZ_1K),
            0xD002 => map_chr(&mut self.common, 5, value as i32, SZ_1K),
            0xE000 => map_chr(&mut self.common, 6, value as i32, SZ_1K),
            0xE002 => map_chr(&mut self.common, 7, value as i32, SZ_1K),
            0xF000 => {
                self.irq_counter = value as u16;
                self.reset_irq();
            }
            _ => {}
        }
    }

    fn write_234(&mut self, addr: u16, value: u8) {
        let mut value = value;
        if self.has_bus_conflicts() {
            value &= prg_rom_byte(&self.common, addr);
        }
        if addr <= 0xFF9F {
            if self.regs[0] & 0x3F == 0 {
                self.regs[0] = value;
                self.recalc();
            }
        } else {
            self.regs[1] = value & 0x71;
            self.recalc();
        }
    }

    fn write_252(&mut self, addr: u16, value: u8) {
        if addr <= 0x8FFF {
            map_prg(&mut self.common, 0, value as i32, SZ_8K);
        } else if (0xA000..=0xAFFF).contains(&addr) {
            map_prg(&mut self.common, 1, value as i32, SZ_8K);
        } else if (0xB000..=0xEFFF).contains(&addr) {
            let shift = (addr & 0x4) as u32;
            let bank = (((((addr as u32).wrapping_sub(0xB000)) >> 1) & 0x1800)
                | (((addr as u32) << 7) & 0x0400))
                / 0x400;
            let bank = (bank & 7) as usize;
            let cur = self.regs[bank];
            self.regs[bank] = (cur & ((0xF0u16 >> shift) as u8))
                | (((value & 0x0F) as u8) << shift);
            self.recalc();
        } else {
            match addr & 0xF00C {
                0xF000 => self.set_vrc_reload_nibble(value, false),
                0xF004 => self.set_vrc_reload_nibble(value, true),
                0xF008 => self.set_vrc_control(value),
                0xF00C => {
                    self.irq_enabled = self.irq_alt;
                    self.reset_irq();
                }
                _ => {}
            }
        }
    }

    fn set_vrc_reload_nibble(&mut self, value: u8, high: bool) {
        if high {
            self.irq_reload = (self.irq_reload & 0x0F) | (((value & 0x0F) as u16) << 4);
        } else {
            self.irq_reload = (self.irq_reload & 0xF0) | ((value & 0x0F) as u16);
        }
    }

    fn set_vrc_control(&mut self, value: u8) {
        self.irq_alt = value & 1 != 0;
        self.irq_enabled = value & 2 != 0;
        self.irq_mode = value & 4;
        if self.irq_enabled {
            self.irq_counter = self.irq_reload;
            self.irq_prescaler = 341;
        }
        self.reset_irq();
    }

    fn write_253(&mut self, addr: u16, value: u8) {
        if (0xB000..=0xE00C).contains(&addr) {
            let slot = ((((addr & 0x08) | (addr >> 8)) >> 3) + 2) & 0x07;
            let shift = (addr & 0x04) as u32;
            let slot = slot as usize;
            let cur = self.regs[slot];
            let low = (cur & ((0xF0u16 >> shift) as u8))
                | ((value as u16).wrapping_shl(shift) as u8);
            self.regs[slot] = low;
            if slot == 0 {
                if low == 0xC8 {
                    self.regs[19] &= !1;
                } else if low == 0x88 {
                    self.regs[19] |= 1;
                }
            }
            if shift != 0 {
                self.regs[8 + slot] = value >> 4;
            }
            self.recalc();
        } else {
            match addr {
                0x8010 => map_prg(&mut self.common, 0, value as i32, SZ_8K),
                0xA010 => map_prg(&mut self.common, 1, value as i32, SZ_8K),
                0x9400 => {
                    self.common.mirroring = match value & 0x03 {
                        0 => Mirroring::Vertical,
                        1 => Mirroring::Horizontal,
                        2 => Mirroring::ScreenAOnly,
                        _ => Mirroring::ScreenBOnly,
                    }
                }
                0xF000 => {
                    self.irq_reload = (self.irq_reload & 0xFFF0) | (value & 0x0F) as u16;
                    self.reset_irq();
                }
                0xF004 => {
                    self.irq_reload =
                        (self.irq_reload & 0xFF0F) | (((value & 0x0F) as u16) << 4);
                    self.reset_irq();
                }
                0xF008 => {
                    self.irq_counter = self.irq_reload;
                    self.irq_enabled = value & 0x02 != 0;
                    self.irq_prescaler = 0;
                    self.reset_irq();
                }
                _ => {}
            }
        }
    }

    fn write_fk23c(&mut self, addr: u16, value: u8) {
        if addr < 0x8000 {
            if self.fk23_regs_enabled || !self.wram_config_enabled {
                let mask = 0x5010;
                if (addr & mask) != mask {
                    return;
                }
                match addr & 0x03 {
                    0 => {
                        self.regs[0] = value & 0x07;
                        self.regs[1] = (value & 0x10) >> 4;
                        self.select_chr_ram = value & 0x20 != 0;
                        self.mmc3_chr_mode = value & 0x40 == 0;
                        self.prg_base_bits = (self.prg_base_bits & !0x180)
                            | (((value & 0x80) as u16) << 1)
                            | (((value & 0x08) as u16) << 4);
                    }
                    1 => {
                        self.prg_base_bits =
                            (self.prg_base_bits & !0x7F) | (value & 0x7F) as u16;
                    }
                    2 => {
                        self.prg_base_bits = (self.prg_base_bits & !0x200)
                            | (((value & 0x40) as u16) << 3);
                        self.regs[2] = value;
                        self.regs[4] = 0;
                    }
                    _ => {
                        self.extended_mmc3 = value & 0x02 != 0;
                        self.cnrom_chr_mode = value & 0x44 != 0;
                    }
                }
                self.recalc();
            } else {
                let idx = (addr as usize).wrapping_sub(0x6000) & 0x7FFF;
                let len = self.common.prg_ram.len().max(1);
                self.common.prg_ram[idx % len] = value;
            }
            return;
        }

        if self.cnrom_chr_mode && (addr <= 0x9FFF || addr >= 0xC000) {
            self.regs[4] = value & 0x03;
            self.recalc();
        }
        match addr & 0xE001 {
            0x8000 => {
                let mut v = value;
                if self.common.prg_rom.len() == 16384 * 1024 && (v == 0x46 || v == 0x47) {
                    v ^= 1;
                }
                self.invert_prg_a14 = v & 0x40 != 0;
                self.invert_chr_a12 = v & 0x80 != 0;
                self.current = v & 0x0F;
                self.recalc();
            }
            0x8001 => {
                let idx = (self.current & if self.extended_mmc3 { 0x0F } else { 0x07 }) as usize;
                if idx < 12 {
                    self.fk_regs[idx] = value;
                    self.recalc();
                }
            }
            0xA000 => {
                self.regs[5] = value & 0x03;
                self.recalc();
            }
            0xA001 => {
                let mut v = value;
                if v & 0x20 == 0 {
                    v &= 0xC0;
                }
                self.regs[3] = v & 0x03;
                self.ram_in_first_chr_bank = v & 0x04 != 0;
                self.allow_single_screen_mirroring = v & 0x08 != 0;
                self.wram_config_enabled = v & 0x20 != 0;
                self.fk23_regs_enabled = v & 0x40 != 0;
                self.wram_write_protected = v & 0x40 != 0;
                self.wram_enabled = v & 0x80 != 0;
                self.recalc();
            }
            0xC000 => self.irq_reload = value as u16,
            0xC001 => {
                self.irq_counter = 0;
                self.irq_reload_flag = true;
            }
            0xE000 => {
                self.irq_enabled = false;
                self.reset_irq();
            }
            0xE001 => self.irq_enabled = true,
            _ => {}
        }
    }

    // -----------------------------------------------------------------
    // CPU bus
    // -----------------------------------------------------------------

    fn read_prg_ram_byte(&self, addr: u16) -> u8 {
        let idx = (addr as usize - 0x6000) % self.common.prg_ram.len().max(1);
        self.common.prg_ram[idx]
    }

    fn write_prg_ram_byte(&mut self, addr: u16, value: u8) {
        let len = self.common.prg_ram.len().max(1);
        let idx = (addr as usize - 0x6000) % len;
        self.common.prg_ram[idx] = value;
    }

    fn read_prg_page(&self, page: usize, addr: u16) -> u8 {
        let len = self.common.prg_rom.len().max(1);
        let off = (page + (addr as usize & 0x1FFF)) % len;
        self.common.prg_rom[off]
    }

    fn cpu_read_impl(&mut self, addr: u16) -> Option<u8> {
        match self.id {
            34 => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    Some(self.read_prg_ram_byte(addr))
                } else if addr >= 0x8000 {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
            40 => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    Some(self.read_prg_page(6 * SZ_8K, addr))
                } else if (0x5000..=0x5FFF).contains(&addr) {
                    Some(self.read_prg_page(8 * SZ_8K, addr))
                } else if addr >= 0x8000 {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
            42 => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    Some(self.read_prg_page(self.prg_2k[0], addr))
                } else if addr >= 0x8000 {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
            43 => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    Some(self.read_prg_page(self.prg_2k[0], addr))
                } else if (0x5000..=0x5FFF).contains(&addr) {
                    Some(self.read_prg_page(8 * SZ_8K, addr))
                } else if addr >= 0x8000 {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
            50 => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    Some(self.read_prg_page(0x0F * SZ_8K, addr))
                } else if addr >= 0x8000 {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
            51 => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    Some(self.read_prg_page(self.prg_2k[0], addr))
                } else if addr >= 0x8000 {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
            103 => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    if self.prg_ram_disabled {
                        Some(self.read_prg_page(self.prg_2k[0], addr))
                    } else {
                        Some(self.common.prg_ram[(addr - 0x6000) as usize])
                    }
                } else if (0xB800..=0xD7FF).contains(&addr) {
                    Some(self.common.prg_ram[0x2000 + (addr - 0xB800) as usize])
                } else if addr >= 0x8000 {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
            108 | 120 | 142 | 183 => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    if self.id == 142 && self.regs[23] == 0xFF {
                        Some(self.read_prg_ram_byte(addr))
                    } else {
                        Some(self.read_prg_page(self.prg_2k[0], addr))
                    }
                } else if addr >= 0x8000 {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
            91 | 95 | 206 | 76 | 154 => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    if self.id == 91 {
                        Some(self.read_prg_ram_byte(addr))
                    } else if self.mmc3.reg_a001 & 0x80 != 0 {
                        Some(self.read_prg_ram_byte(addr))
                    } else {
                        None
                    }
                } else if addr >= 0x8000 {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
            143 => {
                if (0x4100..=0x5FFF).contains(&addr) {
                    Some((!(addr as u8) & 0x3F) | 0x40)
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    Some(self.read_prg_ram_byte(addr))
                } else if addr >= 0x8000 {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
            163 => {
                if (0x5000..=0x5FFF).contains(&addr) {
                    Some(match addr & 0x7700 {
                        0x5100 => self.regs[3] | self.regs[1] | self.regs[0] | (self.regs[2] ^ 0xFF),
                        0x5500 => {
                            if self.toggle {
                                self.regs[3] | self.regs[0]
                            } else {
                                0
                            }
                        }
                        _ => 4,
                    })
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    Some(self.read_prg_ram_byte(addr))
                } else if addr >= 0x8000 {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
            36 | 132 | 136 | 147 | 172 | 173 => {
                if (0x4020..=0x5FFF).contains(&addr) {
                    if (addr & 0x103) == 0x100 {
                        let v = self.txc.read();
                        self.recalc();
                        Some(match self.id {
                            36 => 0xCF | ((v << 4) & 0x30),
                            136 => 0xC0 | (v & 0x3F),
                            147 => ((v & 0x3F) << 2) | ((v & 0xC0) >> 6),
                            172 => 0xC0 | convert_txc_b(v),
                            _ => 0xF0 | (v & 0x0F),
                        })
                    } else {
                        self.recalc();
                        None
                    }
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    Some(self.read_prg_ram_byte(addr))
                } else if addr >= 0x8000 {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
            170 => {
                if addr == 0x7777 {
                    Some(self.regs[0] | ((addr >> 8) as u8 & 0x7F))
                } else if addr == 0x7000 || addr == 0x7001 {
                    Some(self.regs[0] | ((addr >> 8) as u8 & 0x7F))
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    Some(self.read_prg_ram_byte(addr))
                } else if addr >= 0x8000 {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
            175 => {
                if addr == 0xFFFC {
                    self.recalc();
                    Some(self.common.read_prg_rom(addr))
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    Some(self.read_prg_ram_byte(addr))
                } else if addr >= 0x8000 {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
            176 => {
                if (0x4000..=0x5FFF).contains(&addr) || (0x6000..=0x7FFF).contains(&addr) {
                    if self.wram_config_enabled {
                        let bank = if addr < 0x6000 {
                            ((self.regs[3] as usize) + 1) & 0x03
                        } else {
                            self.regs[3] as usize & 0x03
                        };
                        let off = bank * SZ_8K + (addr as usize & 0x1FFF);
                        Some(self.common.prg_ram[off % self.common.prg_ram.len().max(1)])
                    } else if self.wram_enabled && addr >= 0x6000 {
                        Some(self.read_prg_ram_byte(addr))
                    } else {
                        None
                    }
                } else if addr >= 0x8000 {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
            178 => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    let off = self.prg_2k[0] + (addr as usize & 0x1FFF);
                    Some(self.common.prg_ram[off % self.common.prg_ram.len().max(1)])
                } else if addr >= 0x8000 {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
            63 => {
                if addr >= 0x8000 {
                    if self.open_bus && addr <= 0xBFFF {
                        None
                    } else {
                        Some(self.common.read_prg_rom(addr))
                    }
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    Some(self.read_prg_ram_byte(addr))
                } else {
                    None
                }
            }
            83 => {
                if addr == 0x5000 {
                    Some(0)
                } else if (0x5100..=0x5103).contains(&addr) {
                    Some(self.regs[12 + (addr & 0x03) as usize])
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    Some(self.read_prg_ram_byte(addr))
                } else if addr >= 0x8000 {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
            125 => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    Some(self.read_prg_page(self.prg_2k[0], addr))
                } else if (0xC000..=0xDFFF).contains(&addr) {
                    Some(self.common.prg_ram[(addr & 0x1FFF) as usize])
                } else if addr >= 0x8000 {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
            80 | 207 => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    if addr >= 0x7F00 {
                        if self.regs[0] == 0xA3 {
                            Some(self.common.prg_ram[(addr - 0x7F00) as usize & 0xFF])
                        } else {
                            None
                        }
                    } else {
                        let idx = (addr as usize - 0x6000) & 0xFF;
                        Some(self.common.prg_ram[idx])
                    }
                } else if addr >= 0x8000 {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
            90 | 209 | 211 => {
                if addr < 0x8000 {
                    match addr & 0xF803 {
                        0x5000 => Some(0),
                        0x5800 => Some(
                            (self.regs[44] as u16).wrapping_mul(self.regs[45] as u16) as u8,
                        ),
                        0x5801 => Some(
                            ((self.regs[44] as u16).wrapping_mul(self.regs[45] as u16) >> 8) as u8,
                        ),
                        0x5803 => Some(self.regs[46]),
                        _ => {
                            if (0x6000..=0x7FFF).contains(&addr) {
                                if self.regs[27] & 0x04 != 0 {
                                    Some(self.read_prg_page(self.prg_2k[0], addr))
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        }
                    }
                } else {
                    Some(self.common.read_prg_rom(addr))
                }
            }
            212 => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    let mut v = self.read_prg_ram_byte(addr);
                    if (addr & 0xE010) == 0x6000 {
                        v |= 0x80;
                    }
                    Some(v)
                } else if addr >= 0x8000 {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
            216 => {
                if addr == 0x5000 {
                    Some(0)
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    Some(self.read_prg_ram_byte(addr))
                } else if addr >= 0x8000 {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
            220 => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    let window = (addr as usize - 0x6000) / SZ_2K;
                    let off = self.regs[4 + window] as usize * SZ_2K
                        + (addr as usize & 0x7FF);
                    Some(self.common.prg_rom[off % self.common.prg_rom.len().max(1)])
                } else if addr >= 0x8000 {
                    let window = (addr as usize - 0x8000) / SZ_2K;
                    let off = self.prg_2k[window & 15] + (addr as usize & 0x7FF);
                    Some(self.common.prg_rom[off % self.common.prg_rom.len().max(1)])
                } else {
                    None
                }
            }
            234 => {
                if (0xFF80..=0xFF9F).contains(&addr) || (0xFFE8..=0xFFF8).contains(&addr) {
                    let value = self.common.read_prg_rom(addr);
                    if addr <= 0xFF9F {
                        if self.regs[0] & 0x3F == 0 {
                            self.regs[0] = value;
                            self.recalc();
                        }
                    } else {
                        self.regs[1] = value & 0x71;
                        self.recalc();
                    }
                    Some(value)
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    Some(self.read_prg_ram_byte(addr))
                } else if addr >= 0x8000 {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
            236 => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    let mut v = self.read_prg_ram_byte(addr);
                    if self.regs[0] == 0x10 {
                        v = (v & 0xF0) | 0x00; // dip switches default to 0
                    }
                    Some(v)
                } else if addr >= 0x8000 {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
            243 => {
                if (addr & 0xC101) == 0x4101 {
                    Some(0xF8 | (self.regs[(self.current & 7) as usize] & 0x07))
                } else if (0x4100..=0x7FFF).contains(&addr) {
                    None
                } else if addr >= 0x8000 {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
            246 => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    Some(self.read_prg_ram_byte(addr))
                } else if addr >= 0x8000 {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
            235 => {
                if addr >= 0x8000 {
                    if self.open_bus {
                        None
                    } else {
                        Some(self.common.read_prg_rom(addr))
                    }
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    Some(self.read_prg_ram_byte(addr))
                } else {
                    None
                }
            }
            _ => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    Some(self.read_prg_ram_byte(addr))
                } else if addr >= 0x8000 {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
        }
    }

    fn cpu_write_impl(&mut self, addr: u16, value: u8) {
        if addr >= 0x8000 {
            self.cpu_write_flag = true;
        }
        match self.id {
            34 => {
                if (0x7FFD..=0x7FFF).contains(&addr) {
                    self.write_reg(addr, value);
                }
                if (0x6000..=0x7FFF).contains(&addr) {
                    self.write_prg_ram_byte(addr, value);
                } else if addr >= 0x8000 {
                    self.write_reg(addr, value);
                }
            }
            35 | 39 | 54 | 57 | 58 | 61 | 62 | 63 | 78 | 94 | 104 | 106 | 107 | 108
            | 144 | 148 | 149 | 152 | 168 | 174 | 177 | 185 | 190 | 193 | 200 | 201 | 202
            | 203 | 204 | 213 | 214 | 218 | 221
            | 225 | 226 | 227 | 228 | 229 | 230 | 231 | 233 | 235 | 236 | 241 | 242
            | 244 | 252 | 253 | 255 => {
                if addr >= 0x8000 {
                    self.write_reg(addr, value);
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    self.write_prg_ram_byte(addr, value);
                }
            }
            95 | 206 | 76 | 154 => {
                if addr >= 0x8000 {
                    self.write_reg(addr, value);
                } else if (0x6000..=0x7FFF).contains(&addr)
                    && self.mmc3.reg_a001 & 0x80 != 0
                    && self.mmc3.reg_a001 & 0x40 == 0
                {
                    self.write_prg_ram_byte(addr, value);
                }
            }
            40 => {
                if addr >= 0x8000 {
                    self.write_reg(addr, value);
                }
            }
            42 => {
                if addr >= 0x8000 {
                    self.write_reg(addr, value);
                }
            }
            43 => {
                if addr >= 0x4020 {
                    self.write_reg(addr, value);
                }
            }
            46 => {
                if addr >= 0x6000 {
                    self.write_reg(addr, value);
                }
            }
            50 => {
                if addr >= 0x4020 {
                    self.write_reg(addr, value);
                }
            }
            51 => {
                if addr >= 0x6000 {
                    self.write_reg(addr, value);
                }
            }
            56 | 64 | 80 | 91 | 96 | 97 | 101 | 112 | 117 | 120 | 132 | 136 | 137 | 138
            | 139 | 141 | 142 | 146 | 147 | 158 | 170 | 171 | 172 | 173 | 175 | 176 | 183 | 207
            | 216 | 220 | 222 | 240 | 246 | 28 | 36 | 38 | 41 | 83 | 125 | 156 | 162 => {
                self.cpu_write_special(addr, value);
            }
            90 | 209 | 211 => {
                if addr >= 0x5000 {
                    if addr >= 0x8000 || (0x5000..=0x5FFF).contains(&addr) {
                        self.write_reg(addr, value);
                    }
                }
            }
            103 => {
                if addr >= 0x6000 {
                    self.write_reg(addr, value);
                }
            }
            163 => {
                if (0x5000..=0x5FFF).contains(&addr) {
                    self.write_reg(addr, value);
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    self.write_prg_ram_byte(addr, value);
                }
            }
            166 | 167 => {
                if addr >= 0x8000 {
                    self.write_reg(addr, value);
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    self.write_prg_ram_byte(addr, value);
                }
            }
            178 => {
                if (0x4800..=0x4FFF).contains(&addr) {
                    self.write_reg(addr, value);
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    let off = self.prg_2k[0] + (addr as usize & 0x1FFF);
                    let len = self.common.prg_ram.len().max(1);
                    self.common.prg_ram[off % len] = value;
                }
            }
            212 => {
                if addr >= 0x8000 {
                    self.write_reg(addr, value);
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    self.write_prg_ram_byte(addr, value);
                }
            }
            234 => {
                if (0xFF80..=0xFF9F).contains(&addr) || (0xFFE8..=0xFFF8).contains(&addr) {
                    self.write_234(addr, value);
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    self.write_prg_ram_byte(addr, value);
                }
            }
            243 => {
                if (0x4100..=0x7FFF).contains(&addr) {
                    self.write_reg(addr, value);
                }
            }
            _ => {
                if addr >= 0x8000 {
                    self.write_reg(addr, value);
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    self.write_prg_ram_byte(addr, value);
                }
            }
        }
    }

    fn cpu_write_special(&mut self, addr: u16, value: u8) {
        match self.id {
            56 | 142 => {
                if addr >= 0x8000 {
                    self.write_reg(addr, value);
                } else if (0x6000..=0x7FFF).contains(&addr) && self.regs[23] == 0xFF {
                    self.write_prg_ram_byte(addr, value);
                }
            }
            64 | 158 | 96 | 97 | 117 | 175 | 222 => {
                if addr >= 0x8000 {
                    self.write_reg(addr, value);
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    self.write_prg_ram_byte(addr, value);
                }
            }
            80 | 207 => {
                if (0x7EF0..=0x7EFF).contains(&addr) {
                    self.write_reg(addr, value);
                } else if (0x6000..=0x7EFF).contains(&addr) {
                    let idx = (addr as usize - 0x6000) & 0xFF;
                    self.common.prg_ram[idx] = value;
                } else if (0x7F00..=0x7FFF).contains(&addr) && self.regs[0] == 0xA3 {
                    self.common.prg_ram[(addr - 0x7F00) as usize & 0xFF] = value;
                }
            }
            91 => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    self.write_reg(addr, value);
                }
            }
            101 => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    self.write_reg(addr, value);
                }
            }
            112 => {
                if addr >= 0x8000 || (0x4020..=0x5FFF).contains(&addr) {
                    self.write_reg(addr, value);
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    self.write_prg_ram_byte(addr, value);
                }
            }
            120 => {
                if addr == 0x41FF {
                    self.write_reg(addr, value);
                }
            }
            132 | 136 | 147 | 172 | 173 => {
                if addr >= 0x8000 || (0x4020..=0x5FFF).contains(&addr) {
                    self.write_reg(addr, value);
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    self.write_prg_ram_byte(addr, value);
                }
            }
            133 | 137 | 138 | 139 | 141 | 145 => {
                if addr >= 0x4100 {
                    self.write_reg(addr, value);
                }
            }
            143 => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    self.write_prg_ram_byte(addr, value);
                }
            }
            146 => {
                if (0x4100..=0x5FFF).contains(&addr) {
                    self.write_reg(addr, value);
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    self.write_prg_ram_byte(addr, value);
                }
            }
            170 => {
                if addr == 0x6502 {
                    self.write_reg(addr, value);
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    self.write_prg_ram_byte(addr, value);
                }
            }
            171 => {
                if addr >= 0xF000 {
                    self.write_reg(addr, value);
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    self.write_prg_ram_byte(addr, value);
                }
            }
            176 => {
                let registers_active = self.fk23_regs_enabled || !self.wram_config_enabled;
                if addr >= 0x8000 {
                    self.write_reg(addr, value);
                } else if registers_active && (addr & 0x5010) == 0x5010 {
                    self.write_reg(addr, value);
                } else if (0x4000..=0x7FFF).contains(&addr) {
                    if self.wram_config_enabled {
                        let bank = if addr < 0x6000 {
                            ((self.regs[3] as usize) + 1) & 0x03
                        } else {
                            self.regs[3] as usize & 0x03
                        };
                        let off = bank * SZ_8K + (addr as usize & 0x1FFF);
                        let len = self.common.prg_ram.len().max(1);
                        self.common.prg_ram[off % len] = value;
                    } else if self.wram_enabled && !self.wram_write_protected && addr >= 0x6000 {
                        self.write_prg_ram_byte(addr, value);
                    }
                }
            }
            179 => {
                if addr >= 0x8000 || (0x5000..=0x5FFF).contains(&addr) {
                    self.write_reg(addr, value);
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    self.write_prg_ram_byte(addr, value);
                }
            }
            183 => {
                if addr >= 0x6000 {
                    self.write_reg(addr, value);
                }
            }
            28 => {
                if (0x5000..=0x5FFF).contains(&addr) || addr >= 0x8000 {
                    self.write_reg(addr, value);
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    self.write_prg_ram_byte(addr, value);
                }
            }
            36 => {
                if addr >= 0x8000 || (0x4020..=0x5FFF).contains(&addr) {
                    self.write_reg(addr, value);
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    self.write_prg_ram_byte(addr, value);
                }
            }
            38 => {
                if (0x7000..=0x7FFF).contains(&addr) {
                    self.write_reg(addr, value);
                } else if (0x6000..=0x6FFF).contains(&addr) {
                    self.write_prg_ram_byte(addr, value);
                }
            }
            41 => {
                if addr <= 0x67FF || addr >= 0x8000 {
                    self.write_reg(addr, value);
                } else if (0x6800..=0x7FFF).contains(&addr) {
                    self.write_prg_ram_byte(addr, value);
                }
            }
            83 => {
                if addr >= 0x8000 || (0x5100..=0x5103).contains(&addr) {
                    self.write_reg(addr, value);
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    self.write_prg_ram_byte(addr, value);
                }
            }
            125 => {
                if addr == 0x6000 {
                    self.write_reg(addr, value);
                } else if (0xC000..=0xDFFF).contains(&addr) {
                    self.write_prg_ram_byte((addr & 0x1FFF) | 0x6000, value);
                }
            }
            156 => {
                if (0xC000..=0xC014).contains(&addr) {
                    self.write_reg(addr, value);
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    self.write_prg_ram_byte(addr, value);
                }
            }
            162 => {
                if (0x5000..=0x5FFF).contains(&addr) {
                    self.write_reg(addr, value);
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    self.write_prg_ram_byte(addr, value);
                }
            }
            216 => {
                if addr == 0x5000 || addr >= 0x8000 {
                    self.write_reg(addr, value);
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    self.write_prg_ram_byte(addr, value);
                }
            }
            220 => {
                if addr >= 0x8000 {
                    self.write_reg(addr, value);
                }
            }
            240 => {
                if (0x4020..=0x5FFF).contains(&addr) {
                    self.write_reg(addr, value);
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    self.write_prg_ram_byte(addr, value);
                }
            }
            246 => {
                if (0x6000..=0x67FF).contains(&addr) {
                    self.write_reg(addr, value);
                } else if (0x6800..=0x7FFF).contains(&addr) {
                    self.write_prg_ram_byte(addr, value);
                }
            }
            _ => {
                if addr >= 0x8000 {
                    self.write_reg(addr, value);
                } else if (0x6000..=0x7FFF).contains(&addr) {
                    self.write_prg_ram_byte(addr, value);
                }
            }
        }
    }

    // -----------------------------------------------------------------
    // PPU bus / IRQ clocks
    // -----------------------------------------------------------------

    fn ppu_access(&mut self, addr: u16) {
        // IRQ source that fires on PPU pattern fetches.
        match self.id {
            91 | 95 | 206 => {
                let fc = self.frame_cycle();
                self.mmc3.notify_a12(&mut self.common, addr, fc);
            }
            64 | 158 => {
                if self.irq_mode & 1 == 0
                    && self.a12.update(addr, self.frame_cycle(), 30) == A12Change::Rise
                {
                    self.rambo1_clock_irq(2);
                }
            }
            35 => {
                if self.a12.update(addr, self.frame_cycle(), 10) == A12Change::Rise
                    && self.irq_enabled
                {
                    self.irq_counter = self.irq_counter.wrapping_sub(1) & 0xFF;
                    if self.irq_counter == 0 {
                        self.irq_enabled = false;
                        self.common.irq_pending = true;
                    }
                }
            }
            117 => {
                if self.a12.update(addr, self.frame_cycle(), 10) == A12Change::Rise
                    && self.irq_enabled
                    && self.irq_alt
                    && self.irq_counter != 0
                {
                    self.irq_counter = self.irq_counter.wrapping_sub(1) & 0xFF;
                    if self.irq_counter == 0 {
                        self.common.irq_pending = true;
                        self.irq_alt = false;
                    }
                }
            }
            222 => {
                if self.a12.update(addr, self.frame_cycle(), 10) == A12Change::Rise
                    && self.irq_counter != 0
                {
                    self.irq_counter = self.irq_counter.wrapping_add(1);
                    if self.irq_counter >= 240 {
                        self.common.irq_pending = true;
                        self.irq_counter = 0;
                    }
                }
            }
            176 => {
                if self.a12.update(addr, self.frame_cycle(), 10) == A12Change::Rise {
                    if self.irq_counter == 0 || self.irq_reload_flag {
                        self.irq_counter = self.irq_reload;
                    } else {
                        self.irq_counter = self.irq_counter.wrapping_sub(1);
                    }
                    if self.irq_counter == 0 && self.irq_enabled {
                        self.irq_delay = 2;
                    }
                    self.irq_reload_flag = false;
                }
            }
            90 | 209 | 211 => {
                if self.id == 209 {
                    match addr & 0x2FF8 {
                        0x0FD8 | 0x0FE8 => {
                            let latch = ((addr >> 4)
                                & (((addr >> 10) & 0x04) | 0x02))
                                as u8;
                            let idx = ((addr >> 12) & 1) as usize;
                            self.regs[20 + idx] = latch;
                            self.recalc();
                        }
                        _ => {}
                    }
                }
                if self.jy_irq_source == 1 {
                    if self.last_ppu_addr & 0x1000 == 0 && addr & 0x1000 != 0 {
                        self.jy_tick_irq();
                    }
                } else if self.jy_irq_source == 2 {
                    self.jy_tick_irq();
                }
                self.last_ppu_addr = addr;
            }
            96 => {
                if (self.last_ppu_addr & 0x3000) != 0x2000 && (addr & 0x3000) == 0x2000 {
                    self.regs[1] = ((addr >> 8) & 0x03) as u8;
                    self.recalc();
                }
                self.last_ppu_addr = addr;
            }
            _ => {}
        }
    }

    fn jy_tick_irq(&mut self) {
        let mask: u8 = if self.irq_small { 0x07 } else { 0xFF };
        let mut prescaler = self.irq_prescaler as u8 & mask;
        let mut clock = false;
        match self.irq_dir {
            1 => {
                prescaler = prescaler.wrapping_add(1);
                if prescaler & mask == 0 {
                    clock = true;
                }
            }
            2 => {
                prescaler = prescaler.wrapping_sub(1);
                if prescaler == 0 {
                    clock = true;
                }
            }
            _ => {}
        }
        self.irq_prescaler =
            (self.irq_prescaler & !(mask as i16)) | ((prescaler & mask) as i16);
        if clock {
            match self.irq_dir {
                1 => {
                    self.irq_counter = (self.irq_counter + 1) & 0xFF;
                    if self.irq_counter == 0 && self.irq_enabled {
                        self.common.irq_pending = true;
                    }
                }
                2 => {
                    self.irq_counter = (self.irq_counter.wrapping_sub(1)) & 0xFF;
                    if self.irq_counter == 0xFF && self.irq_enabled {
                        self.common.irq_pending = true;
                    }
                }
                _ => {}
            }
        }
    }

    fn rambo1_clock_irq(&mut self, delay: u8) {
        if self.irq_reload_flag {
            if self.irq_reload <= 1 {
                self.irq_counter = self.irq_reload + 1;
            } else {
                self.irq_counter = self.irq_reload + 2;
            }
            self.irq_reload_flag = false;
        } else if self.irq_counter == 0 {
            self.irq_counter = self.irq_reload + 1;
        }
        self.irq_counter = self.irq_counter.wrapping_sub(1) & 0xFF;
        if self.irq_counter == 0 && self.irq_enabled {
            self.irq_delay = delay;
        }
    }

    fn cpu_cycle_impl(&mut self) {
        self.clock = self.clock.wrapping_add(1);
        match self.id {
            35 => {}
            40 => {
                if self.irq_counter > 0 {
                    self.irq_counter -= 1;
                    if self.irq_counter == 0 {
                        self.common.irq_pending = true;
                    }
                }
            }
            42 => {
                if self.irq_enabled {
                    self.irq_counter += 1;
                    if self.irq_counter >= 0x8000 {
                        self.irq_counter -= 0x8000;
                    }
                    self.common.irq_pending = self.irq_counter >= 0x6000;
                }
            }
            43 => {
                if self.irq_enabled {
                    self.irq_counter += 1;
                    if self.irq_counter >= 4096 {
                        self.irq_enabled = false;
                        self.common.irq_pending = true;
                    }
                }
            }
            50 => {
                if self.irq_enabled {
                    self.irq_counter += 1;
                    if self.irq_counter == 0x1000 {
                        self.common.irq_pending = true;
                        self.irq_enabled = false;
                    }
                }
            }
            56 | 142 => {
                if self.irq_enabled {
                    self.irq_counter = self.irq_counter.wrapping_add(1);
                    if self.irq_counter == 0xFFFF {
                        self.irq_counter = self.irq_reload;
                        self.common.irq_pending = true;
                    }
                }
            }
            64 | 158 => {
                if self.irq_delay > 0 {
                    self.irq_delay -= 1;
                    if self.irq_delay == 0 {
                        self.common.irq_pending = true;
                    }
                }
                if self.irq_mode & 1 != 0 || self.need_irq {
                    self.irq_prescaler = (self.irq_prescaler + 1) & 0x03;
                    if self.irq_prescaler == 0 {
                        self.rambo1_clock_irq(1);
                        self.need_irq = false;
                    }
                }
            }
            90 | 209 | 211 => {
                let source = self.jy_irq_source;
                if source == 0 || (source == 3 && self.cpu_write_flag) {
                    self.jy_tick_irq();
                }
                if source == 3 {
                    self.cpu_write_flag = false;
                }
            }
            168 => {
                self.irq_counter = self.irq_counter.wrapping_sub(1);
                if self.irq_counter == 0 {
                    self.irq_counter = 1024;
                    self.common.irq_pending = true;
                }
            }
            83 => {
                if self.irq_enabled {
                    self.irq_counter = self.irq_counter.wrapping_sub(1);
                    if self.irq_counter == 0 {
                        self.irq_enabled = false;
                        self.irq_counter = 0xFFFF;
                        self.common.irq_pending = true;
                    }
                }
            }
            106 => {
                if self.irq_enabled {
                    self.irq_counter = self.irq_counter.wrapping_add(1);
                    if self.irq_counter == 0 {
                        self.common.irq_pending = true;
                        self.irq_enabled = false;
                    }
                }
            }
            183 => {
                if self.need_irq {
                    self.common.irq_pending = true;
                    self.need_irq = false;
                }
                self.irq_prescaler += 1;
                if self.irq_prescaler == 114 {
                    self.irq_prescaler = 0;
                    if self.irq_enabled {
                        self.irq_counter = (self.irq_counter + 1) & 0xFF;
                        if self.irq_counter == 0 {
                            self.need_irq = true;
                        }
                    }
                }
            }
            176 => {
                if self.irq_delay > 0 {
                    self.irq_delay -= 1;
                    if self.irq_delay == 0 {
                        self.common.irq_pending = true;
                    }
                }
            }
            252 => {
                if self.irq_enabled {
                    self.irq_prescaler -= 3;
                    if self.irq_mode & 0x04 != 0 || self.irq_prescaler <= 0 {
                        if self.irq_counter == 0xFF {
                            self.irq_counter = self.irq_reload;
                            self.common.irq_pending = true;
                        } else {
                            self.irq_counter = (self.irq_counter + 1) & 0xFF;
                        }
                        self.irq_prescaler += 341;
                    }
                }
            }
            253 => {
                if self.irq_enabled {
                    self.irq_prescaler += 1;
                    if self.irq_prescaler >= 114 {
                        self.irq_prescaler = 0;
                        self.irq_counter = (self.irq_counter + 1) & 0xFF;
                        if self.irq_counter == 0 {
                            self.irq_counter = self.irq_reload & 0xFF;
                            self.common.irq_pending = true;
                        }
                    }
                }
            }
            91 | 95 | 206 | 117 | 222 => {}
            _ => {}
        }
    }

    // -----------------------------------------------------------------
    // Save state helpers
    // -----------------------------------------------------------------

    fn save_extra(&self, w: &mut StateWriter) {
        w.bytes(&self.regs);
        w.bytes(&self.fk_regs);
        w.u16(self.prg_base_bits);
        w.u8(self.current);
        w.bool(self.cpu_write_flag);
        w.u16(self.irq_reload);
        w.u16(self.irq_counter);
        w.i16(self.irq_prescaler);
        w.u8(self.irq_delay);
        w.u8(self.irq_mode);
        w.u8(self.irq_dir);
        w.u8(self.irq_xor);
        w.bool(self.irq_enabled);
        w.bool(self.irq_alt);
        w.bool(self.irq_reload_flag);
        w.bool(self.need_irq);
        w.bool(self.irq_small);
        w.bool(self.open_bus);
        w.bool(self.enable_mirroring_control);
        w.bool(self.prg_ram_disabled);
        w.bool(self.auto_chr);
        w.bool(self.toggle);
        w.bool(self.contra_mode);
        w.bool(self.reset_toggle);
        w.bool(self.use_outer_bank);
        w.bool(self.select_chr_ram);
        w.bool(self.mmc3_chr_mode);
        w.bool(self.cnrom_chr_mode);
        w.bool(self.extended_mmc3);
        w.bool(self.ram_in_first_chr_bank);
        w.bool(self.allow_single_screen_mirroring);
        w.bool(self.fk23_regs_enabled);
        w.bool(self.wram_config_enabled);
        w.bool(self.wram_enabled);
        w.bool(self.wram_write_protected);
        w.bool(self.invert_prg_a14);
        w.bool(self.invert_chr_a12);
        w.fixed_bytes(&self.nt_map);
        w.bool(self.custom_nt);
        for v in self.prg_2k {
            w.u32(v as u32);
        }
        for v in self.chr_pages {
            w.u16(v);
        }
        w.bytes(&self.chr_ram);
        w.u8(self.chr_ram_mask);
        w.u16(self.last_ppu_addr);
        w.u32(self.clock);
        w.i16(self.scanline);
        w.u8(self.jy_irq_source);
        w.u32(self.a12.last_cycle);
        w.u32(self.a12.cycles_down);
        self.mmc3.save(w);
        self.txc.save(w);
    }

    fn load_extra(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        let regs = r.bytes()?;
        if regs.len() != self.regs.len() {
            return Err("misc mapper register block size mismatch".into());
        }
        self.regs.copy_from_slice(&regs);
        let fk = r.bytes()?;
        if fk.len() != 12 {
            return Err("FK23C register block size mismatch".into());
        }
        self.fk_regs.copy_from_slice(&fk);
        self.prg_base_bits = r.u16()?;
        self.current = r.u8()?;
        self.cpu_write_flag = r.bool()?;
        self.irq_reload = r.u16()?;
        self.irq_counter = r.u16()?;
        self.irq_prescaler = r.i16()?;
        self.irq_delay = r.u8()?;
        self.irq_mode = r.u8()?;
        self.irq_dir = r.u8()?;
        self.irq_xor = r.u8()?;
        self.irq_enabled = r.bool()?;
        self.irq_alt = r.bool()?;
        self.irq_reload_flag = r.bool()?;
        self.need_irq = r.bool()?;
        self.irq_small = r.bool()?;
        self.open_bus = r.bool()?;
        self.enable_mirroring_control = r.bool()?;
        self.prg_ram_disabled = r.bool()?;
        self.auto_chr = r.bool()?;
        self.toggle = r.bool()?;
        self.contra_mode = r.bool()?;
        self.reset_toggle = r.bool()?;
        self.use_outer_bank = r.bool()?;
        self.select_chr_ram = r.bool()?;
        self.mmc3_chr_mode = r.bool()?;
        self.cnrom_chr_mode = r.bool()?;
        self.extended_mmc3 = r.bool()?;
        self.ram_in_first_chr_bank = r.bool()?;
        self.allow_single_screen_mirroring = r.bool()?;
        self.fk23_regs_enabled = r.bool()?;
        self.wram_config_enabled = r.bool()?;
        self.wram_enabled = r.bool()?;
        self.wram_write_protected = r.bool()?;
        self.invert_prg_a14 = r.bool()?;
        self.invert_chr_a12 = r.bool()?;
        let nt = r.fixed_bytes(4)?;
        self.nt_map.copy_from_slice(&nt);
        self.custom_nt = r.bool()?;
        for v in &mut self.prg_2k {
            *v = r.u32()? as usize;
        }
        for v in &mut self.chr_pages {
            *v = r.u16()?;
        }
        let chr_ram = r.bytes()?;
        if chr_ram.len() != self.chr_ram.len() {
            return Err("misc mapper CHR RAM size mismatch".into());
        }
        self.chr_ram.copy_from_slice(&chr_ram);
        self.chr_ram_mask = r.u8()?;
        self.last_ppu_addr = r.u16()?;
        self.clock = r.u32()?;
        self.scanline = r.i16()?;
        self.jy_irq_source = r.u8()?;
        self.a12.last_cycle = r.u32()?;
        self.a12.cycles_down = r.u32()?;
        self.mmc3.load(r)?;
        self.txc.load(r)?;
        self.recalc();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Mapper trait implementation
// ---------------------------------------------------------------------------

impl Mapper for MiscBoard {
    fn common(&self) -> &MapperCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }

    fn mapper_id(&self) -> u16 {
        self.id
    }

    fn reset(&mut self) {
        self.init();
    }

    fn cpu_read(&mut self, addr: u16) -> Option<u8> {
        self.cpu_read_impl(addr)
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        self.cpu_write_impl(addr, value);
    }

    fn ppu_read(&mut self, addr: u16) -> Option<u8> {
        let a = addr & 0x3FFF;
        self.ppu_access(a);
        match self.id {
            176 | 253 => {
                if a < 0x2000 {
                    if let Some(v) = self.read_chr_special(a) {
                        return Some(v);
                    }
                }
            }
            218 => {
                if a < 0x2000 {
                    let mask: usize = match self.common.mirroring {
                        Mirroring::Vertical => 0x400,
                        Mirroring::Horizontal => 0x800,
                        Mirroring::ScreenAOnly => 0x1000,
                        Mirroring::ScreenBOnly => 0x2000,
                        Mirroring::FourScreens => 0x1000,
                    };
                    let bank = if (a as usize) & mask != 0 { 1 } else { 0 };
                    return Some(self.common.nametables[bank * 0x400 + (a as usize & 0x3FF)]);
                }
            }
            90 | 209 | 211 => {
                if a >= 0x2000 && a <= 0x3EFF {
                    let advanced = self.regs[27] & 0x01 != 0 || self.id == 211;
                    if advanced && self.id != 90 {
                        let idx = (((a as usize) & 0x2FFF) - 0x2000) / 0x400;
                        let disable = self.regs[27] & 0x02 != 0;
                        if disable
                            || (self.regs[28 + idx] & 0x80) != (self.regs[23] & 0x80)
                        {
                            let page = self.regs[28 + idx] as usize
                                | ((self.regs[32 + idx] as usize) << 8);
                            let off = page * 0x400 + (a as usize & 0x3FF);
                            return Some(if self.has_chr_rom && off < self.common.chr.len() {
                                self.common.chr[off]
                            } else {
                                0
                            });
                        }
                    }
                }
            }
            _ => {}
        }
        match a {
            0x0000..=0x1FFF => Some(self.common.read_chr(a)),
            0x2000..=0x3EFF => {
                if self.custom_nt {
                    Some(read_nt_map(&self.common, &self.nt_map, a))
                } else {
                    Some(self.common.read_nametable(a))
                }
            }
            _ => None,
        }
    }

    fn ppu_write(&mut self, addr: u16, value: u8) {
        let a = addr & 0x3FFF;
        self.ppu_access(a);
        match self.id {
            176 | 253 => {
                if a < 0x2000 && self.write_chr_special(a, value) {
                    return;
                }
                if a >= 0x2000 && a <= 0x3EFF && self.custom_nt {
                    write_nt_map(&mut self.common, &self.nt_map, a, value);
                    return;
                }
            }
            218 => {
                if a < 0x2000 {
                    let mask: usize = match self.common.mirroring {
                        Mirroring::Vertical => 0x400,
                        Mirroring::Horizontal => 0x800,
                        Mirroring::ScreenAOnly => 0x1000,
                        Mirroring::ScreenBOnly => 0x2000,
                        Mirroring::FourScreens => 0x1000,
                    };
                    let bank = if (a as usize) & mask != 0 { 1 } else { 0 };
                    self.common.nametables[bank * 0x400 + (a as usize & 0x3FF)] = value;
                    return;
                }
            }
            _ => {}
        }
        match a {
            0x0000..=0x1FFF => self.common.write_chr(a, value),
            0x2000..=0x3EFF => {
                if self.custom_nt {
                    write_nt_map(&mut self.common, &self.nt_map, a, value);
                } else {
                    self.common.write_nametable(a, value);
                }
            }
            _ => {}
        }
    }

    fn scanline(&mut self, scanline: i16) {
        self.scanline = scanline;
        if self.id == 163 && self.auto_chr {
            if scanline == 128 {
                map_chr(&mut self.common, 0, 1, SZ_4K);
                map_chr(&mut self.common, 1, 1, SZ_4K);
            } else if scanline == 240 {
                map_chr(&mut self.common, 0, 0, SZ_4K);
                map_chr(&mut self.common, 1, 0, SZ_4K);
            }
        }
    }

    fn cpu_cycle(&mut self) {
        self.cpu_cycle_impl();
    }

    fn irq_pending(&self) -> bool {
        self.common.irq_pending
    }

    fn battery(&self) -> Option<&[u8]> {
        if self.id == 168 {
            return Some(&self.common.chr);
        }
        if matches!(self.id, 80 | 207) {
            if self.common.info.has_battery {
                return Some(&self.common.prg_ram[..0x100.min(self.common.prg_ram.len())]);
            }
            return None;
        }
        if self.common.info.has_battery {
            Some(&self.common.prg_ram)
        } else {
            None
        }
    }

    fn set_battery(&mut self, data: &[u8]) {
        if self.id == 168 {
            let n = data.len().min(self.common.chr.len());
            self.common.chr[..n].copy_from_slice(&data[..n]);
            return;
        }
        if matches!(self.id, 80 | 207) {
            let n = data.len().min(0x100).min(self.common.prg_ram.len());
            self.common.prg_ram[..n].copy_from_slice(&data[..n]);
            return;
        }
        let n = data.len().min(self.common.prg_ram.len());
        self.common.prg_ram[..n].copy_from_slice(&data[..n]);
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);
        self.save_extra(w);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        self.load_extra(r)
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

const SUPPORTED_IDS: &[u16] = &[
    28, 34, 35, 36, 38, 39, 40, 41, 42, 43, 46, 50, 51, 54, 56, 57, 58, 61, 62, 63, 64,
    76, 78, 80, 83, 90, 91, 95, 96, 97, 101, 103, 104, 106, 107, 108, 112, 117, 120,
    125, 132, 133, 136, 137, 138, 139, 141, 142, 143, 145, 146, 147, 148, 149, 152,
    154, 156, 158, 162, 163, 166, 167, 168, 170, 171, 172, 173, 174, 175, 176, 177, 178,
    179, 183, 185, 190, 193, 200, 201, 202, 203, 204, 206, 207, 209, 211, 212, 213, 214,
    216, 218, 220, 221, 222, 225, 226, 227, 228, 229, 230, 231, 233, 234, 235, 236, 240,
    241, 242, 243, 244, 246, 252, 253, 255,
];

pub fn supports(id: u16) -> bool {
    SUPPORTED_IDS.contains(&id)
}

pub fn create(id: u16, rom: RomData) -> Option<Box<dyn Mapper>> {
    if supports(id) {
        Some(Box::new(MiscBoard::new(id, rom)))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Region, RomInfo, RomSystem};

    fn rom(id: u16, prg_kb: usize, chr_kb: usize, mirroring: Mirroring) -> RomData {
        let info = RomInfo {
            mapper: id,
            submapper: 0,
            region: Region::Ntsc,
            mirroring,
            has_battery: false,
            four_screen: false,
            is_nes2: false,
            system: RomSystem::Nes,
            prg_rom_size: prg_kb * 1024,
            chr_rom_size: chr_kb * 1024,
            prg_ram_size: 0x2000,
            chr_ram_size: if chr_kb == 0 { 0x2000 } else { 0 },
            prg_nvram_size: 0,
            chr_nvram_size: 0,
        };
        let mut prg = vec![0u8; prg_kb * 1024];
        for (i, b) in prg.iter_mut().enumerate() {
            *b = (i / 0x2000) as u8;
        }
        let chr = vec![0u8; chr_kb * 1024];
        RomData::new(info, prg, chr)
    }

    fn write(m: &mut MiscBoard, addr: u16, value: u8) {
        m.cpu_write(addr, value);
    }

    fn read(m: &mut MiscBoard, addr: u16) -> u8 {
        m.cpu_read(addr).unwrap_or(0)
    }

    #[test]
    fn mapper34_nina_and_bnrom() {
        let mut nina = MiscBoard::new(34, rom(34, 64, 32, Mirroring::Vertical));
        assert_eq!(read(&mut nina, 0x8000), 0);
        write(&mut nina, 0x7FFD, 1);
        assert_eq!(read(&mut nina, 0x8000), 4);
        write(&mut nina, 0x7FFE, 3);
        write(&mut nina, 0x7FFF, 5);
        assert_eq!(nina.common.chr_map[0], 3 * 0x1000);
        assert_eq!(nina.common.chr_map[4], 5 * 0x1000);
        // Writes through $7FFD also hit PRG RAM.
        assert_eq!(nina.common.prg_ram[0x1FFD], 1);

        let mut bn = MiscBoard::new(34, rom(34, 128, 0, Mirroring::Vertical));
        write(&mut bn, 0x8000, 3);
        assert_eq!(read(&mut bn, 0x8000), 12);
    }

    #[test]
    fn mapper97_irem_tam_s1() {
        let mut m = MiscBoard::new(97, rom(97, 64, 32, Mirroring::Horizontal));
        assert_eq!(read(&mut m, 0x8000), 6); // both 16KB slots last page
        write(&mut m, 0x8000, 0x00);
        assert_eq!(m.common.mirroring, Mirroring::ScreenAOnly);
        assert_eq!(read(&mut m, 0xC000), 0);
    }

    #[test]
    fn mapper101_jaleco_ordered() {
        let mut m = MiscBoard::new(101, rom(101, 32, 128, Mirroring::Vertical));
        write(&mut m, 0x6000, 5);
        assert_eq!(m.common.chr_map[0], 5 * 0x2000);
    }

    #[test]
    fn mapper107() {
        let mut m = MiscBoard::new(107, rom(107, 256, 128, Mirroring::Vertical));
        write(&mut m, 0x8000, 0x05);
        assert_eq!(read(&mut m, 0x8000), 0x10);
        assert_eq!(m.common.chr_map[0], 5 * 0x2000);
    }

    #[test]
    fn mapper148_bus_conflicts() {
        let mut m = MiscBoard::new(148, rom(148, 64, 64, Mirroring::Vertical));
        // PRG byte at $8000 is 0, so the bank write is masked to 0.
        write(&mut m, 0x8000, 0x05);
        assert_eq!(m.common.chr_map[0], 0);
    }

    #[test]
    fn mapper152_bandai() {
        let mut m = MiscBoard::new(152, rom(152, 128, 64, Mirroring::Vertical));
        write(&mut m, 0x8000, 0x30);
        assert_eq!(read(&mut m, 0x8000), 6);
        assert_eq!(m.common.mirroring, Mirroring::ScreenAOnly);
        write(&mut m, 0x8000, 0xB2);
        assert_eq!(m.common.mirroring, Mirroring::ScreenBOnly);
        assert_eq!(m.common.chr_map[0], 2 * 0x2000);
    }

    #[test]
    fn mapper163_nanjing_register_read() {
        let mut m = MiscBoard::new(163, rom(163, 1024, 64, Mirroring::Vertical));
        write(&mut m, 0x5000, 0x81);
        write(&mut m, 0x5200, 0x0F);
        assert_eq!(read(&mut m, 0x8000), 68);
        let v = read(&mut m, 0x5100);
        assert_eq!(v, 0x81 | 0x00 | (0x0F ^ 0xFF));
    }

    #[test]
    fn mapper168_racermate_irq() {
        let mut m = MiscBoard::new(168, rom(168, 64, 0, Mirroring::Vertical));
        write(&mut m, 0xC000, 0);
        for _ in 0..1023 {
            m.cpu_cycle();
        }
        assert!(!m.irq_pending());
        m.cpu_cycle();
        assert!(m.irq_pending());
    }

    #[test]
    fn mapper185_cnrom_bus_conflict() {
        let mut m = MiscBoard::new(185, rom(185, 32, 64, Mirroring::Vertical));
        // power-on PRG byte at $8000 is 0 -> all writes masked to 0
        write(&mut m, 0x8000, 3);
        assert_eq!(m.common.chr_map[0], 0);
    }

    #[test]
    fn mapper213_214() {
        let mut m = MiscBoard::new(213, rom(213, 64, 64, Mirroring::Vertical));
        write(&mut m, 0x8000, 0);
        assert_eq!(read(&mut m, 0x8000), 0);
        let mut m2 = MiscBoard::new(214, rom(214, 64, 64, Mirroring::Vertical));
        write(&mut m2, 0x8000 | (2 << 2) | 1, 0);
        assert_eq!(m2.common.chr_map[0], 0x2000);
    }

    #[test]
    fn mapper90_jycompany_prg_and_irq() {
        let mut m = MiscBoard::new(90, rom(90, 512, 128, Mirroring::Vertical));
        // 4 independent 8KB banks, $6000 ROM mapping enabled
        write(&mut m, 0xD000, 0x86);
        write(&mut m, 0x8000, 5);
        write(&mut m, 0x8001, 6);
        write(&mut m, 0x8002, 7);
        write(&mut m, 0x8003, 8);
        assert_eq!(read(&mut m, 0x8000), 5);
        assert_eq!(read(&mut m, 0xA000), 6);
        assert_eq!(read(&mut m, 0xC000), 7);
        assert_eq!(read(&mut m, 0xE000), 8);
        assert_eq!(read(&mut m, 0x6000), 8); // prgRegs[3]

        // CPU-clock IRQ: direction=up, 8-bit counter
        write(&mut m, 0xC001, 0x40);
        write(&mut m, 0xC004, 0xFF);
        write(&mut m, 0xC005, 0xFF);
        write(&mut m, 0xC000, 0x01);
        m.cpu_cycle();
        assert!(m.irq_pending());
    }

    #[test]
    fn mapper64_rambo1_banks_and_a12_irq() {
        let mut m = MiscBoard::new(64, rom(64, 256, 128, Mirroring::Vertical));
        write(&mut m, 0x8000, 0x00);
        write(&mut m, 0x8001, 0x04);
        assert_eq!(m.common.chr_map[0], 4 * 0x400);
        assert_eq!(m.common.chr_map[1], 5 * 0x400);
        write(&mut m, 0x8000, 0x06);
        write(&mut m, 0x8001, 0x0A);
        write(&mut m, 0x8000, 0x40);
        write(&mut m, 0x8001, 0x00);
        assert_eq!(m.common.prg_map[1], 0x0A * 0x2000);

        // A12-driven IRQ
        write(&mut m, 0xC000, 0x01);
        write(&mut m, 0xC001, 0x00);
        write(&mut m, 0xE001, 0x00);
        for _ in 0..3 {
            let _ = m.ppu_read(0x0000);
            for _ in 0..40 {
                m.cpu_cycle();
            }
            let _ = m.ppu_read(0x1000);
        }
        m.cpu_cycle();
        m.cpu_cycle();
        assert!(m.irq_pending());
    }

    #[test]
    fn mapper95_namco108() {
        let mut m = MiscBoard::new(95, rom(95, 256, 256, Mirroring::Horizontal));
        assert_eq!(read(&mut m, 0xC000), 30); // last 8KB page
        write(&mut m, 0x8000, 0x06); // select PRG register 6
        write(&mut m, 0x8001, 0x05);
        assert_eq!(read(&mut m, 0x8000), 5);
        assert_eq!(m.common.mirroring, Mirroring::Horizontal);
        // Namco 108 CHR register 0/1 are 2KB + 1KB windows
        write(&mut m, 0x8000, 0x80);
        write(&mut m, 0x8001, 0x04);
        assert_eq!(m.common.chr_map[0], 4 * 0x400);
        assert_eq!(m.common.chr_map[1], 5 * 0x400);
    }

    #[test]
    fn mapper220_kaiser7057() {
        let mut m = MiscBoard::new(220, rom(220, 256, 128, Mirroring::Vertical));
        write(&mut m, 0xB000, 0x03);
        write(&mut m, 0xB001, 0x02);
        assert_eq!(read(&mut m, 0x8000), 8);
        // $6000-$7FFF is a banked 2KB PRG window
        write(&mut m, 0xD000, 0x05);
        write(&mut m, 0xD001, 0x00);
        assert_eq!(read(&mut m, 0x6000), 1);
        // $A000-$BFFF fixed at 2KB pages $34-$37
        assert_eq!(read(&mut m, 0xA000), 0x34 / 4);
    }

    #[test]
    fn mapper207_taito_x1005() {
        let mut m = MiscBoard::new(207, rom(207, 256, 256, Mirroring::Vertical));
        write(&mut m, 0x7EFA, 0x05);
        assert_eq!(read(&mut m, 0x8000), 5);
        write(&mut m, 0x7EF0, 0x40);
        assert_eq!(m.common.chr_map[0], 0x40 * 0x400);
        assert_eq!(m.common.chr_map[1], 0x41 * 0x400);
        write(&mut m, 0x7EF0, 0xC0);
        assert_eq!(m.nt_map[0], 1);
        assert_eq!(m.nt_map[1], 1);
        // RAM is locked by default
        assert!(m.cpu_read(0x7F00).is_none());
        write(&mut m, 0x7EF8, 0xA3);
        write(&mut m, 0x7F00, 0x77);
        assert_eq!(m.cpu_read(0x7F00), Some(0x77));
    }

    #[test]
    fn mapper252_waixing_vrc_irq() {
        let mut m = MiscBoard::new(252, rom(252, 256, 256, Mirroring::Vertical));
        write(&mut m, 0x8000, 0x05);
        assert_eq!(read(&mut m, 0x8000), 5);
        write(&mut m, 0xB000, 0x0A);
        write(&mut m, 0xB004, 0x0B);
        assert_eq!(m.common.chr_map[0], 0xBA * 0x400);
        // VRC IRQ, cycle mode off, reload=0xFF -> first clock fires
        write(&mut m, 0xF000, 0x0F);
        write(&mut m, 0xF004, 0x0F);
        write(&mut m, 0xF008, 0x02);
        for _ in 0..114 {
            m.cpu_cycle();
        }
        assert!(m.irq_pending());
        write(&mut m, 0xF00C, 0);
        assert!(!m.irq_pending());
    }

    #[test]
    fn mapper176_fk23c() {
        let mut m = MiscBoard::new(176, rom(176, 512, 256, Mirroring::Vertical));
        write(&mut m, 0x8000, 0x00);
        write(&mut m, 0x8001, 0x06); // CHR reg 0
        assert_eq!(m.common.chr_map[0], 6 * 0x400);
        write(&mut m, 0xA000, 0x00);
        assert_eq!(m.common.mirroring, Mirroring::Vertical);
        write(&mut m, 0x8000, 0x46); // invert PRG A14, select reg 6
        write(&mut m, 0x8001, 0x05);
        assert_eq!(m.common.prg_map[2], 5 * 0x2000);
        write(&mut m, 0xA000, 0x01);
        assert_eq!(m.common.mirroring, Mirroring::Horizontal);
    }

    #[test]
    fn mapper41_caltron() {
        let mut m = MiscBoard::new(41, rom(41, 256, 128, Mirroring::Vertical));
        write(&mut m, 0x6004, 0);
        assert_eq!(read(&mut m, 0x8000), 16);
        write(&mut m, 0x8000, 0x02);
        assert_eq!(m.common.chr_map[0], 2 * 0x2000);
    }

    #[test]
    fn mapper125_lh32() {
        let mut m = MiscBoard::new(125, rom(125, 128, 64, Mirroring::Vertical));
        write(&mut m, 0x6000, 0x03);
        assert_eq!(read(&mut m, 0x6000), 3);
        write(&mut m, 0xC000, 0xAA);
        assert_eq!(read(&mut m, 0xC000), 0xAA);
    }

    #[test]
    fn mapper63_open_bus() {
        let mut m = MiscBoard::new(63, rom(63, 256, 128, Mirroring::Vertical));
        write(&mut m, 0x8300, 0);
        assert!(m.cpu_read(0x8000).is_none());
        assert_eq!(read(&mut m, 0xC000), (0x02 & 0x1FC) as u8);
        write(&mut m, 0x8000, 0);
        assert!(m.cpu_read(0x8000).is_some());
    }

    #[test]
    fn all_supported_ids_smoke() {
        let writes: &[(u16, u8)] = &[
            (0x41FF, 0x05),
            (0x4800, 0x03),
            (0x5000, 0x81),
            (0x5101, 0x06),
            (0x5800, 0x07),
            (0x6000, 0x02),
            (0x6502, 0x40),
            (0x6800, 0x03),
            (0x7000, 0x05),
            (0x7777, 0x01),
            (0x7EF0, 0x80),
            (0x7EF8, 0xA3),
            (0x7F00, 0x55),
            (0x8000, 0x21),
            (0x8001, 0x03),
            (0x9000, 0x04),
            (0x9400, 0x02),
            (0xA000, 0x05),
            (0xA001, 0xC0),
            (0xB000, 0x06),
            (0xB004, 0x07),
            (0xC000, 0x08),
            (0xC001, 0x09),
            (0xD000, 0x86),
            (0xD003, 0x01),
            (0xE000, 0x00),
            (0xE001, 0x01),
            (0xF000, 0x0A),
            (0xF004, 0x0B),
            (0xF008, 0x02),
            (0xF00C, 0x00),
            (0xFFFC, 0x0C),
        ];
        for &id in SUPPORTED_IDS {
            for &(chr_kb, prg_kb) in &[(0usize, 128usize), (128, 512)] {
                let mut m = MiscBoard::new(id, rom(id, prg_kb, chr_kb, Mirroring::Vertical));
                for &(addr, value) in writes {
                    m.cpu_write(addr, value);
                }
                for addr in [0x6000u16, 0x7000, 0x8000, 0xA000, 0xC000, 0xE000, 0xFFFC] {
                    let _ = m.cpu_read(addr);
                }
                for addr in [0x0000u16, 0x0400, 0x1000, 0x2000, 0x2400, 0x3F00] {
                    let _ = m.ppu_read(addr);
                    m.ppu_write(addr, 0x5A);
                    let _ = m.ppu_read(addr);
                }
                for line in [0i16, 127, 128, 239, 240] {
                    m.scanline(line);
                }
                for _ in 0..200 {
                    m.cpu_cycle();
                }
                let mut w = StateWriter::new();
                m.save_state(&mut w);
                let data = w.into_inner();
                let mut n = MiscBoard::new(id, rom(id, prg_kb, chr_kb, Mirroring::Vertical));
                n.load_state(&mut StateReader::new(&data)).unwrap();
                assert_eq!(m.common.prg_map, n.common.prg_map, "mapper {}", id);
            }
        }
    }

    #[test]
    fn save_state_roundtrip() {
        for id in [34u16, 90, 95, 96, 163, 168, 176, 220, 243, 252, 253, 255] {
            let prg = if id == 176 { 512 } else { 256 };
            let chr = if matches!(id, 168 | 253) { 0 } else { 256 };
            let mut a = MiscBoard::new(id, rom(id, prg, chr, Mirroring::Vertical));
            a.cpu_write(0x8000, 0x21);
            a.cpu_write(0xA000, 0x05);
            let mut w = StateWriter::new();
            a.save_state(&mut w);
            let data = w.into_inner();
            let mut b = MiscBoard::new(id, rom(id, prg, chr, Mirroring::Vertical));
            b.load_state(&mut StateReader::new(&data)).unwrap();
            assert_eq!(a.regs, b.regs, "mapper {} state mismatch", id);
            assert_eq!(a.common.prg_map, b.common.prg_map, "mapper {} prg", id);
        }
    }
}

