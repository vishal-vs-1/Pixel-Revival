//! Konami VRC mappers.
//!
//! Ported from Mesen 0.9.9:
//! `VRC1.h`, `VRC2_4.h`, `VRC3.h`, `VRC6.h`, `VRC7.h`, `VrcIrq.h`,
//! `Vrc6Audio.h`, `Vrc6Pulse.h`, `Vrc6Saw.h`, `Vrc7Audio.h`,
//! `OpllEmulator.h`, `OpllChannel.h`, `OpllTables.h`.
//!
//! Supported iNES mapper IDs:
//!   21 (VRC4a/VRC4c), 22 (VRC2a), 23 (VRC2b/VRC4e), 24 (VRC6a),
//!   25 (VRC4b/VRC4d/VRC2c), 26 (VRC6b), 73 (VRC3), 75 (VRC1),
//!   85 (VRC7).
//!
//! Original C++: Copyright (C) 2014-2020 Sour, GPL-3.0.
//! Rust port: GPL-3.0-or-later.
//!
//! OWNER: mapper-konami agent.

use super::{Mapper, MapperCommon};
use crate::state::{StateReader, StateResult, StateWriter};
use crate::types::{Mirroring, RomData};

pub fn supports(id: u16) -> bool {
    matches!(id, 21 | 22 | 23 | 24 | 25 | 26 | 73 | 75 | 85)
}

pub fn create(id: u16, rom: RomData) -> Option<Box<dyn Mapper>> {
    match id {
        75 => Some(Box::new(Vrc1::new(rom))),
        21 | 22 | 23 | 25 => Some(Box::new(Vrc24::new(id, rom))),
        73 => Some(Box::new(Vrc3::new(rom))),
        24 => Some(Box::new(Vrc6::new(Vrc6Model::A, rom))),
        26 => Some(Box::new(Vrc6::new(Vrc6Model::B, rom))),
        85 => Some(Box::new(Vrc7::new(rom))),
        _ => None,
    }
}

const PRG_PAGE_8K: usize = 0x2000;
const PRG_PAGE_16K: usize = 0x4000;
const CHR_PAGE_1K: usize = 0x0400;
const CHR_PAGE_4K: usize = 0x1000;
const CHR_PAGE_8K: usize = 0x2000;

/// Map an 8KB PRG page into `slot`, wrapping like Mesen's page-count modulo.
fn map_prg_8k_page(common: &mut MapperCommon, slot: usize, page: isize) {
    let count = (common.prg_rom.len() / PRG_PAGE_8K).max(1) as isize;
    let wrapped = page.rem_euclid(count) as usize;
    common.map_prg_8k(slot, wrapped * PRG_PAGE_8K);
}

/// Map a 16KB PRG page into a pair of 8KB slots (VRC3).
fn map_prg_16k_page(common: &mut MapperCommon, slot16: usize, page: isize) {
    let count = (common.prg_rom.len() / PRG_PAGE_16K).max(1) as isize;
    let wrapped = page.rem_euclid(count) as usize;
    let base = wrapped * PRG_PAGE_16K;
    common.map_prg_8k(slot16 * 2, base);
    common.map_prg_8k(slot16 * 2 + 1, base + PRG_PAGE_8K);
}

fn map_chr_1k_page(common: &mut MapperCommon, slot: usize, page: usize) {
    common.map_chr_1k(slot, page * CHR_PAGE_1K);
}

fn map_chr_4k_page(common: &mut MapperCommon, slot4k: usize, page: usize) {
    for i in 0..4 {
        let idx = slot4k * 4 + i;
        common.map_chr_1k(idx, page * CHR_PAGE_4K + i * CHR_PAGE_1K);
    }
}

fn map_chr_8k_page(common: &mut MapperCommon, page: usize) {
    for i in 0..8 {
        common.map_chr_1k(i, page * CHR_PAGE_8K + i * CHR_PAGE_1K);
    }
}

fn nt_pages_for_mirroring(mirroring: Mirroring) -> [u8; 4] {
    match mirroring {
        Mirroring::Vertical => [0, 1, 0, 1],
        Mirroring::Horizontal => [0, 0, 1, 1],
        Mirroring::ScreenAOnly => [0, 0, 0, 0],
        Mirroring::ScreenBOnly => [1, 1, 1, 1],
        Mirroring::FourScreens => [0, 1, 2, 3],
    }
}

fn prg_ram_slot_ok(common: &MapperCommon) -> bool {
    common.info.has_battery
}

/// VRC IRQ (VRC4, VRC6, VRC7). Faithful port of `VrcIrq.h`.
#[derive(Clone, Copy, Debug, Default)]
struct VrcIrq {
    reload_value: u8,
    counter: u8,
    /// `int16_t` upstream; cycle mode makes this wrap, which is observable
    /// when switching back to scanline mode.
    prescaler: i16,
    enabled: bool,
    enabled_after_ack: bool,
    cycle_mode: bool,
    /// Level of the mapper's external IRQ line.
    line: bool,
}

impl VrcIrq {
    fn reset(&mut self) {
        self.prescaler = 0;
        self.reload_value = 0;
        self.counter = 0;
        self.enabled = false;
        self.enabled_after_ack = false;
        self.cycle_mode = false;
        self.line = false;
    }

    fn clock(&mut self) {
        if !self.enabled {
            return;
        }

        self.prescaler = self.prescaler.wrapping_sub(3);
        if self.cycle_mode || (self.prescaler <= 0 && !self.cycle_mode) {
            if self.counter == 0xFF {
                self.counter = self.reload_value;
                self.line = true;
            } else {
                self.counter = self.counter.wrapping_add(1);
            }
            self.prescaler = self.prescaler.wrapping_add(341);
        }
    }

    fn set_reload_value(&mut self, value: u8) {
        self.reload_value = value;
    }

    fn set_reload_value_nibble(&mut self, value: u8, high_bits: bool) {
        if high_bits {
            self.reload_value = (self.reload_value & 0x0F) | ((value & 0x0F) << 4);
        } else {
            self.reload_value = (self.reload_value & 0xF0) | (value & 0x0F);
        }
    }

    fn set_control(&mut self, value: u8) {
        self.enabled_after_ack = (value & 0x01) == 0x01;
        self.enabled = (value & 0x02) == 0x02;
        self.cycle_mode = (value & 0x04) == 0x04;

        if self.enabled {
            self.counter = self.reload_value;
            self.prescaler = 341;
        }

        self.line = false;
    }

    fn acknowledge(&mut self) {
        self.enabled = self.enabled_after_ack;
        self.line = false;
    }

    fn save_state(&self, w: &mut StateWriter) {
        w.u8(self.reload_value);
        w.u8(self.counter);
        w.i16(self.prescaler);
        w.bool(self.enabled);
        w.bool(self.enabled_after_ack);
        w.bool(self.cycle_mode);
        w.bool(self.line);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.reload_value = r.u8()?;
        self.counter = r.u8()?;
        self.prescaler = r.i16()?;
        self.enabled = r.bool()?;
        self.enabled_after_ack = r.bool()?;
        self.cycle_mode = r.bool()?;
        self.line = r.bool()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// VRC1 (mapper 75)
// ---------------------------------------------------------------------------

pub struct Vrc1 {
    common: MapperCommon,
    chr_banks: [u8; 2],
    prg_mapped: [bool; 4],
    chr_mapped: bool,
}

impl Vrc1 {
    fn new(rom: RomData) -> Self {
        let common = MapperCommon::new(rom);
        let mut mapper = Self {
            common,
            chr_banks: [0; 2],
            prg_mapped: [false; 4],
            chr_mapped: false,
        };
        mapper.apply_power_on();
        mapper
    }

    fn apply_power_on(&mut self) {
        self.common.init_default_maps();
        self.common.mirroring = self.common.info.mirroring;
        self.chr_banks = [0; 2];
        self.prg_mapped = [false, false, false, true];
        map_prg_8k_page(&mut self.common, 3, -1);

        // With CHR RAM, BaseMapper maps the whole 8KB at power-on. With CHR
        // ROM nothing is mapped until the first CHR register write.
        self.chr_mapped = self.common.chr_is_ram;
        if self.chr_mapped {
            map_chr_4k_page(&mut self.common, 0, 0);
            map_chr_4k_page(&mut self.common, 1, 1);
        } else {
            map_chr_4k_page(&mut self.common, 0, 0);
            map_chr_4k_page(&mut self.common, 1, 0);
        }
    }

    fn update_chr_banks(&mut self) {
        map_chr_4k_page(&mut self.common, 0, self.chr_banks[0] as usize);
        map_chr_4k_page(&mut self.common, 1, self.chr_banks[1] as usize);
        self.chr_mapped = true;
    }

    fn write_register(&mut self, addr: u16, value: u8) {
        // Oversized PRG is always allowed, matching upstream's default
        // (`allowOversizedPrg = true`).
        match addr & 0xF000 {
            0x8000 => {
                map_prg_8k_page(&mut self.common, 0, (value & 0xFF) as isize);
                self.prg_mapped[0] = true;
            }
            0x9000 => {
                if self.common.mirroring != Mirroring::FourScreens {
                    // The mirroring bit is ignored on 4-screen boards.
                    self.common.mirroring = if value & 0x01 != 0 {
                        Mirroring::Horizontal
                    } else {
                        Mirroring::Vertical
                    };
                }
                self.chr_banks[0] = (self.chr_banks[0] & 0x0F) | ((value & 0x02) << 3);
                self.chr_banks[1] = (self.chr_banks[1] & 0x0F) | ((value & 0x04) << 2);
                self.update_chr_banks();
            }
            0xA000 => {
                map_prg_8k_page(&mut self.common, 1, (value & 0xFF) as isize);
                self.prg_mapped[1] = true;
            }
            0xC000 => {
                map_prg_8k_page(&mut self.common, 2, (value & 0xFF) as isize);
                self.prg_mapped[2] = true;
            }
            0xE000 => {
                self.chr_banks[0] = (self.chr_banks[0] & 0x10) | (value & 0x0F);
                self.update_chr_banks();
            }
            0xF000 => {
                self.chr_banks[1] = (self.chr_banks[1] & 0x10) | (value & 0x0F);
                self.update_chr_banks();
            }
            _ => {}
        }
    }
}

impl Mapper for Vrc1 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }

    fn reset(&mut self) {
        // Mesen's BaseMapper::Reset is a no-op; plant the power-on state so
        // the port has a deterministic hard reset.
        self.apply_power_on();
    }

    fn cpu_read(&mut self, addr: u16) -> Option<u8> {
        match addr {
            0x6000..=0x7FFF => Some(self.common.read_prg_ram(addr)),
            0x8000..=0xFFFF => {
                let slot = ((addr - 0x8000) >> 13) as usize;
                if self.prg_mapped[slot] {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x6000..=0x7FFF => self.common.write_prg_ram(addr, value),
            0x8000..=0xFFFF => self.write_register(addr, value),
            _ => {}
        }
    }

    fn ppu_read(&mut self, addr: u16) -> Option<u8> {
        match addr & 0x3FFF {
            0x0000..=0x1FFF => {
                if self.chr_mapped {
                    Some(self.common.read_chr(addr))
                } else {
                    None
                }
            }
            0x2000..=0x3EFF => Some(self.common.read_nametable(addr)),
            _ => None,
        }
    }

    fn ppu_write(&mut self, addr: u16, value: u8) {
        match addr & 0x3FFF {
            0x0000..=0x1FFF => {
                if self.chr_mapped {
                    self.common.write_chr(addr, value);
                }
            }
            0x2000..=0x3EFF => self.common.write_nametable(addr, value),
            _ => {}
        }
    }

    fn battery(&self) -> Option<&[u8]> {
        if prg_ram_slot_ok(&self.common) {
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
        for bank in self.chr_banks {
            w.u8(bank);
        }
        for mapped in self.prg_mapped {
            w.bool(mapped);
        }
        w.bool(self.chr_mapped);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        for bank in &mut self.chr_banks {
            *bank = r.u8()?;
        }
        for mapped in &mut self.prg_mapped {
            *mapped = r.bool()?;
        }
        self.chr_mapped = r.bool()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// VRC2 / VRC4 (mappers 21, 22, 23, 25)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Vrc24Variant {
    Vrc2a,
    Vrc2b,
    Vrc2c,
    Vrc4a,
    Vrc4b,
    Vrc4c,
    Vrc4d,
    Vrc4e,
}

impl Vrc24Variant {
    fn detect(mapper: u16, submapper: u8) -> Self {
        match mapper {
            22 => Vrc24Variant::Vrc2a,
            23 => match submapper {
                2 => Vrc24Variant::Vrc4e,
                _ => Vrc24Variant::Vrc2b,
            },
            25 => match submapper {
                2 => Vrc24Variant::Vrc4d,
                3 => Vrc24Variant::Vrc2c,
                _ => Vrc24Variant::Vrc4b,
            },
            // 21
            _ => match submapper {
                2 => Vrc24Variant::Vrc4c,
                _ => Vrc24Variant::Vrc4a,
            },
        }
    }
}

pub struct Vrc24 {
    common: MapperCommon,
    variant: Vrc24Variant,
    use_heuristics: bool,
    prg_reg0: u8,
    prg_reg1: u8,
    prg_mode: u8,
    lo_chr: [u8; 8],
    hi_chr: [u8; 8],
    latch: bool,
    irq: VrcIrq,
    /// VRC2 Microwire latch is only exposed when the board has no PRG RAM.
    microwire: bool,
}

impl Vrc24 {
    fn new(mapper: u16, rom: RomData) -> Self {
        let variant = Vrc24Variant::detect(mapper, rom.info.submapper);
        let use_heuristics = rom.info.submapper == 0 && mapper != 22;
        let no_prg_ram = rom.info.prg_ram_size == 0 && rom.info.prg_nvram_size == 0;
        let microwire = !use_heuristics && variant <= Vrc24Variant::Vrc2c && no_prg_ram;

        let common = MapperCommon::new(rom);
        let mut mapper = Self {
            common,
            variant,
            use_heuristics,
            prg_reg0: 0,
            prg_reg1: 0,
            prg_mode: 0,
            lo_chr: [0; 8],
            hi_chr: [0; 8],
            latch: false,
            irq: VrcIrq::default(),
            microwire,
        };
        mapper.apply_power_on();
        mapper
    }

    fn apply_power_on(&mut self) {
        self.common.init_default_maps();
        self.common.mirroring = self.common.info.mirroring;
        // GetPowerOnByte() defaults to 0 (randomize-mapper-power-on is off).
        self.prg_reg0 = 0;
        self.prg_reg1 = 0;
        // PRG mode only exists on VRC4+.
        self.prg_mode = 0;
        self.lo_chr = [0; 8];
        self.hi_chr = [0; 8];
        self.latch = false;
        self.irq.reset();
        self.update_state();
    }

    fn update_state(&mut self) {
        for i in 0..8 {
            let mut page = self.lo_chr[i] as usize | ((self.hi_chr[i] as usize) << 4);
            if self.variant == Vrc24Variant::Vrc2a {
                // VRC2a ignores the low CHR bit: effective page is page >> 1.
                page >>= 1;
            }
            map_chr_1k_page(&mut self.common, i, page);
        }

        if self.prg_mode == 0 {
            map_prg_8k_page(&mut self.common, 0, self.prg_reg0 as isize);
            map_prg_8k_page(&mut self.common, 1, self.prg_reg1 as isize);
            map_prg_8k_page(&mut self.common, 2, -2);
            map_prg_8k_page(&mut self.common, 3, -1);
        } else {
            map_prg_8k_page(&mut self.common, 0, -2);
            map_prg_8k_page(&mut self.common, 1, self.prg_reg1 as isize);
            map_prg_8k_page(&mut self.common, 2, self.prg_reg0 as isize);
            map_prg_8k_page(&mut self.common, 3, -1);
        }
    }

    fn translate_address(&self, addr: u16) -> u16 {
        let addr = addr as u32;
        let (a0, a1): (u32, u32);

        if self.use_heuristics {
            match self.variant {
                // Mapper 25: OR the VRC2c/VRC4b and VRC4d bit positions so
                // that both address layouts decode.
                Vrc24Variant::Vrc2c | Vrc24Variant::Vrc4b | Vrc24Variant::Vrc4d => {
                    a0 = ((addr >> 1) & 0x01) | ((addr >> 3) & 0x01);
                    a1 = (addr & 0x01) | ((addr >> 2) & 0x01);
                }
                // Mapper 21: VRC4a and VRC4c bit positions.
                Vrc24Variant::Vrc4a | Vrc24Variant::Vrc4c => {
                    a0 = ((addr >> 1) & 0x01) | ((addr >> 6) & 0x01);
                    a1 = ((addr >> 2) & 0x01) | ((addr >> 7) & 0x01);
                }
                // Mapper 23: VRC2b and VRC4e bit positions.
                Vrc24Variant::Vrc2b | Vrc24Variant::Vrc4e => {
                    a0 = (addr & 0x01) | ((addr >> 2) & 0x01);
                    a1 = ((addr >> 1) & 0x01) | ((addr >> 3) & 0x01);
                }
                // VRC2a is never heuristic.
                Vrc24Variant::Vrc2a => {
                    a0 = (addr >> 1) & 0x01;
                    a1 = addr & 0x01;
                }
            }
        } else {
            match self.variant {
                Vrc24Variant::Vrc2a
                | Vrc24Variant::Vrc2c
                | Vrc24Variant::Vrc4b => {
                    a0 = (addr >> 1) & 0x01;
                    a1 = addr & 0x01;
                }
                Vrc24Variant::Vrc4d => {
                    a0 = (addr >> 3) & 0x01;
                    a1 = (addr >> 2) & 0x01;
                }
                Vrc24Variant::Vrc4a => {
                    a0 = (addr >> 1) & 0x01;
                    a1 = (addr >> 2) & 0x01;
                }
                Vrc24Variant::Vrc4c => {
                    a0 = (addr >> 6) & 0x01;
                    a1 = (addr >> 7) & 0x01;
                }
                Vrc24Variant::Vrc2b => {
                    a0 = addr & 0x01;
                    a1 = (addr >> 1) & 0x01;
                }
                Vrc24Variant::Vrc4e => {
                    a0 = (addr >> 2) & 0x01;
                    a1 = (addr >> 3) & 0x01;
                }
            }
        }

        (addr as u16 & 0xFF00) | ((a1 as u16) << 1) | a0 as u16
    }

    fn write_register(&mut self, addr: u16, value: u8) {
        if addr < 0x8000 {
            // VRC2 Microwire latch ($6000-$6FFF).
            self.latch = (value & 0x01) == 0x01;
            return;
        }

        let addr = self.translate_address(addr) & 0xF00F;

        if (0x8000..=0x8006).contains(&addr) {
            self.prg_reg0 = value & 0x1F;
        } else if ((self.variant <= Vrc24Variant::Vrc2c) && (0x9000..=0x9003).contains(&addr))
            || ((self.variant >= Vrc24Variant::Vrc4a) && (0x9000..=0x9001).contains(&addr))
        {
            let mask = if !self.use_heuristics && self.variant <= Vrc24Variant::Vrc2c {
                // When this is certainly a VRC2 board, bit 0 is the only
                // mirroring bit.
                0x01
            } else {
                0x03
            };

            self.common.mirroring = match value & mask {
                0 => Mirroring::Vertical,
                1 => Mirroring::Horizontal,
                2 => Mirroring::ScreenAOnly,
                _ => Mirroring::ScreenBOnly,
            };
        } else if self.variant >= Vrc24Variant::Vrc4a && (0x9002..=0x9003).contains(&addr) {
            self.prg_mode = (value >> 1) & 0x01;
        } else if (0xA000..=0xA006).contains(&addr) {
            self.prg_reg1 = value & 0x1F;
        } else if (0xB000..=0xE006).contains(&addr) {
            let reg_number =
                ((((addr >> 12) & 0x07) as usize - 3) << 1) + ((addr >> 1) & 0x01) as usize;
            if addr & 0x01 == 0 {
                self.lo_chr[reg_number] = value & 0x0F;
            } else {
                self.hi_chr[reg_number] = value & 0x1F;
            }
        } else if addr == 0xF000 {
            self.irq.set_reload_value_nibble(value, false);
        } else if addr == 0xF001 {
            self.irq.set_reload_value_nibble(value, true);
        } else if addr == 0xF002 {
            self.irq.set_control(value);
        } else if addr == 0xF003 {
            self.irq.acknowledge();
        }

        self.update_state();
    }

    fn clocks_irq(&self) -> bool {
        (self.use_heuristics && self.common.info.mapper != 22)
            || self.variant >= Vrc24Variant::Vrc4a
    }
}

impl Mapper for Vrc24 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }

    fn reset(&mut self) {
        self.apply_power_on();
    }

    fn cpu_read(&mut self, addr: u16) -> Option<u8> {
        match addr {
            0x6000..=0x7FFF => {
                if self.microwire {
                    // Open-bus upper bits are supplied by the CPU side; the
                    // mapper can only expose the latched bit here.
                    Some(self.latch as u8)
                } else {
                    Some(self.common.read_prg_ram(addr))
                }
            }
            0x8000..=0xFFFF => Some(self.common.read_prg_rom(addr)),
            _ => None,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x6000..=0x7FFF => {
                if self.microwire {
                    self.latch = (value & 0x01) == 0x01;
                } else {
                    self.common.write_prg_ram(addr, value);
                }
            }
            0x8000..=0xFFFF => self.write_register(addr, value),
            _ => {}
        }
    }

    fn cpu_cycle(&mut self) {
        if self.clocks_irq() {
            self.irq.clock();
        }
    }

    fn irq_pending(&self) -> bool {
        self.irq.line
    }

    fn battery(&self) -> Option<&[u8]> {
        if prg_ram_slot_ok(&self.common) {
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
        w.u8(self.prg_reg0);
        w.u8(self.prg_reg1);
        w.u8(self.prg_mode);
        for reg in self.lo_chr {
            w.u8(reg);
        }
        for reg in self.hi_chr {
            w.u8(reg);
        }
        w.bool(self.latch);
        self.irq.save_state(w);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        self.prg_reg0 = r.u8()?;
        self.prg_reg1 = r.u8()?;
        self.prg_mode = r.u8()?;
        for reg in &mut self.lo_chr {
            *reg = r.u8()?;
        }
        for reg in &mut self.hi_chr {
            *reg = r.u8()?;
        }
        self.latch = r.bool()?;
        self.irq.load_state(r)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// VRC3 (mapper 73)
// ---------------------------------------------------------------------------

pub struct Vrc3 {
    common: MapperCommon,
    irq_enable_on_ack: bool,
    small_counter: bool,
    irq_enabled: bool,
    irq_reload: u16,
    irq_counter: u16,
    irq_line: bool,
    prg_mapped: [bool; 4],
}

impl Vrc3 {
    fn new(rom: RomData) -> Self {
        let common = MapperCommon::new(rom);
        let mut mapper = Self {
            common,
            irq_enable_on_ack: false,
            small_counter: false,
            irq_enabled: false,
            irq_reload: 0,
            irq_counter: 0,
            irq_line: false,
            prg_mapped: [false; 4],
        };
        mapper.apply_power_on();
        mapper
    }

    fn apply_power_on(&mut self) {
        self.common.init_default_maps();
        self.common.mirroring = self.common.info.mirroring;
        self.irq_enable_on_ack = false;
        self.small_counter = false;
        self.irq_enabled = false;
        self.irq_reload = 0;
        self.irq_counter = 0;
        self.irq_line = false;
        self.prg_mapped = [false, false, true, true];
        map_prg_16k_page(&mut self.common, 1, -1);
        map_chr_8k_page(&mut self.common, 0);
    }

    fn write_register(&mut self, addr: u16, value: u8) {
        let nibble = (value & 0x0F) as u16;
        match addr & 0xF000 {
            0x8000 => self.irq_reload = (self.irq_reload & 0xFFF0) | nibble,
            0x9000 => self.irq_reload = (self.irq_reload & 0xFF0F) | (nibble << 4),
            0xA000 => self.irq_reload = (self.irq_reload & 0xF0FF) | (nibble << 8),
            0xB000 => self.irq_reload = (self.irq_reload & 0x0FFF) | (nibble << 12),
            0xC000 => {
                self.irq_enabled = (value & 0x02) == 0x02;
                if self.irq_enabled {
                    self.irq_counter = self.irq_reload;
                }
                self.small_counter = (value & 0x04) == 0x04;
                self.irq_enable_on_ack = (value & 0x01) == 0x01;
                self.irq_line = false;
            }
            0xD000 => {
                self.irq_line = false;
                self.irq_enabled = self.irq_enable_on_ack;
            }
            0xF000 => {
                let page = (value & 0x07) as isize;
                map_prg_16k_page(&mut self.common, 0, page);
                self.prg_mapped[0] = true;
                self.prg_mapped[1] = true;
            }
            _ => {}
        }
    }
}

impl Mapper for Vrc3 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }

    fn reset(&mut self) {
        self.apply_power_on();
    }

    fn cpu_read(&mut self, addr: u16) -> Option<u8> {
        match addr {
            0x6000..=0x7FFF => Some(self.common.read_prg_ram(addr)),
            0x8000..=0xFFFF => {
                let slot = ((addr - 0x8000) >> 13) as usize;
                if self.prg_mapped[slot] {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x6000..=0x7FFF => self.common.write_prg_ram(addr, value),
            0x8000..=0xFFFF => self.write_register(addr, value),
            _ => {}
        }
    }

    fn cpu_cycle(&mut self) {
        if !self.irq_enabled {
            return;
        }

        if self.small_counter {
            let small = (self.irq_counter & 0x00FF).wrapping_add(1) & 0x00FF;
            let small = if small == 0 {
                self.irq_line = true;
                self.irq_reload & 0x00FF
            } else {
                small
            };
            self.irq_counter = (self.irq_counter & 0xFF00) | small;
        } else {
            self.irq_counter = self.irq_counter.wrapping_add(1);
            if self.irq_counter == 0 {
                self.irq_counter = self.irq_reload;
                self.irq_line = true;
            }
        }
    }

    fn irq_pending(&self) -> bool {
        self.irq_line
    }

    fn battery(&self) -> Option<&[u8]> {
        if prg_ram_slot_ok(&self.common) {
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
        w.bool(self.irq_enable_on_ack);
        w.bool(self.small_counter);
        w.bool(self.irq_enabled);
        w.u16(self.irq_counter);
        w.u16(self.irq_reload);
        w.bool(self.irq_line);
        for mapped in self.prg_mapped {
            w.bool(mapped);
        }
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        self.irq_enable_on_ack = r.bool()?;
        self.small_counter = r.bool()?;
        self.irq_enabled = r.bool()?;
        self.irq_counter = r.u16()?;
        self.irq_reload = r.u16()?;
        self.irq_line = r.bool()?;
        for mapped in &mut self.prg_mapped {
            *mapped = r.bool()?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// VRC6 expansion audio (Vrc6Audio.h / Vrc6Pulse.h / Vrc6Saw.h)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct Vrc6Pulse {
    volume: u8,
    duty_cycle: u8,
    ignore_duty: bool,
    frequency: u16,
    enabled: bool,
    timer: i32,
    step: u8,
    frequency_shift: u8,
}

impl Default for Vrc6Pulse {
    fn default() -> Self {
        Self {
            volume: 0,
            duty_cycle: 0,
            ignore_duty: false,
            frequency: 1,
            enabled: false,
            timer: 1,
            step: 0,
            frequency_shift: 0,
        }
    }
}

impl Vrc6Pulse {
    fn write_reg(&mut self, addr: u16, value: u8) {
        match addr & 0x03 {
            0 => {
                self.volume = value & 0x0F;
                self.duty_cycle = (value & 0x70) >> 4;
                self.ignore_duty = (value & 0x80) == 0x80;
            }
            1 => {
                self.frequency = (self.frequency & 0x0F00) | value as u16;
            }
            2 => {
                self.frequency = (self.frequency & 0x00FF) | (((value & 0x0F) as u16) << 8);
                self.enabled = (value & 0x80) == 0x80;
                if !self.enabled {
                    self.step = 0;
                }
            }
            _ => {}
        }
    }

    fn set_frequency_shift(&mut self, shift: u8) {
        self.frequency_shift = shift;
    }

    fn clock(&mut self) {
        if self.enabled {
            self.timer -= 1;
            if self.timer == 0 {
                self.step = (self.step + 1) & 0x0F;
                self.timer = ((self.frequency >> self.frequency_shift) as i32) + 1;
            }
        }
    }

    fn volume(&self) -> u8 {
        if !self.enabled {
            0
        } else if self.ignore_duty {
            self.volume
        } else if self.step <= self.duty_cycle {
            self.volume
        } else {
            0
        }
    }

    fn save_state(&self, w: &mut StateWriter) {
        w.u8(self.volume);
        w.u8(self.duty_cycle);
        w.bool(self.ignore_duty);
        w.u16(self.frequency);
        w.bool(self.enabled);
        w.i32(self.timer);
        w.u8(self.step);
        w.u8(self.frequency_shift);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.volume = r.u8()?;
        self.duty_cycle = r.u8()?;
        self.ignore_duty = r.bool()?;
        self.frequency = r.u16()?;
        self.enabled = r.bool()?;
        self.timer = r.i32()?;
        self.step = r.u8()?;
        self.frequency_shift = r.u8()?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
struct Vrc6Saw {
    accumulator_rate: u8,
    accumulator: u8,
    frequency: u16,
    enabled: bool,
    timer: i32,
    step: u8,
    frequency_shift: u8,
}

impl Default for Vrc6Saw {
    fn default() -> Self {
        Self {
            accumulator_rate: 0,
            accumulator: 0,
            frequency: 1,
            enabled: false,
            timer: 1,
            step: 0,
            frequency_shift: 0,
        }
    }
}

impl Vrc6Saw {
    fn write_reg(&mut self, addr: u16, value: u8) {
        match addr & 0x03 {
            0 => {
                self.accumulator_rate = value & 0x3F;
            }
            1 => {
                self.frequency = (self.frequency & 0x0F00) | value as u16;
            }
            2 => {
                self.frequency = (self.frequency & 0x00FF) | (((value & 0x0F) as u16) << 8);
                self.enabled = (value & 0x80) == 0x80;
                if !self.enabled {
                    // Clearing E forces the accumulator to zero until E is set
                    // again and resets the phase, without resetting the divider.
                    self.accumulator = 0;
                    self.step = 0;
                }
            }
            _ => {}
        }
    }

    fn set_frequency_shift(&mut self, shift: u8) {
        self.frequency_shift = shift;
    }

    fn clock(&mut self) {
        if self.enabled {
            self.timer -= 1;
            if self.timer == 0 {
                self.step = (self.step + 1) % 14;
                self.timer = ((self.frequency >> self.frequency_shift) as i32) + 1;

                if self.step == 0 {
                    self.accumulator = 0;
                } else if (self.step & 0x01) == 0x00 {
                    self.accumulator = self.accumulator.wrapping_add(self.accumulator_rate);
                }
            }
        }
    }

    fn volume(&self) -> u8 {
        if !self.enabled {
            0
        } else {
            self.accumulator >> 3
        }
    }

    fn save_state(&self, w: &mut StateWriter) {
        w.u8(self.accumulator_rate);
        w.u8(self.accumulator);
        w.u16(self.frequency);
        w.bool(self.enabled);
        w.i32(self.timer);
        w.u8(self.step);
        w.u8(self.frequency_shift);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.accumulator_rate = r.u8()?;
        self.accumulator = r.u8()?;
        self.frequency = r.u16()?;
        self.enabled = r.bool()?;
        self.timer = r.i32()?;
        self.step = r.u8()?;
        self.frequency_shift = r.u8()?;
        Ok(())
    }
}

/// Full-scale sum of two 4-bit pulses and the 5-bit saw output.
const VRC6_AUDIO_FULL_SCALE: f32 = 61.0;

#[derive(Clone, Copy, Debug, Default)]
struct Vrc6Audio {
    pulse1: Vrc6Pulse,
    pulse2: Vrc6Pulse,
    saw: Vrc6Saw,
    halt_audio: bool,
    last_output: i32,
}

impl Vrc6Audio {
    fn new() -> Self {
        Self::default()
    }

    fn write_register(&mut self, addr: u16, value: u8) {
        match addr {
            0x9000 | 0x9001 | 0x9002 => self.pulse1.write_reg(addr, value),
            0x9003 => {
                self.halt_audio = (value & 0x01) == 0x01;
                let frequency_shift = if (value & 0x04) == 0x04 {
                    8
                } else if (value & 0x02) == 0x02 {
                    4
                } else {
                    0
                };
                self.pulse1.set_frequency_shift(frequency_shift);
                self.pulse2.set_frequency_shift(frequency_shift);
                self.saw.set_frequency_shift(frequency_shift);
            }
            0xA000 | 0xA001 | 0xA002 => self.pulse2.write_reg(addr, value),
            0xB000 | 0xB001 | 0xB002 => self.saw.write_reg(addr, value),
            _ => {}
        }
    }

    fn clock(&mut self) {
        if !self.halt_audio {
            self.pulse1.clock();
            self.pulse2.clock();
            self.saw.clock();
        }

        self.last_output =
            self.pulse1.volume() as i32 + self.pulse2.volume() as i32 + self.saw.volume() as i32;
    }

    fn sample(&self) -> f32 {
        (self.last_output as f32 / VRC6_AUDIO_FULL_SCALE).clamp(-1.0, 1.0)
    }

    fn save_state(&self, w: &mut StateWriter) {
        w.i32(self.last_output);
        w.bool(self.halt_audio);
        self.pulse1.save_state(w);
        self.pulse2.save_state(w);
        self.saw.save_state(w);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.last_output = r.i32()?;
        self.halt_audio = r.bool()?;
        self.pulse1.load_state(r)?;
        self.pulse2.load_state(r)?;
        self.saw.load_state(r)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// VRC6 (mappers 24 and 26)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Vrc6Model {
    A,
    B,
}

pub struct Vrc6 {
    common: MapperCommon,
    model: Vrc6Model,
    banking_mode: u8,
    chr_regs: [u8; 8],
    irq: VrcIrq,
    audio: Vrc6Audio,
    prg_ram_enabled: bool,
    prg_mapped: [bool; 4],
    chr_mapped: bool,
    /// PPU nametable mapping: CHR-ROM pages when `nt_from_chr`, otherwise
    /// CIRAM 1KB pages.
    nt_from_chr: bool,
    nt_pages: [u8; 4],
}

impl Vrc6 {
    fn new(model: Vrc6Model, rom: RomData) -> Self {
        let common = MapperCommon::new(rom);
        let mut mapper = Self {
            common,
            model,
            banking_mode: 0,
            chr_regs: [0; 8],
            irq: VrcIrq::default(),
            audio: Vrc6Audio::new(),
            prg_ram_enabled: true,
            prg_mapped: [false; 4],
            chr_mapped: false,
            nt_from_chr: false,
            nt_pages: [0, 1, 0, 1],
        };
        mapper.apply_power_on();
        mapper
    }

    fn apply_power_on(&mut self) {
        self.common.init_default_maps();
        self.common.mirroring = self.common.info.mirroring;
        self.banking_mode = 0;
        self.chr_regs = [0; 8];
        self.irq.reset();
        self.audio = Vrc6Audio::new();
        // BaseMapper's SetupDefaultWorkRam maps PRG RAM read/write at power-on.
        self.prg_ram_enabled = true;
        self.prg_mapped = [false, false, false, true];
        map_prg_8k_page(&mut self.common, 3, -1);

        // With CHR RAM the boot mapping is the full 8KB; with CHR ROM there is
        // no CHR mapping until a banking register is written.
        self.chr_mapped = self.common.chr_is_ram;
        if self.chr_mapped {
            for i in 0..8 {
                map_chr_1k_page(&mut self.common, i, i);
            }
        } else {
            for i in 0..8 {
                map_chr_1k_page(&mut self.common, i, 0);
            }
        }

        self.nt_from_chr = false;
        self.nt_pages = nt_pages_for_mirroring(self.common.mirroring);
    }

    fn update_ppu_banking(&mut self) {
        const MIRROR_TABLE: [Mirroring; 4] = [
            Mirroring::Vertical,
            Mirroring::Horizontal,
            Mirroring::ScreenAOnly,
            Mirroring::ScreenBOnly,
        ];

        let mode = self.banking_mode;
        let regs = self.chr_regs;
        let mask: u8 = if mode & 0x20 != 0 { 0xFE } else { 0xFF };
        let or_mask: u8 = if mode & 0x20 != 0 { 1 } else { 0 };

        match mode & 0x03 {
            0 => {
                for i in 0..8 {
                    map_chr_1k_page(&mut self.common, i, regs[i] as usize);
                }
            }
            1 => {
                for pair in 0..4 {
                    let base = (regs[pair] & mask) as usize;
                    map_chr_1k_page(&mut self.common, pair * 2, base);
                    map_chr_1k_page(&mut self.common, pair * 2 + 1, base | or_mask as usize);
                }
            }
            _ => {
                for i in 0..4 {
                    map_chr_1k_page(&mut self.common, i, regs[i] as usize);
                }
                for pair in 0..2 {
                    let base = (regs[4 + pair] & mask) as usize;
                    map_chr_1k_page(&mut self.common, 4 + pair * 2, base);
                    map_chr_1k_page(&mut self.common, 4 + pair * 2 + 1, base | or_mask as usize);
                }
            }
        }
        self.chr_mapped = true;

        if mode & 0x10 != 0 {
            // Nametables read from CHR ROM/RAM 1KB pages.
            self.nt_from_chr = true;
            let r = regs;
            self.nt_pages = match mode & 0x2F {
                0x20 | 0x27 => [r[6] & 0xFE, (r[6] & 0xFE) | 1, r[7] & 0xFE, (r[7] & 0xFE) | 1],
                0x23 | 0x24 => [r[6] & 0xFE, r[7] & 0xFE, (r[6] & 0xFE) | 1, (r[7] & 0xFE) | 1],
                0x28 | 0x2F => [r[6] & 0xFE, r[6] & 0xFE, r[7] & 0xFE, r[7] & 0xFE],
                0x2B | 0x2C => [
                    (r[6] & 0xFE) | 1,
                    (r[7] & 0xFE) | 1,
                    (r[6] & 0xFE) | 1,
                    (r[7] & 0xFE) | 1,
                ],
                _ => match mode & 0x07 {
                    0 | 6 | 7 => [r[6], r[6], r[7], r[7]],
                    1 | 5 => [r[4], r[5], r[6], r[7]],
                    _ => [r[6], r[7], r[6], r[7]],
                },
            };
        } else {
            // Regular CIRAM nametables.
            self.nt_from_chr = false;
            match mode & 0x2F {
                0x20 | 0x27 => {
                    self.common.mirroring = MIRROR_TABLE[0];
                    self.nt_pages = [0, 1, 0, 1];
                }
                0x23 | 0x24 => {
                    self.common.mirroring = MIRROR_TABLE[1];
                    self.nt_pages = [0, 0, 1, 1];
                }
                0x28 | 0x2F => {
                    self.common.mirroring = MIRROR_TABLE[2];
                    self.nt_pages = [0, 0, 0, 0];
                }
                0x2B | 0x2C => {
                    self.common.mirroring = MIRROR_TABLE[3];
                    self.nt_pages = [1, 1, 1, 1];
                }
                _ => {
                    // Per-nametable CIRAM selection; no enum equivalent, so
                    // ppu_read/ppu_write use nt_pages directly.
                    self.nt_pages = match mode & 0x07 {
                        0 | 6 | 7 => [regs[6] & 1, regs[6] & 1, regs[7] & 1, regs[7] & 1],
                        1 | 5 => [regs[4] & 1, regs[5] & 1, regs[6] & 1, regs[7] & 1],
                        _ => [regs[6] & 1, regs[7] & 1, regs[6] & 1, regs[7] & 1],
                    };
                }
            }
        }

        self.prg_ram_enabled = mode & 0x80 != 0;
    }

    fn write_register(&mut self, addr: u16, value: u8) {
        let addr = if self.model == Vrc6Model::B {
            (addr & 0xFFFC) | ((addr & 0x0001) << 1) | ((addr & 0x0002) >> 1)
        } else {
            addr
        };

        match addr & 0xF003 {
            0x8000..=0x8003 => {
                let page = ((value & 0x0F) as isize) << 1;
                map_prg_8k_page(&mut self.common, 0, page);
                map_prg_8k_page(&mut self.common, 1, page + 1);
                self.prg_mapped[0] = true;
                self.prg_mapped[1] = true;
            }
            0x9000..=0x9003 => self.audio.write_register(addr, value),
            0xA000..=0xA002 => self.audio.write_register(addr, value),
            0xB000..=0xB002 => self.audio.write_register(addr, value),
            0xB003 => {
                self.banking_mode = value;
                self.update_ppu_banking();
            }
            0xC000..=0xC003 => {
                map_prg_8k_page(&mut self.common, 2, (value & 0x1F) as isize);
                self.prg_mapped[2] = true;
            }
            0xD000..=0xD003 => {
                self.chr_regs[(addr & 0x03) as usize] = value;
                self.update_ppu_banking();
            }
            0xE000..=0xE003 => {
                self.chr_regs[4 + (addr & 0x03) as usize] = value;
                self.update_ppu_banking();
            }
            0xF000 => self.irq.set_reload_value(value),
            0xF001 => self.irq.set_control(value),
            0xF002 => self.irq.acknowledge(),
            _ => {}
        }
    }
}

impl Mapper for Vrc6 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }

    fn reset(&mut self) {
        self.apply_power_on();
    }

    fn cpu_read(&mut self, addr: u16) -> Option<u8> {
        match addr {
            0x6000..=0x7FFF => {
                if self.prg_ram_enabled {
                    Some(self.common.read_prg_ram(addr))
                } else {
                    None
                }
            }
            0x8000..=0xFFFF => {
                let slot = ((addr - 0x8000) >> 13) as usize;
                if self.prg_mapped[slot] {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x6000..=0x7FFF => {
                if self.prg_ram_enabled {
                    self.common.write_prg_ram(addr, value);
                }
            }
            0x8000..=0xFFFF => self.write_register(addr, value),
            _ => {}
        }
    }

    fn ppu_read(&mut self, addr: u16) -> Option<u8> {
        let addr = addr & 0x3FFF;
        match addr {
            0x0000..=0x1FFF => {
                if self.chr_mapped {
                    Some(self.common.read_chr(addr))
                } else {
                    None
                }
            }
            0x2000..=0x3EFF => {
                let index = (((addr - 0x2000) & 0x0FFF) >> 10) as usize;
                let offset = (addr & 0x03FF) as usize;
                if self.nt_from_chr {
                    let page = self.nt_pages[index] as usize;
                    let len = self.common.chr.len().max(1);
                    Some(self.common.chr[(page * CHR_PAGE_1K + offset) % len])
                } else {
                    let page = (self.nt_pages[index] & 0x03) as usize;
                    Some(self.common.nametables[page * CHR_PAGE_1K + offset])
                }
            }
            _ => None,
        }
    }

    fn ppu_write(&mut self, addr: u16, value: u8) {
        let addr = addr & 0x3FFF;
        match addr {
            0x0000..=0x1FFF => {
                if self.chr_mapped {
                    self.common.write_chr(addr, value);
                }
            }
            0x2000..=0x3EFF => {
                let index = (((addr - 0x2000) & 0x0FFF) >> 10) as usize;
                let offset = (addr & 0x03FF) as usize;
                if self.nt_from_chr {
                    if self.common.chr_is_ram {
                        let page = self.nt_pages[index] as usize;
                        let len = self.common.chr.len().max(1);
                        self.common.chr[(page * CHR_PAGE_1K + offset) % len] = value;
                    }
                } else {
                    let page = (self.nt_pages[index] & 0x03) as usize;
                    self.common.nametables[page * CHR_PAGE_1K + offset] = value;
                }
            }
            _ => {}
        }
    }

    fn cpu_cycle(&mut self) {
        self.irq.clock();
        self.audio.clock();
    }

    fn irq_pending(&self) -> bool {
        self.irq.line
    }

    fn expansion_audio_sample(&self) -> f32 {
        self.audio.sample()
    }

    fn battery(&self) -> Option<&[u8]> {
        if prg_ram_slot_ok(&self.common) {
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
        w.u8(self.banking_mode);
        for reg in self.chr_regs {
            w.u8(reg);
        }
        self.irq.save_state(w);
        self.audio.save_state(w);
        w.bool(self.prg_ram_enabled);
        for mapped in self.prg_mapped {
            w.bool(mapped);
        }
        w.bool(self.chr_mapped);
        w.bool(self.nt_from_chr);
        for page in self.nt_pages {
            w.u8(page);
        }
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        self.banking_mode = r.u8()?;
        for reg in &mut self.chr_regs {
            *reg = r.u8()?;
        }
        self.irq.load_state(r)?;
        self.audio.load_state(r)?;
        self.prg_ram_enabled = r.bool()?;
        for mapped in &mut self.prg_mapped {
            *mapped = r.bool()?;
        }
        self.chr_mapped = r.bool()?;
        self.nt_from_chr = r.bool()?;
        for page in &mut self.nt_pages {
            *page = r.u8()?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// VRC7 OPLL FM synthesizer (OpllEmulator.h, OpllChannel.h, OpllTables.h)
//
// Adapted from Nintendulator/Mitsutaka Okazaki's emu2413, as used by Mesen.
// ---------------------------------------------------------------------------

mod opll {
    use std::sync::Arc;

    use crate::state::{StateReader, StateResult, StateWriter};

    /* Size of Sintable (8 -- 18 can be used, 9 recommended). */
    const PG_BITS: u32 = 9;
    const PG_WIDTH: usize = 1 << PG_BITS;

    /* Phase increment counter. */
    const DP_BITS: u32 = 18;
    const DP_WIDTH: u32 = 1 << DP_BITS;
    const DP_BASE_BITS: u32 = DP_BITS - PG_BITS;

    /* Dynamic range (accuracy of sin table). */
    const DB_BITS: u32 = 8;
    const DB_STEP: f64 = 48.0 / (1u32 << DB_BITS) as f64;
    const DB_MUTE: i32 = 1 << DB_BITS;

    /* Dynamic range of envelope. */
    const EG_STEP: f64 = 0.375;
    const EG_BITS: u32 = 7;

    /* Dynamic range of total level. */
    const TL_STEP: f64 = 0.75;
    const TL_BITS: u32 = 6;

    /* Bits for linear value. */
    const DB2LIN_AMP_BITS: u32 = 11;
    const SLOT_AMP_BITS: u32 = DB2LIN_AMP_BITS;

    /* Bits for envelope phase incremental counter. */
    const EG_DP_BITS: u32 = 22;
    const EG_DP_WIDTH: u32 = 1 << EG_DP_BITS;

    /* Bits for pitch and amp modulator. */
    const PM_PG_BITS: u32 = 8;
    const PM_PG_WIDTH: usize = 1 << PM_PG_BITS;
    const PM_DP_BITS: u32 = 16;
    const PM_DP_WIDTH: u32 = 1 << PM_DP_BITS;
    const AM_PG_BITS: u32 = 8;
    const AM_PG_WIDTH: usize = 1 << AM_PG_BITS;
    const AM_DP_BITS: u32 = 16;
    const AM_DP_WIDTH: u32 = 1 << AM_DP_BITS;

    /* PM table is calculated by PM_AMP * pow(2, PM_DEPTH * sin(x) / 1200). */
    const PM_AMP_BITS: u32 = 8;
    const PM_AMP: f64 = (1u32 << PM_AMP_BITS) as f64;
    const PM_SPEED: f64 = 6.4;
    const PM_DEPTH: f64 = 13.75;
    const AM_SPEED: f64 = 3.7;
    const AM_DEPTH: f64 = 4.8;
    const PI: f64 = 3.14159265358979323846;

    const EG_DP_SHIFT: u32 = EG_DP_BITS - EG_BITS; // 15
    const DB2LIN_TABLE_LEN: usize = (DB_MUTE as usize) * 4;

    /* Envelope modes. */
    const SETTLE: i32 = 0;
    const ATTACK: i32 = 1;
    const DECAY: i32 = 2;
    const SUSHOLD: i32 = 3;
    const SUSTINE: i32 = 4;
    const RELEASE: i32 = 5;
    const FINISH: i32 = 6;

    /// Sustain level to envelope phase, `S2E(SL_STEP * n)`.
    const SL_TABLE: [u32; 16] = [
        0,
        262_144,
        524_288,
        786_432,
        1_048_576,
        1_310_720,
        1_572_864,
        1_835_008,
        2_097_152,
        2_359_296,
        2_621_440,
        2_883_584,
        3_145_728,
        3_407_872,
        3_670_016,
        4_194_304,
    ];

    /// Default VRC7 instrument patch ROM (NukeYKT dump).
    const DEFAULT_INST: [[u8; 8]; 15] = [
        [0x03, 0x21, 0x05, 0x06, 0xE8, 0x81, 0x42, 0x27],
        [0x13, 0x41, 0x14, 0x0D, 0xD8, 0xF6, 0x23, 0x12],
        [0x11, 0x11, 0x08, 0x08, 0xFA, 0xB2, 0x20, 0x12],
        [0x31, 0x61, 0x0C, 0x07, 0xA8, 0x64, 0x61, 0x27],
        [0x32, 0x21, 0x1E, 0x06, 0xE1, 0x76, 0x01, 0x28],
        [0x02, 0x01, 0x06, 0x00, 0xA3, 0xE2, 0xF4, 0xF4],
        [0x21, 0x61, 0x1D, 0x07, 0x82, 0x81, 0x11, 0x07],
        [0x23, 0x21, 0x22, 0x17, 0xA2, 0x72, 0x01, 0x17],
        [0x35, 0x11, 0x25, 0x00, 0x40, 0x73, 0x72, 0x01],
        [0xB5, 0x01, 0x0F, 0x0F, 0xA8, 0xA5, 0x51, 0x02],
        [0x17, 0xC1, 0x24, 0x07, 0xF8, 0xF8, 0x22, 0x12],
        [0x71, 0x23, 0x11, 0x06, 0x65, 0x74, 0x18, 0x16],
        [0x01, 0x02, 0xD3, 0x05, 0xC9, 0x95, 0x03, 0x02],
        [0x61, 0x63, 0x0C, 0x00, 0x94, 0xC0, 0x33, 0xF6],
        [0x21, 0x72, 0x0D, 0x00, 0xC1, 0xD5, 0x56, 0x06],
    ];

    #[derive(Clone, Copy, Debug, Default)]
    pub struct OpllPatch {
        pub tl: u32,
        pub fb: u32,
        pub eg: u32,
        pub ml: u32,
        pub ar: u32,
        pub dr: u32,
        pub sl: u32,
        pub rr: u32,
        pub kr: u32,
        pub kl: u32,
        pub am: u32,
        pub pm: u32,
        pub wf: u32,
    }

    pub struct OpllTables {
        pub clk: u32,
        pub rate: u32,

        pub fullsintable: [u16; PG_WIDTH],
        pub halfsintable: [u16; PG_WIDTH],

        pub pmtable: [i32; PM_PG_WIDTH],
        pub amtable: [i32; AM_PG_WIDTH],

        pub db2lin_table: [i16; DB2LIN_TABLE_LEN],

        pub ar_adjust_table: [u16; 1 << EG_BITS],

        pub dphase_ar_table: [[u32; 16]; 16],
        pub dphase_dr_table: [[u32; 16]; 16],

        pub tll_table: [[[[u32; 4]; 1 << TL_BITS]; 8]; 16],
        pub rks_table: [[[i32; 2]; 8]; 2],

        pub dphase_table: [[[u32; 16]; 8]; 512],

        pub pm_dphase: u32,
        pub am_dphase: u32,
    }

    fn tl2eg(d: u32) -> u32 {
        d * ((TL_STEP / EG_STEP) as u32)
    }

    impl OpllTables {
        pub fn new(clk: u32, rate: u32) -> Self {
            let mut tables = Self {
                clk,
                rate,
                fullsintable: [0; PG_WIDTH],
                halfsintable: [0; PG_WIDTH],
                pmtable: [0; PM_PG_WIDTH],
                amtable: [0; AM_PG_WIDTH],
                db2lin_table: [0; DB2LIN_TABLE_LEN],
                ar_adjust_table: [0; 1 << EG_BITS],
                dphase_ar_table: [[0; 16]; 16],
                dphase_dr_table: [[0; 16]; 16],
                tll_table: [[[[0; 4]; 1 << TL_BITS]; 8]; 16],
                rks_table: [[[0; 2]; 8]; 2],
                dphase_table: [[[0; 16]; 8]; 512],
                pm_dphase: 0,
                am_dphase: 0,
            };
            tables.maketables(clk, rate);
            tables
        }

        /// Adjust envelope speed which depends on sampling rate.
        fn rate_adjust(&self, x: f64) -> u32 {
            if self.rate == 49716 {
                x as u32
            } else {
                ((x * self.clk as f64 / 72.0 / self.rate as f64) + 0.5) as u32
            }
        }

        fn make_adjust_table(&mut self) {
            self.ar_adjust_table[0] = (1u32 << EG_BITS) as u16;
            let eg_height = (1u32 << EG_BITS) as f64;
            for i in 1..(1usize << EG_BITS) {
                let v = eg_height - 1.0 - eg_height * (i as f64).ln() / 128f64.ln();
                self.ar_adjust_table[i] = v as u16;
            }
        }

        fn make_db2lin_table(&mut self) {
            for i in 0..(DB_MUTE as usize) * 2 {
                let mut lin = (((1u32 << DB2LIN_AMP_BITS) - 1) as f64
                    * 10f64.powf(-(i as f64) * DB_STEP / 20.0)) as i16;
                if i >= DB_MUTE as usize {
                    lin = 0;
                }
                self.db2lin_table[i] = lin;
                self.db2lin_table[i + (DB_MUTE as usize) * 2] = -lin;
            }
        }

        fn lin2db(&self, d: f64) -> i32 {
            if d == 0.0 {
                DB_MUTE - 1
            } else {
                (-((20.0 * d.log10() / DB_STEP) as i32)).min(DB_MUTE - 1)
            }
        }

        fn make_sin_table(&mut self) {
            for i in 0..PG_WIDTH / 4 {
                let v = self.lin2db((2.0 * PI * i as f64 / PG_WIDTH as f64).sin());
                self.fullsintable[i] = v as u16;
            }
            for i in 0..PG_WIDTH / 4 {
                self.fullsintable[PG_WIDTH / 2 - 1 - i] = self.fullsintable[i];
            }
            for i in 0..PG_WIDTH / 2 {
                self.fullsintable[PG_WIDTH / 2 + i] =
                    ((DB_MUTE + DB_MUTE) as u16).wrapping_add(self.fullsintable[i]);
            }
            for i in 0..PG_WIDTH / 2 {
                self.halfsintable[i] = self.fullsintable[i];
            }
            let first = self.fullsintable[0];
            for i in PG_WIDTH / 2..PG_WIDTH {
                self.halfsintable[i] = first;
            }
        }

        fn make_pm_table(&mut self) {
            for i in 0..PM_PG_WIDTH {
                self.pmtable[i] = (PM_AMP
                    * 2f64.powf(PM_DEPTH * (2.0 * PI * i as f64 / PM_PG_WIDTH as f64).sin() / 1200.0))
                    as i32;
            }
        }

        fn make_am_table(&mut self) {
            for i in 0..AM_PG_WIDTH {
                self.amtable[i] = (AM_DEPTH / 2.0 / DB_STEP
                    * (1.0 + (2.0 * PI * i as f64 / PM_PG_WIDTH as f64).sin()))
                    as i32;
            }
        }

        fn make_dphase_table(&mut self) {
            const MLTABLE: [u32; 16] = [
                1, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 20, 24, 24, 30, 30,
            ];

            for fnum in 0..512usize {
                for block in 0..8usize {
                    for ml in 0..16usize {
                        let x = ((fnum as u32 * MLTABLE[ml]) << block) >> (20 - DP_BITS);
                        let v = self.rate_adjust(x as f64);
                        self.dphase_table[fnum][block][ml] = v;
                    }
                }
            }
        }

        fn make_tll_table(&mut self) {
            const KLTABLE: [f64; 16] = [
                0.00, 18.00, 24.00, 27.75, 30.00, 32.25, 33.75, 35.25, 36.00, 37.50, 38.25, 39.00,
                39.75, 40.50, 41.25, 42.00,
            ];

            for fnum in 0..16usize {
                for block in 0..8usize {
                    for tl in 0..(1usize << TL_BITS) {
                        for kl in 0..4usize {
                            let v = if kl == 0 {
                                tl2eg(tl as u32)
                            } else {
                                let tmp = (KLTABLE[fnum] - 6.0 * (7 - block) as f64) as i32;
                                if tmp <= 0 {
                                    tl2eg(tl as u32)
                                } else {
                                    ((tmp >> (3 - kl)) as f64 / EG_STEP) as u32
                                        + tl2eg(tl as u32)
                                }
                            };
                            self.tll_table[fnum][block][tl][kl] = v;
                        }
                    }
                }
            }
        }

        fn make_dphase_ar_table(&mut self) {
            for ar in 0..16usize {
                for rks in 0..16usize {
                    let rm = (ar + (rks >> 2)).min(15) as i32;
                    let rl = (rks & 3) as i32;
                    let v = match ar {
                        0 | 15 => 0,
                        _ => {
                            let x = (3 * (rl + 4)) << (rm + 1);
                            self.rate_adjust(x as f64)
                        }
                    };
                    self.dphase_ar_table[ar][rks] = v;
                }
            }
        }

        fn make_dphase_dr_table(&mut self) {
            for dr in 0..16usize {
                for rks in 0..16usize {
                    let rm = (dr + (rks >> 2)).min(15) as i32;
                    let rl = (rks & 3) as i32;
                    let v = if dr == 0 {
                        0
                    } else {
                        let x = (rl + 4) << (rm - 1);
                        self.rate_adjust(x as f64)
                    };
                    self.dphase_dr_table[dr][rks] = v;
                }
            }
        }

        fn make_rks_table(&mut self) {
            for fnum8 in 0..2usize {
                for block in 0..8usize {
                    for kr in 0..2usize {
                        self.rks_table[fnum8][block][kr] = if kr != 0 {
                            ((block << 1) + fnum8) as i32
                        } else {
                            (block >> 1) as i32
                        };
                    }
                }
            }
        }

        fn internal_refresh(&mut self) {
            self.make_dphase_table();
            self.make_dphase_ar_table();
            self.make_dphase_dr_table();
            let opll_rate = (self.clk / 72) as f64;
            self.pm_dphase =
                self.rate_adjust(PM_SPEED * PM_DP_WIDTH as f64 / opll_rate);
            self.am_dphase =
                self.rate_adjust(AM_SPEED * AM_DP_WIDTH as f64 / opll_rate);
        }

        fn maketables(&mut self, clk: u32, rate: u32) {
            self.clk = clk;
            self.make_pm_table();
            self.make_am_table();
            self.make_db2lin_table();
            self.make_adjust_table();
            self.make_tll_table();
            self.make_rks_table();
            self.make_sin_table();
            self.rate = rate;
            self.internal_refresh();
        }

        fn waveform_sample(&self, waveform: usize, index: usize) -> u16 {
            if waveform == 0 {
                self.fullsintable[index & (PG_WIDTH - 1)]
            } else {
                self.halfsintable[index & (PG_WIDTH - 1)]
            }
        }
    }

    pub struct OpllChannel {
        tables: Arc<OpllTables>,
        pub patch: OpllPatch,
        slot_type: i32,
        feedback: i32,
        output: [i32; 2],
        waveform: usize,
        phase: u32,
        dphase: u32,
        pgout: u32,
        fnum: i32,
        block: i32,
        volume: i32,
        sustine: i32,
        tll: u32,
        rks: u32,
        eg_mode: i32,
        eg_phase: u32,
        eg_dphase: u32,
        egout: u32,
    }

    impl OpllChannel {
        fn new(tables: Arc<OpllTables>, slot_type: i32) -> Self {
            Self {
                tables,
                patch: OpllPatch::default(),
                slot_type,
                feedback: 0,
                output: [0; 2],
                waveform: 0,
                phase: 0,
                dphase: 0,
                pgout: 0,
                fnum: 0,
                block: 0,
                volume: 0,
                sustine: 0,
                tll: 0,
                rks: 0,
                eg_mode: SETTLE,
                eg_phase: EG_DP_WIDTH,
                eg_dphase: 0,
                egout: 0,
            }
        }

        pub fn reset(&mut self, slot_type: i32) {
            self.slot_type = slot_type;
            self.waveform = 0;
            self.phase = 0;
            self.dphase = 0;
            self.output = [0; 2];
            self.feedback = 0;
            self.eg_mode = SETTLE;
            self.eg_phase = EG_DP_WIDTH;
            self.eg_dphase = 0;
            self.rks = 0;
            self.tll = 0;
            self.sustine = 0;
            self.fnum = 0;
            self.block = 0;
            self.volume = 0;
            self.pgout = 0;
            self.egout = 0;
        }

        pub fn set_sustine(&mut self, sustine: i32) {
            self.sustine = sustine;
        }

        pub fn set_volume(&mut self, volume: i32) {
            self.volume = volume;
        }

        pub fn set_fnumber(&mut self, fnum: i32) {
            self.fnum = fnum;
        }

        pub fn set_block(&mut self, block: i32) {
            self.block = block;
        }

        pub fn slot_on(&mut self) {
            self.eg_mode = ATTACK;
            self.eg_phase = 0;
            self.phase = 0;
        }

        pub fn slot_off(&mut self) {
            if self.eg_mode == ATTACK {
                let index = (self.eg_phase >> EG_DP_SHIFT).min(127) as usize;
                self.eg_phase = (self.tables.ar_adjust_table[index] as u32) << EG_DP_SHIFT;
            }
            self.eg_mode = RELEASE;
        }

        fn wave2_4pi(e: i32) -> i32 {
            e >> (SLOT_AMP_BITS as i32 - PG_BITS as i32 - 1)
        }

        fn wave2_8pi(e: i32) -> i32 {
            e >> (SLOT_AMP_BITS as i32 - PG_BITS as i32 - 2)
        }

        fn calc_phase(&mut self, lfo: i32) {
            if self.patch.pm != 0 {
                let delta = self.dphase.wrapping_mul(lfo as u32) >> PM_AMP_BITS;
                self.phase = self.phase.wrapping_add(delta);
            } else {
                self.phase = self.phase.wrapping_add(self.dphase);
            }

            self.phase &= DP_WIDTH - 1;
            self.pgout = self.phase >> DP_BASE_BITS;
        }

        fn calc_slot_car(&mut self, fm: i32) -> i32 {
            self.output[1] = self.output[0];

            if self.egout >= (DB_MUTE - 1) as u32 {
                self.output[0] = 0;
            } else {
                let phase_index = (self.pgout.wrapping_add(Self::wave2_8pi(fm) as u32)
                    & (PG_WIDTH as u32 - 1)) as usize;
                let sintbl = self.tables.waveform_sample(self.waveform, phase_index) as usize;
                self.output[0] = self.tables.db2lin_table[sintbl + self.egout as usize] as i32;
            }

            (self.output[1] + self.output[0]) >> 1
        }

        fn calc_slot_mod(&mut self) -> i32 {
            self.output[1] = self.output[0];

            if self.egout >= (DB_MUTE - 1) as u32 {
                self.output[0] = 0;
            } else if self.patch.fb != 0 {
                let fm = Self::wave2_4pi(self.feedback) >> (7 - self.patch.fb);
                let phase_index =
                    (self.pgout.wrapping_add(fm as u32) & (PG_WIDTH as u32 - 1)) as usize;
                let sintbl = self.tables.waveform_sample(self.waveform, phase_index) as usize;
                self.output[0] = self.tables.db2lin_table[sintbl + self.egout as usize] as i32;
            } else {
                let sintbl =
                    self.tables.waveform_sample(self.waveform, self.pgout as usize) as usize;
                self.output[0] = self.tables.db2lin_table[sintbl + self.egout as usize] as i32;
            }

            self.feedback = (self.output[1] + self.output[0]) >> 1;
            self.feedback
        }

        pub fn update_pg(&mut self) {
            self.dphase = self.tables.dphase_table[self.fnum as usize][self.block as usize]
                [self.patch.ml as usize];
        }

        pub fn update_tll(&mut self) {
            if self.slot_type == 0 {
                self.tll = self.tables.tll_table[(self.fnum >> 5) as usize]
                    [self.block as usize][self.patch.tl as usize][self.patch.kl as usize];
            } else {
                self.tll = self.tables.tll_table[(self.fnum >> 5) as usize]
                    [self.block as usize][self.volume as usize][self.patch.kl as usize];
            }
        }

        pub fn update_rks(&mut self) {
            self.rks = self.tables.rks_table[(self.fnum >> 8) as usize][self.block as usize]
                [self.patch.kr as usize] as u32;
        }

        pub fn update_wf(&mut self) {
            self.waveform = (self.patch.wf & 1) as usize;
        }

        pub fn update_eg(&mut self) {
            self.eg_dphase = self.calc_eg_dphase();
        }

        pub fn update_all(&mut self) {
            self.update_pg();
            self.update_tll();
            self.update_rks();
            self.update_wf();
            self.update_eg(); // EG must be updated last.
        }

        fn calc_eg_dphase(&self) -> u32 {
            match self.eg_mode {
                ATTACK => self.tables.dphase_ar_table[self.patch.ar as usize][self.rks as usize],
                DECAY => self.tables.dphase_dr_table[self.patch.dr as usize][self.rks as usize],
                SUSHOLD => 0,
                SUSTINE => self.tables.dphase_dr_table[self.patch.rr as usize][self.rks as usize],
                RELEASE => {
                    if self.sustine != 0 {
                        self.tables.dphase_dr_table[5][self.rks as usize]
                    } else if self.patch.eg != 0 {
                        self.tables.dphase_dr_table[self.patch.rr as usize][self.rks as usize]
                    } else {
                        self.tables.dphase_dr_table[7][self.rks as usize]
                    }
                }
                FINISH => 0,
                _ => 0,
            }
        }

        fn calc_envelope(&mut self, lfo: i32) {
            let mut egout: u32;

            match self.eg_mode {
                ATTACK => {
                    let index = (self.eg_phase >> EG_DP_SHIFT).min(127) as usize;
                    egout = self.tables.ar_adjust_table[index] as u32;
                    self.eg_phase = self.eg_phase.wrapping_add(self.eg_dphase);
                    if (EG_DP_WIDTH & self.eg_phase) != 0 || self.patch.ar == 15 {
                        egout = 0;
                        self.eg_phase = 0;
                        self.eg_mode = DECAY;
                        self.update_eg();
                    }
                }
                DECAY => {
                    egout = self.eg_phase >> EG_DP_SHIFT;
                    self.eg_phase = self.eg_phase.wrapping_add(self.eg_dphase);
                    if self.eg_phase >= SL_TABLE[self.patch.sl as usize] {
                        self.eg_phase = SL_TABLE[self.patch.sl as usize];
                        self.eg_mode = if self.patch.eg != 0 { SUSHOLD } else { SUSTINE };
                        self.update_eg();
                    }
                }
                SUSHOLD => {
                    egout = self.eg_phase >> EG_DP_SHIFT;
                    if self.patch.eg == 0 {
                        self.eg_mode = SUSTINE;
                        self.update_eg();
                    }
                }
                SUSTINE | RELEASE => {
                    egout = self.eg_phase >> EG_DP_SHIFT;
                    self.eg_phase = self.eg_phase.wrapping_add(self.eg_dphase);
                    if egout >= (1u32 << EG_BITS) {
                        self.eg_mode = FINISH;
                        egout = (1u32 << EG_BITS) - 1;
                    }
                }
                _ => {
                    egout = (1u32 << EG_BITS) - 1;
                }
            }

            let db_out = if self.patch.am != 0 {
                Self::eg2db((egout + self.tll) as i32) + lfo
            } else {
                Self::eg2db((egout + self.tll) as i32)
            };
            let clamped = db_out.min(DB_MUTE - 1);
            self.egout = clamped.max(0) as u32;
        }

        fn eg2db(d: i32) -> i32 {
            d * (EG_STEP / DB_STEP) as i32
        }

        pub fn save_state(&self, w: &mut StateWriter) {
            w.i32(self.slot_type);
            w.i32(self.feedback);
            w.i32(self.output[0]);
            w.i32(self.output[1]);
            w.u32(self.phase);
            w.u32(self.dphase);
            w.u32(self.pgout);
            w.i32(self.fnum);
            w.i32(self.block);
            w.i32(self.volume);
            w.i32(self.sustine);
            w.u32(self.tll);
            w.u32(self.rks);
            w.i32(self.eg_mode);
            w.u32(self.eg_phase);
            w.u32(self.eg_dphase);
            w.u32(self.egout);
            w.u32(self.patch.tl);
            w.u32(self.patch.fb);
            w.u32(self.patch.eg);
            w.u32(self.patch.ml);
            w.u32(self.patch.ar);
            w.u32(self.patch.dr);
            w.u32(self.patch.sl);
            w.u32(self.patch.rr);
            w.u32(self.patch.kr);
            w.u32(self.patch.kl);
            w.u32(self.patch.am);
            w.u32(self.patch.pm);
            w.u32(self.patch.wf);
        }

        pub fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
            self.slot_type = r.i32()?;
            self.feedback = r.i32()?;
            self.output[0] = r.i32()?;
            self.output[1] = r.i32()?;
            self.phase = r.u32()?;
            self.dphase = r.u32()?;
            self.pgout = r.u32()?;
            self.fnum = r.i32()?;
            self.block = r.i32()?;
            self.volume = r.i32()?;
            self.sustine = r.i32()?;
            self.tll = r.u32()?;
            self.rks = r.u32()?;
            self.eg_mode = r.i32()?;
            self.eg_phase = r.u32()?;
            self.eg_dphase = r.u32()?;
            self.egout = r.u32()?;
            self.patch.tl = r.u32()?;
            self.patch.fb = r.u32()?;
            self.patch.eg = r.u32()?;
            self.patch.ml = r.u32()?;
            self.patch.ar = r.u32()?;
            self.patch.dr = r.u32()?;
            self.patch.sl = r.u32()?;
            self.patch.rr = r.u32()?;
            self.patch.kr = r.u32()?;
            self.patch.kl = r.u32()?;
            self.patch.am = r.u32()?;
            self.patch.pm = r.u32()?;
            self.patch.wf = r.u32()?;
            Ok(())
        }
    }

    pub struct OpllEmulator {
        adr: u32,
        out: i32,

        realstep: u32,
        oplltime: u32,
        opllstep: u32,
        prev: i32,
        next: i32,

        low_freq: [u8; 6],
        hi_freq: [u8; 6],
        inst_vol: [u8; 6],
        cust_inst: [u8; 8],
        slot_on_flag: [i32; 12],

        pm_phase: u32,
        lfo_pm: i32,
        am_phase: u32,
        lfo_am: i32,

        patch_number: [i32; 6],
        key_status: [i32; 6],

        slots: [OpllChannel; 12],

        mask: u32,
        tables: Arc<OpllTables>,
    }

    impl OpllEmulator {
        pub fn new() -> Self {
            let tables = Arc::new(OpllTables::new(3579545, 49716));
            let slots = std::array::from_fn(|i| OpllChannel::new(tables.clone(), (i % 2) as i32));
            let mut emulator = Self {
                adr: 0,
                out: 0,
                realstep: 0,
                oplltime: 0,
                opllstep: 0,
                prev: 0,
                next: 0,
                low_freq: [0; 6],
                hi_freq: [0; 6],
                inst_vol: [0; 6],
                cust_inst: [0; 8],
                slot_on_flag: [0; 12],
                pm_phase: 0,
                lfo_pm: 0,
                am_phase: 0,
                lfo_am: 0,
                patch_number: [0; 6],
                key_status: [0; 6],
                slots,
                mask: 0,
                tables,
            };
            emulator.reset();
            emulator
        }

        pub fn reset(&mut self) {
            for i in 0..12 {
                self.slots[i].reset((i % 2) as i32);
            }

            for status in &mut self.key_status {
                *status = 0;
            }

            for i in 0..0x40 {
                self.write_reg(i, 0);
            }

            self.realstep = (1u32 << 31) / self.tables.rate;
            self.opllstep = (1u32 << 31) / (self.tables.clk / 72);
        }

        pub fn write_reg(&mut self, reg: u32, val: u32) {
            let data = val & 0xFF;
            let reg = reg & 0x3F;

            match reg {
                0x00..=0x07 => {
                    self.cust_inst[reg as usize] = data as u8;
                    for i in 0..6usize {
                        if self.patch_number[i] != 0 {
                            continue;
                        }
                        self.set_instrument(i, 0);
                        match reg {
                            0x00 => {
                                self.slots[i * 2].update_pg();
                                self.slots[i * 2].update_rks();
                                self.slots[i * 2].update_eg();
                            }
                            0x01 => {
                                self.slots[i * 2 + 1].update_pg();
                                self.slots[i * 2 + 1].update_rks();
                                self.slots[i * 2 + 1].update_eg();
                            }
                            0x02 => self.slots[i * 2].update_tll(),
                            0x03 => {
                                self.slots[i * 2].update_wf();
                                self.slots[i * 2 + 1].update_wf();
                            }
                            0x04 | 0x06 => self.slots[i * 2].update_eg(),
                            _ => self.slots[i * 2 + 1].update_eg(),
                        }
                    }
                }

                0x10..=0x15 => {
                    let ch = (reg - 0x10) as usize;
                    self.low_freq[ch] = data as u8;
                    let fnum = data as i32 + (((self.hi_freq[ch] & 1) as i32) << 8);
                    self.set_fnumber(ch, fnum);
                    self.slots[ch * 2].update_all();
                    self.slots[ch * 2 + 1].update_all();
                }

                0x20..=0x25 => {
                    let ch = (reg - 0x20) as usize;
                    self.hi_freq[ch] = data as u8;

                    let fnum = (((data & 1) as i32) << 8) + self.low_freq[ch] as i32;
                    self.set_fnumber(ch, fnum);
                    self.set_block(ch, ((data >> 1) & 7) as i32);
                    self.set_sustine(ch, ((data >> 5) & 1) as i32);
                    if data & 0x10 != 0 {
                        self.key_on(ch);
                    } else {
                        self.key_off(ch);
                    }
                    self.slots[ch * 2].update_all();
                    self.slots[ch * 2 + 1].update_all();
                    self.update_key_status();
                }

                0x30..=0x35 => {
                    let ch = (reg - 0x30) as usize;
                    self.inst_vol[ch] = data as u8;
                    let inst = (data >> 4) & 15;
                    let volume = data & 15;
                    self.set_instrument(ch, inst);
                    self.set_volume(ch, (volume << 2) as i32);
                    self.slots[ch * 2].update_all();
                    self.slots[ch * 2 + 1].update_all();
                }

                _ => {}
            }
        }

        fn set_instrument(&mut self, i: usize, inst: u32) {
            self.patch_number[i] = inst as i32;

            let src: [u8; 8] = if inst == 0 {
                self.cust_inst
            } else {
                DEFAULT_INST[(inst - 1) as usize]
            };

            let m = i * 2;
            let c = m + 1;

            self.slots[m].patch.am = ((src[0] >> 7) & 1) as u32;
            self.slots[m].patch.pm = ((src[0] >> 6) & 1) as u32;
            self.slots[m].patch.eg = ((src[0] >> 5) & 1) as u32;
            self.slots[m].patch.kr = ((src[0] >> 4) & 1) as u32;
            self.slots[m].patch.ml = (src[0] & 0xF) as u32;

            self.slots[c].patch.am = ((src[1] >> 7) & 1) as u32;
            self.slots[c].patch.pm = ((src[1] >> 6) & 1) as u32;
            self.slots[c].patch.eg = ((src[1] >> 5) & 1) as u32;
            self.slots[c].patch.kr = ((src[1] >> 4) & 1) as u32;
            self.slots[c].patch.ml = (src[1] & 0xF) as u32;

            self.slots[m].patch.kl = ((src[2] >> 6) & 3) as u32;
            self.slots[m].patch.tl = (src[2] & 0x3F) as u32;

            self.slots[c].patch.kl = ((src[3] >> 6) & 3) as u32;
            self.slots[c].patch.wf = ((src[3] >> 4) & 1) as u32;

            self.slots[m].patch.wf = ((src[3] >> 3) & 1) as u32;
            self.slots[m].patch.fb = (src[3] & 7) as u32;

            self.slots[m].patch.ar = ((src[4] >> 4) & 0xF) as u32;
            self.slots[m].patch.dr = (src[4] & 0xF) as u32;

            self.slots[c].patch.ar = ((src[5] >> 4) & 0xF) as u32;
            self.slots[c].patch.dr = (src[5] & 0xF) as u32;

            self.slots[m].patch.sl = ((src[6] >> 4) & 0xF) as u32;
            self.slots[m].patch.rr = (src[6] & 0xF) as u32;

            self.slots[c].patch.sl = ((src[7] >> 4) & 0xF) as u32;
            self.slots[c].patch.rr = (src[7] & 0xF) as u32;
        }

        fn key_on(&mut self, i: usize) {
            if self.slot_on_flag[i * 2] == 0 {
                self.slots[i * 2].slot_on();
            }
            if self.slot_on_flag[i * 2 + 1] == 0 {
                self.slots[i * 2 + 1].slot_on();
            }
            self.key_status[i] = 1;
        }

        fn key_off(&mut self, i: usize) {
            if self.slot_on_flag[i * 2 + 1] != 0 {
                self.slots[i * 2 + 1].slot_off();
            }
            self.key_status[i] = 0;
        }

        fn set_sustine(&mut self, c: usize, sustine: i32) {
            self.slots[c * 2 + 1].set_sustine(sustine);
            if self.slots[c * 2].slot_type != 0 {
                self.slots[c * 2].set_sustine(sustine);
            }
        }

        fn set_volume(&mut self, c: usize, volume: i32) {
            self.slots[c * 2 + 1].set_volume(volume);
        }

        fn set_fnumber(&mut self, c: usize, fnum: i32) {
            self.slots[c * 2 + 1].set_fnumber(fnum);
            self.slots[c * 2].set_fnumber(fnum);
        }

        fn set_block(&mut self, c: usize, block: i32) {
            self.slots[c * 2 + 1].set_block(block);
            self.slots[c * 2].set_block(block);
        }

        fn update_key_status(&mut self) {
            for ch in 0..6 {
                let value = (self.hi_freq[ch] & 0x10) as i32;
                self.slot_on_flag[ch * 2] = value;
                self.slot_on_flag[ch * 2 + 1] = value;
            }
        }

        fn update_ampm(&mut self) {
            self.pm_phase = self.pm_phase.wrapping_add(self.tables.pm_dphase) & (PM_DP_WIDTH - 1);
            self.am_phase = self.am_phase.wrapping_add(self.tables.am_dphase) & (AM_DP_WIDTH - 1);
            self.lfo_am = self.tables.amtable
                [(self.am_phase >> (AM_DP_BITS - AM_PG_BITS)) as usize];
            self.lfo_pm = self.tables.pmtable
                [(self.pm_phase >> (PM_DP_BITS - PM_PG_BITS)) as usize];
        }

        fn calc(&mut self) -> i32 {
            self.update_ampm();

            for i in 0..12 {
                let lfo_pm = self.lfo_pm;
                self.slots[i].calc_phase(lfo_pm);
                let lfo_am = self.lfo_am;
                self.slots[i].calc_envelope(lfo_am);
            }

            let mut inst = 0i32;
            for i in 0..6 {
                if (self.mask & (1 << i)) == 0 && self.slots[i * 2 + 1].eg_mode != FINISH {
                    let modulation = self.slots[i * 2].calc_slot_mod();
                    inst = inst.wrapping_add(self.slots[i * 2 + 1].calc_slot_car(modulation));
                }
            }

            self.out = inst;
            inst
        }

        pub fn get_output(&mut self) -> i16 {
            while self.realstep > self.oplltime {
                self.oplltime = self.oplltime.wrapping_add(self.opllstep);
                self.prev = self.next;
                let next = self.calc();
                self.next = next;
            }

            self.oplltime = self.oplltime.wrapping_sub(self.realstep);
            let interpolated = (self.next as f64 * (self.opllstep - self.oplltime) as f64
                + self.prev as f64 * self.oplltime as f64)
                / self.opllstep as f64;
            let output = interpolated as i16;
            self.out = output as i32;
            output
        }

        pub fn save_state(&self, w: &mut StateWriter) {
            w.u32(self.adr);
            w.i32(self.out);
            w.u32(self.realstep);
            w.u32(self.oplltime);
            w.u32(self.opllstep);
            w.i32(self.prev);
            w.i32(self.next);
            w.u32(self.pm_phase);
            w.i32(self.lfo_pm);
            w.u32(self.am_phase);
            w.i32(self.lfo_am);
            w.u32(self.mask);

            for value in self.low_freq {
                w.u8(value);
            }
            for value in self.hi_freq {
                w.u8(value);
            }
            for value in self.inst_vol {
                w.u8(value);
            }
            for value in self.cust_inst {
                w.u8(value);
            }
            for value in self.slot_on_flag {
                w.i32(value);
            }
            for value in self.patch_number {
                w.i32(value);
            }
            for value in self.key_status {
                w.i32(value);
            }

            w.u32(self.tables.clk);
            w.u32(self.tables.rate);

            for slot in &self.slots {
                slot.save_state(w);
            }
        }

        pub fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
            self.adr = r.u32()?;
            self.out = r.i32()?;
            self.realstep = r.u32()?;
            self.oplltime = r.u32()?;
            self.opllstep = r.u32()?;
            self.prev = r.i32()?;
            self.next = r.i32()?;
            self.pm_phase = r.u32()?;
            self.lfo_pm = r.i32()?;
            self.am_phase = r.u32()?;
            self.lfo_am = r.i32()?;
            self.mask = r.u32()?;

            for value in &mut self.low_freq {
                *value = r.u8()?;
            }
            for value in &mut self.hi_freq {
                *value = r.u8()?;
            }
            for value in &mut self.inst_vol {
                *value = r.u8()?;
            }
            for value in &mut self.cust_inst {
                *value = r.u8()?;
            }
            for value in &mut self.slot_on_flag {
                *value = r.i32()?;
            }
            for value in &mut self.patch_number {
                *value = r.i32()?;
            }
            for value in &mut self.key_status {
                *value = r.i32()?;
            }

            let clk = r.u32()?;
            let rate = r.u32()?;
            if clk != self.tables.clk || rate != self.tables.rate {
                self.tables = Arc::new(OpllTables::new(clk, rate));
                for slot in &mut self.slots {
                    slot.tables = self.tables.clone();
                }
            }

            for slot in &mut self.slots {
                slot.load_state(r)?;
            }

            for slot in &mut self.slots {
                slot.update_all();
            }

            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn opll_tables_match_reference_values() {
            let t = OpllTables::new(3579545, 49716);

            // dB-to-linear table and its mirrored negative half.
            assert_eq!(t.db2lin_table[0], 2047);
            assert_eq!(t.db2lin_table[256], 0);
            assert_eq!(t.db2lin_table[512], -2047);
            assert_eq!(t.db2lin_table[768], 0);

            // Attack-rate adjustment table.
            assert_eq!(t.ar_adjust_table[0], 128);

            // LFO increments: 6.4*65536/49715 and 3.7*65536/49715.
            assert_eq!(t.pm_dphase, 8);
            assert_eq!(t.am_dphase, 4);

            // Phase increment: fnum 64, block 0, ML 0 => 64*1 >> 2.
            assert_eq!(t.dphase_table[64][0][0], 16);

            // Attack/decay rate tables.
            assert_eq!(t.dphase_ar_table[1][0], 48);
            assert_eq!(t.dphase_dr_table[1][0], 4);

            // Key-scale level contribution.
            assert_eq!(t.tll_table[15][7][0][1], 26);
            assert_eq!(t.tll_table[0][0][1][0], 2);

            // Key-scale rate select.
            assert_eq!(t.rks_table[1][7][1], 15);
            assert_eq!(t.rks_table[0][7][0], 3);
        }
    }
}

use opll::OpllEmulator;

// ---------------------------------------------------------------------------
// VRC7 expansion audio (Vrc7Audio.h)
// ---------------------------------------------------------------------------

const VRC7_OPLL_RATE: f64 = 49716.0;
/// Six carriers at the maximum `DB2LIN` amplitude (6 * 2047).
const VRC7_AUDIO_FULL_SCALE: f32 = 6.0 * 2047.0;

struct Vrc7Audio {
    opll: OpllEmulator,
    current_reg: u8,
    previous_output: i16,
    clock_timer: f64,
    muted: bool,
    cpu_clock: u32,
}

impl Vrc7Audio {
    fn new(cpu_clock: u32) -> Self {
        Self {
            opll: OpllEmulator::new(),
            current_reg: 0,
            previous_output: 0,
            clock_timer: 0.0,
            muted: false,
            cpu_clock,
        }
    }

    fn clock(&mut self) {
        if self.clock_timer == 0.0 {
            self.clock_timer = self.cpu_clock as f64 / VRC7_OPLL_RATE;
        }

        self.clock_timer -= 1.0;
        if self.clock_timer <= 0.0 {
            let output = self.opll.get_output();
            self.previous_output = output;
            self.clock_timer = self.cpu_clock as f64 / VRC7_OPLL_RATE;
        }
    }

    fn set_mute_audio(&mut self, muted: bool) {
        self.muted = muted;
    }

    fn write_reg(&mut self, addr: u16, value: u8) {
        match addr & 0xF030 {
            0x9010 => self.current_reg = value,
            0x9030 => self.opll.write_reg(self.current_reg as u32, value as u32),
            _ => {}
        }
    }

    fn sample(&self) -> f32 {
        if self.muted {
            0.0
        } else {
            (self.previous_output as f32 / VRC7_AUDIO_FULL_SCALE).clamp(-1.0, 1.0)
        }
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.opll.save_state(w);
        w.u8(self.current_reg);
        w.i16(self.previous_output);
        w.f64(self.clock_timer);
        w.bool(self.muted);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.opll.load_state(r)?;
        self.current_reg = r.u8()?;
        self.previous_output = r.i16()?;
        self.clock_timer = r.f64()?;
        self.muted = r.bool()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// VRC7 (mapper 85)
// ---------------------------------------------------------------------------

pub struct Vrc7 {
    common: MapperCommon,
    control_flags: u8,
    chr_regs: [u8; 8],
    irq: VrcIrq,
    audio: Vrc7Audio,
    prg_ram_enabled: bool,
    prg_mapped: [bool; 4],
    chr_mapped: [bool; 8],
}

impl Vrc7 {
    fn new(rom: RomData) -> Self {
        let cpu_clock = rom.info.region.cpu_clock();
        let common = MapperCommon::new(rom);
        let mut mapper = Self {
            common,
            control_flags: 0,
            chr_regs: [0; 8],
            irq: VrcIrq::default(),
            audio: Vrc7Audio::new(cpu_clock),
            prg_ram_enabled: true,
            prg_mapped: [false; 4],
            chr_mapped: [false; 8],
        };
        mapper.apply_power_on();
        mapper
    }

    fn apply_power_on(&mut self) {
        self.common.init_default_maps();
        self.common.mirroring = self.common.info.mirroring;
        self.control_flags = 0;
        self.chr_regs = [0; 8];
        self.irq.reset();
        let cpu_clock = self.common.info.region.cpu_clock();
        self.audio = Vrc7Audio::new(cpu_clock);
        // SetupDefaultWorkRam leaves PRG RAM enabled until $E000 is written.
        self.prg_ram_enabled = true;
        self.prg_mapped = [false, false, false, true];
        map_prg_8k_page(&mut self.common, 3, -1);

        self.chr_mapped = [self.common.chr_is_ram; 8];
        for i in 0..8 {
            map_chr_1k_page(&mut self.common, i, if self.chr_mapped[i] { i } else { 0 });
        }
    }

    fn update_state(&mut self) {
        self.common.mirroring = match self.control_flags & 0x03 {
            0 => Mirroring::Vertical,
            1 => Mirroring::Horizontal,
            2 => Mirroring::ScreenAOnly,
            _ => Mirroring::ScreenBOnly,
        };

        self.prg_ram_enabled = (self.control_flags & 0x80) != 0;
        self.audio.set_mute_audio((self.control_flags & 0x40) != 0);
    }

    fn write_register(&mut self, mut addr: u16, value: u8) {
        if (addr & 0x10) != 0 && (addr & 0xF010) != 0x9010 {
            addr |= 0x08;
            addr &= !0x10;
        }

        match addr & 0xF038 {
            0x8000 => {
                map_prg_8k_page(&mut self.common, 0, (value & 0x3F) as isize);
                self.prg_mapped[0] = true;
            }
            0x8008 => {
                map_prg_8k_page(&mut self.common, 1, (value & 0x3F) as isize);
                self.prg_mapped[1] = true;
            }
            0x9000 => {
                map_prg_8k_page(&mut self.common, 2, (value & 0x3F) as isize);
                self.prg_mapped[2] = true;
            }
            0x9010 | 0x9030 => self.audio.write_reg(addr, value),
            0xA000 | 0xA008 | 0xB000 | 0xB008 | 0xC000 | 0xC008 | 0xD000 | 0xD008 => {
                let index = match addr & 0xF038 {
                    0xA000 => 0,
                    0xA008 => 1,
                    0xB000 => 2,
                    0xB008 => 3,
                    0xC000 => 4,
                    0xC008 => 5,
                    0xD000 => 6,
                    _ => 7,
                };
                self.chr_regs[index] = value;
                map_chr_1k_page(&mut self.common, index, value as usize);
                self.chr_mapped[index] = true;
            }
            0xE000 => {
                self.control_flags = value;
                self.update_state();
            }
            0xE008 => self.irq.set_reload_value(value),
            0xF000 => self.irq.set_control(value),
            0xF008 => self.irq.acknowledge(),
            _ => {}
        }
    }
}

impl Mapper for Vrc7 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }

    fn reset(&mut self) {
        self.apply_power_on();
    }

    fn cpu_read(&mut self, addr: u16) -> Option<u8> {
        match addr {
            0x6000..=0x7FFF => {
                if self.prg_ram_enabled {
                    Some(self.common.read_prg_ram(addr))
                } else {
                    None
                }
            }
            0x8000..=0xFFFF => {
                let slot = ((addr - 0x8000) >> 13) as usize;
                if self.prg_mapped[slot] {
                    Some(self.common.read_prg_rom(addr))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x6000..=0x7FFF => {
                if self.prg_ram_enabled {
                    self.common.write_prg_ram(addr, value);
                }
            }
            0x8000..=0xFFFF => self.write_register(addr, value),
            _ => {}
        }
    }

    fn ppu_read(&mut self, addr: u16) -> Option<u8> {
        match addr & 0x3FFF {
            0x0000..=0x1FFF => {
                let slot = ((addr & 0x1FFF) >> 10) as usize;
                if self.chr_mapped[slot] {
                    Some(self.common.read_chr(addr))
                } else {
                    None
                }
            }
            0x2000..=0x3EFF => Some(self.common.read_nametable(addr)),
            _ => None,
        }
    }

    fn ppu_write(&mut self, addr: u16, value: u8) {
        match addr & 0x3FFF {
            0x0000..=0x1FFF => {
                let slot = ((addr & 0x1FFF) >> 10) as usize;
                if self.chr_mapped[slot] {
                    self.common.write_chr(addr, value);
                }
            }
            0x2000..=0x3EFF => self.common.write_nametable(addr, value),
            _ => {}
        }
    }

    fn cpu_cycle(&mut self) {
        self.irq.clock();
        self.audio.clock();
    }

    fn irq_pending(&self) -> bool {
        self.irq.line
    }

    fn expansion_audio_sample(&self) -> f32 {
        self.audio.sample()
    }

    fn battery(&self) -> Option<&[u8]> {
        if prg_ram_slot_ok(&self.common) {
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
        w.u8(self.control_flags);
        for reg in self.chr_regs {
            w.u8(reg);
        }
        self.irq.save_state(w);
        self.audio.save_state(w);
        w.bool(self.prg_ram_enabled);
        for mapped in self.prg_mapped {
            w.bool(mapped);
        }
        for mapped in self.chr_mapped {
            w.bool(mapped);
        }
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        self.control_flags = r.u8()?;
        for reg in &mut self.chr_regs {
            *reg = r.u8()?;
        }
        self.irq.load_state(r)?;
        self.audio.load_state(r)?;
        self.prg_ram_enabled = r.bool()?;
        for mapped in &mut self.prg_mapped {
            *mapped = r.bool()?;
        }
        for mapped in &mut self.chr_mapped {
            *mapped = r.bool()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Mirroring, RomData, RomInfo};

    fn make_rom(mapper: u16, submapper: u8, prg_8k: usize, chr_1k: usize) -> RomData {
        let mut info = RomInfo::default();
        info.mapper = mapper;
        info.submapper = submapper;
        info.mirroring = Mirroring::Horizontal;
        info.prg_rom_size = prg_8k * 0x2000;
        info.chr_rom_size = chr_1k * 0x400;
        info.prg_ram_size = 0x2000;

        let prg = (0..prg_8k)
            .flat_map(|bank| std::iter::repeat(bank as u8).take(0x2000))
            .collect();
        let chr = (0..chr_1k)
            .flat_map(|page| std::iter::repeat(page as u8).take(0x400))
            .collect();

        RomData::new(info, prg, chr)
    }

    fn make_mapper(mapper: u16, submapper: u8, prg_8k: usize, chr_1k: usize) -> Box<dyn Mapper> {
        create(mapper, make_rom(mapper, submapper, prg_8k, chr_1k))
            .expect("mapper should exist")
    }

    fn read(mapper: &mut Box<dyn Mapper>, addr: u16) -> u8 {
        mapper.cpu_read(addr).expect("mapped PRG read")
    }

    fn read_chr(mapper: &mut Box<dyn Mapper>, addr: u16) -> u8 {
        mapper.ppu_read(addr).expect("mapped CHR read")
    }

    #[test]
    fn supported_ids_are_exact() {
        for id in [21u16, 22, 23, 24, 25, 26, 73, 75, 85] {
            assert!(supports(id), "mapper {id} should be supported");
            assert!(create(id, make_rom(id, 0, 8, 8)).is_some());
        }
        for id in [0u16, 20, 27, 74, 84, 86, 150] {
            assert!(!supports(id), "mapper {id} should not be claimed");
            assert!(create(id, make_rom(id, 0, 8, 8)).is_none());
        }
    }

    #[test]
    fn vrc1_prg_chr_and_mirroring() {
        let mut m = make_mapper(75, 0, 16, 128);

        // Only the last 8KB slot is mapped at power-on.
        assert_eq!(read(&mut m, 0xE000), 15);
        assert!(m.cpu_read(0x8000).is_none());

        m.cpu_write(0x8000, 5);
        m.cpu_write(0xA000, 6);
        m.cpu_write(0xC000, 7);
        assert_eq!(read(&mut m, 0x8000), 5);
        assert_eq!(read(&mut m, 0xA000), 6);
        assert_eq!(read(&mut m, 0xC000), 7);

        // 8-bit bank value wraps modulo the 8KB page count.
        m.cpu_write(0x8000, 0x12);
        assert_eq!(read(&mut m, 0x8000), 2);

        // CHR: 4KB banks, high bits come from $9000.
        m.cpu_write(0xE000, 0x02);
        assert_eq!(read_chr(&mut m, 0x0000), 8);
        m.cpu_write(0x9000, 0x02);
        assert_eq!(read_chr(&mut m, 0x0000), 72);
        assert_eq!(m.common().mirroring, Mirroring::Vertical);

        m.cpu_write(0xF000, 0x03);
        assert_eq!(read_chr(&mut m, 0x1000), 12);
        m.cpu_write(0x9000, 0x04);
        assert_eq!(read_chr(&mut m, 0x1000), 76);
        assert_eq!(m.common().mirroring, Mirroring::Vertical);

        m.cpu_write(0x9000, 0x01);
        assert_eq!(m.common().mirroring, Mirroring::Horizontal);
    }

    #[test]
    fn vrc4e_banking_irq_and_address_decode() {
        let mut m = make_mapper(23, 2, 16, 32);

        assert_eq!(read(&mut m, 0x8000), 0);
        assert_eq!(read(&mut m, 0xA000), 0);
        assert_eq!(read(&mut m, 0xC000), 14);
        assert_eq!(read(&mut m, 0xE000), 15);

        m.cpu_write(0x8000, 4);
        m.cpu_write(0xA000, 5);
        assert_eq!(read(&mut m, 0x8000), 4);
        assert_eq!(read(&mut m, 0xA000), 5);

        // VRC4e uses A2 for low/high and A3 for the second register.
        m.cpu_write(0xB000, 0x0A);
        m.cpu_write(0xB004, 0x02);
        assert_eq!(read_chr(&mut m, 0x0000), 10); // page $2A wraps to 10

        // IRQ registers are at $F000/$F004/$F008/$F00C.
        m.cpu_write(0xF000, 0x0F);
        m.cpu_write(0xF004, 0x0F);
        m.cpu_write(0xF008, 0x02);
        for _ in 0..113 {
            m.cpu_cycle();
        }
        assert!(!m.irq_pending());
        m.cpu_cycle();
        assert!(m.irq_pending());
        m.cpu_write(0xF00C, 0x00);
        assert!(!m.irq_pending());

        m.cpu_write(0x9000, 0x02);
        assert_eq!(m.common().mirroring, Mirroring::ScreenAOnly);

        // $9008 translates to $9002: PRG mode 1.
        m.cpu_write(0x9008, 0x02);
        assert_eq!(read(&mut m, 0x8000), 14); // fixed second-to-last 8KB
        assert_eq!(read(&mut m, 0xA000), 5); // prgReg1
        assert_eq!(read(&mut m, 0xC000), 4); // prgReg0
        assert_eq!(read(&mut m, 0xE000), 15); // fixed last 8KB
    }

    #[test]
    fn vrc2a_chr_page_shift_and_vrc2b_decode() {
        let mut vrc2a = make_mapper(22, 0, 8, 32);
        vrc2a.cpu_write(0xB000, 0x0A);
        vrc2a.cpu_write(0xB002, 0x02);
        // page = ($02 << 4 | $0A) >> 1 = $15
        assert_eq!(read_chr(&mut vrc2a, 0x0000), 0x15);

        vrc2a.cpu_write(0xB001, 0x06);
        assert_eq!(read_chr(&mut vrc2a, 0x0400), 3);

        let mut vrc2b = make_mapper(23, 1, 8, 32);
        vrc2b.cpu_write(0xB000, 0x05);
        vrc2b.cpu_write(0xB001, 0x01);
        assert_eq!(read_chr(&mut vrc2b, 0x0000), 0x15);
    }

    #[test]
    fn vrc3_prg_irq_and_small_counter() {
        let mut m = make_mapper(73, 0, 16, 8);

        assert_eq!(read(&mut m, 0xC000), 14);
        assert_eq!(read(&mut m, 0xE000), 15);
        assert!(m.cpu_read(0x8000).is_none());

        m.cpu_write(0xF000, 3);
        assert_eq!(read(&mut m, 0x8000), 6);
        assert_eq!(read(&mut m, 0xA000), 7);

        // Reload $FFFF, enable: one CPU cycle wraps the 16-bit counter.
        m.cpu_write(0x8000, 0x0F);
        m.cpu_write(0x9000, 0x0F);
        m.cpu_write(0xA000, 0x0F);
        m.cpu_write(0xB000, 0x0F);
        m.cpu_write(0xC000, 0x02);
        assert!(!m.irq_pending());
        m.cpu_cycle();
        assert!(m.irq_pending());
        m.cpu_write(0xD000, 0x00);
        assert!(!m.irq_pending());

        // 8-bit counter mode: low byte wraps and reloads.
        m.cpu_write(0xC000, 0x06);
        m.cpu_cycle();
        assert!(m.irq_pending());
        m.cpu_write(0xD000, 0x00);
        assert!(!m.irq_pending());
        m.cpu_cycle();
        assert!(!m.irq_pending());
    }

    #[test]
    fn vrc6_prg_chr_nametables() {
        let mut m = make_mapper(24, 0, 16, 16);

        assert_eq!(read(&mut m, 0xE000), 15);
        assert!(m.cpu_read(0x8000).is_none());

        m.cpu_write(0x8000, 0x0A);
        assert_eq!(read(&mut m, 0x8000), 4);
        assert_eq!(read(&mut m, 0xA000), 5);
        m.cpu_write(0xC000, 3);
        assert_eq!(read(&mut m, 0xC000), 3);

        m.cpu_write(0xD000, 5);
        assert_eq!(read_chr(&mut m, 0x0000), 5);
        m.cpu_write(0xD001, 6);
        assert_eq!(read_chr(&mut m, 0x0400), 6);

        // Banking mode 1: 2KB CHR pairs from registers 0-3.
        m.cpu_write(0xB003, 0x01);
        assert_eq!(read_chr(&mut m, 0x0000), 5);
        assert_eq!(read_chr(&mut m, 0x0400), 5);

        // Bit 5 masks the low CHR bit in pair modes.
        m.cpu_write(0xB003, 0x21);
        assert_eq!(read_chr(&mut m, 0x0000), 4);
        assert_eq!(read_chr(&mut m, 0x0400), 5);

        // Per-nametable CIRAM mapping (mode 0). Registers 6/7 live at $E002/$E003.
        m.cpu_write(0xB003, 0x00);
        m.cpu_write(0xE002, 0x01); // r6 = 1
        m.cpu_write(0xE003, 0x00); // r7 = 0
        m.ppu_write(0x2000, 0xAA);
        assert_eq!(m.ppu_read(0x2400).map(|v| v), Some(0xAA));
        assert_ne!(m.ppu_read(0x2800).map(|v| v), Some(0xAA));
        m.ppu_write(0x3000, 0x55);
        assert_eq!(m.ppu_read(0x2000), Some(0x55));

        // CHR nametables (mode $10).
        m.cpu_write(0xB003, 0x10);
        m.cpu_write(0xE002, 3);
        m.cpu_write(0xE003, 4);
        assert_eq!(m.ppu_read(0x2000).map(|v| v), Some(3));
        assert_eq!(m.ppu_read(0x2400).map(|v| v), Some(3));
        assert_eq!(m.ppu_read(0x2800).map(|v| v), Some(4));

        // PRG RAM gating via banking mode bit 7.
        let mut ram = make_mapper(24, 0, 16, 16);
        ram.cpu_write(0x6000, 0x12);
        assert_eq!(read(&mut ram, 0x6000), 0x12);
        ram.cpu_write(0xD000, 0x00);
        assert!(ram.cpu_read(0x6000).is_none());
        ram.cpu_write(0xB003, 0x80);
        assert_eq!(read(&mut ram, 0x6000), 0x12);
    }

    #[test]
    fn vrc6b_swaps_register_address_bits() {
        let mut m = make_mapper(26, 0, 16, 16);
        m.cpu_write(0xD002, 7); // -> $D001 (CHR register 1)
        assert_eq!(read_chr(&mut m, 0x0400), 7);
        m.cpu_write(0xD001, 8); // -> $D002 (CHR register 2)
        assert_eq!(read_chr(&mut m, 0x0800), 8);
    }

    #[test]
    fn vrc6_irq_and_audio() {
        let mut m = make_mapper(24, 0, 16, 16);

        // Pulse 1: ignore duty, volume 15, enabled with frequency 0.
        m.cpu_write(0x9000, 0x8F);
        m.cpu_write(0x9001, 0x00);
        m.cpu_write(0x9002, 0x80);
        m.cpu_cycle();
        assert!((m.expansion_audio_sample() - 15.0 / 61.0).abs() < 1e-6);

        // Halting freezes the output level.
        m.cpu_write(0x9003, 0x01);
        m.cpu_cycle();
        assert!((m.expansion_audio_sample() - 15.0 / 61.0).abs() < 1e-6);

        // Scanline mode: reload $FF fires after 114 CPU clocks.
        m.cpu_write(0xF000, 0xFF);
        m.cpu_write(0xF001, 0x02);
        for _ in 0..113 {
            m.cpu_cycle();
        }
        assert!(!m.irq_pending());
        m.cpu_cycle();
        assert!(m.irq_pending());
        m.cpu_write(0xF002, 0x00);
        assert!(!m.irq_pending());

        // Cycle mode: one clock after $FF reload raises the IRQ.
        m.cpu_write(0xF001, 0x06);
        m.cpu_cycle();
        assert!(m.irq_pending());
    }

    #[test]
    fn vrc7_banking_ram_irq_and_audio() {
        let mut m = make_mapper(85, 0, 16, 16);

        assert_eq!(read(&mut m, 0xE000), 15);
        assert!(m.cpu_read(0x8000).is_none());
        assert!(m.ppu_read(0x0000).is_none());

        m.cpu_write(0x8000, 7);
        assert_eq!(read(&mut m, 0x8000), 7);

        // $8010 is remapped to $8008 by the VRC7 register decoder.
        m.cpu_write(0x8010, 8);
        assert_eq!(read(&mut m, 0xA000), 8);
        m.cpu_write(0x9000, 9);
        assert_eq!(read(&mut m, 0xC000), 9);

        m.cpu_write(0xA000, 3);
        assert_eq!(read_chr(&mut m, 0x0000), 3);
        assert!(m.ppu_read(0x0400).is_none());
        m.cpu_write(0xA008, 4);
        assert_eq!(read_chr(&mut m, 0x0400), 4);
        m.cpu_write(0xA010, 5); // remapped to $A008
        assert_eq!(read_chr(&mut m, 0x0400), 5);

        m.cpu_write(0xE000, 0x80);
        assert_eq!(m.common().mirroring, Mirroring::Vertical);
        m.cpu_write(0x6000, 0x12);
        assert_eq!(read(&mut m, 0x6000), 0x12);

        m.cpu_write(0xE000, 0x02);
        assert_eq!(m.common().mirroring, Mirroring::ScreenAOnly);
        assert!(m.cpu_read(0x6000).is_none());

        // IRQ (reload $FF, scanline mode).
        m.cpu_write(0xE008, 0xFF);
        m.cpu_write(0xF000, 0x02);
        for _ in 0..113 {
            m.cpu_cycle();
        }
        assert!(!m.irq_pending());
        m.cpu_cycle();
        assert!(m.irq_pending());
        m.cpu_write(0xF008, 0x00);
        assert!(!m.irq_pending());

        // FM: instrument 2 on channel 0, keyed on, must produce output.
        m.cpu_write(0x9010, 0x30);
        m.cpu_write(0x9030, 0x20);
        m.cpu_write(0x9010, 0x10);
        m.cpu_write(0x9030, 0x80);
        m.cpu_write(0x9010, 0x20);
        m.cpu_write(0x9030, 0x18);
        for _ in 0..200 {
            m.cpu_cycle();
        }
        assert!(m.expansion_audio_sample().abs() > 0.0);

        // $E000 bit 6 mutes the FM output.
        m.cpu_write(0xE000, 0x40);
        assert_eq!(m.expansion_audio_sample(), 0.0);
    }

    #[test]
    fn vrc6_save_state_roundtrip() {
        let rom = make_rom(24, 0, 16, 16);
        let mut a = create(24, rom.clone()).unwrap();
        a.cpu_write(0x8000, 4);
        a.cpu_write(0xD000, 5);
        a.cpu_write(0x9000, 0x8F);
        a.cpu_write(0x9002, 0x80);
        a.cpu_cycle();
        a.cpu_write(0xF000, 0xFF);
        a.cpu_write(0xF001, 0x02);
        for _ in 0..50 {
            a.cpu_cycle();
        }

        let mut w = StateWriter::new();
        a.save_state(&mut w);
        let data = w.into_inner();

        let mut b = create(24, rom).unwrap();
        let mut r = StateReader::new(&data);
        b.load_state(&mut r).unwrap();

        assert_eq!(b.cpu_read(0x8000), a.cpu_read(0x8000));
        assert_eq!(b.cpu_read(0xE000), a.cpu_read(0xE000));
        assert_eq!(b.ppu_read(0x0000), a.ppu_read(0x0000));
        assert_eq!(b.irq_pending(), a.irq_pending());
        assert_eq!(b.expansion_audio_sample(), a.expansion_audio_sample());

        for _ in 0..50 {
            a.cpu_cycle();
            b.cpu_cycle();
        }
        assert_eq!(b.expansion_audio_sample(), a.expansion_audio_sample());
        assert_eq!(b.irq_pending(), a.irq_pending());
    }

    #[test]
    fn vrc7_save_state_roundtrip_includes_opll() {
        let rom = make_rom(85, 0, 16, 16);
        let mut a = create(85, rom.clone()).unwrap();
        a.cpu_write(0x8000, 3);
        a.cpu_write(0xA000, 5);
        a.cpu_write(0x9010, 0x30);
        a.cpu_write(0x9030, 0x20);
        a.cpu_write(0x9010, 0x10);
        a.cpu_write(0x9030, 0x80);
        a.cpu_write(0x9010, 0x20);
        a.cpu_write(0x9030, 0x18);
        for _ in 0..200 {
            a.cpu_cycle();
        }

        let mut w = StateWriter::new();
        a.save_state(&mut w);
        let data = w.into_inner();

        let mut b = create(85, rom).unwrap();
        let mut r = StateReader::new(&data);
        b.load_state(&mut r).unwrap();

        assert_eq!(b.cpu_read(0x8000), a.cpu_read(0x8000));
        assert_eq!(b.ppu_read(0x0000), a.ppu_read(0x0000));
        assert_eq!(b.expansion_audio_sample(), a.expansion_audio_sample());

        for _ in 0..200 {
            a.cpu_cycle();
            b.cpu_cycle();
        }
        assert_eq!(b.expansion_audio_sample(), a.expansion_audio_sample());
    }
}
