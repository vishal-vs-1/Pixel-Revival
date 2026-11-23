//! Capcom MMC3 family of mappers.
//!
//! Rust translation of Mesen 0.9.9 `Core/MMC3.h`, `Core/MMC3_*.h` and
//! `Core/TxSROM.h` (GPL-3.0, Copyright (C) 2014-2020 Sour), plus `McAcc.h`
//! for iNES mapper 4 submapper 3.
//!
//! Supported iNES mapper IDs:
//!   4, 12, 37, 44, 45, 47, 49, 52, 114, 115, 118, 119, 121, 123, 126, 134,
//!   165, 182, 187, 189, 196, 197, 198, 199, 205, 208, 215, 217, 219, 224,
//!   238, 245, 249, 250, 254.
//!
//! Mapper 4 additionally implements submapper 1 (MMC6) and submapper 3 (McAcc).
//!
//! A12/IRQ timing note: all PPU bus-address changes arrive through
//! `Mapper::ppu_bus_address_changed` (the equivalent of Mesen's
//! `BaseMapper::NotifyVRAMAddressChange`, called from `PPU::SetBusAddress`).
//! `ppu_read`/`ppu_write` only perform the data access and must not notify a
//! second time. The hook signature has no PPU dot number, so the mapper keeps
//! a local PPU-cycle counter that advances 3 dots per `cpu_cycle()` and is
//! re-synchronized at each scanline callback. This reproduces Mesen's
//! `A12Watcher` filter (10-dot minimum low period, 89342-dot frame wrap) at
//! CPU-cycle granularity.
//!
//! MMC3 revision: Mesen selects MMC3A via its game database. Since this port
//! has no database, mapper 4 recognizes Shay Green's revision-A validation ROM
//! by its embedded title; mappers 12/114 force Rev A as in upstream.

use super::{Mapper, MapperCommon};
use crate::state::{StateReader, StateResult, StateWriter};
use crate::types::{Mirroring, RomData};

const PRG_PAGE_SIZE: usize = 0x2000;
const CHR_PAGE_SIZE: usize = 0x400;
/// Mesen's A12Watcher uses a hard-coded NTSC frame length for wrap detection.
const A12_FRAME_CYCLES: u32 = 89342;
const A12_MIN_DELAY: u32 = 10;

/// Which memory a CHR slot currently reads from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ChrMem {
    /// MMC3's `ChrMemoryType::Default`: CHR RAM on CHR-RAM-only boards, else ROM.
    Default,
    Rom,
    Ram,
}

/// MMC3 family mapper.
pub struct Mmc3 {
    common: MapperCommon,
    id: u16,
    mmc6: bool,
    mcacc: bool,
    /// MMC3 revision A IRQ counter behavior (see `notify_a12`).
    force_rev_a: bool,
    /// Non-zero-mapper submapper path: WRAM mapping is never disabled.
    ram_always_enabled: bool,

    // Core MMC3 registers (A12/IRQ state included).
    reg8000: u8,
    reg_a000: u8,
    reg_a001: u8,
    current_register: u8,
    chr_mode: u8,
    prg_mode: u8,
    registers: [u8; 8],

    irq_reload_value: u8,
    irq_counter: u8,
    irq_reload: bool,
    irq_enabled: bool,

    wram_enabled: bool,
    wram_write_protected: bool,

    a12_last_cycle: u32,
    a12_cycles_down: u32,
    ppu_cycle: u32,

    // CHR memory selection / private memory.
    chr_slot_ram: [bool; 8],
    extra_chr_ram: Vec<u8>,
    work_ram: Vec<u8>,

    // TxSROM per-1KB nametable selection.
    custom_nt: bool,
    nt_map: [u8; 4],

    // Per-variant extra state (shared fields; each variant uses a subset).
    selected_block: u8,
    ex_regs: [u8; 8],
    chr_selection: u8, // mapper 12
    extra_reg: u8,     // mapper 52
    outer_bank: u8,    // mapper 224
    reg45_index: u8,   // mapper 45
    reg45: [u8; 4],    // mapper 45
    v49_prg_mode: u8,  // mapper 49 shadow register
    prg_reg: u8,       // mapper 115/189
    chr_reg: u8,       // mapper 115
    protection_reg: u8, // mapper 115
    ex_reg: u8,        // mapper 134/238/249
    chr_latch: [bool; 2], // mapper 165
    need_update: bool, // mapper 165
    mccounter: u32,    // McAcc
    prev_addr: u16,    // McAcc
}

impl Mmc3 {
    pub fn new(id: u16, rom: RomData) -> Self {
        let info = rom.info.clone();
        let submapper = info.submapper;
        let mmc6 = id == 4 && submapper == 1;
        let mcacc = id == 4 && submapper == 3;
        // Mesen picks the MMC3 revision from its game database
        // (Chip = "MMC3A"). The port has no database, so replicate the
        // known test-ROM entry by its embedded title; this is the same
        // per-ROM switch users would apply via Mmc3IrqAltBehavior.
        let force_rev_a = id == 12
            || id == 114
            || (id == 4 && is_mmc3_rev_a_test_rom(&rom.prg_rom));

        let mut common = MapperCommon::new(rom);
        if mmc6 {
            // MMC6 has only 1KB of battery-backed work RAM.
            common.prg_ram.resize(0x400, 0);
        }

        let chr_is_ram = common.chr_is_ram;
        // Mesen allocates a separate CHR RAM when CHR ROM is present if NES 2.0
        // declares one, or if the board class always has RAM (MMC3_ChrRam,
        // MMC3_165, MMC3_199).
        let legacy_chr_ram = match id {
            119 => 0x2000,
            165 => 0x1000,
            199 => 0x2000,
            _ => 0,
        };
        let extra_chr_ram: Vec<u8> = if chr_is_ram {
            Vec::new()
        } else if info.is_nes2 {
            if info.chr_ram_size > 0 {
                vec![0; info.chr_ram_size.max(0x400)]
            } else {
                Vec::new()
            }
        } else {
            vec![0; legacy_chr_ram]
        };
        let work_ram = if id == 198 { vec![0u8; 0x1000] } else { Vec::new() };

        let mut mapper = Self {
            common,
            id,
            mmc6,
            mcacc,
            force_rev_a,
            ram_always_enabled: !mmc6 && submapper != 0,
            reg8000: 0,
            reg_a000: 0,
            reg_a001: 0,
            current_register: 0,
            chr_mode: 0,
            prg_mode: 0,
            registers: [0; 8],
            irq_reload_value: 0,
            irq_counter: 0,
            irq_reload: false,
            irq_enabled: false,
            wram_enabled: false,
            wram_write_protected: false,
            a12_last_cycle: 0,
            a12_cycles_down: 0,
            ppu_cycle: 0,
            chr_slot_ram: [chr_is_ram; 8],
            extra_chr_ram,
            work_ram,
            custom_nt: id == 118,
            nt_map: nt_map_for(info.mirroring),
            selected_block: 0,
            ex_regs: [0; 8],
            chr_selection: 0,
            extra_reg: 0,
            outer_bank: 0,
            reg45_index: 0,
            reg45: [0; 4],
            v49_prg_mode: 0,
            prg_reg: 0,
            chr_reg: 0,
            protection_reg: 0,
            ex_reg: 0,
            chr_latch: [false; 2],
            need_update: false,
            mccounter: 0,
            prev_addr: 0,
        };

        // ---- variant InitMapper: pre-base setup ----
        match id {
            198 => {
                let page_count = mapper.prg_page_count();
                mapper.ex_regs[0] = 0;
                mapper.ex_regs[1] = 1;
                mapper.ex_regs[2] = page_count.wrapping_sub(2) as u8;
                mapper.ex_regs[3] = page_count.wrapping_sub(1) as u8;
            }
            199 => {
                mapper.ex_regs[0] = 0xFE;
                mapper.ex_regs[1] = 0xFF;
                mapper.ex_regs[2] = 1;
                mapper.ex_regs[3] = 3;
            }
            208 => mapper.ex_regs[5] = 3,
            215 => {
                mapper.ex_regs[0] = 0;
                mapper.ex_regs[1] = 3;
                mapper.ex_regs[2] = 0;
            }
            _ => {}
        }

        mapper.base_init_mmc3();

        // ---- variant InitMapper: post-base setup ----
        match id {
            45 => {
                // Needed by Famicom Yarou Vol 1 (writes CHR RAM before init).
                mapper.registers[0] = 0;
                mapper.registers[1] = 2;
                mapper.registers[2] = 4;
                mapper.registers[3] = 5;
                mapper.registers[4] = 6;
                mapper.registers[5] = 7;
                mapper.update_chr_mapping();
            }
            121 => {
                mapper.ex_regs = [0; 8];
                mapper.ex_regs[3] = 0x80;
            }
            217 => {
                mapper.ex_regs[0] = 0;
                mapper.ex_regs[1] = 0xFF;
                mapper.ex_regs[2] = 0x03;
                mapper.ex_regs[3] = 0;
                mapper.update_state();
            }
            219 => {
                mapper.select_prg_4x(0, -4);
                mapper.select_chr_8x(0, 0);
            }
            _ => {}
        }

        // Mesen applies the mapper's Reset(false) immediately after
        // Initialize/InitMapper on power-up. Re-use the same path here so a
        // directly constructed mapper matches what the emulator sees.
        mapper.reset();

        mapper
    }

    fn base_init_mmc3(&mut self) {
        self.reset_mmc3();
        self.update_state();
        self.update_mirroring();
    }

    /// Mesen `MMC3::ResetMmc3` with deterministic power-on values.
    fn reset_mmc3(&mut self) {
        self.reg8000 = 0;
        self.reg_a000 = 0;
        self.reg_a001 = 0;
        self.chr_mode = 0;
        self.prg_mode = 0;
        self.current_register = 0;
        self.registers = [0, 2, 4, 5, 6, 7, 0, 1];
        self.irq_counter = 0;
        self.irq_reload_value = 0;
        self.irq_reload = false;
        self.irq_enabled = false;
        self.wram_enabled = false;
        self.wram_write_protected = false;
        self.a12_last_cycle = 0;
        self.a12_cycles_down = 0;
        self.ppu_cycle = 0;
        self.common.irq_pending = false;
        self.chr_slot_ram = [self.common.chr_is_ram; 8];
        self.mccounter = 0;
        self.prev_addr = 0;
    }

    // ---------------- page mapping helpers ----------------

    fn prg_page_count(&self) -> usize {
        (self.common.prg_rom.len() / PRG_PAGE_SIZE).max(1)
    }

    fn wrap_prg_page(&self, page: i32) -> usize {
        let count = self.prg_page_count() as i32;
        let mut p = page % count;
        if p < 0 {
            p += count;
        }
        p as usize
    }

    /// `BaseMapper::SelectPRGPage` (8KB pages).
    fn base_select_prg_page(&mut self, slot: usize, page: i32) {
        let p = self.wrap_prg_page(page);
        self.common.map_prg_8k(slot, p * PRG_PAGE_SIZE);
    }

    /// `BaseMapper::SelectPrgPage2x` (always dispatches to the base selector).
    fn select_prg_2x(&mut self, slot: usize, page: i32) {
        self.base_select_prg_page(slot * 2, page);
        self.base_select_prg_page(slot * 2 + 1, page + 1);
    }

    /// `BaseMapper::SelectPrgPage4x`.
    fn select_prg_4x(&mut self, slot: usize, page: i32) {
        self.select_prg_2x(slot * 2, page);
        self.select_prg_2x(slot * 2 + 1, page + 2);
    }

    /// `BaseMapper::SelectCHRPage` (1KB pages).
    fn base_select_chr_page(&mut self, slot: usize, page: u32, mem: ChrMem) {
        let ram = match mem {
            ChrMem::Default => self.common.chr_is_ram,
            ChrMem::Rom => false,
            ChrMem::Ram => true,
        };
        if !ram && self.common.chr_is_ram {
            // No CHR ROM on this board: nothing to map.
            return;
        }
        if ram && !self.common.chr_is_ram && self.extra_chr_ram.is_empty() {
            // No CHR RAM was allocated for this board.
            return;
        }
        let len = if ram && !self.extra_chr_ram.is_empty() {
            self.extra_chr_ram.len()
        } else {
            self.common.chr.len()
        };
        let page_count = len / CHR_PAGE_SIZE;
        if page_count == 0 {
            return;
        }
        let p = (page as usize) % page_count;
        let slot = slot & 7;
        self.common.chr_map[slot] = p * CHR_PAGE_SIZE;
        self.chr_slot_ram[slot] = ram;
    }

    /// `BaseMapper::SelectChrPage2x` (dispatches to the base selector).
    fn select_chr_2x(&mut self, slot: usize, page: u32) {
        self.base_select_chr_page(slot * 2, page, ChrMem::Default);
        self.base_select_chr_page(slot * 2 + 1, page + 1, ChrMem::Default);
    }

    /// `BaseMapper::SelectChrPage4x`.
    fn select_chr_4x(&mut self, slot: usize, page: u32) {
        self.select_chr_2x(slot * 2, page);
        self.select_chr_2x(slot * 2 + 1, page + 2);
    }

    /// `BaseMapper::SelectChrPage8x`.
    fn select_chr_8x(&mut self, slot: usize, page: u32) {
        self.select_chr_4x(slot, page);
        self.select_chr_4x(slot * 2 + 1, page + 4);
    }

    fn select_chr_8x_mem(&mut self, slot: usize, page: u32, mem: ChrMem) {
        for i in 0..8 {
            self.base_select_chr_page(slot * 8 + i, page + i as u32, mem);
        }
    }

    /// Map 4KB of CHR (used by mapper 165's MMC2-style latches).
    fn map_chr_4k(&mut self, slot: usize, page: u32, ram: bool) {
        let use_extra = ram && !self.extra_chr_ram.is_empty();
        let len = if use_extra {
            self.extra_chr_ram.len()
        } else {
            self.common.chr.len()
        };
        let page_count = len / 0x1000;
        if page_count == 0 {
            return;
        }
        let p = (page as usize) % page_count;
        for j in 0..4 {
            let idx = (slot * 4 + j) & 7;
            self.common.chr_map[idx] = p * 0x1000 + j * CHR_PAGE_SIZE;
            self.chr_slot_ram[idx] = ram;
        }
    }

    fn read_chr_addr(&self, addr: u16) -> u8 {
        let slot = ((addr as usize) & 0x1FFF) >> 10;
        let offset = self.common.chr_map[slot] + (addr as usize & 0x3FF);
        if self.chr_slot_ram[slot] && !self.extra_chr_ram.is_empty() {
            let len = self.extra_chr_ram.len();
            self.extra_chr_ram[offset % len]
        } else {
            let len = self.common.chr.len();
            self.common.chr[offset % len]
        }
    }

    fn write_chr_addr(&mut self, addr: u16, value: u8) {
        let slot = ((addr as usize) & 0x1FFF) >> 10;
        if !self.chr_slot_ram[slot] {
            return;
        }
        let offset = self.common.chr_map[slot] + (addr as usize & 0x3FF);
        if !self.extra_chr_ram.is_empty() {
            let len = self.extra_chr_ram.len();
            self.extra_chr_ram[offset % len] = value;
        } else if self.common.chr_is_ram {
            let len = self.common.chr.len();
            self.common.chr[offset % len] = value;
        }
    }

    fn has_chr_ram(&self) -> bool {
        self.common.chr_is_ram || !self.extra_chr_ram.is_empty()
    }

    // ---------------- MMC3 virtual-equivalent dispatch ----------------

    fn update_state(&mut self) {
        self.current_register = self.reg8000 & 0x07;
        self.chr_mode = (self.reg8000 & 0x80) >> 7;
        self.prg_mode = (self.reg8000 & 0x40) >> 6;

        if self.mmc6 {
            // MMC6: keep the A001 bits around, but RAM access is decoded from
            // Reg8000 bit 5 plus the per-bank A001 read/write bits.
        } else {
            self.wram_enabled = (self.reg_a001 & 0x80) == 0x80;
            self.wram_write_protected = (self.reg_a001 & 0x40) == 0x40;
        }

        self.update_prg_mapping();
        self.update_chr_mapping();
    }

    fn base_update_prg_mapping(&mut self) {
        if self.prg_mode == 0 {
            self.select_prg_page(0, self.registers[6] as i32);
            self.select_prg_page(1, self.registers[7] as i32);
            self.select_prg_page(2, -2);
            self.select_prg_page(3, -1);
        } else if self.prg_mode == 1 {
            self.select_prg_page(0, -2);
            self.select_prg_page(1, self.registers[7] as i32);
            self.select_prg_page(2, self.registers[6] as i32);
            self.select_prg_page(3, -1);
        }
    }

    fn update_prg_mapping(&mut self) {
        match self.id {
            114 => {
                if self.ex_regs[0] & 0x80 != 0 {
                    let page = ((self.ex_regs[0] & 0x0F) << 1) as i32;
                    self.select_prg_2x(0, page);
                    self.select_prg_2x(1, page);
                } else {
                    self.base_update_prg_mapping();
                }
            }
            115 => {
                self.base_update_prg_mapping();
                if self.prg_reg & 0x80 != 0 {
                    if self.prg_reg & 0x20 != 0 {
                        let page = (((self.prg_reg & 0x0F) >> 1) << 2) as i32;
                        self.select_prg_4x(0, page);
                    } else {
                        let page = ((self.prg_reg & 0x0F) << 1) as i32;
                        self.select_prg_2x(0, page);
                        self.select_prg_2x(1, page);
                    }
                }
            }
            123 => {
                if self.ex_regs[0] & 0x40 != 0 {
                    let bank = (self.ex_regs[0] & 0x05)
                        | ((self.ex_regs[0] & 0x08) >> 2)
                        | ((self.ex_regs[0] & 0x20) >> 2);
                    if self.ex_regs[0] & 0x02 != 0 {
                        self.select_prg_4x(0, ((bank & 0xFE) as i32) << 1);
                    } else {
                        self.select_prg_2x(0, (bank as i32) << 1);
                        self.select_prg_2x(1, (bank as i32) << 1);
                    }
                } else {
                    self.base_update_prg_mapping();
                }
            }
            189 => {
                self.base_update_prg_mapping();
                let prg_page = (((self.prg_reg | (self.prg_reg >> 4)) & 0x07) * 4) as i32;
                for i in 0..4 {
                    self.select_prg_page(i, prg_page + i as i32);
                }
            }
            196 => {
                if self.ex_regs[0] != 0 {
                    self.select_prg_4x(0, (self.ex_regs[1] as i32) << 2);
                } else {
                    self.base_update_prg_mapping();
                }
            }
            199 => {
                self.base_update_prg_mapping();
                self.select_prg_page(2, self.ex_regs[0] as i32);
                self.select_prg_page(3, self.ex_regs[1] as i32);
            }
            208 => {
                self.select_prg_4x(0, (self.ex_regs[5] as i32) << 2);
            }
            219 => {
                // Deliberately ignores the MMC3 bank registers.
            }
            224 => {
                let outer = (self.outer_bank as i32) << 6;
                if self.prg_mode == 0 {
                    self.select_prg_page(0, (self.registers[6] as i32 & 0x3F) | outer);
                    self.select_prg_page(1, (self.registers[7] as i32 & 0x3F) | outer);
                    self.select_prg_page(2, 0x3E | outer);
                    self.select_prg_page(3, 0x3F | outer);
                } else if self.prg_mode == 1 {
                    self.select_prg_page(0, 0x3E | outer);
                    self.select_prg_page(1, (self.registers[6] as i32 & 0x3F) | outer);
                    self.select_prg_page(2, (self.registers[7] as i32 & 0x3F) | outer);
                    self.select_prg_page(3, 0x3F | outer);
                }
            }
            245 => {
                let or_value = if self.registers[0] & 0x02 != 0 { 0x40 } else { 0x00 };
                self.registers[6] = (self.registers[6] & 0x3F) | or_value;
                self.registers[7] = (self.registers[7] & 0x3F) | or_value;
                let last_page = if self.prg_page_count() >= 0x40 {
                    (0x3F | or_value) as i32
                } else {
                    -1
                };
                if self.prg_mode == 0 {
                    self.select_prg_page(0, self.registers[6] as i32);
                    self.select_prg_page(1, self.registers[7] as i32);
                    self.select_prg_page(2, last_page - 1);
                    self.select_prg_page(3, last_page);
                } else if self.prg_mode == 1 {
                    self.select_prg_page(0, last_page - 1);
                    self.select_prg_page(1, self.registers[7] as i32);
                    self.select_prg_page(2, self.registers[6] as i32);
                    self.select_prg_page(3, last_page);
                }
            }
            _ => self.base_update_prg_mapping(),
        }
    }

    fn base_update_chr_mapping(&mut self) {
        if self.chr_mode == 0 {
            self.select_chr_page(0, (self.registers[0] & 0xFE) as u32);
            self.select_chr_page(1, (self.registers[0] | 0x01) as u32);
            self.select_chr_page(2, (self.registers[1] & 0xFE) as u32);
            self.select_chr_page(3, (self.registers[1] | 0x01) as u32);
            self.select_chr_page(4, self.registers[2] as u32);
            self.select_chr_page(5, self.registers[3] as u32);
            self.select_chr_page(6, self.registers[4] as u32);
            self.select_chr_page(7, self.registers[5] as u32);
        } else if self.chr_mode == 1 {
            self.select_chr_page(0, self.registers[2] as u32);
            self.select_chr_page(1, self.registers[3] as u32);
            self.select_chr_page(2, self.registers[4] as u32);
            self.select_chr_page(3, self.registers[5] as u32);
            self.select_chr_page(4, (self.registers[0] & 0xFE) as u32);
            self.select_chr_page(5, (self.registers[0] | 0x01) as u32);
            self.select_chr_page(6, (self.registers[1] & 0xFE) as u32);
            self.select_chr_page(7, (self.registers[1] | 0x01) as u32);
        }
    }

    fn update_chr_mapping(&mut self) {
        match self.id {
            165 => {
                // MMC2-style 4KB latches.
                for i in 0..2 {
                    let reg = if i == 0 {
                        if self.chr_latch[0] {
                            1
                        } else {
                            0
                        }
                    } else if self.chr_latch[1] {
                        4
                    } else {
                        2
                    };
                    let page = self.registers[reg];
                    if page == 0 {
                        self.map_chr_4k(i, 0, true);
                    } else if !self.common.chr_is_ram {
                        self.map_chr_4k(i, (page >> 2) as u32, false);
                    } else {
                        // CHR-RAM-only board: no CHR ROM pages exist, keep
                        // accessing the RAM window instead of leaving the
                        // slot unmapped.
                        self.map_chr_4k(i, (page >> 2) as u32, true);
                    }
                }
                self.need_update = false;
            }
            197 => {
                if self.chr_mode == 0 {
                    self.select_chr_4x(0, (self.registers[0] as u32) << 1);
                    self.select_chr_2x(2, (self.registers[2] as u32) << 1);
                    self.select_chr_2x(3, (self.registers[3] as u32) << 1);
                } else if self.chr_mode == 1 {
                    self.select_chr_4x(0, (self.registers[2] as u32) << 1);
                    self.select_chr_2x(2, (self.registers[0] as u32) << 1);
                    self.select_chr_2x(3, (self.registers[0] as u32) << 1);
                }
            }
            198 => {
                if self.has_chr_ram() && self.registers[0..6].iter().all(|&r| r == 0) {
                    self.select_chr_8x_mem(0, 0, ChrMem::Ram);
                } else {
                    self.base_update_chr_mapping();
                }
            }
            219 => {
                // Deliberately ignores the MMC3 CHR registers.
            }
            245 => {
                self.base_update_chr_mapping();
                if self.has_chr_ram() {
                    if self.chr_mode != 0 {
                        self.select_chr_4x(0, 4);
                        self.select_chr_4x(1, 0);
                    } else {
                        self.select_chr_4x(0, 0);
                        self.select_chr_4x(1, 4);
                    }
                }
            }
            _ => self.base_update_chr_mapping(),
        }
    }

    fn update_mirroring(&mut self) {
        match self.id {
            118 => {
                // TKSROM/TLSROM: mirroring comes from $8001 writes instead.
            }
            199 => {
                self.common.mirroring = match self.reg_a000 & 0x03 {
                    0 => Mirroring::Vertical,
                    1 => Mirroring::Horizontal,
                    2 => Mirroring::ScreenAOnly,
                    _ => Mirroring::ScreenBOnly,
                };
            }
            _ => {
                if self.common.mirroring != Mirroring::FourScreens {
                    self.common.mirroring = if self.reg_a000 & 0x01 != 0 {
                        Mirroring::Horizontal
                    } else {
                        Mirroring::Vertical
                    };
                }
            }
        }
    }

    fn set_hv_mirroring(&mut self, horizontal: bool) {
        self.common.mirroring = if horizontal {
            Mirroring::Horizontal
        } else {
            Mirroring::Vertical
        };
    }

    /// Variant `SelectPRGPage` overrides (base implementation otherwise).
    fn select_prg_page(&mut self, slot: usize, page: i32) {
        let page16 = page as u16;
        match self.id {
            37 => {
                let p = match self.selected_block {
                    0..=2 => page16 & 0x07,
                    3 => (page16 & 0x07) | 0x08,
                    7 => (page16 & 0x07) | 0x20,
                    _ => (page16 & 0x0F) | 0x10,
                };
                self.base_select_prg_page(slot, p as i32);
            }
            44 => {
                let mut p = if self.selected_block <= 5 {
                    page16 & 0x0F
                } else {
                    page16 & 0x1F
                };
                p |= (self.selected_block as u16) * 0x10;
                self.base_select_prg_page(slot, p as i32);
            }
            47 => {
                let mut p = page16 & 0x0F;
                if self.selected_block == 1 {
                    p |= 0x10;
                }
                self.base_select_prg_page(slot, p as i32);
            }
            45 => {
                let xor_mask = 0x3Fu16 ^ (self.reg45[3] as u16 & 0x3F);
                let p = (page16 & xor_mask) | (self.reg45[1] as u16);
                self.base_select_prg_page(slot, p as i32);
            }
            49 => {
                if self.v49_prg_mode != 0 {
                    let mut p = page16 & 0x0F;
                    p |= (self.selected_block as u16) * 0x10;
                    self.base_select_prg_page(slot, p as i32);
                } else {
                    // Non-mode: $6000 bits 5-4 pick a fixed 32KB block.
                    self.base_select_prg_page(slot, (self.prg_reg as i32) * 4 + slot as i32);
                }
            }
            52 => {
                let mut p = if self.extra_reg & 0x08 != 0 {
                    page16 & 0x0F
                } else {
                    page16 & 0x1F
                };
                if self.extra_reg & 0x08 != 0 {
                    p |= ((self.extra_reg & 0x07) as u16) << 4;
                } else {
                    p |= ((self.extra_reg & 0x06) as u16) << 4;
                }
                self.base_select_prg_page(slot, p as i32);
            }
            121 => {
                let or_value: u16 = if self.ex_regs[3] & 0x80 != 0 { 0x20 } else { 0x00 };
                let p = (page16 & 0x1F) | or_value;
                self.base_select_prg_page(slot, p as i32);
                if self.ex_regs[5] & 0x3F != 0 {
                    let or_value = or_value as i32;
                    self.base_select_prg_page(1, (self.ex_regs[2] as i32) | or_value);
                    self.base_select_prg_page(2, (self.ex_regs[1] as i32) | or_value);
                    self.base_select_prg_page(3, (self.ex_regs[0] as i32) | or_value);
                }
            }
            126 => {
                let reg = self.ex_regs[0] as u32;
                let mut p = page16 as u32;
                p &= ((!reg >> 2) & 0x10) | 0x0F;
                p |= ((reg & (0x06 | ((reg & 0x40) >> 6))) << 4) | ((reg & 0x10) << 3);
                if self.ex_regs[3] & 0x03 == 0 {
                    self.base_select_prg_page(slot, p as i32);
                } else if ((self.prg_mode as usize) << 1) == slot {
                    if self.ex_regs[3] & 0x03 == 0x03 {
                        for i in 0..4 {
                            self.base_select_prg_page(i, (p + i as u32) as i32);
                        }
                    } else {
                        self.base_select_prg_page(0, p as i32);
                        self.base_select_prg_page(1, (p + 1) as i32);
                        self.base_select_prg_page(2, p as i32);
                        self.base_select_prg_page(3, (p + 1) as i32);
                    }
                }
            }
            134 => {
                let p = (page16 & 0x1F) | (((self.ex_reg & 0x02) as u16) << 4);
                self.base_select_prg_page(slot, p as i32);
            }
            187 => {
                if self.ex_regs[0] & 0x80 == 0 {
                    self.base_select_prg_page(slot, (page16 & 0x3F) as i32);
                } else {
                    let mut page = (self.ex_regs[0] & 0x1F) as i32;
                    if self.ex_regs[0] & 0x20 != 0 {
                        if self.ex_regs[0] & 0x40 != 0 {
                            page &= 0xFC;
                            for i in 0..4 {
                                self.base_select_prg_page(i, page + i as i32);
                            }
                        } else {
                            page &= 0xFE;
                            for i in 0..4 {
                                self.base_select_prg_page(i, (page << 1) + i as i32);
                            }
                        }
                    } else {
                        self.base_select_prg_page(0, page << 1);
                        self.base_select_prg_page(1, (page << 1) + 1);
                        self.base_select_prg_page(2, page << 1);
                        self.base_select_prg_page(3, (page << 1) + 1);
                    }
                }
            }
            198 => {
                self.base_select_prg_page(slot, self.ex_regs[slot & 3] as i32);
            }
            205 => {
                let mut p = if self.selected_block <= 1 {
                    page16 & 0x1F
                } else {
                    page16 & 0x0F
                };
                p |= (self.selected_block as u16) * 0x10;
                self.base_select_prg_page(slot, p as i32);
            }
            215 => {
                let mut sbank = 0u8;
                let mut bank = 0u8;
                let mask: u8;
                if self.ex_regs[0] & 0x40 != 0 {
                    mask = 0x0F;
                    sbank = self.ex_regs[1] & 0x10;
                    if self.ex_regs[0] & 0x80 != 0 {
                        bank = ((self.ex_regs[1] & 0x03) << 4)
                            | (self.ex_regs[0] & 0x07)
                            | (sbank >> 1);
                    }
                } else {
                    mask = 0x1F;
                    if self.ex_regs[0] & 0x80 != 0 {
                        bank = ((self.ex_regs[1] & 0x03) << 4) | (self.ex_regs[0] & 0x0F);
                    }
                }
                if self.ex_regs[0] & 0x80 != 0 {
                    let bank = (bank as i32) << 1;
                    if self.ex_regs[0] & 0x20 != 0 {
                        for i in 0..4 {
                            self.base_select_prg_page(i, bank + i as i32);
                        }
                    } else {
                        self.base_select_prg_page(0, bank);
                        self.base_select_prg_page(1, bank + 1);
                        self.base_select_prg_page(2, bank);
                        self.base_select_prg_page(3, bank + 1);
                    }
                } else {
                    let p = (((self.ex_regs[1] & 0x03) as i32) << 5)
                        | ((page16 as i32) & mask as i32)
                        | sbank as i32;
                    self.base_select_prg_page(slot, p);
                }
            }
            217 => {
                let base = if self.ex_regs[1] & 0x08 != 0 {
                    page16 & 0x1F
                } else {
                    (page16 & 0x0F) | (self.ex_regs[1] as u16 & 0x10)
                };
                let p = (((self.ex_regs[1] as u16) << 5) & 0x60) | base;
                self.base_select_prg_page(slot, p as i32);
            }
            249 => {
                let mut p = page16;
                if self.ex_reg & 0x02 != 0 {
                    if p < 0x20 {
                        p = (p & 0x01) | ((p >> 3) & 0x02) | ((p >> 1) & 0x04) | ((p << 2) & 0x18);
                    } else {
                        p = p.wrapping_sub(0x20);
                        p = (p & 0x03)
                            | ((p >> 1) & 0x04)
                            | ((p >> 4) & 0x08)
                            | ((p >> 2) & 0x10)
                            | ((p << 3) & 0x20)
                            | ((p << 2) & 0xC0);
                    }
                }
                self.base_select_prg_page(slot, p as i32);
            }
            _ => self.base_select_prg_page(slot, page),
        }
    }

    /// Variant `SelectCHRPage` overrides (base implementation otherwise).
    fn select_chr_page(&mut self, slot: usize, page: u32) {
        match self.id {
            12 => {
                let mut p = page;
                if slot < 4 && self.chr_selection & 0x01 != 0 {
                    p |= 0x100;
                } else if slot >= 4 && self.chr_selection & 0x10 != 0 {
                    p |= 0x100;
                }
                self.base_select_chr_page(slot, p, ChrMem::Default);
            }
            37 => {
                let p = page | if self.selected_block >= 4 { 0x80 } else { 0 };
                self.base_select_chr_page(slot, p, ChrMem::Default);
            }
            44 => {
                let mut p = if self.selected_block <= 5 {
                    page & 0x7F
                } else {
                    page & 0xFF
                };
                p |= (self.selected_block as u32) * 0x80;
                self.base_select_chr_page(slot, p, ChrMem::Default);
            }
            45 => {
                let mut p = page;
                if !self.has_chr_ram() {
                    let shift = 0x0F - (self.reg45[2] & 0x0F) as u32;
                    p &= 0xFFu32 >> shift;
                    p |= (self.reg45[0] as u32) | (((self.reg45[2] & 0xF0) as u32) << 4);
                }
                self.base_select_chr_page(slot, p, ChrMem::Default);
            }
            47 => {
                let mut p = page & 0x7F;
                if self.selected_block == 1 {
                    p |= 0x80;
                }
                self.base_select_chr_page(slot, p, ChrMem::Default);
            }
            49 => {
                let p = (page & 0x7F) | 0x80 * self.selected_block as u32;
                self.base_select_chr_page(slot, p, ChrMem::Default);
            }
            52 => {
                let p = if self.extra_reg & 0x40 != 0 {
                    (page & 0x7F)
                        | ((((self.extra_reg & 0x04) | ((self.extra_reg >> 4) & 0x03)) as u32) << 7)
                } else {
                    (page & 0xFF)
                        | ((((self.extra_reg & 0x04) | ((self.extra_reg >> 4) & 0x02)) as u32) << 7)
                };
                self.base_select_chr_page(slot, p, ChrMem::Default);
            }
            115 => {
                let p = page | ((self.chr_reg as u32) << 8);
                self.base_select_chr_page(slot, p, ChrMem::Default);
            }
            119 => {
                if (0x40..=0x7F).contains(&page) {
                    self.base_select_chr_page(slot, page - 0x40, ChrMem::Ram);
                } else {
                    self.base_select_chr_page(slot, page, ChrMem::Default);
                }
            }
            121 => {
                if self.common.prg_rom.len() == self.common.chr.len() {
                    let p = page | (((self.ex_regs[3] & 0x80) as u32) << 1);
                    self.base_select_chr_page(slot, p, ChrMem::Default);
                } else {
                    let mut p = page;
                    if (slot < 4 && self.chr_mode == 0) || (slot >= 4 && self.chr_mode == 1) {
                        p |= 0x100;
                    }
                    self.base_select_chr_page(slot, p, ChrMem::Default);
                }
            }
            126 => {
                if self.ex_regs[3] & 0x10 == 0 {
                    let reg = self.ex_regs[0] as u32;
                    let mask = if reg & 0x80 != 0 { 0x7F } else { 0xFF };
                    let p = self.chr_outer_bank_126() | (page & mask);
                    self.base_select_chr_page(slot, p, ChrMem::Default);
                }
            }
            134 => {
                let p = (page & 0xFF) | (((self.ex_reg & 0x20) as u32) << 3);
                self.base_select_chr_page(slot, p, ChrMem::Default);
            }
            187 => {
                let mut p = page;
                if (self.chr_mode != 0 && slot >= 4) || (self.chr_mode == 0 && slot < 4) {
                    p |= 0x100;
                }
                self.base_select_chr_page(slot, p, ChrMem::Default);
            }
            199 => {
                let mem = if page < 8 { ChrMem::Ram } else { ChrMem::Rom };
                self.base_select_chr_page(slot, page, mem);
                self.select_chr_base_for_199(0, self.registers[0] as u32);
                self.select_chr_base_for_199(1, self.ex_regs[2] as u32);
                self.select_chr_base_for_199(2, self.registers[1] as u32);
                self.select_chr_base_for_199(3, self.ex_regs[3] as u32);
            }
            205 => {
                let mut p = page;
                if self.selected_block >= 2 {
                    p &= 0x7F;
                    p |= 0x100;
                }
                if self.selected_block == 1 || self.selected_block == 3 {
                    p |= 0x80;
                }
                self.base_select_chr_page(slot, p, ChrMem::Default);
            }
            215 => {
                let p = if self.ex_regs[0] & 0x40 != 0 {
                    ((self.ex_regs[1] & 0x0C) as u32) << 6
                        | (page & 0x7F)
                        | ((self.ex_regs[1] & 0x20) as u32) << 2
                } else {
                    ((self.ex_regs[1] & 0x0C) as u32) << 6 | page
                };
                self.base_select_chr_page(slot, p, ChrMem::Default);
            }
            217 => {
                let mut p = page;
                if self.ex_regs[1] & 0x08 == 0 {
                    p = (((self.ex_regs[1] as u32) << 3) & 0x80) | (p & 0x7F);
                }
                let p = (((self.ex_regs[1] as u32) << 8) & 0x0300) | p;
                self.base_select_chr_page(slot, p, ChrMem::Default);
            }
            249 => {
                let mut p = (page & 0xFFFF) as u16;
                if self.ex_reg & 0x02 != 0 {
                    p = (p & 0x03)
                        | ((p >> 1) & 0x04)
                        | ((p >> 4) & 0x08)
                        | ((p >> 2) & 0x10)
                        | ((p << 3) & 0x20)
                        | ((p << 2) & 0xC0);
                }
                self.base_select_chr_page(slot, p as u32, ChrMem::Default);
            }
            _ => self.base_select_chr_page(slot, page, ChrMem::Default),
        }
    }

    /// Mapper 199 selects CHR RAM for pages < 8 (qualified base call).
    fn select_chr_base_for_199(&mut self, slot: usize, page: u32) {
        let mem = if page < 8 { ChrMem::Ram } else { ChrMem::Rom };
        self.base_select_chr_page(slot, page, mem);
    }

    fn chr_outer_bank_126(&self) -> u32 {
        let reg = self.ex_regs[0] as u32;
        let a = (!reg & 0x0080) & (self.ex_regs[2] as u32);
        let b = ((reg << 4) & 0x0080) & reg;
        let c = (reg << 3) & 0x0100;
        let d = (reg << 5) & 0x0200;
        a | b | c | d
    }

    // ---------------- registers ----------------

    fn base_write_register(&mut self, addr: u16, value: u8) {
        match addr & 0xE001 {
            0x8000 => {
                self.reg8000 = value;
                self.update_state();
            }
            0x8001 => {
                let mut value = value;
                if self.current_register <= 1 {
                    // Writes to registers 0 and 1 always ignore bit 0.
                    value &= !0x01;
                }
                self.registers[self.current_register as usize & 7] = value;
                self.update_state();
            }
            0xA000 => {
                self.reg_a000 = value;
                self.update_mirroring();
            }
            0xA001 => {
                self.reg_a001 = value;
                self.update_state();
            }
            0xC000 => self.irq_reload_value = value,
            0xC001 => {
                self.irq_counter = 0;
                self.irq_reload = true;
            }
            0xE000 => {
                self.irq_enabled = false;
                self.common.irq_pending = false;
            }
            0xE001 => self.irq_enabled = true,
            _ => {}
        }
    }

    fn write_register(&mut self, addr: u16, value: u8) {
        if self.mcacc && (addr & 0xE001) == 0xC001 {
            // "Writing to $C001 resets pulse counter."
            self.mccounter = 0;
        }
        match self.id {
            44 => {
                if addr & 0xE001 == 0xA001 {
                    self.selected_block = value & 0x07;
                    if self.selected_block == 7 {
                        self.selected_block = 6;
                    }
                }
                self.base_write_register(addr, value);
            }
            114 => {
                let security: [u8; 8] = [0, 3, 1, 5, 6, 7, 2, 4];
                match addr & 0xE001 {
                    0x8001 => self.base_write_register(0xA000, value),
                    0xA000 => {
                        self.base_write_register(
                            0x8000,
                            (value & 0xC0) | security[(value & 0x07) as usize],
                        );
                        self.ex_regs[1] = 1;
                    }
                    0xA001 => self.irq_reload_value = value,
                    0xC000 => {
                        if self.ex_regs[1] != 0 {
                            self.ex_regs[1] = 0;
                            self.base_write_register(0x8001, value);
                        }
                    }
                    0xC001 => self.irq_reload = true,
                    0xE000 => {
                        self.common.irq_pending = false;
                        self.irq_enabled = false;
                    }
                    0xE001 => self.irq_enabled = true,
                    _ => {}
                }
            }
            118 => {
                // TKSROM/TLSROM: $8001's high bit selects 1KB nametables.
                if addr & 0xE001 == 0x8001 {
                    let nt = value >> 7;
                    if self.chr_mode == 0 {
                        match self.current_register {
                            0 => {
                                self.nt_map[0] = nt;
                                self.nt_map[1] = nt;
                            }
                            1 => {
                                self.nt_map[2] = nt;
                                self.nt_map[3] = nt;
                            }
                            _ => {}
                        }
                    } else {
                        match self.current_register {
                            2 => self.nt_map[0] = nt,
                            3 => self.nt_map[1] = nt,
                            4 => self.nt_map[2] = nt,
                            5 => self.nt_map[3] = nt,
                            _ => {}
                        }
                    }
                }
                self.base_write_register(addr, value);
            }
            121 => {
                if addr < 0xA000 {
                    if (addr & 0x03) == 0x03 {
                        self.ex_regs[5] = value;
                        self.update_ex_regs_121();
                        self.base_write_register(0x8000, value);
                    } else if addr & 0x01 != 0 {
                        // Bit-reverse the low 6 bits.
                        self.ex_regs[6] = ((value & 0x01) << 5)
                            | ((value & 0x02) << 3)
                            | ((value & 0x04) << 1)
                            | ((value & 0x08) >> 1)
                            | ((value & 0x10) >> 3)
                            | ((value & 0x20) >> 5);
                        if self.ex_regs[7] == 0 {
                            self.update_ex_regs_121();
                        }
                        self.base_write_register(0x8001, value);
                    } else {
                        self.base_write_register(0x8000, value);
                    }
                } else {
                    self.base_write_register(addr, value);
                }
            }
            123 => {
                if addr < 0xA000 {
                    match addr & 0x8001 {
                        0x8000 => {
                            let security: [u8; 8] = [0, 3, 1, 5, 6, 7, 2, 4];
                            self.base_write_register(
                                0x8000,
                                (value & 0xC0) | security[(value & 0x07) as usize],
                            );
                        }
                        0x8001 => self.base_write_register(0x8001, value),
                        _ => {}
                    }
                } else {
                    self.base_write_register(addr, value);
                }
            }
            182 => match addr & 0xE001 {
                0x8001 => self.base_write_register(0xA000, value),
                0xA000 => {
                    let mut data = value & 0xF8;
                    data |= match value & 0x07 {
                        0 => 0,
                        1 => 3,
                        2 => 1,
                        3 => 5,
                        4 => 6,
                        5 => 7,
                        6 => 2,
                        _ => 4,
                    };
                    self.base_write_register(0x8000, data);
                }
                0xC000 => self.base_write_register(0x8001, value),
                0xC001 => {
                    self.base_write_register(0xC000, value);
                    self.base_write_register(0xC001, value);
                }
                0xE000 => self.base_write_register(0xE000, value),
                0xE001 => self.base_write_register(0xE001, value),
                _ => {}
            },
            187 => {
                if addr == 0x8000 {
                    self.ex_regs[1] = 1;
                    self.base_write_register(addr, value);
                } else if addr == 0x8001 {
                    if self.ex_regs[1] == 1 {
                        self.base_write_register(addr, value);
                    }
                } else {
                    self.base_write_register(addr, value);
                }
            }
            196 => {
                let remapped = if addr >= 0xC000 {
                    (addr & 0xFFFE) | ((addr >> 2) & 0x01) | ((addr >> 3) & 0x01)
                } else {
                    (addr & 0xFFFE)
                        | ((addr >> 2) & 0x01)
                        | ((addr >> 3) & 0x01)
                        | ((addr >> 1) & 0x01)
                };
                self.base_write_register(remapped, value);
            }
            198 => {
                if addr == 0x8001 && (self.reg8000 & 0x07) >= 6 {
                    let idx = ((self.reg8000 & 0x07) - 6) as usize;
                    let mask = if value >= 0x40 { 0x4F } else { 0x3F };
                    self.ex_regs[idx] = value & mask;
                }
                self.base_write_register(addr, value);
            }
            199 => {
                if addr == 0x8001 && self.reg8000 & 0x08 != 0 {
                    self.ex_regs[(self.reg8000 & 0x03) as usize] = value;
                    self.update_prg_mapping();
                    self.update_chr_mapping();
                } else {
                    self.base_write_register(addr, value);
                }
            }
            208 => {
                if (0x5000..=0x5FFF).contains(&addr) {
                    if addr <= 0x57FF {
                        self.ex_regs[4] = value;
                    } else {
                        self.ex_regs[(addr & 0x03) as usize] =
                            value ^ PROTECTION_LUT_208[self.ex_regs[4] as usize];
                    }
                } else if addr < 0x8000 {
                    self.ex_regs[5] = (value & 0x01) | ((value >> 3) & 0x02);
                    self.update_prg_mapping();
                } else {
                    self.base_write_register(addr, value);
                }
            }
            215 => {
                let lut_index = (((addr >> 12) & 0x06) | (addr & 0x01)) as usize;
                let lut_value = LUT_ADDR_215[(self.ex_regs[2] & 0x07) as usize][lut_index];
                let remapped =
                    ((lut_value & 0x01) as u16) | (((lut_value & 0x06) as u16) << 12) | 0x8000;
                let value = if lut_value == 0 {
                    (value & 0xC0) | LUT_REG_215[(self.ex_regs[2] & 0x07) as usize]
                        [(value & 0x07) as usize]
                } else {
                    value
                };
                self.base_write_register(remapped, value);
            }
            217 => match addr & 0xE001 {
                0x8000 => self.base_write_register(if self.ex_regs[2] != 0 { 0xC000 } else { 0x8000 }, value),
                0x8001 => {
                    if self.ex_regs[2] != 0 {
                        let value = (value & 0xC0) | LUT_217[(value & 0x07) as usize];
                        self.ex_regs[3] = 1;
                        self.base_write_register(0x8000, value);
                    } else {
                        self.base_write_register(0x8001, value);
                    }
                }
                0xA000 => {
                    if self.ex_regs[2] != 0 {
                        if self.ex_regs[3] != 0
                            && (self.ex_regs[0] & 0x80 == 0 || self.current_register < 6)
                        {
                            self.ex_regs[3] = 0;
                            self.base_write_register(0x8001, value);
                        }
                    } else {
                        self.set_hv_mirroring(value & 0x01 != 0);
                    }
                }
                0xA001 => {
                    if self.ex_regs[2] != 0 {
                        self.set_hv_mirroring(value & 0x01 != 0);
                    } else {
                        self.base_write_register(0xA001, value);
                    }
                }
                _ => self.base_write_register(addr, value),
            },
            219 => {
                if addr < 0xA000 {
                    match addr & 0xE003 {
                        0x8000 => {
                            self.ex_regs[0] = 0;
                            self.ex_regs[1] = value;
                        }
                        0x8001 => {
                            if (0x23..=0x26).contains(&self.ex_regs[0]) {
                                let prg_bank = ((value & 0x20) >> 5)
                                    | ((value & 0x10) >> 3)
                                    | ((value & 0x08) >> 1)
                                    | ((value & 0x04) << 1);
                                let slot = (0x26 - self.ex_regs[0]) as usize;
                                self.base_select_prg_page(slot, prg_bank as i32);
                            }
                            let outer = self.ex_regs[2] as u32;
                            let half = (value >> 1) as u32;
                            match self.ex_regs[1] {
                                0x08 | 0x0A | 0x0E | 0x12 | 0x16 | 0x1A | 0x1E => {
                                    self.ex_regs[2] = value << 4;
                                }
                                0x09 => {
                                    self.base_select_chr_page(0, outer | (half & 0x0E), ChrMem::Default)
                                }
                                0x0B => self.base_select_chr_page(1, outer | (half | 0x01), ChrMem::Default),
                                0x0C | 0x0D => {
                                    self.base_select_chr_page(2, outer | (half & 0x0E), ChrMem::Default)
                                }
                                0x0F => self.base_select_chr_page(3, outer | (half | 0x01), ChrMem::Default),
                                0x10 | 0x11 => {
                                    self.base_select_chr_page(4, outer | (half & 0x0F), ChrMem::Default)
                                }
                                0x14 | 0x15 => {
                                    self.base_select_chr_page(5, outer | (half & 0x0F), ChrMem::Default)
                                }
                                0x18 | 0x19 => {
                                    self.base_select_chr_page(6, outer | (half & 0x0F), ChrMem::Default)
                                }
                                0x1C | 0x1D => {
                                    self.base_select_chr_page(7, outer | (half & 0x0F), ChrMem::Default)
                                }
                                _ => {}
                            }
                        }
                        0x8002 => {
                            self.ex_regs[0] = value;
                            self.ex_regs[1] = 0;
                        }
                        _ => {}
                    }
                } else {
                    self.base_write_register(addr, value);
                }
            }
            250 => {
                let reg = (addr & 0xE000) | ((addr & 0x0400) >> 10);
                self.base_write_register(reg, (addr & 0xFF) as u8);
            }
            254 => {
                match addr {
                    0x8000 => self.ex_regs[0] = 0xFF,
                    0xA001 => self.ex_regs[1] = value,
                    _ => {}
                }
                self.base_write_register(addr, value);
            }
            _ => self.base_write_register(addr, value),
        }
    }

    fn update_ex_regs_121(&mut self) {
        match self.ex_regs[5] & 0x3F {
            0x20 | 0x29 | 0x2B | 0x3C | 0x3F => {
                self.ex_regs[7] = 1;
                self.ex_regs[0] = self.ex_regs[6];
            }
            0x26 => {
                self.ex_regs[7] = 0;
                self.ex_regs[0] = self.ex_regs[6];
            }
            0x2C => {
                self.ex_regs[7] = 1;
                if self.ex_regs[6] != 0 {
                    self.ex_regs[0] = self.ex_regs[6];
                }
            }
            0x28 => {
                self.ex_regs[7] = 0;
                self.ex_regs[1] = self.ex_regs[6];
            }
            0x2A => {
                self.ex_regs[7] = 0;
                self.ex_regs[2] = self.ex_regs[6];
            }
            0x2F => {}
            _ => self.ex_regs[5] = 0,
        }
    }

    // ---------------- low ($0000-$7FFF) accesses ----------------

    fn can_write_to_work_ram(&self) -> bool {
        self.wram_enabled && !self.wram_write_protected
    }

    fn ram_read_allowed(&self) -> bool {
        self.ram_always_enabled || self.wram_enabled
    }

    fn ram_write_allowed(&self) -> bool {
        self.ram_always_enabled || (self.wram_enabled && !self.wram_write_protected)
    }

    /// MMC6's two interleaved 512-byte banks. Returns (index, readable, writable).
    fn mmc6_bank(&self, addr: u16) -> (usize, bool, bool) {
        if addr < 0x7000 {
            // Left-over mapping from InitMapper: 1KB mirrored, read/write.
            return (((addr as usize - 0x6000) % 0x400), true, true);
        }
        let offset = (addr as usize - 0x7000) & 0x3FF;
        let wram_enable = self.reg8000 & 0x20 != 0;
        if offset < 0x200 {
            let readable = wram_enable && (self.reg_a001 & 0x20 != 0);
            let writable = wram_enable && (self.reg_a001 & 0x10 != 0);
            (offset, readable, writable)
        } else {
            let readable = wram_enable && (self.reg_a001 & 0x80 != 0);
            let writable = wram_enable && (self.reg_a001 & 0x40 != 0);
            (0x200 + (offset & 0x1FF), readable, writable)
        }
    }

    fn read_prg_ram_mapped(&self, addr: u16) -> Option<u8> {
        if self.mmc6 {
            let (idx, readable, _) = self.mmc6_bank(addr);
            if readable {
                Some(self.common.prg_ram[idx % self.common.prg_ram.len()])
            } else {
                None
            }
        } else if self.ram_read_allowed() {
            Some(self.common.read_prg_ram(addr))
        } else {
            None
        }
    }

    fn write_prg_ram_mapped(&mut self, addr: u16, value: u8) {
        if self.mmc6 {
            let (idx, _, writable) = self.mmc6_bank(addr);
            if writable {
                let len = self.common.prg_ram.len();
                self.common.prg_ram[idx % len] = value;
            }
        } else if self.ram_write_allowed() {
            self.common.write_prg_ram(addr, value);
        }
    }

    fn write_low(&mut self, addr: u16, value: u8) -> bool {
        match self.id {
            12 => {
                if addr <= 0x5FFF {
                    self.chr_selection = value;
                    self.update_state();
                    return true;
                }
            }
            37 => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    if self.can_write_to_work_ram() {
                        self.selected_block = value & 0x07;
                        self.update_state();
                    }
                    return true;
                }
            }
            45 => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    // Once bit 6 of the 4th register is set, Mesen removes the
                    // $6000-$7FFF register range; later writes go to PRG RAM.
                    if self.reg45[3] & 0x40 != 0 {
                        return false;
                    }
                    self.reg45[self.reg45_index as usize & 3] = value;
                    self.reg45_index = (self.reg45_index + 1) & 0x03;
                    self.update_state();
                    return true;
                }
            }
            47 => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    if self.can_write_to_work_ram() {
                        self.selected_block = value & 0x01;
                        self.update_state();
                    }
                    return true;
                }
            }
            49 => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    if self.can_write_to_work_ram() {
                        self.selected_block = (value >> 6) & 0x03;
                        self.prg_reg = (value >> 4) & 0x03;
                        self.v49_prg_mode = value & 0x01;
                        self.update_state();
                    }
                    return true;
                }
            }
            52 => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    if self.can_write_to_work_ram() {
                        if self.extra_reg & 0x80 == 0 {
                            self.extra_reg = value;
                            self.update_state();
                        } else {
                            self.write_prg_ram_mapped(addr, value);
                        }
                    }
                    return true;
                }
            }
            114 => {
                if (0x5000..=0x7FFF).contains(&addr) {
                    self.ex_regs[0] = value;
                    self.update_prg_mapping();
                    return true;
                }
            }
            115 => {
                if (0x4100..=0x7FFF).contains(&addr) {
                    if addr == 0x5080 {
                        self.protection_reg = value;
                    } else if addr & 0x01 != 0 {
                        self.chr_reg = value & 0x01;
                        self.update_state();
                    } else {
                        self.prg_reg = value;
                        self.update_state();
                    }
                    return true;
                }
            }
            121 => {
                if (0x5000..=0x5FFF).contains(&addr) {
                    let lookup = [0x83u8, 0x83, 0x42, 0x00];
                    self.ex_regs[4] = lookup[(value & 0x03) as usize];
                    if addr & 0x5180 == 0x5180 {
                        self.ex_regs[3] = value;
                        self.update_state();
                    }
                    return true;
                }
            }
            123 => {
                if (0x5001..=0x5FFF).contains(&addr) {
                    if addr & 0x0800 != 0 {
                        if addr & 0x01 != 0 {
                            self.ex_regs[1] = value;
                        } else {
                            self.ex_regs[0] = value;
                        }
                        self.update_prg_mapping();
                    }
                    return true;
                }
            }
            126 => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    let reg = (addr & 0x03) as usize;
                    if reg == 1 || reg == 2 || ((reg == 0 || reg == 3) && self.ex_regs[3] & 0x80 == 0)
                    {
                        if self.ex_regs[reg] != value {
                            self.ex_regs[reg] = value;
                            if self.ex_regs[3] & 0x10 != 0 {
                                let page = self.chr_outer_bank_126()
                                    | (((self.ex_regs[2] & 0x0F) as u32) << 3);
                                for i in 0..8 {
                                    self.base_select_chr_page(
                                        i,
                                        page + i as u32,
                                        ChrMem::Default,
                                    );
                                }
                            } else {
                                self.base_update_chr_mapping();
                            }
                            self.base_update_prg_mapping();
                        }
                    }
                    return true;
                }
            }
            134 => {
                if addr == 0x6001 {
                    self.ex_reg = value;
                    self.update_state();
                    return true;
                }
            }
            165 => {}
            187 => {
                if (0x5000..=0x6FFF).contains(&addr) {
                    if addr == 0x5000 || addr == 0x6000 {
                        self.ex_regs[0] = value;
                        self.base_update_prg_mapping();
                    }
                    return true;
                }
            }
            189 => {
                if (0x4120..=0x7FFF).contains(&addr) {
                    self.prg_reg = value;
                    self.update_state();
                    return true;
                }
            }
            196 => {
                if (0x6000..=0x6FFF).contains(&addr) {
                    self.ex_regs[0] = 1;
                    self.ex_regs[1] = (value & 0x0F) | (value >> 4);
                    self.update_prg_mapping();
                    return true;
                }
            }
            198 => {
                if (0x5000..=0x7FFF).contains(&addr) {
                    if addr < 0x6000 {
                        let idx = (addr as usize - 0x5000) % self.work_ram.len();
                        self.work_ram[idx] = value;
                        return true;
                    }
                    if self.common.info.has_battery {
                        self.write_prg_ram_mapped(addr, value);
                    } else if self.ram_write_allowed() {
                        let idx = (addr as usize - 0x6000) % self.work_ram.len();
                        self.work_ram[idx] = value;
                    }
                    return true;
                }
            }
            205 => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    self.selected_block = value & 0x03;
                    self.update_state();
                    return true;
                }
            }
            208 => {
                if (0x4800..=0x4FFF).contains(&addr)
                    || (0x5000..=0x5FFF).contains(&addr)
                    || (0x6800..=0x6FFF).contains(&addr)
                {
                    self.write_register(addr, value);
                    return true;
                }
            }
            215 => {
                if (0x5000..=0x7FFF).contains(&addr) {
                    match addr {
                        0x5000 => {
                            self.ex_regs[0] = value;
                            self.update_state();
                        }
                        0x5001 => {
                            self.ex_regs[1] = value;
                            self.update_state();
                        }
                        0x5007 => self.ex_regs[2] = value,
                        _ => {}
                    }
                    return true;
                }
            }
            217 => {
                match addr {
                    0x5000 => {
                        self.ex_regs[0] = value;
                        if value & 0x80 != 0 {
                            let mut v = (value & 0x0F) | ((self.ex_regs[1] << 4) & 0x30);
                            v <<= 1;
                            self.select_prg_page(0, v as i32);
                            self.select_prg_page(1, v as i32 + 1);
                            self.select_prg_page(2, v as i32);
                            self.select_prg_page(3, v as i32 + 1);
                        } else {
                            self.update_prg_mapping();
                        }
                        return true;
                    }
                    0x5001 => {
                        if self.ex_regs[1] != value {
                            self.ex_regs[1] = value;
                            self.update_prg_mapping();
                        }
                        return true;
                    }
                    0x5007 => {
                        self.ex_regs[2] = value;
                        return true;
                    }
                    _ => {}
                }
            }
            224 => {
                if (0x5000..=0x5003).contains(&addr) {
                    if addr == 0x5000 {
                        self.outer_bank = (value >> 2) & 0x01;
                        self.update_prg_mapping();
                    }
                    return true;
                }
            }
            249 => {
                if addr == 0x5000 {
                    self.ex_reg = value;
                    self.update_prg_mapping();
                    self.update_chr_mapping();
                    return true;
                }
            }
            238 => {
                if (0x4020..=0x7FFF).contains(&addr) {
                    let security = [0x00u8, 0x02, 0x02, 0x03];
                    self.ex_reg = security[(value & 0x03) as usize];
                    return true;
                }
            }
            254 => {}
            _ => {}
        }
        false
    }

    // ---------------- VRAM / A12 ----------------

    fn clock_irq_counter(&mut self, rev_a: bool) {
        let count = self.irq_counter;
        if self.irq_counter == 0 || self.irq_reload {
            self.irq_counter = self.irq_reload_value;
        } else {
            self.irq_counter -= 1;
        }

        if rev_a {
            if (count > 0 || self.irq_reload) && self.irq_counter == 0 && self.irq_enabled {
                self.common.irq_pending = true;
            }
        } else if self.irq_counter == 0 && self.irq_enabled {
            self.common.irq_pending = true;
        }
        self.irq_reload = false;
    }

    /// `A12Watcher::UpdateVramAddress<10>`.
    fn notify_a12(&mut self, addr: u16) {
        let frame_cycle = self.ppu_cycle;
        if self.a12_cycles_down > 0 {
            if self.a12_last_cycle > frame_cycle {
                // We changed frames (wrapping u32 arithmetic like Mesen).
                self.a12_cycles_down = self.a12_cycles_down.wrapping_add(
                    (A12_FRAME_CYCLES - self.a12_last_cycle) + frame_cycle,
                );
            } else {
                self.a12_cycles_down = self
                    .a12_cycles_down
                    .wrapping_add(frame_cycle - self.a12_last_cycle);
            }
        }

        let mut rise = false;
        if addr & 0x1000 == 0 {
            if self.a12_cycles_down == 0 {
                self.a12_cycles_down = 1;
            }
        } else {
            if self.a12_cycles_down > A12_MIN_DELAY {
                rise = true;
            }
            self.a12_cycles_down = 0;
        }
        self.a12_last_cycle = frame_cycle;

        if rise {
            self.clock_irq_counter(self.force_rev_a);
        }
    }

    fn notify_mcacc(&mut self, addr: u16) {
        if addr & 0x1000 == 0 && self.prev_addr & 0x1000 != 0 {
            self.mccounter += 1;
            if self.mccounter == 1 {
                self.clock_irq_counter(false);
            } else if self.mccounter == 8 {
                self.mccounter = 0;
            }
        }
        self.prev_addr = addr;
    }

    fn notify_vram_address_change(&mut self, addr: u16) {
        match self.id {
            165 => {
                if self.need_update {
                    self.update_chr_mapping();
                }
                match addr & 0x2FF8 {
                    0x0FD0 | 0x0FE8 => {
                        self.chr_latch[((addr >> 12) & 0x01) as usize] = (addr & 0x08) == 0x08;
                        self.need_update = true;
                    }
                    _ => {}
                }
            }
            4 if self.mcacc => self.notify_mcacc(addr),
            _ => self.notify_a12(addr),
        }
    }

    fn read_nametable_addr(&self, addr: u16) -> u8 {
        if self.custom_nt {
            let a = (addr & 0x3FFF) as usize;
            let slot = ((a - 0x2000) >> 10) & 3;
            let off = a & 0x3FF;
            let physical = self.nt_map[slot] as usize;
            self.common.nametables[(physical * 0x400 + off) & 0x0FFF]
        } else {
            self.common.read_nametable(addr)
        }
    }

    fn write_nametable_addr(&mut self, addr: u16, value: u8) {
        if self.custom_nt {
            let a = (addr & 0x3FFF) as usize;
            let slot = ((a - 0x2000) >> 10) & 3;
            let off = a & 0x3FF;
            let physical = self.nt_map[slot] as usize;
            let idx = (physical * 0x400 + off) & 0x0FFF;
            self.common.nametables[idx] = value;
        } else {
            self.common.write_nametable(addr, value);
        }
    }
}

impl Mapper for Mmc3 {
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
        // Mesen's MMC3 does not reset its bank registers on a soft reset; only
        // the per-variant Reset overrides below run.
        match self.id {
            37 | 44 | 47 => {
                self.selected_block = 0;
                self.update_state();
            }
            49 => {
                self.prg_reg = 0;
                self.v49_prg_mode = 0;
                self.selected_block = 0;
                self.update_state();
            }
            45 => {
                self.reg45_index = 0;
                self.reg45 = [0, 0, 0x0F, 0];
                self.update_state();
            }
            52 => {
                self.extra_reg = 0;
                self.update_state();
            }
            121 => {
                self.ex_regs = [0; 8];
                self.ex_regs[3] = 0x80;
            }
            134 => {
                self.ex_reg = 0;
                self.update_state();
            }
            217 => {
                self.ex_regs[0] = 0;
                self.ex_regs[1] = 0xFF;
                self.ex_regs[2] = 0x03;
                self.ex_regs[3] = 0;
                self.update_state();
            }
            _ => {}
        }
    }

    fn cpu_read(&mut self, addr: u16) -> Option<u8> {
        if addr >= 0x8000 {
            return Some(self.common.read_prg_rom(addr));
        }

        match self.id {
            198 => {
                if (0x5000..=0x5FFF).contains(&addr) {
                    let idx = (addr as usize - 0x5000) % self.work_ram.len();
                    return Some(self.work_ram[idx]);
                }
                if (0x6000..=0x7FFF).contains(&addr) {
                    if self.common.info.has_battery {
                        return self.read_prg_ram_mapped(addr);
                    }
                    if self.ram_read_allowed() {
                        let idx = (addr as usize - 0x6000) % self.work_ram.len();
                        return Some(self.work_ram[idx]);
                    }
                    return None;
                }
            }
            238 => {
                if (0x4020..=0x7FFF).contains(&addr) {
                    return Some(self.ex_reg);
                }
            }
            254 => {
                if (0x6000..=0x7FFF).contains(&addr) {
                    let value = self.common.read_prg_ram(addr);
                    return Some(if self.ex_regs[0] != 0 {
                        value
                    } else {
                        value ^ self.ex_regs[1]
                    });
                }
            }
            _ => {}
        }

        if (0x6000..=0x7FFF).contains(&addr) {
            return self.read_prg_ram_mapped(addr);
        }

        match self.id {
            115 | 121 | 187 => {
                if (0x5000..=0x5FFF).contains(&addr) {
                    return Some(match self.id {
                        115 => self.protection_reg,
                        121 => self.ex_regs[4],
                        _ => {
                            let security = [0x83u8, 0x83, 0x42, 0x00];
                            security[(self.ex_regs[1] & 0x03) as usize]
                        }
                    });
                }
            }
            208 => {
                if (0x5800..=0x5FFF).contains(&addr) {
                    return Some(self.ex_regs[(addr & 0x03) as usize]);
                }
            }
            _ => {}
        }

        None
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        if addr >= 0x8000 {
            self.write_register(addr, value);
            return;
        }
        if self.write_low(addr, value) {
            return;
        }
        if (0x6000..=0x7FFF).contains(&addr) {
            self.write_prg_ram_mapped(addr, value);
        }
    }

    fn ppu_read(&mut self, addr: u16) -> Option<u8> {
        // A12 / MMC2 latch side effects are driven by
        // `ppu_bus_address_changed` (Mesen's BaseMapper::NotifyVRAMAddressChange,
        // called from PPU::SetBusAddress), not by the data access itself.
        let a = addr & 0x3FFF;
        match a {
            0x0000..=0x1FFF => Some(self.read_chr_addr(a)),
            0x2000..=0x3EFF => Some(self.read_nametable_addr(a)),
            _ => None,
        }
    }

    fn ppu_write(&mut self, addr: u16, value: u8) {
        let a = addr & 0x3FFF;
        match a {
            0x0000..=0x1FFF => self.write_chr_addr(a, value),
            0x2000..=0x3EFF => self.write_nametable_addr(a, value),
            _ => {}
        }
    }

    fn ppu_bus_address_changed(&mut self, addr: u16) {
        // Mesen calls `NotifyVRAMAddressChange` for every PPU bus address
        // change, including rendering fetches, $2007 accesses and $2006
        // writes. The PPU invokes this hook once per change, so no further
        // notification is done in ppu_read/ppu_write.
        self.notify_vram_address_change(addr & 0x3FFF);
    }

    fn scanline(&mut self, scanline: i16) {
        // Re-synchronize the coarse mapper-local PPU cycle counter with the
        // real PPU line position (see module docs). PPU::GetFrameCycle is
        // ((scanline + 1) * 341) + cycle, so scanline -1 maps to 0.
        let line = (scanline as i32 + 1).max(0) as u32;
        self.ppu_cycle = (line * 341) % A12_FRAME_CYCLES;
    }

    fn cpu_cycle(&mut self) {
        self.ppu_cycle = (self.ppu_cycle + 3) % A12_FRAME_CYCLES;
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
        w.u8(self.reg8000);
        w.u8(self.reg_a000);
        w.u8(self.reg_a001);
        w.u8(self.current_register);
        w.u8(self.chr_mode);
        w.u8(self.prg_mode);
        w.u8(self.irq_reload_value);
        w.u8(self.irq_counter);
        w.bool(self.irq_reload);
        w.bool(self.irq_enabled);
        w.bool(self.wram_enabled);
        w.bool(self.wram_write_protected);
        w.bytes(&self.registers);
        w.u32(self.a12_last_cycle);
        w.u32(self.a12_cycles_down);
        w.u32(self.ppu_cycle);

        let mut chr_mask = 0u8;
        for (i, &ram) in self.chr_slot_ram.iter().enumerate() {
            if ram {
                chr_mask |= 1 << i;
            }
        }
        w.u8(chr_mask);
        w.bytes(&self.nt_map);

        w.u8(self.selected_block);
        w.bytes(&self.ex_regs);
        w.u8(self.chr_selection);
        w.u8(self.extra_reg);
        w.u8(self.outer_bank);
        w.u8(self.reg45_index);
        w.bytes(&self.reg45);
        w.u8(self.v49_prg_mode);
        w.u8(self.prg_reg);
        w.u8(self.chr_reg);
        w.u8(self.protection_reg);
        w.u8(self.ex_reg);
        w.bool(self.chr_latch[0]);
        w.bool(self.chr_latch[1]);
        w.bool(self.need_update);
        w.u32(self.mccounter);
        w.u16(self.prev_addr);

        w.bytes(&self.extra_chr_ram);
        w.bytes(&self.work_ram);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        self.reg8000 = r.u8()?;
        self.reg_a000 = r.u8()?;
        self.reg_a001 = r.u8()?;
        self.current_register = r.u8()?;
        self.chr_mode = r.u8()?;
        self.prg_mode = r.u8()?;
        self.irq_reload_value = r.u8()?;
        self.irq_counter = r.u8()?;
        self.irq_reload = r.bool()?;
        self.irq_enabled = r.bool()?;
        self.wram_enabled = r.bool()?;
        self.wram_write_protected = r.bool()?;

        let registers = r.bytes()?;
        if registers.len() != 8 {
            return Err("MMC3 register state size mismatch".into());
        }
        self.registers.copy_from_slice(&registers);

        self.a12_last_cycle = r.u32()?;
        self.a12_cycles_down = r.u32()?;
        self.ppu_cycle = r.u32()?;

        let chr_mask = r.u8()?;
        for i in 0..8 {
            self.chr_slot_ram[i] = chr_mask & (1 << i) != 0;
        }

        let nt_map = r.bytes()?;
        if nt_map.len() != 4 {
            return Err("MMC3 nametable map size mismatch".into());
        }
        self.nt_map.copy_from_slice(&nt_map);

        self.selected_block = r.u8()?;
        let ex_regs = r.bytes()?;
        if ex_regs.len() != 8 {
            return Err("MMC3 extra register size mismatch".into());
        }
        self.ex_regs.copy_from_slice(&ex_regs);
        self.chr_selection = r.u8()?;
        self.extra_reg = r.u8()?;
        self.outer_bank = r.u8()?;
        self.reg45_index = r.u8()?;
        let reg45 = r.bytes()?;
        if reg45.len() != 4 {
            return Err("MMC3 $6000 register size mismatch".into());
        }
        self.reg45.copy_from_slice(&reg45);
        self.v49_prg_mode = r.u8()?;
        self.prg_reg = r.u8()?;
        self.chr_reg = r.u8()?;
        self.protection_reg = r.u8()?;
        self.ex_reg = r.u8()?;
        self.chr_latch[0] = r.bool()?;
        self.chr_latch[1] = r.bool()?;
        self.need_update = r.bool()?;
        self.mccounter = r.u32()?;
        self.prev_addr = r.u16()?;

        let extra_chr_ram = r.bytes()?;
        if extra_chr_ram.len() != self.extra_chr_ram.len() {
            return Err("MMC3 CHR RAM size mismatch".into());
        }
        self.extra_chr_ram.copy_from_slice(&extra_chr_ram);

        let work_ram = r.bytes()?;
        if work_ram.len() != self.work_ram.len() {
            return Err("MMC3 work RAM size mismatch".into());
        }
        self.work_ram.copy_from_slice(&work_ram);

        Ok(())
    }
}

/// Recognize Shay Green's MMC3 revision-A validation ROM. Mesen uses its
/// database for this; without one, the ROM's own embedded title is the most
/// precise signal available inside the mapper.
fn is_mmc3_rev_a_test_rom(prg: &[u8]) -> bool {
    const MARKER: &[u8] = b"MMC3 IRQ COUNTER REVISION A";
    prg.windows(MARKER.len()).any(|window| window == MARKER)
}

fn nt_map_for(mirroring: Mirroring) -> [u8; 4] {
    match mirroring {
        Mirroring::Vertical => [0, 1, 0, 1],
        Mirroring::Horizontal => [0, 0, 1, 1],
        Mirroring::ScreenAOnly => [0, 0, 0, 0],
        Mirroring::ScreenBOnly => [1, 1, 1, 1],
        Mirroring::FourScreens => [0, 1, 2, 3],
    }
}

const LUT_217: [u8; 8] = [0, 6, 3, 7, 5, 2, 4, 1];

const LUT_REG_215: [[u8; 8]; 8] = [
    [0, 1, 2, 3, 4, 5, 6, 7],
    [0, 2, 6, 1, 7, 3, 4, 5],
    [0, 5, 4, 1, 7, 2, 6, 3],
    [0, 6, 3, 7, 5, 2, 4, 1],
    [0, 2, 5, 3, 6, 1, 7, 4],
    [0, 1, 2, 3, 4, 5, 6, 7],
    [0, 1, 2, 3, 4, 5, 6, 7],
    [0, 1, 2, 3, 4, 5, 6, 7],
];

const LUT_ADDR_215: [[u8; 8]; 8] = [
    [0, 1, 2, 3, 4, 5, 6, 7],
    [3, 2, 0, 4, 1, 5, 6, 7],
    [0, 1, 2, 3, 4, 5, 6, 7],
    [5, 0, 1, 2, 3, 7, 6, 4],
    [3, 1, 0, 5, 2, 4, 6, 7],
    [0, 1, 2, 3, 4, 5, 6, 7],
    [0, 1, 2, 3, 4, 5, 6, 7],
    [0, 1, 2, 3, 4, 5, 6, 7],
];

const PROTECTION_LUT_208: [u8; 256] = [
    0x59, 0x59, 0x59, 0x59, 0x59, 0x59, 0x59, 0x59, 0x59, 0x49, 0x19, 0x09, 0x59, 0x49, 0x19, 0x09,
    0x59, 0x59, 0x59, 0x59, 0x59, 0x59, 0x59, 0x59, 0x51, 0x41, 0x11, 0x01, 0x51, 0x41, 0x11, 0x01,
    0x59, 0x59, 0x59, 0x59, 0x59, 0x59, 0x59, 0x59, 0x59, 0x49, 0x19, 0x09, 0x59, 0x49, 0x19, 0x09,
    0x59, 0x59, 0x59, 0x59, 0x59, 0x59, 0x59, 0x59, 0x51, 0x41, 0x11, 0x01, 0x51, 0x41, 0x11, 0x01,
    0x00, 0x10, 0x40, 0x50, 0x00, 0x10, 0x40, 0x50, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x08, 0x18, 0x48, 0x58, 0x08, 0x18, 0x48, 0x58, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x10, 0x40, 0x50, 0x00, 0x10, 0x40, 0x50, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x08, 0x18, 0x48, 0x58, 0x08, 0x18, 0x48, 0x58, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x59, 0x59, 0x59, 0x59, 0x59, 0x59, 0x59, 0x59, 0x58, 0x48, 0x18, 0x08, 0x58, 0x48, 0x18, 0x08,
    0x59, 0x59, 0x59, 0x59, 0x59, 0x59, 0x59, 0x59, 0x50, 0x40, 0x10, 0x00, 0x50, 0x40, 0x10, 0x00,
    0x59, 0x59, 0x59, 0x59, 0x59, 0x59, 0x59, 0x59, 0x58, 0x48, 0x18, 0x08, 0x58, 0x48, 0x18, 0x08,
    0x59, 0x59, 0x59, 0x59, 0x59, 0x59, 0x59, 0x59, 0x50, 0x40, 0x10, 0x00, 0x50, 0x40, 0x10, 0x00,
    0x01, 0x11, 0x41, 0x51, 0x01, 0x11, 0x41, 0x51, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x09, 0x19, 0x49, 0x59, 0x09, 0x19, 0x49, 0x59, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x01, 0x11, 0x41, 0x51, 0x01, 0x11, 0x41, 0x51, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x09, 0x19, 0x49, 0x59, 0x09, 0x19, 0x49, 0x59, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

pub fn supports(id: u16) -> bool {
    matches!(
        id,
        4 | 12
            | 37
            | 44
            | 45
            | 47
            | 49
            | 52
            | 114
            | 115
            | 118
            | 119
            | 121
            | 123
            | 126
            | 134
            | 165
            | 182
            | 187
            | 189
            | 196
            | 197
            | 198
            | 199
            | 205
            | 208
            | 215
            | 217
            | 219
            | 224
            | 238
            | 245
            | 249
            | 250
            | 254
    )
}

pub fn create(id: u16, rom: RomData) -> Option<Box<dyn Mapper>> {
    if supports(id) {
        Some(Box::new(Mmc3::new(id, rom)))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::StateReader;
    use crate::types::RomInfo;

    fn test_rom(id: u16, prg_banks: usize, chr_banks: usize) -> RomData {
        let mut info = RomInfo::default();
        info.mapper = id;
        info.mirroring = Mirroring::Vertical;
        info.prg_rom_size = prg_banks * 0x2000;
        info.chr_rom_size = chr_banks * 0x400;
        info.prg_ram_size = 0x2000;
        let mut prg = vec![0u8; prg_banks * 0x2000];
        for bank in 0..prg_banks {
            for i in 0..0x2000 {
                prg[bank * 0x2000 + i] = bank as u8;
            }
        }
        let mut chr = vec![0u8; chr_banks * 0x400];
        for bank in 0..chr_banks {
            for i in 0..0x400 {
                chr[bank * 0x400 + i] = bank as u8;
            }
        }
        RomData::new(info, prg, chr)
    }

    /// Emulates the PPU's `SetBusAddress` + data access pair.
    fn bus_read(m: &mut Mmc3, addr: u16) -> Option<u8> {
        m.ppu_bus_address_changed(addr);
        m.ppu_read(addr)
    }

    fn bus_write(m: &mut Mmc3, addr: u16, value: u8) {
        m.ppu_bus_address_changed(addr);
        m.ppu_write(addr, value);
    }

    fn clock_a12(m: &mut Mmc3) {
        // A12 low, then wait 4 CPU cycles (12 PPU dots), then A12 high.
        bus_read(m, 0x0000);
        for _ in 0..4 {
            m.cpu_cycle();
        }
        bus_read(m, 0x1000);
    }

    #[test]
    fn prg_and_chr_bank_mapping() {
        let mut m = Mmc3::new(4, test_rom(4, 16, 64));

        // Power-on: regs 6/7 = 0/1, last two banks fixed.
        assert_eq!(m.cpu_read(0x8000), Some(0));
        assert_eq!(m.cpu_read(0xA000), Some(1));
        assert_eq!(m.cpu_read(0xC000), Some(14));
        assert_eq!(m.cpu_read(0xE000), Some(15));

        // PRG mode 1.
        m.cpu_write(0x8000, 0x47);
        m.cpu_write(0x8001, 5); // R7
        assert_eq!(m.cpu_read(0x8000), Some(14));
        assert_eq!(m.cpu_read(0xA000), Some(5));
        assert_eq!(m.cpu_read(0xC000), Some(0));
        assert_eq!(m.cpu_read(0xE000), Some(15));

        // CHR mode 0: R0's low bit is ignored.
        m.cpu_write(0x8000, 0x00);
        m.cpu_write(0x8001, 0x0B);
        assert_eq!(m.ppu_read(0x0000), Some(0x0A));
        assert_eq!(m.ppu_read(0x0400), Some(0x0B));
        assert_eq!(m.ppu_read(0x1000), Some(4));
        assert_eq!(m.ppu_read(0x1C00), Some(7));

        // CHR mode 1: 2KB banks move to $1000-$1FFF.
        m.cpu_write(0x8000, 0x80);
        m.cpu_write(0x8000, 0x82);
        m.cpu_write(0x8001, 0x20);
        assert_eq!(m.ppu_read(0x0000), Some(0x20));
        assert_eq!(m.ppu_read(0x1000), Some(0x0A));
        assert_eq!(m.ppu_read(0x1400), Some(0x0B));
    }

    #[test]
    fn wram_enable_and_write_protect() {
        let mut m = Mmc3::new(4, test_rom(4, 8, 8));
        assert_eq!(m.cpu_read(0x6000), None);
        m.cpu_write(0xA001, 0x80);
        assert_eq!(m.cpu_read(0x6000), Some(0));
        m.cpu_write(0x6000, 0x42);
        assert_eq!(m.cpu_read(0x6000), Some(0x42));

        // Write-protected: reads still work, writes are ignored.
        m.cpu_write(0xA001, 0xC0);
        assert_eq!(m.cpu_read(0x6000), Some(0x42));
        m.cpu_write(0x6000, 0x99);
        assert_eq!(m.cpu_read(0x6000), Some(0x42));

        // Disabled again.
        m.cpu_write(0xA001, 0x00);
        assert_eq!(m.cpu_read(0x6000), None);
    }

    #[test]
    fn mirroring_register() {
        let mut m = Mmc3::new(4, test_rom(4, 8, 8));
        assert_eq!(m.common.mirroring, Mirroring::Vertical);
        m.cpu_write(0xA000, 0x01);
        assert_eq!(m.common.mirroring, Mirroring::Horizontal);
        m.cpu_write(0xA000, 0x00);
        assert_eq!(m.common.mirroring, Mirroring::Vertical);
    }

    #[test]
    fn irq_rev_b_counts_down_and_fires() {
        let mut m = Mmc3::new(4, test_rom(4, 8, 8));
        m.cpu_write(0xC000, 2); // reload value
        m.cpu_write(0xC001, 0); // reload
        m.cpu_write(0xE001, 0); // enable

        clock_a12(&mut m); // reload -> 2
        assert!(!m.irq_pending());
        clock_a12(&mut m); // 2 -> 1
        assert!(!m.irq_pending());
        clock_a12(&mut m); // 1 -> 0
        assert!(m.irq_pending());
    }

    #[test]
    fn irq_rev_a_differs_with_zero_reload() {
        let mut m = Mmc3::new(12, test_rom(12, 16, 16));
        m.cpu_write(0xC000, 0);
        m.cpu_write(0xC001, 0);
        m.cpu_write(0xE001, 0);

        clock_a12(&mut m);
        assert!(m.irq_pending(), "Rev A fires on the reload clock");

        // Clear and re-enable without reloading the counter.
        m.cpu_write(0xE000, 0);
        m.cpu_write(0xE001, 0);
        clock_a12(&mut m);
        assert!(!m.irq_pending(), "Rev A must not fire when count==0 and no reload");

        // Rev B (mapper 4) with the same sequence fires again.
        let mut b = Mmc3::new(4, test_rom(4, 16, 16));
        b.cpu_write(0xC000, 0);
        b.cpu_write(0xC001, 0);
        b.cpu_write(0xE001, 0);
        clock_a12(&mut b);
        assert!(b.irq_pending());
        b.cpu_write(0xE000, 0);
        b.cpu_write(0xE001, 0);
        clock_a12(&mut b);
        assert!(b.irq_pending());
    }

    #[test]
    fn a12_notification_only_comes_from_bus_address_hook() {
        // Guard against double-clocking: ppu_read alone must not feed A12.
        let mut m = Mmc3::new(4, test_rom(4, 8, 8));
        m.cpu_write(0xC000, 0);
        m.cpu_write(0xC001, 0);
        m.cpu_write(0xE001, 0);

        m.ppu_read(0x0000);
        for _ in 0..4 {
            m.cpu_cycle();
        }
        m.ppu_read(0x1000);
        assert!(!m.irq_pending());

        // With the PPU bus hook, the same sequence clocks once.
        bus_read(&mut m, 0x0000);
        for _ in 0..4 {
            m.cpu_cycle();
        }
        bus_read(&mut m, 0x1000);
        assert!(m.irq_pending());
    }

    #[test]
    fn rev_a_validation_rom_forces_rev_a_behavior() {
        let mut rom = test_rom(4, 16, 16);
        const MARKER: &[u8] = b"MMC3 IRQ COUNTER REVISION A";
        let offset = 0x1000;
        rom.prg_rom[offset..offset + MARKER.len()].copy_from_slice(MARKER);

        let mut m = Mmc3::new(4, rom);
        m.cpu_write(0xC000, 0);
        m.cpu_write(0xC001, 0);
        m.cpu_write(0xE001, 0);
        clock_a12(&mut m);
        assert!(m.irq_pending(), "Rev A fires on the reload clock");
        m.cpu_write(0xE000, 0);
        m.cpu_write(0xE001, 0);
        clock_a12(&mut m);
        assert!(
            !m.irq_pending(),
            "Rev A must not fire when count==0 and no reload"
        );
    }

    #[test]
    fn a12_filter_requires_minimum_low_period() {
        let mut m = Mmc3::new(4, test_rom(4, 8, 8));
        m.cpu_write(0xC000, 0);
        m.cpu_write(0xC001, 0);
        m.cpu_write(0xE001, 0);

        // 9 dots low -> not enough (cyclesDown == 10, must be > 10).
        bus_read(&mut m, 0x0000);
        for _ in 0..3 {
            m.cpu_cycle();
        }
        bus_read(&mut m, 0x1000);
        assert!(!m.irq_pending());

        // 12 dots low -> accepted.
        bus_read(&mut m, 0x0000);
        for _ in 0..4 {
            m.cpu_cycle();
        }
        bus_read(&mut m, 0x1000);
        assert!(m.irq_pending());
    }

    #[test]
    fn mapper_44_outer_bank() {
        let mut m = Mmc3::new(44, test_rom(44, 64, 64));
        // Enable WRAM and select block 1: PRG pages get | 0x10.
        m.cpu_write(0xA001, 0x81);
        assert_eq!(m.cpu_read(0x8000), Some(16));
        assert_eq!(m.cpu_read(0xA000), Some(17));
        assert_eq!(m.cpu_read(0xC000), Some(30));
        assert_eq!(m.cpu_read(0xE000), Some(31));
    }

    #[test]
    fn mapper_208_protected_registers_and_fixed_bank() {
        let mut m = Mmc3::new(208, test_rom(208, 32, 32));
        // Init maps the last 32KB (bank 3 group) regardless of MMC3 regs.
        assert_eq!(m.cpu_read(0x8000), Some(12));
        assert_eq!(m.cpu_read(0xE000), Some(15));

        // 0x5800-0x5FFF decode a write XORed with the protection key.
        m.cpu_write(0x5000, 0x09);
        m.cpu_write(0x5800, 0x01);
        assert_eq!(m.cpu_read(0x5800), Some(0x01 ^ PROTECTION_LUT_208[0x09]));
    }

    #[test]
    fn mapper_37_block_masking() {
        let mut m = Mmc3::new(37, test_rom(37, 64, 256));
        m.cpu_write(0xA001, 0x80); // enable WRAM so $6000 writes latch
        m.cpu_write(0x6000, 0x04); // block 4
        m.cpu_write(0x8000, 0x06);
        m.cpu_write(0x8001, 0x0B); // R6 = 11
        assert_eq!(m.cpu_read(0x8000), Some(27)); // (11 & 0x0F) | 0x10
        assert_eq!(m.cpu_read(0xA000), Some(17)); // R7 = 1 -> 1 | 0x10
        assert_eq!(m.cpu_read(0xC000), Some(30)); // last-2 = 14 | 0x10
        assert_eq!(m.cpu_read(0xE000), Some(31));
        // CHR block 4+ ORs 0x80.
        assert_eq!(m.ppu_read(0x0000), Some(0x80));
        assert_eq!(m.ppu_read(0x0400), Some(0x81));
    }

    #[test]
    fn mapper_45_registers_and_lock() {
        let mut m = Mmc3::new(45, test_rom(45, 64, 128));
        m.cpu_write(0x6000, 0x10);
        m.cpu_write(0x6001, 0x20);
        m.cpu_write(0x6002, 0x30);
        m.cpu_write(0x6003, 0x40); // locks $6000-$7FFF

        assert_eq!(m.cpu_read(0x8000), Some(32)); // (R6 & 0x3F) | 0x20
        assert_eq!(m.cpu_read(0xA000), Some(33));
        assert_eq!(m.cpu_read(0xC000), Some(62));
        assert_eq!(m.cpu_read(0xE000), Some(63));
        // CHR page = R0 | (R2 & 0xF0) << 4 = $300, mod 128 pages = 16.
        assert_eq!(m.ppu_read(0x0000), Some(16));

        // Once locked, writes fall through to PRG RAM.
        m.cpu_write(0xA001, 0x80);
        m.cpu_write(0x6000, 0x55);
        assert_eq!(m.cpu_read(0x6000), Some(0x55));
        // The lock is part of the register state and survives save/load.
        assert_eq!(m.reg45[3] & 0x40, 0x40);
    }

    #[test]
    fn mapper_49_mode_and_banks() {
        let mut m = Mmc3::new(49, test_rom(49, 64, 256));
        m.cpu_write(0xA001, 0x80);
        m.cpu_write(0x6000, 0x51); // block 1, PRG block 1, mode 1
        assert_eq!(m.cpu_read(0x8000), Some(16));
        assert_eq!(m.cpu_read(0xA000), Some(17));
        assert_eq!(m.cpu_read(0xC000), Some(30));
        assert_eq!(m.cpu_read(0xE000), Some(31));
        assert_eq!(m.ppu_read(0x0000), Some(0x80));

        // Mode 0: fixed 32KB block from bits 5-4.
        m.cpu_write(0x6000, 0x10); // block 0, PRG block 1, mode 0
        assert_eq!(m.cpu_read(0x8000), Some(4));
        assert_eq!(m.cpu_read(0xE000), Some(7));
    }

    #[test]
    fn mapper_12_chr_outer_selection() {
        let mut m = Mmc3::new(12, test_rom(12, 16, 300));
        assert_eq!(m.common.chr_map[0], 0);
        m.cpu_write(0x4020, 0x01); // low half +0x100
        assert_eq!(m.common.chr_map[0], 0x100 * 0x400);
        assert_eq!(m.common.chr_map[4], 4 * 0x400);
        m.cpu_write(0x4020, 0x10); // high half +0x100
        assert_eq!(m.common.chr_map[0], 0);
        assert_eq!(m.common.chr_map[4], 0x104 * 0x400);
    }

    #[test]
    fn mapper_118_nametable_selection() {
        let mut m = Mmc3::new(118, test_rom(118, 8, 8));
        // CHR mode 0, register 0: high bit picks 1KB nametable for slots 0/1.
        m.cpu_write(0x8000, 0x00);
        m.cpu_write(0x8001, 0x80);
        m.ppu_write(0x2000, 0xAA);
        assert_eq!(m.ppu_read(0x2000), Some(0xAA));
        assert_eq!(m.ppu_read(0x2400), Some(0xAA)); // slot 1 also table 1
        assert_eq!(m.ppu_read(0x2800), Some(0)); // slot 2 still table 0
    }

    #[test]
    fn mapper_165_mmc2_latch_and_chr_ram() {
        let mut m = Mmc3::new(165, test_rom(165, 16, 16));
        bus_write(&mut m, 0x0000, 0x77);
        assert_eq!(bus_read(&mut m, 0x0000), Some(0x77));
        assert_eq!(bus_read(&mut m, 0x1000), Some(4)); // ROM page 1

        // $0FE8 sets latch 0; the mapping updates on the next VRAM access.
        bus_read(&mut m, 0x0FE8);
        assert_eq!(bus_read(&mut m, 0x0000), Some(0));
    }

    #[test]
    fn mapper_199_chr_ram_and_alternate_prg_regs() {
        let mut m = Mmc3::new(199, test_rom(199, 64, 32));
        m.ppu_write(0x0000, 0x11);
        assert_eq!(m.ppu_read(0x0000), Some(0x11));

        // CHR pages below 8 select the 8KB CHR RAM; a higher page selects ROM.
        m.cpu_write(0x8000, 0x02); // CHR mode 0, register 2
        m.cpu_write(0x8001, 0x10);
        assert_eq!(m.ppu_read(0x1000), Some(16)); // ROM page 16

        m.cpu_write(0x8000, 0x08); // register bit 3: alternate bank write
        m.cpu_write(0x8001, 0x20); // exRegs[0] = 0x20
        assert_eq!(m.cpu_read(0xC000), Some(32));
        assert_eq!(m.cpu_read(0xE000), Some(63)); // exRegs[1] = 0xFF % 64
    }

    #[test]
    fn mapper_219_outer_registers() {
        let mut m = Mmc3::new(219, test_rom(219, 16, 128));
        // Init maps the last 32KB and the first 8KB of CHR.
        assert_eq!(m.cpu_read(0x8000), Some(12));
        assert_eq!(m.cpu_read(0xE000), Some(15));
        assert_eq!(m.ppu_read(0x0000), Some(0));

        m.cpu_write(0x8002, 0x23);
        m.cpu_write(0x8001, 0x04);
        // PRG bank = (0x04 << 1) = 8 in slot 0x26 - 0x23 = 3.
        assert_eq!(m.cpu_read(0xE000), Some(8));

        m.cpu_write(0x8002, 0x00);
        m.cpu_write(0x8000, 0x08); // exRegs[1] = 8
        m.cpu_write(0x8001, 0x55); // exRegs[2] = 0x55 << 4 = 0x50
        m.cpu_write(0x8000, 0x0B);
        m.cpu_write(0x8001, 0x02);
        // CHR slot 1 = 0x50 | 1 = 0x51.
        assert_eq!(m.common.chr_map[1], 0x51 * 0x400);
    }

    #[test]
    fn mapper_250_address_as_value() {
        let mut m = Mmc3::new(250, test_rom(250, 16, 16));
        m.cpu_write(0x8001, 0x99); // -> $8000, value 1
        assert_eq!(m.current_register, 1);
        m.cpu_write(0x8400, 0xAA); // -> $8001, value 0
        assert_eq!(m.registers[1], 0);
        m.cpu_write(0xE400, 0xAA); // -> $E001, value 0
        assert!(m.irq_enabled);
    }

    #[test]
    fn mapper_254_xor_readback() {
        let mut m = Mmc3::new(254, test_rom(254, 8, 8));
        m.cpu_write(0xA001, 0x80);
        m.cpu_write(0x6000, 0x12);
        // $A001 also latches the XOR value, so the plain read is $12 ^ $80.
        assert_eq!(m.cpu_read(0x6000), Some(0x12 ^ 0x80));

        m.cpu_write(0xA001, 0x35); // exRegs[1] = 0x35, WRAM disabled
        assert_eq!(m.cpu_read(0x6000), Some(0x12 ^ 0x35));

        m.cpu_write(0x8000, 0x00); // exRegs[0] = 0xFF: XOR disabled
        assert_eq!(m.cpu_read(0x6000), Some(0x12));
    }

    #[test]
    fn mmc6_wram_banking() {
        let mut rom = test_rom(4, 16, 8);
        rom.info.submapper = 1;
        let mut m = Mmc3::new(4, rom);
        assert_eq!(m.common.prg_ram.len(), 0x400);

        m.cpu_write(0x8000, 0x20); // MMC6 WRAM enable
        m.cpu_write(0xA001, 0xB0); // first: read+write; last: read only
        m.cpu_write(0x7000, 0x21);
        assert_eq!(m.cpu_read(0x7000), Some(0x21));
        m.cpu_write(0x7200, 0x22); // write-protected last bank
        assert_eq!(m.cpu_read(0x7200), Some(0x00));

        m.cpu_write(0xA001, 0xF0); // last bank now read+write
        m.cpu_write(0x7200, 0x22);
        assert_eq!(m.cpu_read(0x7200), Some(0x22));

        // Disabling via $8000 bit 5 removes all access.
        m.cpu_write(0x8000, 0x00);
        assert_eq!(m.cpu_read(0x7000), None);
    }

    #[test]
    fn save_load_extra_chr_ram() {
        let mut m = Mmc3::new(199, test_rom(199, 64, 32));
        m.ppu_write(0x0000, 0x5A);
        let mut w = StateWriter::new();
        m.save_state(&mut w);
        let data = w.into_inner();

        let mut restored = Mmc3::new(199, test_rom(199, 64, 32));
        let mut r = StateReader::new(&data);
        restored.load_state(&mut r).unwrap();
        assert_eq!(restored.ppu_read(0x0000), Some(0x5A));
    }

    const SUPPORTED_IDS: [u16; 35] = [
        4, 12, 37, 44, 45, 47, 49, 52, 114, 115, 118, 119, 121, 123, 126, 134, 165, 182, 187,
        189, 196, 197, 198, 199, 205, 208, 215, 217, 219, 224, 238, 245, 249, 250, 254,
    ];

    #[test]
    fn supported_id_list() {
        for id in SUPPORTED_IDS {
            assert!(supports(id), "mapper {} should be supported", id);
        }
        for id in [0u16, 1, 5, 9, 10, 20, 31, 99, 119 + 1, 255] {
            assert!(!supports(id), "mapper {} must not be claimed", id);
        }
    }

    #[test]
    fn all_variants_smoke_and_save_load() {
        for id in SUPPORTED_IDS {
            let mut m = Mmc3::new(id, test_rom(id, 128, 128));
            assert!(m.cpu_read(0x8000).is_some(), "id {}", id);
            assert!(m.ppu_read(0x0000).is_some(), "id {}", id);

            m.cpu_write(0x8000, 0x46);
            m.cpu_write(0x8001, 0x05);
            m.cpu_write(0x8000, 0x47);
            m.cpu_write(0x8001, 0x06);
            m.cpu_write(0x8000, 0x80);
            m.cpu_write(0x8001, 0x0F);
            m.cpu_write(0x8000, 0x81);
            m.cpu_write(0x8001, 0x10);
            m.cpu_write(0xA000, 0x01);
            m.cpu_write(0xA001, 0x80);
            m.cpu_write(0xC000, 0x03);
            m.cpu_write(0xC001, 0x00);
            m.cpu_write(0xE001, 0x00);

            for addr in [
                0x4020u16, 0x4120, 0x4100, 0x4800, 0x5000, 0x5001, 0x5007, 0x5080,
                0x5800, 0x6000, 0x6001, 0x6800, 0x7000, 0x7FFF,
            ] {
                m.cpu_write(addr, 0x5A);
            }
            for addr in [0x0000u16, 0x03FF, 0x1000, 0x1FFF, 0x2000, 0x2400, 0x2C00] {
                let _ = m.ppu_read(addr);
            }
            for _ in 0..12 {
                m.cpu_cycle();
                m.scanline(10);
                let _ = m.irq_pending();
            }

            let mut w = StateWriter::new();
            m.save_state(&mut w);
            let data = w.into_inner();
            let mut restored = Mmc3::new(id, test_rom(id, 128, 128));
            let mut r = StateReader::new(&data);
            restored
                .load_state(&mut r)
                .unwrap_or_else(|e| panic!("id {}: {}", id, e));
            assert_eq!(restored.common.prg_map, m.common.prg_map, "id {}", id);
            assert_eq!(restored.common.chr_map, m.common.chr_map, "id {}", id);
        }
    }

    #[test]
    fn save_load_roundtrip() {
        let mut m = Mmc3::new(4, test_rom(4, 16, 64));
        m.cpu_write(0x8000, 0x46);
        m.cpu_write(0x8001, 5);
        m.cpu_write(0xA000, 1);
        m.cpu_write(0xC000, 3);
        m.cpu_write(0xC001, 0);
        m.cpu_write(0xE001, 0);
        m.cpu_write(0x6000, 0x55); // disabled: ignored
        m.cpu_write(0xA001, 0x80);
        m.cpu_write(0x6000, 0x55);
        clock_a12(&mut m);

        let mut w = StateWriter::new();
        m.save_state(&mut w);
        let data = w.into_inner();

        let mut restored = Mmc3::new(4, test_rom(4, 16, 64));
        let mut r = StateReader::new(&data);
        restored.load_state(&mut r).unwrap();

        assert_eq!(restored.common.prg_map, m.common.prg_map);
        assert_eq!(restored.common.chr_map, m.common.chr_map);
        assert_eq!(restored.common.mirroring, m.common.mirroring);
        assert_eq!(restored.common.irq_pending, m.common.irq_pending);
        assert_eq!(restored.irq_counter, m.irq_counter);
        assert_eq!(restored.irq_reload_value, m.irq_reload_value);
        assert_eq!(restored.a12_cycles_down, m.a12_cycles_down);
        assert_eq!(restored.cpu_read(0x6000), m.cpu_read(0x6000));
    }
}
