//! Nintendo discrete mappers: NROM, MMC1, UxROM/CNROM/AxROM/GxROM, etc.
//!
//! Direct port of the corresponding Mesen 0.9.9 sources (`Core/NROM.h`,
//! `MMC1.h`, `MMC1_105.h`, `MMC1_155.h`, `UNROM.h`, `UnRom_94.h`,
//! `UnRom_180.h`, `UnRom512.h`, `CNROM.h`, `AXROM.h`, `GxRom.h`,
//! `ColorDreams.h`, `CpRom.h`, `BF909x.h`, `BF9096.h`, `Nina03_06.h`,
//! `Smb2j.h`), including register quirks, power-on state and bus conflicts.
//!
//! Mapper IDs owned by the other mapper groups (notably 34/40/46/118/146/185/
//! 241) are intentionally not claimed here.
//!
//! Original C++: Copyright (C) 2014-2020 Sour, GPL-3.0.
//! Rust port: GPL-3.0-or-later.
//!
//! OWNER: mapper-discrete agent.

use super::{Mapper, MapperCommon};
use crate::state::{StateReader, StateResult, StateWriter};
use crate::types::{Mirroring, RomData};

// ---------------------------------------------------------------------------
// Bank mapping helpers.
//
// Mesen selects pages through `SelectPRGPage`/`SelectCHRPage`; the page index
// is wrapped modulo the number of pages. MapperCommon stores byte offsets and
// wraps modulo the memory size, which is equivalent for the power-of-two ROM
// sizes these boards use.
// ---------------------------------------------------------------------------

fn map_prg_16k_page(common: &mut MapperCommon, slot: usize, page: usize) {
    let base = page * 0x4000;
    common.map_prg_8k(slot * 2, base);
    common.map_prg_8k(slot * 2 + 1, base + 0x2000);
}

/// `SelectPrgPage2x(0, page)`: two consecutive 16KB pages at $8000/$C000.
fn map_prg_16k_pair(common: &mut MapperCommon, page: usize) {
    map_prg_16k_page(common, 0, page);
    map_prg_16k_page(common, 1, page + 1);
}

fn map_prg_32k_page(common: &mut MapperCommon, page: usize) {
    let base = page * 0x8000;
    for i in 0..4 {
        common.map_prg_8k(i, base + i * 0x2000);
    }
}

fn map_chr_4k_page(common: &mut MapperCommon, slot: usize, page: usize) {
    let base = page * 0x1000;
    for i in 0..4 {
        common.map_chr_1k(slot * 4 + i, base + i * 0x400);
    }
}

fn map_chr_8k_page(common: &mut MapperCommon, page: usize) {
    let base = page * 0x2000;
    for i in 0..8 {
        common.map_chr_1k(i, base + i * 0x400);
    }
}

/// Bus conflict: the value written is ANDed with the byte currently readable at
/// the register address (Mesen `BaseMapper::WriteRAM`).
fn bus_conflict(common: &MapperCommon, addr: u16, value: u8) -> u8 {
    value & common.read_prg_rom(addr)
}

fn prg_ram_battery(common: &MapperCommon) -> Option<&[u8]> {
    if common.info.has_battery {
        Some(&common.prg_ram)
    } else {
        None
    }
}

fn set_prg_ram_battery(common: &mut MapperCommon, data: &[u8]) {
    if common.info.has_battery {
        let n = data.len().min(common.prg_ram.len());
        common.prg_ram[..n].copy_from_slice(&data[..n]);
    }
}

/// Default work/save RAM write at $6000-$7FFF used by boards without special
/// low-address handling.
fn write_prg_ram(common: &mut MapperCommon, addr: u16, value: u8) {
    if (0x6000..=0x7FFF).contains(&addr) {
        common.write_prg_ram(addr, value);
    }
}

// ---------------------------------------------------------------------------
// NROM (mapper 0)
// ---------------------------------------------------------------------------

pub struct Nrom {
    common: MapperCommon,
    mapper_id: u16,
}

impl Nrom {
    pub fn new(mapper_id: u16, rom: RomData) -> Self {
        let mut mapper = Self {
            common: MapperCommon::new(rom),
            mapper_id,
        };
        mapper.init_mapping();
        mapper
    }

    fn init_mapping(&mut self) {
        // NROM: 16KB PRG page 0 at $8000, page 1 at $C000 (16KB ROMs mirror),
        // and the first 8KB of CHR.
        map_prg_16k_page(&mut self.common, 0, 0);
        map_prg_16k_page(&mut self.common, 1, 1);
        map_chr_8k_page(&mut self.common, 0);
    }
}

impl Mapper for Nrom {
    fn common(&self) -> &MapperCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }

    fn mapper_id(&self) -> u16 {
        self.mapper_id
    }

    fn reset(&mut self) {
        // NROM has no registers; re-applying the fixed mapping is a no-op for
        // the ROM data but keeps header mirroring authoritative on reset.
        self.init_mapping();
        if self.mapper_id == 0 {
            self.common.mirroring = self.common.info.mirroring;
        }
    }

    fn battery(&self) -> Option<&[u8]> {
        prg_ram_battery(&self.common)
    }

    fn set_battery(&mut self, data: &[u8]) {
        set_prg_ram_battery(&mut self.common, data);
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);
        w.u16(self.mapper_id);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        self.mapper_id = r.u16()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// MMC1 (mapper 1, 105, 155)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mmc1Kind {
    Base,
    Mapper105,
    Mapper155,
}

const MMC1_REG_A000: u8 = 0;
const MMC1_REG_C000: u8 = 1;

pub struct Mmc1 {
    common: MapperCommon,
    kind: Mmc1Kind,

    reg_8000: u8,
    reg_a000: u8,
    reg_c000: u8,
    reg_e000: u8,

    write_buffer: u8,
    shift_count: u8,
    last_write_cycle: u64,
    last_chr_reg: u8,
    cycle_count: u64,

    // Derived by UpdateState().
    wram_disabled: bool,
    ram_bank: usize,

    // Mapper 105 only.
    init_state: u8,
    irq_counter: u32,
    irq_enabled: bool,
}

impl Mmc1 {
    fn new(kind: Mmc1Kind, rom: RomData) -> Self {
        let mut common = MapperCommon::new(rom);

        // Mesen creates a unified save/work RAM sized from the header/database.
        // SXROM needs 32KB, SOROM 16KB; the loader reports both sizes separately.
        let ram_size = (common.info.prg_ram_size + common.info.prg_nvram_size).max(0x2000);
        if common.prg_ram.len() < ram_size {
            common.prg_ram.resize(ram_size, 0);
        }

        let mut mapper = Self {
            common,
            kind,
            reg_8000: 0,
            reg_a000: 0,
            reg_c000: 0,
            reg_e000: 0,
            write_buffer: 0,
            shift_count: 0,
            last_write_cycle: u64::MAX,
            last_chr_reg: MMC1_REG_A000,
            cycle_count: 0,
            wram_disabled: false,
            ram_bank: 0,
            init_state: 0,
            irq_counter: 0,
            irq_enabled: false,
        };

        // InitMapper(): power-on bits 2/3 of $8000 set (16KB mode, $8000
        // swappable) so SEROM/SHROM/SH1ROM boot from bank 0.
        mapper.reg_8000 = 0x0C;
        mapper.reg_a000 = 0;
        mapper.reg_c000 = 0;
        // MMC1B would start with WRAM disabled; we have no board database.
        mapper.reg_e000 = 0;
        mapper.last_chr_reg = MMC1_REG_A000;
        mapper.update_state();

        if kind == Mmc1Kind::Mapper105 {
            // MMC1_105::InitMapper() resets these *after* MMC1::InitMapper()
            // and forces the I bit (register $A000 bit 4).
            mapper.init_state = 0;
            mapper.irq_counter = 0;
            mapper.irq_enabled = false;
            mapper.reg_a000 |= 0x10;
        }

        mapper
    }

    fn reset_buffers(&mut self) {
        self.shift_count = 0;
        self.write_buffer = 0;
    }

    fn apply_mirroring(&mut self) {
        self.common.mirroring = match self.reg_8000 & 0x03 {
            0 => Mirroring::ScreenAOnly,
            1 => Mirroring::ScreenBOnly,
            2 => Mirroring::Vertical,
            _ => Mirroring::Horizontal,
        };
    }

    fn update_state(&mut self) {
        // Mapper 155: the WRAM disable bit does not exist.
        if self.kind == Mmc1Kind::Mapper155 {
            self.reg_e000 &= 0x0F;
        }

        self.apply_mirroring();

        if self.kind == Mmc1Kind::Mapper105 {
            if self.init_state == 0 && (self.reg_a000 & 0x10) == 0 {
                self.init_state = 1;
            } else if self.init_state == 1 && (self.reg_a000 & 0x10) != 0 {
                self.init_state = 2;
            }

            if self.reg_a000 & 0x10 != 0 {
                self.irq_enabled = false;
                self.irq_counter = 0;
                self.common.irq_pending = false;
            } else {
                self.irq_enabled = true;
            }

            self.wram_disabled = self.reg_e000 & 0x10 != 0;
            self.ram_bank = 0;

            if self.init_state == 2 {
                if self.reg_a000 & 0x08 != 0 {
                    // MMC1-compatible mode.
                    let prg_reg = ((self.reg_e000 & 0x07) | 0x08) as usize;
                    if self.reg_8000 & 0x08 != 0 {
                        if self.reg_8000 & 0x04 != 0 {
                            map_prg_16k_page(&mut self.common, 0, prg_reg);
                            map_prg_16k_page(&mut self.common, 1, 0x0F);
                        } else {
                            map_prg_16k_page(&mut self.common, 0, 0x08);
                            map_prg_16k_page(&mut self.common, 1, prg_reg);
                        }
                    } else {
                        map_prg_16k_pair(&mut self.common, prg_reg & 0xFE);
                    }
                } else {
                    let page = (self.reg_a000 & 0x06) as usize;
                    map_prg_16k_pair(&mut self.common, page);
                }
            } else {
                map_prg_16k_pair(&mut self.common, 0);
            }
            return;
        }

        self.wram_disabled = self.reg_e000 & 0x10 != 0;

        let chr_mode_4k = self.reg_8000 & 0x10 != 0;
        let prg_mode_16k = self.reg_8000 & 0x08 != 0;
        let slot_x8000 = self.reg_8000 & 0x04 != 0;
        let chr_reg0 = self.reg_a000 & 0x1F;
        let chr_reg1 = self.reg_c000 & 0x1F;
        let prg_reg = self.reg_e000 & 0x0F;

        // 4KB CHR mode: the outer bank bits come from whichever CHR register
        // was written last (SUROM/SXROM behavior).
        let extra_reg = if self.last_chr_reg == MMC1_REG_C000 && chr_mode_4k {
            chr_reg1
        } else {
            chr_reg0
        };

        // SUROM: 512KB PRG carts use bit 4 of $A000/$C000 as an outer bank.
        let prg_bank_select = if self.common.prg_rom.len() == 0x80000 {
            (extra_reg & 0x10) as usize
        } else {
            0
        };

        // SXROM/SOROM PRG RAM banking.
        let ram_size = self.common.prg_ram.len();
        let save_size = self.common.info.prg_nvram_size;
        let work_size = self.common.info.prg_ram_size;
        if ram_size > 0x4000 {
            // SXROM: 32KB of save RAM, 8KB banks from $A000/$C000 bits 2-3.
            self.ram_bank = ((extra_reg >> 2) & 0x03) as usize;
        } else if ram_size > 0x2000 {
            if save_size == 0x2000 && work_size == 0x2000 {
                // SOROM: half of the 16KB is battery-backed (bit 3 selects).
                self.ram_bank = ((extra_reg >> 3) & 0x01) as usize;
            } else {
                self.ram_bank = ((extra_reg >> 2) & 0x01) as usize;
            }
        } else {
            self.ram_bank = 0;
        }

        if self.common.info.submapper == 5 {
            // SEROM/SHROM/SH1ROM: fixed 32KB PRG, no banking support.
            map_prg_16k_page(&mut self.common, 0, 0);
            map_prg_16k_page(&mut self.common, 1, 1);
        } else if prg_mode_16k {
            let bank = prg_reg as usize | prg_bank_select;
            if slot_x8000 {
                map_prg_16k_page(&mut self.common, 0, bank);
                map_prg_16k_page(&mut self.common, 1, 0x0F | prg_bank_select);
            } else {
                map_prg_16k_page(&mut self.common, 0, prg_bank_select);
                map_prg_16k_page(&mut self.common, 1, bank);
            }
        } else {
            map_prg_16k_pair(
                &mut self.common,
                (prg_reg & 0xFE) as usize | prg_bank_select,
            );
        }

        if chr_mode_4k {
            map_chr_4k_page(&mut self.common, 0, chr_reg0 as usize);
            map_chr_4k_page(&mut self.common, 1, chr_reg1 as usize);
        } else {
            // 8KB CHR mode: two consecutive 4KB pages, low register bit ignored.
            let page = (chr_reg0 & 0x1E) as usize;
            map_chr_4k_page(&mut self.common, 0, page);
            map_chr_4k_page(&mut self.common, 1, page + 1);
        }
    }

    fn write_register(&mut self, addr: u16, value: u8) {
        let current_cycle = self.cycle_count;

        // Ignore a write that occurs within 2 cycles of another write (the
        // real write after the 6502 RMW dummy write).
        let accepted = self.last_write_cycle == u64::MAX
            || current_cycle.wrapping_sub(self.last_write_cycle) >= 2;

        if accepted {
            if value & 0x80 != 0 {
                // Reset bit: d is ignored, the shift register is cleared and
                // $8000 bits 2/3 are forced (16KB mode, $8000 swappable).
                self.reset_buffers();
                self.reg_8000 |= 0x0C;
                self.update_state();
            } else {
                self.write_buffer >>= 1;
                self.write_buffer |= (value & 0x01) << 4;
                self.shift_count += 1;

                if self.shift_count == 5 {
                    match (addr & 0x6000) >> 13 {
                        0 => self.reg_8000 = self.write_buffer,
                        1 => {
                            self.last_chr_reg = MMC1_REG_A000;
                            self.reg_a000 = self.write_buffer;
                        }
                        2 => {
                            self.last_chr_reg = MMC1_REG_C000;
                            self.reg_c000 = self.write_buffer;
                        }
                        _ => self.reg_e000 = self.write_buffer,
                    }
                    self.update_state();
                    self.reset_buffers();
                }
            }
        }

        self.last_write_cycle = current_cycle;
    }
}

impl Mapper for Mmc1 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }

    fn reset(&mut self) {
        // Mesen's BaseMapper::Reset is empty; registers persist. The CPU reset
        // clears the external IRQ latch.
        self.common.irq_pending = false;
    }

    fn cpu_read(&mut self, addr: u16) -> Option<u8> {
        match addr {
            0x6000..=0x7FFF => {
                if self.wram_disabled {
                    return None;
                }
                let len = self.common.prg_ram.len();
                let idx = (self.ram_bank * 0x2000 + (addr as usize - 0x6000)) % len;
                Some(self.common.prg_ram[idx])
            }
            0x8000..=0xFFFF => Some(self.common.read_prg_rom(addr)),
            _ => None,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x6000..=0x7FFF => {
                if !self.wram_disabled {
                    let len = self.common.prg_ram.len();
                    let idx = (self.ram_bank * 0x2000 + (addr as usize - 0x6000)) % len;
                    self.common.prg_ram[idx] = value;
                }
            }
            0x8000..=0xFFFF => self.write_register(addr, value),
            _ => {}
        }
    }

    fn cpu_cycle(&mut self) {
        self.cycle_count = self.cycle_count.wrapping_add(1);

        if self.kind == Mmc1Kind::Mapper105 && self.irq_enabled {
            self.irq_counter = self.irq_counter.wrapping_add(1);
            // 0x20000000 | (dip switches << 25); this port has no DIP input.
            if self.irq_counter >= 0x2000_0000 {
                self.common.irq_pending = true;
                self.irq_enabled = false;
            }
        }
    }

    fn irq_pending(&self) -> bool {
        self.common.irq_pending
    }

    fn battery(&self) -> Option<&[u8]> {
        prg_ram_battery(&self.common)
    }

    fn set_battery(&mut self, data: &[u8]) {
        set_prg_ram_battery(&mut self.common, data);
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);
        w.u8(self.reg_8000);
        w.u8(self.reg_a000);
        w.u8(self.reg_c000);
        w.u8(self.reg_e000);
        w.u8(self.write_buffer);
        w.u8(self.shift_count);
        w.u64(self.last_write_cycle);
        w.u8(self.last_chr_reg);
        w.u64(self.cycle_count);
        if self.kind == Mmc1Kind::Mapper105 {
            w.u8(self.init_state);
            w.u32(self.irq_counter);
            w.bool(self.irq_enabled);
        }
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        self.reg_8000 = r.u8()?;
        self.reg_a000 = r.u8()?;
        self.reg_c000 = r.u8()?;
        self.reg_e000 = r.u8()?;
        self.write_buffer = r.u8()?;
        self.shift_count = r.u8()?;
        self.last_write_cycle = r.u64()?;
        self.last_chr_reg = r.u8()?;
        self.cycle_count = r.u64()?;

        // Mesen calls UpdateState() inside MMC1::StreamState, before the
        // mapper-105 extras are restored.
        self.update_state();

        if self.kind == Mmc1Kind::Mapper105 {
            self.init_state = r.u8()?;
            self.irq_counter = r.u32()?;
            self.irq_enabled = r.bool()?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// UxROM family (mapper 2, 94, 180)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum UxRomKind {
    Unrom,
    Unrom180,
    Unrom94,
}

pub struct UxRom {
    common: MapperCommon,
    kind: UxRomKind,
}

impl UxRom {
    fn new(kind: UxRomKind, rom: RomData) -> Self {
        let mut mapper = Self {
            common: MapperCommon::new(rom),
            kind,
        };
        mapper.init_mapping();
        mapper
    }

    fn last_prg_page(&self) -> usize {
        (self.common.prg_rom.len() / 0x4000).max(1) - 1
    }

    fn init_mapping(&mut self) {
        map_chr_8k_page(&mut self.common, 0);
        let last = self.last_prg_page();
        match self.kind {
            UxRomKind::Unrom => {
                map_prg_16k_page(&mut self.common, 0, 0);
                map_prg_16k_page(&mut self.common, 1, last);
            }
            UxRomKind::Unrom180 => {
                // $C000 is the switchable bank, power-on value 0.
                map_prg_16k_page(&mut self.common, 0, 0);
                map_prg_16k_page(&mut self.common, 1, 0);
            }
            UxRomKind::Unrom94 => {
                map_prg_16k_page(&mut self.common, 0, 0);
                map_prg_16k_page(&mut self.common, 1, last);
            }
        }
    }

    fn bus_conflicts(&self) -> bool {
        self.common.info.mapper == 2 && self.common.info.submapper == 2
    }
}

impl Mapper for UxRom {
    fn common(&self) -> &MapperCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }

    fn reset(&mut self) {
        // No reset behavior in Mesen; registers persist.
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x6000..=0x7FFF => write_prg_ram(&mut self.common, addr, value),
            0x8000..=0xFFFF => {
                let value = if self.bus_conflicts() {
                    bus_conflict(&self.common, addr, value)
                } else {
                    value
                };
                match self.kind {
                    UxRomKind::Unrom => {
                        map_prg_16k_page(&mut self.common, 0, value as usize);
                    }
                    UxRomKind::Unrom180 => {
                        map_prg_16k_page(&mut self.common, 1, (value & 0x07) as usize);
                    }
                    UxRomKind::Unrom94 => {
                        map_prg_16k_page(&mut self.common, 0, ((value >> 2) & 0x07) as usize);
                    }
                }
            }
            _ => {}
        }
    }

    fn battery(&self) -> Option<&[u8]> {
        prg_ram_battery(&self.common)
    }

    fn set_battery(&mut self, data: &[u8]) {
        set_prg_ram_battery(&mut self.common, data);
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)
    }
}

// ---------------------------------------------------------------------------
// UNROM 512 (mapper 30)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum FlashMode {
    WaitingForCommand,
    Write,
    Erase,
}

pub struct UnRom512 {
    common: MapperCommon,
    prg_bank: u8,
    enable_mirroring_bit: bool,
    four_screen_chr_map: bool,
    flash_mode: FlashMode,
    flash_cycle: u8,
    flash_software_id: bool,
}

impl UnRom512 {
    fn new(rom: RomData) -> Self {
        let mut common = MapperCommon::new(rom);

        // Mesen defaults this board to 32KB of CHR RAM.
        if common.chr_is_ram {
            let target = if common.info.is_nes2 {
                common.info.chr_ram_size.max(0x2000)
            } else {
                0x8000
            };
            common.chr.resize(target, 0);
        }

        // Mirroring setup (UnRom512::InitMapper). The header's four-screen bit
        // has a board-specific meaning: 0x08 enables register-controlled
        // one-screen mirroring, while 0x09 is the InfiniteNesLives CHR-RAM
        // four-screen variation. RomInfo does not expose bit 0 of the header
        // once the four-screen flag is set, so use the CHR RAM size to tell
        // the two apart.
        let mut enable_mirroring_bit = false;
        let mut mirroring = common.info.mirroring;
        if mirroring == Mirroring::ScreenAOnly || mirroring == Mirroring::ScreenBOnly {
            mirroring = Mirroring::ScreenAOnly;
            enable_mirroring_bit = true;
        } else if common.info.four_screen {
            if common.info.chr_ram_size >= 0x8000 {
                mirroring = Mirroring::FourScreens;
            } else {
                mirroring = Mirroring::ScreenAOnly;
                enable_mirroring_bit = true;
            }
        }
        common.mirroring = mirroring;

        let four_screen_chr_map =
            mirroring == Mirroring::FourScreens && common.chr_is_ram && common.chr.len() >= 0x8000;

        let mut mapper = Self {
            common,
            prg_bank: 0,
            enable_mirroring_bit,
            four_screen_chr_map,
            flash_mode: FlashMode::WaitingForCommand,
            flash_cycle: 0,
            flash_software_id: false,
        };

        map_prg_16k_page(&mut mapper.common, 0, 0);
        let last = (mapper.common.prg_rom.len() / 0x4000).max(1) - 1;
        map_prg_16k_page(&mut mapper.common, 1, last);
        map_chr_8k_page(&mut mapper.common, 0);

        mapper
    }

    fn has_battery(&self) -> bool {
        self.common.info.has_battery
    }

    fn flash_read(&self, addr: u16) -> Option<u8> {
        if self.flash_software_id {
            Some(match addr & 0x1FF {
                0x00 => 0xBF,
                0x01 => 0xB7,
                _ => 0xFF,
            })
        } else {
            None
        }
    }

    fn reset_flash_state(&mut self) {
        self.flash_mode = FlashMode::WaitingForCommand;
        self.flash_cycle = 0;
    }

    fn flash_write(&mut self, addr: u32, value: u8) {
        let cmd = (addr & 0x7FFF) as u16;

        match self.flash_mode {
            FlashMode::WaitingForCommand => {
                if self.flash_cycle == 0 {
                    if cmd == 0x5555 && value == 0xAA {
                        self.flash_cycle = 1;
                    } else if value == 0xF0 {
                        // Software ID exit.
                        self.reset_flash_state();
                        self.flash_software_id = false;
                    }
                } else if self.flash_cycle == 1 && cmd == 0x2AAA && value == 0x55 {
                    self.flash_cycle = 2;
                } else if self.flash_cycle == 2 && cmd == 0x5555 {
                    self.flash_cycle = 3;
                    match value {
                        0x80 => self.flash_mode = FlashMode::Erase,
                        0x90 => {
                            self.reset_flash_state();
                            self.flash_software_id = true;
                        }
                        0xA0 => self.flash_mode = FlashMode::Write,
                        0xF0 => {
                            self.reset_flash_state();
                            self.flash_software_id = false;
                        }
                        _ => {}
                    }
                } else {
                    self.flash_cycle = 0;
                }
            }
            FlashMode::Write => {
                let idx = addr as usize;
                if idx < self.common.prg_rom.len() {
                    self.common.prg_rom[idx] &= value;
                }
                self.reset_flash_state();
            }
            FlashMode::Erase => {
                if self.flash_cycle == 3 {
                    if cmd == 0x5555 && value == 0xAA {
                        self.flash_cycle = 4;
                    } else {
                        self.reset_flash_state();
                    }
                } else if self.flash_cycle == 4 {
                    if cmd == 0x2AAA && value == 0x55 {
                        self.flash_cycle = 5;
                    } else {
                        self.reset_flash_state();
                    }
                } else if self.flash_cycle == 5 {
                    if cmd == 0x5555 && value == 0x10 {
                        self.common.prg_rom.fill(0xFF);
                    } else if value == 0x30 {
                        let offset = addr as usize & 0x7F000;
                        let end = (offset + 0x1000).min(self.common.prg_rom.len());
                        if offset < end {
                            self.common.prg_rom[offset..end].fill(0xFF);
                        }
                    }
                    self.reset_flash_state();
                }
            }
        }
    }
}

impl Mapper for UnRom512 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }

    fn reset(&mut self) {
        // No reset behavior in Mesen; registers/flash state persist.
    }

    fn cpu_read(&mut self, addr: u16) -> Option<u8> {
        match addr {
            0x8000..=0xFFFF => {
                if self.has_battery() {
                    if let Some(value) = self.flash_read(addr) {
                        return Some(value);
                    }
                }
                Some(self.common.read_prg_rom(addr))
            }
            // GetSaveRamSize()/GetWorkRamSize() are both 0 on this board.
            _ => None,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        if !(0x8000..=0xFFFF).contains(&addr) {
            return;
        }

        let value = if self.has_battery() {
            value
        } else {
            bus_conflict(&self.common, addr, value)
        };

        if !self.has_battery() || addr >= 0xC000 {
            self.prg_bank = value & 0x1F;
            map_prg_16k_page(&mut self.common, 0, self.prg_bank as usize);
            map_chr_8k_page(&mut self.common, ((value >> 5) & 0x03) as usize);

            if self.enable_mirroring_bit {
                self.common.mirroring = if value & 0x80 != 0 {
                    Mirroring::ScreenBOnly
                } else {
                    Mirroring::ScreenAOnly
                };
            }
        } else {
            // Flash writes only exist on battery boards below $C000.
            let flash_addr =
                (addr as u32 & 0x3FFF) | ((self.prg_bank as u32) << 14);
            self.flash_write(flash_addr, value);
        }
    }

    fn ppu_read(&mut self, addr: u16) -> Option<u8> {
        match addr & 0x3FFF {
            0x0000..=0x1FFF => Some(self.common.read_chr(addr)),
            0x2000..=0x3EFF if self.four_screen_chr_map => {
                let idx = 0x6000 + (addr as usize & 0x1FFF);
                Some(self.common.chr[idx % self.common.chr.len().max(1)])
            }
            0x2000..=0x3EFF => Some(self.common.read_nametable(addr)),
            _ => None,
        }
    }

    fn ppu_write(&mut self, addr: u16, value: u8) {
        match addr & 0x3FFF {
            0x0000..=0x1FFF => self.common.write_chr(addr, value),
            0x2000..=0x3EFF if self.four_screen_chr_map => {
                let len = self.common.chr.len().max(1);
                let idx = (0x6000 + (addr as usize & 0x1FFF)) % len;
                self.common.chr[idx] = value;
            }
            0x2000..=0x3EFF => self.common.write_nametable(addr, value),
            _ => {}
        }
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);
        w.u8(self.prg_bank);
        w.bool(self.enable_mirroring_bit);
        w.bool(self.four_screen_chr_map);
        w.u8(match self.flash_mode {
            FlashMode::WaitingForCommand => 0,
            FlashMode::Write => 1,
            FlashMode::Erase => 2,
        });
        w.u8(self.flash_cycle);
        w.bool(self.flash_software_id);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        self.prg_bank = r.u8()?;
        self.enable_mirroring_bit = r.bool()?;
        self.four_screen_chr_map = r.bool()?;
        self.flash_mode = match r.u8()? {
            1 => FlashMode::Write,
            2 => FlashMode::Erase,
            _ => FlashMode::WaitingForCommand,
        };
        self.flash_cycle = r.u8()?;
        self.flash_software_id = r.bool()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// CNROM (mapper 3)
// ---------------------------------------------------------------------------

pub struct Cnrom {
    common: MapperCommon,
}

impl Cnrom {
    fn new(rom: RomData) -> Self {
        let mut mapper = Self {
            common: MapperCommon::new(rom),
        };
        map_prg_32k_page(&mut mapper.common, 0);
        map_chr_8k_page(&mut mapper.common, 0);
        mapper
    }

    fn bus_conflicts(&self) -> bool {
        // NES 2.0 submapper 2: bus conflicts.
        self.common.info.submapper == 2
    }
}

impl Mapper for Cnrom {
    fn common(&self) -> &MapperCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }

    fn reset(&mut self) {
        // No reset behavior in Mesen.
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x6000..=0x7FFF => write_prg_ram(&mut self.common, addr, value),
            0x8000..=0xFFFF => {
                let value = if self.bus_conflicts() {
                    bus_conflict(&self.common, addr, value)
                } else {
                    value
                };
                map_chr_8k_page(&mut self.common, value as usize);
            }
            _ => {}
        }
    }

    fn battery(&self) -> Option<&[u8]> {
        prg_ram_battery(&self.common)
    }

    fn set_battery(&mut self, data: &[u8]) {
        set_prg_ram_battery(&mut self.common, data);
    }
}

// ---------------------------------------------------------------------------
// AxROM (mapper 7)
// ---------------------------------------------------------------------------

pub struct AxRom {
    common: MapperCommon,
}

impl AxRom {
    fn new(rom: RomData) -> Self {
        let mut mapper = Self {
            common: MapperCommon::new(rom),
        };
        map_chr_8k_page(&mut mapper.common, 0);
        map_prg_32k_page(&mut mapper.common, 0);
        mapper.common.mirroring = Mirroring::ScreenAOnly;
        mapper
    }

    fn bus_conflicts(&self) -> bool {
        self.common.info.submapper == 2
    }
}

impl Mapper for AxRom {
    fn common(&self) -> &MapperCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }

    fn reset(&mut self) {
        // No reset behavior in Mesen.
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x6000..=0x7FFF => write_prg_ram(&mut self.common, addr, value),
            0x8000..=0xFFFF => {
                let value = if self.bus_conflicts() {
                    bus_conflict(&self.common, addr, value)
                } else {
                    value
                };
                map_prg_32k_page(&mut self.common, (value & 0x0F) as usize);
                self.common.mirroring = if value & 0x10 != 0 {
                    Mirroring::ScreenBOnly
                } else {
                    Mirroring::ScreenAOnly
                };
            }
            _ => {}
        }
    }

    fn battery(&self) -> Option<&[u8]> {
        prg_ram_battery(&self.common)
    }

    fn set_battery(&mut self, data: &[u8]) {
        set_prg_ram_battery(&mut self.common, data);
    }
}

// ---------------------------------------------------------------------------
// GxROM (mapper 66)
// ---------------------------------------------------------------------------

pub struct GxRom {
    common: MapperCommon,
}

impl GxRom {
    fn new(rom: RomData) -> Self {
        let mut mapper = Self {
            common: MapperCommon::new(rom),
        };
        map_prg_32k_page(&mut mapper.common, 0);
        map_chr_8k_page(&mut mapper.common, 0);
        mapper
    }
}

impl Mapper for GxRom {
    fn common(&self) -> &MapperCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }

    fn reset(&mut self) {
        // No reset behavior in Mesen.
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x6000..=0x7FFF => write_prg_ram(&mut self.common, addr, value),
            0x8000..=0xFFFF => {
                map_prg_32k_page(&mut self.common, ((value >> 4) & 0x03) as usize);
                map_chr_8k_page(&mut self.common, (value & 0x03) as usize);
            }
            _ => {}
        }
    }

    fn battery(&self) -> Option<&[u8]> {
        prg_ram_battery(&self.common)
    }

    fn set_battery(&mut self, data: &[u8]) {
        set_prg_ram_battery(&mut self.common, data);
    }
}

// ---------------------------------------------------------------------------
// Color Dreams (mapper 11, 144)
// ---------------------------------------------------------------------------

pub struct ColorDreams {
    common: MapperCommon,
    mapper_144: bool,
}

impl ColorDreams {
    fn new(rom: RomData) -> Self {
        let mapper_144 = rom.info.mapper == 144;
        let mut mapper = Self {
            common: MapperCommon::new(rom),
            mapper_144,
        };
        map_prg_32k_page(&mut mapper.common, 0);
        map_chr_8k_page(&mut mapper.common, 0);
        mapper
    }
}

impl Mapper for ColorDreams {
    fn common(&self) -> &MapperCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }

    fn reset(&mut self) {
        // No reset behavior in Mesen.
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x6000..=0x7FFF => write_prg_ram(&mut self.common, addr, value),
            0x8000..=0xFFFF => {
                // This board always has bus conflicts.
                let prg_value = self.common.read_prg_rom(addr);
                let mut value = value & prg_value;
                if self.mapper_144 {
                    // Mapper 144: only the ROM's LSB always wins.
                    value |= prg_value & 0x01;
                }
                map_prg_32k_page(&mut self.common, (value & 0x0F) as usize);
                map_chr_8k_page(&mut self.common, ((value >> 4) & 0x0F) as usize);
            }
            _ => {}
        }
    }

    fn battery(&self) -> Option<&[u8]> {
        prg_ram_battery(&self.common)
    }

    fn set_battery(&mut self, data: &[u8]) {
        set_prg_ram_battery(&mut self.common, data);
    }
}

// ---------------------------------------------------------------------------
// CpROM (mapper 13)
// ---------------------------------------------------------------------------

pub struct CpRom {
    common: MapperCommon,
}

impl CpRom {
    fn new(rom: RomData) -> Self {
        let mut common = MapperCommon::new(rom);

        // GetChrRamSize() = 16KB; Mesen honors an explicit NES 2.0 size.
        if common.chr_is_ram {
            let target = if common.info.is_nes2 {
                common.info.chr_ram_size.max(0x2000)
            } else {
                0x4000
            };
            common.chr.resize(target, 0);
        }

        let mut mapper = Self { common };
        map_prg_32k_page(&mut mapper.common, 0);
        map_chr_4k_page(&mut mapper.common, 0, 0);
        map_chr_4k_page(&mut mapper.common, 1, 1);
        mapper.common.mirroring = Mirroring::Vertical;
        mapper
    }
}

impl Mapper for CpRom {
    fn common(&self) -> &MapperCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }

    fn reset(&mut self) {
        // No reset behavior in Mesen.
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x6000..=0x7FFF => write_prg_ram(&mut self.common, addr, value),
            0x8000..=0xFFFF => {
                map_chr_4k_page(&mut self.common, 1, (value & 0x03) as usize);
            }
            _ => {}
        }
    }

    fn battery(&self) -> Option<&[u8]> {
        prg_ram_battery(&self.common)
    }

    fn set_battery(&mut self, data: &[u8]) {
        set_prg_ram_battery(&mut self.common, data);
    }
}

// ---------------------------------------------------------------------------
// BF909x (mapper 71)
// ---------------------------------------------------------------------------

pub struct Bf909x {
    common: MapperCommon,
    bf9097_mode: bool,
}

impl Bf909x {
    fn new(rom: RomData) -> Self {
        let bf9097_mode = rom.info.submapper == 1;
        let mut mapper = Self {
            common: MapperCommon::new(rom),
            bf9097_mode,
        };
        map_chr_8k_page(&mut mapper.common, 0);
        map_prg_16k_page(&mut mapper.common, 0, 0);
        let last = (mapper.common.prg_rom.len() / 0x4000).max(1) - 1;
        map_prg_16k_page(&mut mapper.common, 1, last);
        mapper
    }
}

impl Mapper for Bf909x {
    fn common(&self) -> &MapperCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }

    fn reset(&mut self) {
        // No reset behavior in Mesen.
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x6000..=0x7FFF => write_prg_ram(&mut self.common, addr, value),
            0x8000..=0xFFFF => {
                if addr == 0x9000 {
                    // Firehawk uses $9000 to enable mirroring control.
                    self.bf9097_mode = true;
                }

                if addr >= 0xC000 || !self.bf9097_mode {
                    map_prg_16k_page(&mut self.common, 0, value as usize);
                } else {
                    self.common.mirroring = if value & 0x10 != 0 {
                        Mirroring::ScreenAOnly
                    } else {
                        Mirroring::ScreenBOnly
                    };
                }
            }
            _ => {}
        }
    }

    fn battery(&self) -> Option<&[u8]> {
        prg_ram_battery(&self.common)
    }

    fn set_battery(&mut self, data: &[u8]) {
        set_prg_ram_battery(&mut self.common, data);
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);
        w.bool(self.bf9097_mode);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        self.bf9097_mode = r.bool()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// BF9096 / Quattro (mapper 232)
// ---------------------------------------------------------------------------

pub struct Bf9096 {
    common: MapperCommon,
    prg_block: u8,
    prg_page: u8,
}

impl Bf9096 {
    fn new(rom: RomData) -> Self {
        let mut mapper = Self {
            common: MapperCommon::new(rom),
            prg_block: 0,
            prg_page: 0,
        };
        map_chr_8k_page(&mut mapper.common, 0);
        map_prg_16k_page(&mut mapper.common, 0, 0);
        map_prg_16k_page(&mut mapper.common, 1, 3);
        mapper
    }
}

impl Mapper for Bf9096 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }

    fn reset(&mut self) {
        // No reset behavior in Mesen.
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x6000..=0x7FFF => write_prg_ram(&mut self.common, addr, value),
            0x8000..=0xFFFF => {
                if addr >= 0xC000 {
                    self.prg_page = value & 0x03;
                } else if self.common.info.submapper == 1 {
                    // Aladdin Deck Enhancer variation: outer bank bits swapped.
                    self.prg_block =
                        ((value >> 4) & 0x01) | ((value >> 2) & 0x02);
                } else {
                    self.prg_block = (value >> 3) & 0x03;
                }

                let block = (self.prg_block as usize) << 2;
                map_prg_16k_page(&mut self.common, 0, block | self.prg_page as usize);
                map_prg_16k_page(&mut self.common, 1, block | 3);
            }
            _ => {}
        }
    }

    fn battery(&self) -> Option<&[u8]> {
        prg_ram_battery(&self.common)
    }

    fn set_battery(&mut self, data: &[u8]) {
        set_prg_ram_battery(&mut self.common, data);
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);
        w.u8(self.prg_block);
        w.u8(self.prg_page);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        self.prg_block = r.u8()?;
        self.prg_page = r.u8()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Nina-03/06 (mapper 79, 113)
// ---------------------------------------------------------------------------

pub struct Nina0306 {
    common: MapperCommon,
    multicart: bool,
}

impl Nina0306 {
    fn new(rom: RomData) -> Self {
        let multicart = rom.info.mapper == 113;
        let mut mapper = Self {
            common: MapperCommon::new(rom),
            multicart,
        };
        map_prg_32k_page(&mut mapper.common, 0);
        map_chr_8k_page(&mut mapper.common, 0);
        mapper
    }
}

impl Mapper for Nina0306 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }

    fn reset(&mut self) {
        // No reset behavior in Mesen.
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        if (0x6000..=0x7FFF).contains(&addr) {
            write_prg_ram(&mut self.common, addr, value);
        } else if (addr & 0xE100) == 0x4100 {
            if self.multicart {
                // Mapper 113 (Nina-03/06 multicart).
                map_prg_32k_page(&mut self.common, ((value >> 3) & 0x07) as usize);
                let chr = ((value & 0x07) | ((value >> 3) & 0x08)) as usize;
                map_chr_8k_page(&mut self.common, chr);
                self.common.mirroring = if value & 0x80 != 0 {
                    Mirroring::Vertical
                } else {
                    Mirroring::Horizontal
                };
            } else {
                // Mapper 79 (non-multicart Nina-03/06).
                map_prg_32k_page(&mut self.common, ((value >> 3) & 0x01) as usize);
                map_chr_8k_page(&mut self.common, (value & 0x07) as usize);
            }
        }
    }

    fn battery(&self) -> Option<&[u8]> {
        prg_ram_battery(&self.common)
    }

    fn set_battery(&mut self, data: &[u8]) {
        set_prg_ram_battery(&mut self.common, data);
    }
}

// ---------------------------------------------------------------------------
// SMB2j IRQ board (Mesen extended mapper id 304)
// ---------------------------------------------------------------------------

pub struct Smb2j {
    common: MapperCommon,
    prg_4k: [usize; 8],
    irq_counter: u16,
    irq_enabled: bool,
}

impl Smb2j {
    fn new(rom: RomData) -> Self {
        let mut mapper = Self {
            common: MapperCommon::new(rom),
            prg_4k: [0; 8],
            irq_counter: 0,
            irq_enabled: false,
        };
        map_chr_8k_page(&mut mapper.common, 0);
        for (slot, entry) in mapper.prg_4k.iter_mut().enumerate() {
            *entry = (slot * 0x1000) % mapper.common.prg_rom.len().max(1);
        }
        mapper
    }

    fn prg_4k_byte(&self, offset: usize, addr: u16) -> u8 {
        let idx = (offset + (addr as usize & 0x0FFF)) % self.common.prg_rom.len().max(1);
        self.common.prg_rom[idx]
    }

    /// Offset of the PRG page mapped at $5000/$6000/$7000. Mesen maps
    /// pageCount-3, -2 and -1 respectively.
    fn low_page_offset(&self, index: usize) -> usize {
        let pages = (self.common.prg_rom.len() / 0x1000).max(1);
        let page = pages.saturating_sub(3) + index;
        (page * 0x1000) % self.common.prg_rom.len().max(1)
    }

    fn reset_irq(&mut self) {
        self.irq_counter = 0;
        self.common.irq_pending = false;
    }
}

impl Mapper for Smb2j {
    fn common(&self) -> &MapperCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }

    fn reset(&mut self) {
        // No reset behavior in Mesen; only the CPU's IRQ latch is cleared.
        self.common.irq_pending = false;
    }

    fn cpu_read(&mut self, addr: u16) -> Option<u8> {
        match addr {
            0x5000..=0x5FFF => Some(self.prg_4k_byte(self.low_page_offset(0), addr)),
            0x6000..=0x6FFF => Some(self.prg_4k_byte(self.low_page_offset(1), addr)),
            0x7000..=0x7FFF => Some(self.prg_4k_byte(self.low_page_offset(2), addr)),
            0x8000..=0xFFFF => {
                let slot = ((addr as usize - 0x8000) >> 12) & 0x07;
                Some(self.prg_4k_byte(self.prg_4k[slot], addr))
            }
            _ => None,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x4022 if self.common.prg_rom.len() >= 0x10000 => {
                let base = ((value & 0x01) as usize) << 2;
                for i in 0..4 {
                    self.prg_4k[i] = ((base + i) * 0x1000)
                        % self.common.prg_rom.len().max(1);
                }
                for i in 0..4 {
                    self.prg_4k[4 + i] = ((base + 4 + i) * 0x1000)
                        % self.common.prg_rom.len().max(1);
                }
            }
            0x4122 => {
                self.irq_enabled = (value & 0x03) != 0;
                self.reset_irq();
            }
            _ => {}
        }
    }

    fn cpu_cycle(&mut self) {
        if self.irq_enabled {
            self.irq_counter = (self.irq_counter.wrapping_add(1)) & 0x0FFF;
            if self.irq_counter == 0 {
                self.irq_enabled = false;
                self.common.irq_pending = true;
            }
        }
    }

    fn irq_pending(&self) -> bool {
        self.common.irq_pending
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);
        w.u16(self.irq_counter);
        w.bool(self.irq_enabled);
        for offset in self.prg_4k {
            w.u32(offset as u32);
        }
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        self.irq_counter = r.u16()?;
        self.irq_enabled = r.bool()?;
        for offset in &mut self.prg_4k {
            *offset = r.u32()? as usize;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

pub fn supports(id: u16) -> bool {
    matches!(
        id,
        0 | 1
            | 2
            | 3
            | 7
            | 11
            | 13
            | 30
            | 66
            | 71
            | 79
            | 94
            | 105
            | 113
            | 144
            | 155
            | 180
            | 232
            | 304
    )
}

pub fn create(id: u16, rom: RomData) -> Option<Box<dyn Mapper>> {
    Some(match id {
        0 => Box::new(Nrom::new(id, rom)),
        1 => Box::new(Mmc1::new(Mmc1Kind::Base, rom)),
        105 => Box::new(Mmc1::new(Mmc1Kind::Mapper105, rom)),
        155 => Box::new(Mmc1::new(Mmc1Kind::Mapper155, rom)),
        2 => Box::new(UxRom::new(UxRomKind::Unrom, rom)),
        94 => Box::new(UxRom::new(UxRomKind::Unrom94, rom)),
        180 => Box::new(UxRom::new(UxRomKind::Unrom180, rom)),
        30 => Box::new(UnRom512::new(rom)),
        3 => Box::new(Cnrom::new(rom)),
        7 => Box::new(AxRom::new(rom)),
        11 | 144 => Box::new(ColorDreams::new(rom)),
        13 => Box::new(CpRom::new(rom)),
        66 => Box::new(GxRom::new(rom)),
        71 => Box::new(Bf909x::new(rom)),
        79 | 113 => Box::new(Nina0306::new(rom)),
        232 => Box::new(Bf9096::new(rom)),
        304 => Box::new(Smb2j::new(rom)),
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{RomInfo, RomSystem};

    fn rom_info(mapper: u16) -> RomInfo {
        RomInfo {
            mapper,
            system: RomSystem::Nes,
            mirroring: Mirroring::Horizontal,
            prg_ram_size: 0x2000,
            ..Default::default()
        }
    }

    /// PRG ROM with the index of each 8KB page stored at its first byte.
    fn prg_16k_banks(banks_16k: usize) -> Vec<u8> {
        let mut prg = vec![0xFFu8; banks_16k * 0x4000];
        for i in 0..banks_16k * 2 {
            prg[i * 0x2000] = i as u8;
        }
        prg
    }

    /// PRG ROM with the index of each 4KB page stored at its first byte.
    fn prg_4k_pages(pages: usize) -> Vec<u8> {
        let mut prg = vec![0xFFu8; pages * 0x1000];
        for i in 0..pages {
            prg[i * 0x1000] = i as u8;
        }
        prg
    }

    /// CHR ROM with the index of each 8KB page stored at its first byte.
    fn chr_8k_banks(banks: usize) -> Vec<u8> {
        let mut chr = vec![0u8; banks * 0x2000];
        for i in 0..banks {
            chr[i * 0x2000] = i as u8;
        }
        chr
    }

    fn rom(mapper: u16, prg: Vec<u8>, chr: Vec<u8>) -> RomData {
        let mut info = rom_info(mapper);
        info.prg_rom_size = prg.len();
        info.chr_rom_size = chr.len();
        RomData::new(info, prg, chr)
    }

    fn mmc1_serial_write(mapper: &mut Mmc1, addr: u16, value: u8) {
        for bit in 0..5 {
            // Keep writes at least 2 CPU cycles apart (the RMW write filter).
            mapper.cpu_cycle();
            mapper.cpu_cycle();
            mapper.cpu_write(addr, (value >> bit) & 0x01);
        }
    }

    fn read(mapper: &mut dyn Mapper, addr: u16) -> u8 {
        mapper.cpu_read(addr).expect("address should be mapped")
    }

    #[test]
    fn nrom_mirrors_16k_prg() {
        let prg = prg_16k_banks(1);
        let mut mapper = Nrom::new(0, rom(0, prg, chr_8k_banks(1)));

        assert_eq!(read(&mut mapper, 0x8000), 0); // first 8KB
        assert_eq!(read(&mut mapper, 0xA000), 1); // second 8KB
        assert_eq!(read(&mut mapper, 0xC000), 0); // mirrored
        assert_eq!(read(&mut mapper, 0xE000), 1);
    }

    #[test]
    fn nrom_maps_32k_prg_and_chr() {
        let prg = prg_16k_banks(2);
        let chr = chr_8k_banks(2);
        let mut mapper = Nrom::new(0, rom(0, prg, chr));

        assert_eq!(read(&mut mapper, 0x8000), 0);
        assert_eq!(read(&mut mapper, 0xA000), 1);
        assert_eq!(read(&mut mapper, 0xC000), 2);
        assert_eq!(read(&mut mapper, 0xE000), 3);
        assert_eq!(mapper.ppu_read(0x0000), Some(0));
        mapper.cpu_write(0x8000, 0xFF);
        assert_eq!(read(&mut mapper, 0x8000), 0);
    }

    #[test]
    fn mmc1_power_on_state_and_serial_register() {
        let mut mapper = Mmc1::new(Mmc1Kind::Base, rom(1, prg_16k_banks(4), Vec::new()));

        // Power-on: 16KB mode, $8000 swappable, one-screen A.
        assert_eq!(mapper.common.mirroring, Mirroring::ScreenAOnly);
        assert_eq!(read(&mut mapper, 0x8000), 0);
        assert_eq!(read(&mut mapper, 0xC000), 6); // last 16KB page = 8KB page 6

        // $8000: bits 0-1 = 2 (vertical), bit 3 = 1 (16KB), bit 4 = 0 (8KB CHR)
        mmc1_serial_write(&mut mapper, 0x8000, 0x0E);
        assert_eq!(mapper.common.mirroring, Mirroring::Vertical);
        assert_eq!(mapper.reg_8000, 0x0E);
        assert_eq!(read(&mut mapper, 0x8000), 0);
        assert_eq!(read(&mut mapper, 0xC000), 6);
    }

    #[test]
    fn mmc1_prg_modes_and_last_bank() {
        let mut mapper = Mmc1::new(Mmc1Kind::Base, rom(1, prg_16k_banks(4), Vec::new()));

        // 16KB mode, $C000 swappable, bank 2.
        mmc1_serial_write(&mut mapper, 0x8000, 0x08);
        mmc1_serial_write(&mut mapper, 0xE000, 0x02);
        assert_eq!(read(&mut mapper, 0x8000), 0);
        assert_eq!(read(&mut mapper, 0xA000), 1);
        assert_eq!(read(&mut mapper, 0xC000), 4); // page 2
        assert_eq!(read(&mut mapper, 0xE000), 5);

        // 32KB mode with bank 2 -> 16KB pages 2 and 3.
        mmc1_serial_write(&mut mapper, 0x8000, 0x02);
        assert_eq!(read(&mut mapper, 0x8000), 4);
        assert_eq!(read(&mut mapper, 0xC000), 6);
    }

    #[test]
    fn mmc1_chr_4k_and_8k_modes() {
        let mut mapper = Mmc1::new(
            Mmc1Kind::Base,
            rom(1, prg_16k_banks(4), chr_8k_banks(8)),
        );

        mmc1_serial_write(&mut mapper, 0xA000, 0x05);
        mmc1_serial_write(&mut mapper, 0xC000, 0x06);

        // 4KB CHR mode (bit 4 of $8000 set), 16KB PRG, $8000 swappable.
        mmc1_serial_write(&mut mapper, 0x8000, 0x1E);
        assert_eq!(mapper.common.chr_map[0], 0x5000);
        assert_eq!(mapper.common.chr_map[4], 0x6000);

        // 8KB CHR mode uses bits 1-4 of $A000 and ignores bit 0; the two
        // 4KB pages are consecutive.
        mmc1_serial_write(&mut mapper, 0x8000, 0x0E);
        assert_eq!(mapper.common.chr_map[0], 0x4000);
        assert_eq!(mapper.common.chr_map[4], 0x5000);
    }

    #[test]
    fn mmc1_reset_bit_forces_16k_mode() {
        let mut mapper = Mmc1::new(Mmc1Kind::Base, rom(1, prg_16k_banks(4), Vec::new()));

        // Switch to 32KB mode temporarily, then set the reset bit.
        mmc1_serial_write(&mut mapper, 0x8000, 0x02);
        mapper.cpu_cycle();
        mapper.cpu_cycle();
        mapper.cpu_write(0x8000, 0x80);
        assert_eq!(mapper.reg_8000 & 0x0C, 0x0C);
        assert_eq!(mapper.shift_count, 0);
        assert_eq!(read(&mut mapper, 0x8000), 0);
        assert_eq!(read(&mut mapper, 0xC000), 6);
    }

    #[test]
    fn mmc1_surom_outer_bank() {
        // 512KB = 32 x 16KB banks.
        let prg = prg_16k_banks(32);
        let mut mapper = Mmc1::new(Mmc1Kind::Base, rom(1, prg, Vec::new()));

        // $A000 bit 4 is the SUROM outer bank (256KB).
        mmc1_serial_write(&mut mapper, 0xA000, 0x10);
        mmc1_serial_write(&mut mapper, 0x8000, 0x0E);
        assert_eq!(read(&mut mapper, 0x8000), 32); // 16KB bank 16

        mmc1_serial_write(&mut mapper, 0xE000, 0x03);
        assert_eq!(read(&mut mapper, 0x8000), 38); // 16KB bank 19
        assert_eq!(read(&mut mapper, 0xC000), 62); // page 0x1F = bank 31
    }

    #[test]
    fn mmc1_sxrom_ram_banking() {
        let mut data = rom(1, prg_16k_banks(4), Vec::new());
        data.info.prg_ram_size = 0;
        data.info.prg_nvram_size = 0x8000; // 32KB battery-backed PRG RAM
        let mut mapper = Mmc1::new(Mmc1Kind::Base, data);

        assert_eq!(mapper.common.prg_ram.len(), 0x8000);
        for bank in 0..4usize {
            mapper.ram_bank = bank;
            mapper.cpu_write(0x6000, 0x10 + bank as u8);
        }
        // Writing $A000 with bits 2-3 selects the 8KB bank.
        mmc1_serial_write(&mut mapper, 0xA000, 0x04);
        assert_eq!(mapper.ram_bank, 1);
        assert_eq!(read(&mut mapper, 0x6000), 0x11);

        mmc1_serial_write(&mut mapper, 0xA000, 0x08);
        assert_eq!(mapper.ram_bank, 2);
        assert_eq!(read(&mut mapper, 0x6000), 0x12);
    }

    #[test]
    fn mmc1_wram_disable() {
        let mut mapper = Mmc1::new(Mmc1Kind::Base, rom(1, prg_16k_banks(2), Vec::new()));

        mapper.cpu_write(0x6000, 0x42);
        assert_eq!(read(&mut mapper, 0x6000), 0x42);

        mmc1_serial_write(&mut mapper, 0xE000, 0x10);
        assert_eq!(mapper.cpu_read(0x6000), None);
        mapper.cpu_write(0x6000, 0x99);
        assert_eq!(mapper.cpu_read(0x6000), None);
    }

    #[test]
    fn mmc1_155_ignores_wram_disable_bit() {
        let mut mapper = Mmc1::new(Mmc1Kind::Mapper155, rom(155, prg_16k_banks(2), Vec::new()));

        mmc1_serial_write(&mut mapper, 0xE000, 0x13);
        assert_eq!(mapper.reg_e000, 0x03);
        assert!(!mapper.wram_disabled);
        mapper.cpu_write(0x6000, 0x77);
        assert_eq!(read(&mut mapper, 0x6000), 0x77);
    }

    #[test]
    fn mmc1_105_init_sequence_and_timer_irq() {
        let mut mapper =
            Mmc1::new(Mmc1Kind::Mapper105, rom(105, prg_16k_banks(16), Vec::new()));

        // Power-on: 32KB bank 0; IRQ disabled by the forced I bit.
        assert_eq!(mapper.reg_a000, 0x10);
        assert_eq!(mapper.init_state, 0);
        assert!(!mapper.irq_enabled);
        assert_eq!(read(&mut mapper, 0x8000), 0);
        assert_eq!(read(&mut mapper, 0xC000), 2);

        // Clear the I bit -> init state 1, IRQ timer enabled.
        mmc1_serial_write(&mut mapper, 0xA000, 0x08);
        assert_eq!(mapper.init_state, 1);
        assert!(mapper.irq_enabled);
        assert_eq!(read(&mut mapper, 0x8000), 0);

        // Set the I bit again -> init state 2, IRQ disabled/cleared.
        mmc1_serial_write(&mut mapper, 0xA000, 0x18);
        assert_eq!(mapper.init_state, 2);
        assert!(!mapper.irq_enabled);
        assert!(!mapper.irq_pending());

        // MMC1 mode: PRG reg = ($E000 & 7) | 8 = 0x0A -> 16KB bank 10 at $8000.
        mmc1_serial_write(&mut mapper, 0xE000, 0x02);
        assert_eq!(read(&mut mapper, 0x8000), 20);
        assert_eq!(read(&mut mapper, 0xC000), 30); // last bank (0x0F)

        // 32KB mode: pages 0x0A/0x0B (bit 3 of $8000 clear).
        mmc1_serial_write(&mut mapper, 0x8000, 0x02);
        assert_eq!(read(&mut mapper, 0x8000), 20);
        assert_eq!(read(&mut mapper, 0x8000 + 0x4000), 22);

        // Timer comparison: 0x20000000 cycles (DIP switches = 0).
        mmc1_serial_write(&mut mapper, 0xA000, 0x08); // I bit clear, IRQ on
        mapper.irq_counter = 0x1FFF_FFFE;
        mapper.cpu_cycle();
        assert!(!mapper.irq_pending());
        mapper.cpu_cycle();
        assert!(mapper.irq_pending());
        assert!(!mapper.irq_enabled);

        // Writing the I bit clears the latched IRQ.
        mmc1_serial_write(&mut mapper, 0xA000, 0x18);
        assert!(!mapper.irq_pending());
    }

    #[test]
    fn mmc1_save_load_roundtrip() {
        let mut mapper = Mmc1::new(
            Mmc1Kind::Base,
            rom(1, prg_16k_banks(4), chr_8k_banks(4)),
        );
        mmc1_serial_write(&mut mapper, 0x8000, 0x0E);
        mmc1_serial_write(&mut mapper, 0xE000, 0x03);
        mmc1_serial_write(&mut mapper, 0xA000, 0x02);

        let mut w = StateWriter::new();
        mapper.save_state(&mut w);
        let data = w.into_inner();

        // Corrupt live state, then restore.
        mmc1_serial_write(&mut mapper, 0xE000, 0x01);
        assert_eq!(read(&mut mapper, 0x8000), 2);

        let mut r = StateReader::new(&data);
        mapper.load_state(&mut r).unwrap();

        assert_eq!(mapper.reg_8000, 0x0E);
        assert_eq!(mapper.reg_e000, 0x03);
        assert_eq!(mapper.reg_a000, 0x02);
        assert_eq!(mapper.common.mirroring, Mirroring::Vertical);
        assert_eq!(read(&mut mapper, 0x8000), 6); // bank 3
        assert_eq!(read(&mut mapper, 0xC000), 6); // fixed last bank (page 0x0F)
        assert_eq!(mapper.common.chr_map[0], 0x2000);
    }

    #[test]
    fn unrom_switches_8000_and_keeps_last_bank() {
        let mut mapper = UxRom::new(UxRomKind::Unrom, rom(2, prg_16k_banks(4), Vec::new()));

        assert_eq!(read(&mut mapper, 0x8000), 0);
        assert_eq!(read(&mut mapper, 0xC000), 6);
        mapper.cpu_write(0x8000, 2);
        assert_eq!(read(&mut mapper, 0x8000), 4);
        assert_eq!(read(&mut mapper, 0xC000), 6);
    }

    #[test]
    fn unrom_180_switches_c000() {
        let mut mapper = UxRom::new(UxRomKind::Unrom180, rom(180, prg_16k_banks(4), Vec::new()));

        assert_eq!(read(&mut mapper, 0x8000), 0);
        assert_eq!(read(&mut mapper, 0xC000), 0);
        mapper.cpu_write(0x8000, 0xF2);
        assert_eq!(read(&mut mapper, 0x8000), 0);
        assert_eq!(read(&mut mapper, 0xC000), 4); // 2 & 7
        mapper.cpu_write(0x8000, 0x0F);
        assert_eq!(read(&mut mapper, 0xC000), 6); // 7 & 7 -> bank 3
    }

    #[test]
    fn unrom_94_uses_bits_2_4() {
        let mut mapper = UxRom::new(UxRomKind::Unrom94, rom(94, prg_16k_banks(4), Vec::new()));

        mapper.cpu_write(0x8000, 0x1C);
        assert_eq!(read(&mut mapper, 0x8000), 6); // (0x1C >> 2) & 7 = 7 -> bank 3
        assert_eq!(read(&mut mapper, 0xC000), 6);
    }

    #[test]
    fn unrom_bus_conflicts_only_for_submapper_2() {
        let mut data = rom(2, prg_16k_banks(4), Vec::new());
        data.prg_rom[0] = 0x03; // byte at $8000
        data.info.submapper = 2;
        let mut mapper = UxRom::new(UxRomKind::Unrom, data);

        mapper.cpu_write(0x8000, 0x07);
        assert_eq!(read(&mut mapper, 0x8000), 6); // 0x07 & 0x03 = 3
    }

    #[test]
    fn cnrom_selects_chr_bank() {
        let mut mapper = Cnrom::new(rom(3, prg_16k_banks(2), chr_8k_banks(4)));

        mapper.cpu_write(0x8000, 0x02);
        assert_eq!(mapper.common.chr_map[0], 0x4000);
        assert_eq!(mapper.ppu_read(0x0000), Some(2));
    }

    #[test]
    fn cnrom_submapper_2_bus_conflict() {
        let mut data = rom(3, prg_16k_banks(2), chr_8k_banks(4));
        data.prg_rom[0] = 0x01; // byte at $8000
        data.info.submapper = 2;
        let mut mapper = Cnrom::new(data);

        mapper.cpu_write(0x8000, 0x03);
        assert_eq!(mapper.common.chr_map[0], 0x2000);
    }

    #[test]
    fn axrom_32k_banks_and_one_screen_mirroring() {
        let mut mapper = AxRom::new(rom(7, prg_16k_banks(8), Vec::new()));

        assert_eq!(read(&mut mapper, 0x8000), 0);
        assert_eq!(mapper.common.mirroring, Mirroring::ScreenAOnly);

        mapper.cpu_write(0x8000, 0x12); // PRG bank 2, screen B
        assert_eq!(read(&mut mapper, 0x8000), 8);
        assert_eq!(mapper.common.mirroring, Mirroring::ScreenBOnly);

        mapper.cpu_write(0x8000, 0x03); // PRG bank 3, screen A
        assert_eq!(read(&mut mapper, 0x8000), 12);
        assert_eq!(mapper.common.mirroring, Mirroring::ScreenAOnly);
    }

    #[test]
    fn gxrom_selects_prg_and_chr() {
        let mut mapper = GxRom::new(rom(66, prg_16k_banks(8), chr_8k_banks(4)));

        mapper.cpu_write(0x8000, 0x32);
        assert_eq!(read(&mut mapper, 0x8000), 12); // PRG 32KB page 3
        assert_eq!(mapper.common.chr_map[0], 0x4000); // CHR page 2
    }

    #[test]
    fn color_dreams_bus_conflicts_and_mapper_144_lsb() {
        let mut data = rom(11, prg_16k_banks(8), chr_8k_banks(4));
        data.prg_rom[0] = 0xFE;
        let mut mapper = ColorDreams::new(data);
        mapper.cpu_write(0x8000, 0xFF);
        // 0xFF & 0xFE = 0xFE: PRG bank 14 % 4 = 2, CHR bank 15 % 4 = 3.
        assert_eq!(read(&mut mapper, 0x8000), 8);
        assert_eq!(mapper.common.chr_map[0], 0x6000);

        let mut data = rom(144, prg_16k_banks(8), chr_8k_banks(4));
        data.prg_rom[0] = 0x01;
        let mut mapper = ColorDreams::new(data);
        // Bus conflict would clear bit 0, but mapper 144 forces the ROM LSB.
        mapper.cpu_write(0x8000, 0x00);
        assert_eq!(read(&mut mapper, 0x8000), 4); // PRG bank 1 (32KB page 1)
    }

    #[test]
    fn cprom_16k_chr_ram_and_fixed_prg() {
        let mut mapper = CpRom::new(rom(13, prg_16k_banks(2), Vec::new()));

        assert_eq!(mapper.common.chr.len(), 0x4000);
        assert_eq!(mapper.common.mirroring, Mirroring::Vertical);
        assert_eq!(mapper.common.chr_map[0], 0x0000);
        assert_eq!(mapper.common.chr_map[4], 0x1000);

        mapper.ppu_write(0x1000, 0x5A);
        assert_eq!(mapper.ppu_read(0x1000), Some(0x5A));

        mapper.cpu_write(0x8000, 0x03);
        assert_eq!(mapper.common.chr_map[4], 0x3000);
        assert_eq!(mapper.ppu_read(0x1000), Some(0)); // fresh bank
        mapper.ppu_write(0x1000, 0xA5);
        assert_eq!(mapper.ppu_read(0x1000), Some(0xA5));

        // PRG is fixed regardless of writes.
        assert_eq!(read(&mut mapper, 0x8000), 0);
        assert_eq!(read(&mut mapper, 0xC000), 2);
    }

    #[test]
    fn bf909x_mirroring_mode() {
        let mut mapper = Bf909x::new(rom(71, prg_16k_banks(4), Vec::new()));

        assert_eq!(read(&mut mapper, 0x8000), 0);
        assert_eq!(read(&mut mapper, 0xC000), 6);

        // Before $9000 is written, low writes still select the PRG bank.
        mapper.cpu_write(0x8000, 0x01);
        assert_eq!(read(&mut mapper, 0x8000), 2);

        // $9000 enables mirroring writes (Firehawk behavior).
        mapper.cpu_write(0x9000, 0x00);
        mapper.cpu_write(0x8000, 0x10);
        assert_eq!(mapper.common.mirroring, Mirroring::ScreenAOnly);
        mapper.cpu_write(0x8000, 0x00);
        assert_eq!(mapper.common.mirroring, Mirroring::ScreenBOnly);

        // Writes at $C000+ always change the PRG bank.
        mapper.cpu_write(0xC000, 0x03);
        assert_eq!(read(&mut mapper, 0x8000), 6);
    }

    #[test]
    fn bf9096_quattro_banking() {
        let mut mapper = Bf9096::new(rom(232, prg_16k_banks(16), Vec::new()));

        assert_eq!(read(&mut mapper, 0x8000), 0);
        assert_eq!(read(&mut mapper, 0xC000), 6); // page 3

        mapper.cpu_write(0xC000, 0x02);
        assert_eq!(read(&mut mapper, 0x8000), 4); // page 2
        assert_eq!(read(&mut mapper, 0xC000), 6); // page 3

        mapper.cpu_write(0x8000, 0x10); // block 2
        assert_eq!(read(&mut mapper, 0x8000), 20); // (2 << 2) | 2 = 10
        assert_eq!(read(&mut mapper, 0xC000), 22); // (2 << 2) | 3 = 11
    }

    #[test]
    fn nina_79_register_window() {
        let mut mapper = Nina0306::new(rom(79, prg_16k_banks(4), chr_8k_banks(8)));

        mapper.cpu_write(0x4100, 0x0D); // PRG bit 1, CHR 5
        assert_eq!(read(&mut mapper, 0x8000), 4); // 32KB page 1
        assert_eq!(mapper.common.chr_map[0], 0x2000 * 5);

        mapper.cpu_write(0x4200, 0xFF); // outside the decoded window
        assert_eq!(read(&mut mapper, 0x8000), 4);
        assert_eq!(mapper.common.chr_map[0], 0x2000 * 5);
    }

    #[test]
    fn nina_113_multicart() {
        let mut mapper = Nina0306::new(rom(113, prg_16k_banks(16), chr_8k_banks(16)));

        mapper.cpu_write(0x4100, 0x5D);
        assert_eq!(read(&mut mapper, 0x8000), 12); // PRG 32KB page 3
        assert_eq!(mapper.common.chr_map[0], 13 * 0x2000);
        assert_eq!(mapper.common.mirroring, Mirroring::Horizontal);

        mapper.cpu_write(0x4100, 0xDD); // bit 7 -> vertical
        assert_eq!(read(&mut mapper, 0x8000), 12);
        assert_eq!(mapper.common.chr_map[0], 13 * 0x2000);
        assert_eq!(mapper.common.mirroring, Mirroring::Vertical);
    }

    #[test]
    fn smb2j_4k_banking_and_irq() {
        let mut mapper = Smb2j::new(rom(304, prg_4k_pages(32), Vec::new()));

        assert_eq!(read(&mut mapper, 0x5000), 29);
        assert_eq!(read(&mut mapper, 0x6000), 30);
        assert_eq!(read(&mut mapper, 0x7000), 31);
        assert_eq!(read(&mut mapper, 0x8000), 0);
        assert_eq!(read(&mut mapper, 0xC000), 4);

        mapper.cpu_write(0x4022, 0x01);
        assert_eq!(read(&mut mapper, 0x8000), 4);
        assert_eq!(read(&mut mapper, 0xC000), 8);

        mapper.cpu_write(0x4022, 0x00);
        assert_eq!(read(&mut mapper, 0x8000), 0);

        // IRQ counter wraps after 4096 enabled CPU cycles.
        mapper.cpu_write(0x4122, 0x03);
        for _ in 0..4095 {
            mapper.cpu_cycle();
        }
        assert!(!mapper.irq_pending());
        mapper.cpu_cycle();
        assert!(mapper.irq_pending());
        assert!(!mapper.irq_enabled);

        mapper.cpu_write(0x4122, 0x00);
        assert!(!mapper.irq_pending());
    }

    #[test]
    fn unrom512_banking_and_chr() {
        let mut mapper = UnRom512::new(rom(30, prg_16k_banks(16), Vec::new()));

        assert_eq!(mapper.common.chr.len(), 0x8000);
        assert_eq!(read(&mut mapper, 0x8000), 0);
        assert_eq!(read(&mut mapper, 0xC000), 30);
        assert_eq!(mapper.common.mirroring, Mirroring::Horizontal);

        // Non-battery board: bus conflicts, so make the PRG byte $FF.
        mapper.common.prg_rom[0] = 0xFF;
        mapper.cpu_write(0x8000, 0x25);
        assert_eq!(read(&mut mapper, 0x8000), 10); // PRG bank 5
        assert_eq!(read(&mut mapper, 0xC000), 30);
        assert_eq!(mapper.common.chr_map[0], 0x2000); // CHR 8KB page 1
    }

    #[test]
    fn unrom512_four_screen_chr_variation() {
        let mut data = rom(30, prg_16k_banks(4), Vec::new());
        data.info.chr_ram_size = 0x8000;
        data.info.is_nes2 = true;
        data.info.four_screen = true;
        data.info.mirroring = Mirroring::FourScreens;
        let mut mapper = UnRom512::new(data);

        assert!(mapper.four_screen_chr_map);
        assert_eq!(mapper.common.mirroring, Mirroring::FourScreens);
        mapper.ppu_write(0x2000, 0x11);
        mapper.ppu_write(0x3000, 0x22);
        assert_eq!(mapper.ppu_read(0x2000), Some(0x11));
        assert_eq!(mapper.ppu_read(0x3000), Some(0x22));
        assert_eq!(mapper.common.chr[0x6000], 0x11);
        assert_eq!(mapper.common.chr[0x7000], 0x22);
    }

    #[test]
    fn unrom512_one_screen_switch_variation() {
        let mut data = rom(30, prg_16k_banks(4), Vec::new());
        data.info.four_screen = true;
        data.info.mirroring = Mirroring::FourScreens;
        data.info.chr_ram_size = 0x2000; // iNES default: case 8 heuristic
        let mut mapper = UnRom512::new(data);

        assert_eq!(mapper.common.mirroring, Mirroring::ScreenAOnly);
        mapper.common.prg_rom[0] = 0xFF;
        mapper.cpu_write(0x8000, 0x80);
        assert_eq!(mapper.common.mirroring, Mirroring::ScreenBOnly);
    }

    #[test]
    fn smb2j_save_load_roundtrip() {
        let mut mapper = Smb2j::new(rom(304, prg_4k_pages(32), Vec::new()));

        mapper.cpu_write(0x4022, 0x01);
        mapper.cpu_write(0x4122, 0x03);
        for _ in 0..100 {
            mapper.cpu_cycle();
        }

        let mut w = StateWriter::new();
        mapper.save_state(&mut w);
        let data = w.into_inner();

        mapper.cpu_write(0x4022, 0x00);
        mapper.cpu_write(0x4122, 0x00);
        assert_eq!(read(&mut mapper, 0x8000), 0);
        assert_eq!(mapper.irq_counter, 0);

        let mut r = StateReader::new(&data);
        mapper.load_state(&mut r).unwrap();

        assert_eq!(read(&mut mapper, 0x8000), 4);
        assert_eq!(read(&mut mapper, 0xC000), 8);
        assert_eq!(mapper.irq_counter, 100);
        assert!(mapper.irq_enabled);
    }

    #[test]
    fn unrom512_save_load_roundtrip() {
        let mut mapper = UnRom512::new(rom(30, prg_16k_banks(16), Vec::new()));
        mapper.common.prg_rom[0] = 0xFF;
        mapper.cpu_write(0x8000, 0x25);

        let mut w = StateWriter::new();
        mapper.save_state(&mut w);
        let data = w.into_inner();

        // Bank 5's marker at $8000 is 0x0A, so 0x0A survives the bus conflict.
        mapper.cpu_write(0x8000, 0x0A);
        assert_eq!(read(&mut mapper, 0x8000), 20); // bank 10

        let mut r = StateReader::new(&data);
        mapper.load_state(&mut r).unwrap();

        assert_eq!(mapper.prg_bank, 5);
        assert_eq!(read(&mut mapper, 0x8000), 10);
        assert_eq!(mapper.common.chr_map[0], 0x2000);
    }

    #[test]
    fn factory_supports_and_creates_all_ids() {
        let ids = [
            0u16, 1, 2, 3, 7, 11, 13, 30, 66, 71, 79, 94, 105, 113, 144, 155, 180, 232, 304,
        ];
        for id in ids {
            assert!(supports(id), "supports({id})");
            let mapper = create(id, rom(id, prg_16k_banks(2), chr_8k_banks(2)));
            assert!(mapper.is_some(), "create({id})");
            assert_eq!(mapper.unwrap().mapper_id(), id);
        }
        // IDs owned by other mapper groups must not be claimed here.
        assert!(!supports(4));
        assert!(!supports(34));
        assert!(!supports(40));
        assert!(!supports(46));
        assert!(!supports(118));
        assert!(!supports(185));
        assert!(!supports(241));
        assert!(create(34, rom(34, Vec::new(), Vec::new())).is_none());
    }
}
