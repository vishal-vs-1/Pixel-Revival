//! Capcom MMC2/MMC4 (mappers 9/10) and MMC5 (mapper 5).
//!
//! Direct translation of Mesen 0.9.9 `MMC2.h`, `MMC4.h`, `MMC5.h`,
//! `MMC5Audio.h` and `MMC5MemoryHandler.h`.
//!
//! Original C++: Copyright (C) 2014-2020 Sour, GPL-3.0.
//! Rust port: GPL-3.0-or-later.
//!
//! OWNER: mapper-capcom-extra agent.
//!
//! MMC5 integration note: the mapper needs to observe fully decoded writes to
//! PPU `$2000` (sprite size) and `$2001` (rendering enable). The `Mapper`
//! trait has no hook for PPU register writes, so `Mmc5` exposes
//! [`Mmc5::ppu_register_write`] for the PPU/bus owner to call. Until that hook
//! is wired, a conservative fallback infers 8x16 sprite mode when sprite-phase
//! fetches touch both 4 KB pattern tables and the `$5128-$512B` background
//! banks have been written (see `ppu_read`). Normal 8x8 games, 1/2/4/8 KB CHR
//! modes, upper CHR bits, ExRAM and the multiplier are fully functional
//! without the hook.

use super::{Mapper, MapperCommon};
use crate::state::{StateReader, StateResult, StateWriter};
use crate::types::{Mirroring, RomData};

// ---------------------------------------------------------------------------
// MMC2 / MMC4
// ---------------------------------------------------------------------------

/// MMC2 (mapper 9) and MMC4 (mapper 10). MMC4 only changes the PRG page size
/// and the left-latch address ranges.
pub struct Mmc2 {
    common: MapperCommon,
    is_mmc4: bool,

    left_latch: u8,
    right_latch: u8,
    left_chr_page: [u8; 2],
    right_chr_page: [u8; 2],
    need_chr_update: bool,

    /// Mesen leaves `$8000` unmapped until `$A000` is written. Open bus is
    /// returned instead of the power-on default mapping.
    slot0_mapped: bool,
}

impl Mmc2 {
    pub fn new(rom: RomData) -> Self {
        let is_mmc4 = rom.info.mapper == 10;
        let mut common = MapperCommon::new(rom);
        common.init_default_maps();
        let mut mapper = Self {
            common,
            is_mmc4,
            left_latch: 1,
            right_latch: 1,
            left_chr_page: [0; 2],
            right_chr_page: [0; 2],
            need_chr_update: false,
            slot0_mapped: false,
        };
        mapper.power_on();
        mapper
    }

    fn prg_page_size(&self) -> usize {
        if self.is_mmc4 {
            0x4000
        } else {
            0x2000
        }
    }

    fn select_prg_slot0(&mut self, value: u8) {
        let page_size = self.prg_page_size();
        let page_count = (self.common.prg_rom.len() / page_size).max(1);
        let page = (value as usize & 0x0F) % page_count;
        let base = page * page_size;
        if self.is_mmc4 {
            self.common.map_prg_8k(0, base);
            self.common.map_prg_8k(1, base + 0x2000);
        } else {
            self.common.map_prg_8k(0, base);
        }
        self.slot0_mapped = true;
    }

    fn select_chr_page(&mut self, slot: usize, page: u8) {
        let base = page as usize * 0x1000;
        for i in 0..4 {
            self.common
                .map_chr_1k(slot * 4 + i, base + i * 0x400);
        }
    }

    fn power_on(&mut self) {
        self.left_latch = 1;
        self.right_latch = 1;
        self.left_chr_page = [0; 2];
        self.right_chr_page = [0; 2];
        self.need_chr_update = false;
        self.slot0_mapped = false;
        self.common.mirroring = self.common.info.mirroring;

        let prg_len = self.common.prg_rom.len();
        if self.is_mmc4 {
            let page_count = (prg_len / 0x4000).max(1);
            let base = (page_count - 1) * 0x4000;
            self.common.map_prg_8k(2, base);
            self.common.map_prg_8k(3, base + 0x2000);
            self.common.map_prg_8k(0, 0);
            self.common.map_prg_8k(1, 0);
        } else {
            let page_count = (prg_len / 0x2000).max(1);
            let page = |back: usize| (page_count - (back % page_count)) % page_count;
            self.common.map_prg_8k(1, page(3) * 0x2000);
            self.common.map_prg_8k(2, page(2) * 0x2000);
            self.common.map_prg_8k(3, page(1) * 0x2000);
            self.common.map_prg_8k(0, 0);
        }

        self.select_chr_page(0, self.left_chr_page[self.left_latch as usize]);
        self.select_chr_page(1, self.right_chr_page[self.right_latch as usize]);
    }

    /// Equivalent of Mesen's `NotifyVRAMAddressChange`, called for every PPU
    /// bus address change before the read/write is served.
    fn notify_vram_address(&mut self, addr: u16) {
        let addr = addr & 0x3FFF;
        if self.need_chr_update {
            self.select_chr_page(0, self.left_chr_page[self.left_latch as usize]);
            self.select_chr_page(1, self.right_chr_page[self.right_latch as usize]);
            self.need_chr_update = false;
        }

        if self.is_mmc4 {
            if (0x0FD8..=0x0FDF).contains(&addr) {
                self.left_latch = 0;
                self.need_chr_update = true;
            } else if (0x0FE8..=0x0FEF).contains(&addr) {
                self.left_latch = 1;
                self.need_chr_update = true;
            } else if (0x1FD8..=0x1FDF).contains(&addr) {
                self.right_latch = 0;
                self.need_chr_update = true;
            } else if (0x1FE8..=0x1FEF).contains(&addr) {
                self.right_latch = 1;
                self.need_chr_update = true;
            }
        } else {
            // MMC2 only decodes the exact left-fetch addresses.
            if addr == 0x0FD8 {
                self.left_latch = 0;
                self.need_chr_update = true;
            } else if addr == 0x0FE8 {
                self.left_latch = 1;
                self.need_chr_update = true;
            } else if (0x1FD8..=0x1FDF).contains(&addr) {
                self.right_latch = 0;
                self.need_chr_update = true;
            } else if (0x1FE8..=0x1FEF).contains(&addr) {
                self.right_latch = 1;
                self.need_chr_update = true;
            }
        }
    }
}

impl Mapper for Mmc2 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }

    fn reset(&mut self) {
        // Mesen's BaseMapper::Reset is a no-op for MMC2/MMC4: latches and
        // bank registers survive a soft reset.
    }

    fn cpu_read(&mut self, addr: u16) -> Option<u8> {
        let slot0_range_end = if self.is_mmc4 { 0xBFFF } else { 0x9FFF };
        match addr {
            0x6000..=0x7FFF => Some(self.common.read_prg_ram(addr)),
            a if (0x8000..=slot0_range_end).contains(&a) && !self.slot0_mapped => None,
            0x8000..=0xFFFF => Some(self.common.read_prg_rom(addr)),
            _ => None,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        if (0x6000..=0x7FFF).contains(&addr) {
            self.common.write_prg_ram(addr, value);
            return;
        }

        match addr >> 12 {
            0xA => self.select_prg_slot0(value),
            0xB => {
                self.left_chr_page[0] = value & 0x1F;
                self.select_chr_page(0, self.left_chr_page[self.left_latch as usize]);
            }
            0xC => {
                self.left_chr_page[1] = value & 0x1F;
                self.select_chr_page(0, self.left_chr_page[self.left_latch as usize]);
            }
            0xD => {
                self.right_chr_page[0] = value & 0x1F;
                self.select_chr_page(1, self.right_chr_page[self.right_latch as usize]);
            }
            0xE => {
                self.right_chr_page[1] = value & 0x1F;
                self.select_chr_page(1, self.right_chr_page[self.right_latch as usize]);
            }
            0xF => {
                self.common.mirroring = if value & 0x01 != 0 {
                    Mirroring::Horizontal
                } else {
                    Mirroring::Vertical
                };
            }
            _ => {}
        }
    }

    fn ppu_read(&mut self, addr: u16) -> Option<u8> {
        self.notify_vram_address(addr);
        let addr = addr & 0x3FFF;
        match addr {
            0x0000..=0x1FFF => Some(self.common.read_chr(addr)),
            // $3000-$3EFF mirrors $2000-$2EFF.
            0x2000..=0x3EFF => Some(self.common.read_nametable(addr & 0x2FFF)),
            _ => None,
        }
    }

    fn ppu_write(&mut self, addr: u16, value: u8) {
        self.notify_vram_address(addr);
        match addr & 0x3FFF {
            0x0000..=0x1FFF => self.common.write_chr(addr, value),
            0x2000..=0x3EFF => self.common.write_nametable(addr & 0x2FFF, value),
            _ => {}
        }
    }

    fn battery(&self) -> Option<&[u8]> {
        if self.common.info.has_battery {
            Some(&self.common.prg_ram)
        } else {
            None
        }
    }

    fn set_battery(&mut self, data: &[u8]) {
        if self.common.info.has_battery {
            let len = self.common.prg_ram.len();
            let n = data.len().min(len);
            self.common.prg_ram[..n].copy_from_slice(&data[..n]);
        }
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);
        w.bool(self.is_mmc4);
        w.u8(self.left_latch);
        w.u8(self.right_latch);
        w.u8(self.left_chr_page[0]);
        w.u8(self.left_chr_page[1]);
        w.u8(self.right_chr_page[0]);
        w.u8(self.right_chr_page[1]);
        w.bool(self.need_chr_update);
        w.bool(self.slot0_mapped);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        self.is_mmc4 = r.bool()?;
        self.left_latch = r.u8()? & 1;
        self.right_latch = r.u8()? & 1;
        self.left_chr_page[0] = r.u8()?;
        self.left_chr_page[1] = r.u8()?;
        self.right_chr_page[0] = r.u8()?;
        self.right_chr_page[1] = r.u8()?;
        self.need_chr_update = r.bool()?;
        self.slot0_mapped = r.bool()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// MMC5
// ---------------------------------------------------------------------------

const EXRAM_SIZE: usize = 0x400;
const NT_EMPTY_INDEX: u8 = 2;
const NT_FILL_INDEX: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PrgKind {
    Rom,
    WorkRam,
    SaveRam,
    Unmapped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PrgMapping {
    kind: PrgKind,
    /// Byte offset of the start of the 8 KB page.
    offset: usize,
    writable: bool,
}

impl PrgMapping {
    const fn unmapped() -> Self {
        Self {
            kind: PrgKind::Unmapped,
            offset: 0,
            writable: false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct BankInfo {
    bank: usize,
    kind: PrgKind,
    writable: bool,
}

impl BankInfo {
    fn unmapped() -> Self {
        Self {
            bank: 0,
            kind: PrgKind::Unmapped,
            writable: false,
        }
    }
}

/// One MMC5 pulse channel (MMC5Audio.h `MMC5Square`). The sweep unit is absent.
#[derive(Clone, Copy, Debug, Default)]
struct Mmc5Square {
    enabled: bool,
    length_counter: u8,
    length_counter_halt: bool,
    new_halt_value: bool,
    length_counter_reload_value: u8,
    length_counter_previous_value: u8,

    constant_volume: bool,
    volume: u8,
    envelope_divider: i8,
    envelope_counter: u8,
    envelope_start: bool,

    duty: u8,
    duty_pos: u8,
    real_period: u16,
    timer: u16,
    period: u16,
    current_output: i8,
}

const DUTY_SEQUENCES: [[u8; 8]; 4] = [
    [0, 0, 0, 0, 0, 0, 0, 1],
    [0, 0, 0, 0, 0, 0, 1, 1],
    [0, 0, 0, 0, 1, 1, 1, 1],
    [1, 1, 1, 1, 1, 1, 0, 0],
];

const LENGTH_LOOKUP: [u8; 32] = [
    10, 254, 20, 2, 40, 4, 80, 6, 160, 8, 60, 10, 14, 12, 26, 14, 12, 16, 24, 18, 48, 20, 96,
    22, 192, 24, 72, 26, 16, 28, 32, 30,
];

impl Mmc5Square {
    fn write_register(&mut self, addr: u16, value: u8) {
        match addr & 0x03 {
            0 => {
                // The halt flag only takes effect on the next length-counter
                // reload, like the APU's `InitializeLengthCounter`.
                self.new_halt_value = value & 0x20 != 0;
                self.constant_volume = value & 0x10 != 0;
                self.volume = value & 0x0F;
                self.duty = (value & 0xC0) >> 6;
            }
            1 => {
                // "$5001 has no effect. The MMC5 pulse channels will not sweep"
            }
            2 => {
                self.set_period((self.real_period & 0x0700) | value as u16);
            }
            3 => {
                self.load_length_counter(value >> 3);
                self.set_period((self.real_period & 0x00FF) | (((value & 0x07) as u16) << 8));
                self.duty_pos = 0;
                self.envelope_start = true;
            }
            _ => unreachable!(),
        }
    }

    fn set_period(&mut self, period: u16) {
        self.real_period = period & 0x7FF;
        self.period = self.real_period.wrapping_mul(2).wrapping_add(1);
    }

    fn volume(&self) -> u8 {
        if self.length_counter > 0 {
            if self.constant_volume {
                self.volume
            } else {
                self.envelope_counter
            }
        } else {
            0
        }
    }

    fn run_channel(&mut self) {
        if self.timer == 0 {
            self.duty_pos = self.duty_pos.wrapping_sub(1) & 0x07;
            // "Frequency values less than 8 do not silence the MMC5 pulse
            // channels; they can output ultrasonic frequencies."
            self.current_output =
                (DUTY_SEQUENCES[self.duty as usize & 3][self.duty_pos as usize]
                    * self.volume()) as i8;
            self.timer = self.period;
        } else {
            self.timer -= 1;
        }
    }

    fn tick_length_counter(&mut self) {
        if self.length_counter > 0 && !self.length_counter_halt {
            self.length_counter -= 1;
        }
    }

    fn tick_envelope(&mut self) {
        if !self.envelope_start {
            self.envelope_divider -= 1;
            if self.envelope_divider < 0 {
                self.envelope_divider = self.volume as i8;
                if self.envelope_counter > 0 {
                    self.envelope_counter -= 1;
                } else if self.length_counter_halt {
                    self.envelope_counter = 15;
                }
            }
        } else {
            self.envelope_start = false;
            self.envelope_counter = 15;
            self.envelope_divider = self.volume as i8;
        }
    }

    fn reload_counter(&mut self) {
        if self.length_counter_reload_value != 0 {
            if self.length_counter == self.length_counter_previous_value {
                self.length_counter = self.length_counter_reload_value;
            }
            self.length_counter_reload_value = 0;
        }
        self.length_counter_halt = self.new_halt_value;
    }

    fn load_length_counter(&mut self, value: u8) {
        if self.enabled {
            self.length_counter_reload_value = LENGTH_LOOKUP[(value & 0x1F) as usize];
            self.length_counter_previous_value = self.length_counter;
        }
    }

    fn set_enabled(&mut self, enabled: bool) {
        if !enabled {
            self.length_counter = 0;
        }
        self.enabled = enabled;
    }

    fn status(&self) -> bool {
        self.length_counter > 0
    }

    fn save_state(&self, w: &mut StateWriter) {
        w.bool(self.enabled);
        w.u8(self.length_counter);
        w.bool(self.length_counter_halt);
        w.bool(self.new_halt_value);
        w.u8(self.length_counter_reload_value);
        w.u8(self.length_counter_previous_value);
        w.bool(self.constant_volume);
        w.u8(self.volume);
        w.u8(self.envelope_divider as u8);
        w.u8(self.envelope_counter);
        w.bool(self.envelope_start);
        w.u8(self.duty);
        w.u8(self.duty_pos);
        w.u16(self.real_period);
        w.u16(self.timer);
        w.u16(self.period);
        w.u8(self.current_output as u8);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.enabled = r.bool()?;
        self.length_counter = r.u8()?;
        self.length_counter_halt = r.bool()?;
        self.new_halt_value = r.bool()?;
        self.length_counter_reload_value = r.u8()?;
        self.length_counter_previous_value = r.u8()?;
        self.constant_volume = r.bool()?;
        self.volume = r.u8()?;
        self.envelope_divider = r.u8()? as i8;
        self.envelope_counter = r.u8()?;
        self.envelope_start = r.bool()?;
        self.duty = r.u8()? & 3;
        self.duty_pos = r.u8()? & 7;
        self.real_period = r.u16()? & 0x7FF;
        self.timer = r.u16()?;
        self.period = r.u16()?;
        self.current_output = r.u8()? as i8;
        Ok(())
    }
}

/// MMC5 expansion audio (two pulses + raw PCM, no sweep).
#[derive(Clone, Copy, Debug, Default)]
struct Mmc5Audio {
    square1: Mmc5Square,
    square2: Mmc5Square,
    audio_counter: i16,
    last_output: i16,
    pcm_read_mode: bool,
    pcm_irq_enabled: bool,
    pcm_output: u8,
    /// "IRQ trip" latch from MMC5 audio ($5010 bit 7 read-back).
    pcm_irq_trip: bool,
}

impl Mmc5Audio {
    fn clock(&mut self, clock_rate: u32) {
        self.audio_counter = self.audio_counter.wrapping_sub(1);
        self.square1.run_channel();
        self.square2.run_channel();
        if self.audio_counter <= 0 {
            // ~240 Hz envelope/length counter, independent of the APU frame
            // sequencer.
            self.audio_counter = (clock_rate / 240).max(1) as i16;
            self.square1.tick_length_counter();
            self.square1.tick_envelope();
            self.square2.tick_length_counter();
            self.square2.tick_envelope();
        }

        // "The polarity of all MMC5 channels is reversed compared to the APU."
        self.last_output = -((self.square1.current_output as i16)
            + (self.square2.current_output as i16)
            + self.pcm_output as i16);

        self.square1.reload_counter();
        self.square2.reload_counter();
    }

    /// Instantaneous level in -1.0..=0.0. The MMC5 mixer in Mesen applies the
    /// same gain to pulses and PCM, so retain the hardware ratio here.
    fn sample(&self) -> f32 {
        (self.last_output as f32 / 160.0).clamp(-1.0, 1.0)
    }

    fn read_register(&mut self, addr: u16) -> u8 {
        match addr {
            0x5010 => {
                let value = ((self.pcm_irq_trip && self.pcm_irq_enabled) as u8) << 7
                    | self.pcm_read_mode as u8;
                self.pcm_irq_trip = false;
                value
            }
            0x5015 => {
                (self.square1.status() as u8) | ((self.square2.status() as u8) << 1)
            }
            _ => 0,
        }
    }

    fn write_register(&mut self, addr: u16, value: u8) {
        match addr {
            0x5000..=0x5003 => self.square1.write_register(addr, value),
            0x5004..=0x5007 => self.square2.write_register(addr, value),
            0x5010 => {
                self.pcm_read_mode = value & 0x01 != 0;
                self.pcm_irq_enabled = value & 0x80 != 0;
            }
            0x5011 => {
                if !self.pcm_read_mode {
                    self.pcm_dac_write(value);
                }
            }
            0x5015 => {
                self.square1.set_enabled(value & 0x01 != 0);
                self.square2.set_enabled(value & 0x02 != 0);
            }
            _ => {}
        }
    }

    fn pcm_dac_write(&mut self, value: u8) {
        // "If you try to assign a value of $00, the DAC is not changed; an IRQ
        // is generated instead."
        if value == 0 {
            self.pcm_irq_trip = true;
        } else {
            self.pcm_output = value;
            self.pcm_irq_trip = false;
        }
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.square1.save_state(w);
        self.square2.save_state(w);
        w.i16(self.audio_counter);
        w.i16(self.last_output);
        w.bool(self.pcm_read_mode);
        w.bool(self.pcm_irq_enabled);
        w.u8(self.pcm_output);
        w.bool(self.pcm_irq_trip);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.square1.load_state(r)?;
        self.square2.load_state(r)?;
        self.audio_counter = r.i16()?;
        self.last_output = r.i16()?;
        self.pcm_read_mode = r.bool()?;
        self.pcm_irq_enabled = r.bool()?;
        self.pcm_output = r.u8()?;
        self.pcm_irq_trip = r.bool()?;
        Ok(())
    }
}

/// MMC5 (mapper 5).
pub struct Mmc5 {
    common: MapperCommon,

    // PRG RAM. The MMC5 has two separately addressable RAM chips; Mesen
    // emulates a 64 KB superset for iNES 1.0 boards. ExRAM is appended to the
    // active RAM array exactly like Mesen's `GetSaveRamSize`/`GetWorkRamSize`.
    work_ram: Vec<u8>,
    save_ram: Vec<u8>,
    work_real_size: usize,
    save_real_size: usize,
    exram_offset: usize,
    exram_in_save: bool,

    // PRG banking
    prg_mode: u8,
    prg_banks: [u8; 5],
    prg_ram_protect1: u8,
    prg_ram_protect2: u8,
    prg_slots: [PrgMapping; 4],
    prg_6000: PrgMapping,

    // CHR banking
    chr_mode: u8,
    chr_upper_bits: u8,
    chr_banks: [u16; 12],
    last_chr_reg: u16,
    prev_chr_a: bool,

    /// Not a hardware register: fallback for 8x16 sprite detection.
    large_sprites: bool,
    ppu_ctrl_hook_used: bool,
    chr_b_regs_written: bool,
    sprite_tables_seen: [bool; 2],

    // ExRAM / nametables
    nametable_mapping: u8,
    extended_ram_mode: u8,
    fill_mode_tile: u8,
    fill_mode_color: u8,

    // Extended attribute mode
    ex_attr_last_nametable_fetch: u16,
    ex_attr_last_fetch_counter: i8,
    ex_attr_selected_chr_bank: u16,

    // Vertical split
    vertical_split_enabled: bool,
    vertical_split_right_side: bool,
    vertical_split_delimiter_tile: u8,
    vertical_split_scroll: u8,
    vertical_split_bank: u8,
    split_in_split_region: bool,
    split_vertical_scroll: u32,
    split_tile: u32,
    split_tile_number: i32,

    // Scanline IRQ
    irq_counter_target: u8,
    irq_enabled: bool,
    scanline_counter: u8,
    // Pending flag lives in `common.irq_pending`.

    // PPU frame detection
    need_in_frame: bool,
    ppu_in_frame: bool,
    ppu_idle_counter: u8,
    last_ppu_read_addr: u16,
    nt_read_counter: u8,

    // Multiplier
    multiplier_value1: u8,
    multiplier_value2: u8,

    audio: Mmc5Audio,
}

impl Mmc5 {
    pub fn new(rom: RomData) -> Self {
        let info = rom.info.clone();
        let battery_data = rom.battery.clone();
        let trainer = rom.trainer.clone();
        let mut common = MapperCommon::new(rom);

        let has_battery = info.has_battery;
        let base_work = if info.is_nes2 {
            info.prg_ram_size
        } else if has_battery {
            0
        } else {
            0x10000
        };
        let base_save = if info.is_nes2 {
            info.prg_nvram_size
        } else if has_battery {
            0x10000
        } else {
            0
        };

        let exram_in_save = has_battery;
        let mut work_ram = vec![0u8; base_work + if exram_in_save { 0 } else { EXRAM_SIZE }];
        let mut save_ram = vec![0u8; base_save + if exram_in_save { EXRAM_SIZE } else { 0 }];

        if let Some(trainer) = trainer {
            let dst = if has_battery { &mut save_ram } else { &mut work_ram };
            if dst.len() >= 0x2000 && trainer.len() >= 0x200 {
                dst[0x1000..0x1200].copy_from_slice(&trainer[..0x200]);
            }
        }

        if let Some(battery) = battery_data {
            if has_battery {
                let n = battery.len().min(save_ram.len());
                save_ram[..n].copy_from_slice(&battery[..n]);
            }
        }

        common.nametables[NT_EMPTY_INDEX as usize * 0x400..(NT_EMPTY_INDEX as usize + 1) * 0x400]
            .fill(0);

        let mut mapper = Self {
            common,
            work_ram,
            save_ram,
            work_real_size: base_work,
            save_real_size: base_save,
            exram_offset: if exram_in_save { base_save } else { base_work },
            exram_in_save,

            prg_mode: 0,
            prg_banks: [0; 5],
            prg_ram_protect1: 0,
            prg_ram_protect2: 0,
            prg_slots: [PrgMapping::unmapped(); 4],
            prg_6000: PrgMapping::unmapped(),

            chr_mode: 0,
            chr_upper_bits: 0,
            chr_banks: [0; 12],
            last_chr_reg: 0,
            prev_chr_a: false,

            large_sprites: false,
            ppu_ctrl_hook_used: false,
            chr_b_regs_written: false,
            sprite_tables_seen: [false; 2],

            nametable_mapping: 0,
            extended_ram_mode: 0,
            fill_mode_tile: 0,
            fill_mode_color: 0,

            ex_attr_last_nametable_fetch: 0,
            ex_attr_last_fetch_counter: 0,
            ex_attr_selected_chr_bank: 0,

            vertical_split_enabled: false,
            vertical_split_right_side: false,
            vertical_split_delimiter_tile: 0,
            vertical_split_scroll: 0,
            vertical_split_bank: 0,
            split_in_split_region: false,
            split_vertical_scroll: 0,
            split_tile: 0,
            split_tile_number: -1,

            irq_counter_target: 0,
            irq_enabled: false,
            scanline_counter: 0,

            need_in_frame: false,
            ppu_in_frame: false,
            ppu_idle_counter: 0,
            last_ppu_read_addr: 0,
            nt_read_counter: 0,

            multiplier_value1: 0,
            multiplier_value2: 0,

            audio: Mmc5Audio::default(),
        };
        mapper.power_on();
        mapper
    }

    fn has_battery(&self) -> bool {
        self.common.info.has_battery
    }

    fn exram(&self) -> &[u8] {
        let ram = if self.exram_in_save {
            &self.save_ram
        } else {
            &self.work_ram
        };
        &ram[self.exram_offset..self.exram_offset + EXRAM_SIZE]
    }

    fn exram_mut(&mut self) -> &mut [u8] {
        let offset = self.exram_offset;
        let ram = if self.exram_in_save {
            &mut self.save_ram
        } else {
            &mut self.work_ram
        };
        &mut ram[offset..offset + EXRAM_SIZE]
    }

    fn power_on(&mut self) {
        // Reset mapper registers to the Mesen `InitMapper` power-on values.
        self.prg_mode = 0;
        self.prg_banks = [0; 5];
        self.prg_ram_protect1 = 0;
        self.prg_ram_protect2 = 0;
        self.prg_slots = [PrgMapping::unmapped(); 4];
        self.prg_6000 = PrgMapping::unmapped();

        self.chr_mode = 0;
        self.chr_upper_bits = 0;
        self.chr_banks = [0; 12];
        self.last_chr_reg = 0;
        self.prev_chr_a = false;
        self.large_sprites = false;
        self.ppu_ctrl_hook_used = false;
        self.chr_b_regs_written = false;
        self.sprite_tables_seen = [false; 2];

        self.nametable_mapping = 0;
        self.extended_ram_mode = 0;
        self.fill_mode_tile = 0;
        self.fill_mode_color = 0;

        self.ex_attr_last_nametable_fetch = 0;
        self.ex_attr_last_fetch_counter = 0;
        self.ex_attr_selected_chr_bank = 0;

        self.vertical_split_enabled = false;
        self.vertical_split_right_side = false;
        self.vertical_split_delimiter_tile = 0;
        self.vertical_split_scroll = 0;
        self.vertical_split_bank = 0;
        self.split_in_split_region = false;
        self.split_vertical_scroll = 0;
        self.split_tile = 0;
        self.split_tile_number = -1;

        self.irq_counter_target = 0;
        self.irq_enabled = false;
        self.scanline_counter = 0;
        self.common.irq_pending = false;

        self.need_in_frame = false;
        self.ppu_in_frame = false;
        self.ppu_idle_counter = 0;
        self.last_ppu_read_addr = 0;
        self.nt_read_counter = 0;

        self.multiplier_value1 = 0;
        self.multiplier_value2 = 0;

        self.audio = Mmc5Audio::default();

        self.common.nametables[NT_EMPTY_INDEX as usize * 0x400
            ..(NT_EMPTY_INDEX as usize + 1) * 0x400]
            .fill(0);
        self.common.mirroring = self.common.info.mirroring;

        // "Additionally, Romance of the 3 Kingdoms 2 seems to expect it to be
        // in 8k PRG mode ($5100 = $03)."
        self.write_mapper_register(0x5100, 0x03);
        // "Games seem to expect $5117 to be $FF on powerup."
        self.write_mapper_register(0x5117, 0xFF);
        self.update_chr_banks(true);
    }

    // -- CHR banking --------------------------------------------------------

    fn update_chr_banks(&mut self, force_update: bool) {
        let large_sprites = self.large_sprites;
        if !large_sprites {
            // "Using 8x8 sprites resets the last written to bank logic"
            self.last_chr_reg = 0;
        }

        let chr_a = !large_sprites
            || (self.split_tile_number >= 32 && self.split_tile_number < 40)
            || (!self.ppu_in_frame && self.last_chr_reg <= 0x5127);
        if !force_update && chr_a == self.prev_chr_a {
            return;
        }
        self.prev_chr_a = chr_a;

        let banks = self.chr_banks;
        let bank = |index: usize| banks[index] as usize;
        match self.chr_mode {
            0 => {
                let base = bank(if chr_a { 0x07 } else { 0x0B }) * 8;
                for i in 0..8 {
                    self.common.map_chr_1k(i, (base + i) * 0x400);
                }
            }
            1 => {
                let first = bank(if chr_a { 0x03 } else { 0x0B }) * 4;
                let second = bank(if chr_a { 0x07 } else { 0x0B }) * 4;
                for i in 0..4 {
                    self.common.map_chr_1k(i, (first + i) * 0x400);
                    self.common.map_chr_1k(4 + i, (second + i) * 0x400);
                }
            }
            2 => {
                let pages = [
                    bank(if chr_a { 0x01 } else { 0x09 }),
                    bank(if chr_a { 0x03 } else { 0x0B }),
                    bank(if chr_a { 0x05 } else { 0x09 }),
                    bank(if chr_a { 0x07 } else { 0x0B }),
                ];
                for (slot, page) in pages.iter().enumerate() {
                    let base = page * 2;
                    self.common.map_chr_1k(slot * 2, base * 0x400);
                    self.common.map_chr_1k(slot * 2 + 1, (base + 1) * 0x400);
                }
            }
            _ => {
                let indexes_a = [0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
                let indexes_b = [0x08, 0x09, 0x0A, 0x0B, 0x08, 0x09, 0x0A, 0x0B];
                for slot in 0..8 {
                    let page = bank(if chr_a {
                        indexes_a[slot]
                    } else {
                        indexes_b[slot]
                    });
                    self.common.map_chr_1k(slot, page * 0x400);
                }
            }
        }
    }

    fn switch_chr_bank(&mut self, reg: u16, value: u8) {
        let index = (reg - 0x5120) as usize;
        let new_value = value as u16 | ((self.chr_upper_bits as u16) << 8);
        if new_value != self.chr_banks[index] || self.last_chr_reg != reg {
            self.chr_banks[index] = new_value;
            self.last_chr_reg = reg;
            if reg >= 0x5128 {
                // Fallback signal for 8x16 sprite mode (see module docs).
                self.chr_b_regs_written = true;
            }
            self.update_chr_banks(true);
        }
    }

    fn chr_read_absolute(&self, position: usize) -> u8 {
        let len = self.common.chr.len().max(1);
        self.common.chr[position % len]
    }

    // -- PRG banking --------------------------------------------------------

    fn current_bank_info(&self, reg: u16) -> BankInfo {
        let raw = self.prg_banks[(reg - 0x5113) as usize];
        let is_ram = ((raw & 0x80) == 0 && reg != 0x5117) || reg == 0x5113;
        if !is_ram {
            return BankInfo {
                bank: (raw & 0x7F) as usize,
                kind: PrgKind::Rom,
                writable: false,
            };
        }

        let mut bank = (raw & 0x07) as usize;
        let writable = self.prg_ram_protect1 == 0x02 && self.prg_ram_protect2 == 0x01;
        let real_work = self.work_real_size;
        let real_save = self.save_real_size;

        if self.common.info.is_nes2 && (real_work >= 0x10000 || real_save >= 0x10000) {
            // Mesen 0.9.9 lacks this case; newer Mesen allows a single
            // 64/128 KB block for NES 2.0/homebrew boards.
            let kind = if self.has_battery() {
                PrgKind::SaveRam
            } else {
                PrgKind::WorkRam
            };
            bank &= 0x0F;
            return self.finish_ram_bank(bank, kind, writable);
        }

        let kind;
        if self.common.info.is_nes2 {
            kind = if self.has_battery() && (bank <= 3 || real_save > 0x2000) {
                PrgKind::SaveRam
            } else {
                PrgKind::WorkRam
            };
            if real_save + real_work != 0x4000 && bank >= 4 {
                // "When not 2x 8kb (=16kb), banks 4/5/6/7 select the empty
                // socket and return open bus"
                return BankInfo::unmapped();
            }
        } else {
            kind = if self.has_battery() {
                PrgKind::SaveRam
            } else {
                PrgKind::WorkRam
            };
        }

        bank &= 0x07;
        self.finish_ram_bank(bank, kind, writable)
    }

    fn finish_ram_bank(&self, bank: usize, kind: PrgKind, writable: bool) -> BankInfo {
        let size = match kind {
            PrgKind::WorkRam => self.work_real_size,
            PrgKind::SaveRam => self.save_real_size,
            _ => 0,
        };
        if size < 0x2000 {
            return BankInfo::unmapped();
        }
        let page_count = size / 0x2000;
        BankInfo {
            bank: bank % page_count,
            kind,
            writable,
        }
    }

    fn make_mapping(&self, info: BankInfo, page_offset: usize) -> PrgMapping {
        match info.kind {
            PrgKind::Rom => {
                let pages = (self.common.prg_rom.len() / 0x2000).max(1);
                let page = (info.bank + page_offset) % pages;
                PrgMapping {
                    kind: PrgKind::Rom,
                    offset: page * 0x2000,
                    writable: false,
                }
            }
            PrgKind::WorkRam | PrgKind::SaveRam => {
                let size = if info.kind == PrgKind::WorkRam {
                    self.work_real_size
                } else {
                    self.save_real_size
                };
                let pages = (size / 0x2000).max(1);
                PrgMapping {
                    kind: info.kind,
                    offset: ((info.bank + page_offset) % pages) * 0x2000,
                    writable: info.writable,
                }
            }
            PrgKind::Unmapped => PrgMapping::unmapped(),
        }
    }

    fn update_prg_banks(&mut self) {
        let mut slots = [PrgMapping::unmapped(); 4];

        self.prg_6000 = {
            let info = self.current_bank_info(0x5113);
            self.make_mapping(info, 0)
        };

        let bank15 = self.current_bank_info(0x5115);
        let bank17 = self.current_bank_info(0x5117);

        match self.prg_mode {
            0 => {
                // 32 KB at $8000-$FFFF, bottom 2 bits ignored.
                let base = BankInfo {
                    bank: bank17.bank & !3,
                    kind: bank17.kind,
                    writable: bank17.writable,
                };
                for (i, slot) in slots.iter_mut().enumerate() {
                    *slot = self.make_mapping(base, i);
                }
            }
            1 => {
                // 16 KB at $8000 (register 5115), 16 KB at $C000 (5117).
                // Register bit 0 is ignored for both 16 KB windows.
                let base15 = BankInfo {
                    bank: bank15.bank & !1,
                    kind: bank15.kind,
                    writable: bank15.writable,
                };
                for i in 0..2 {
                    slots[i] = self.make_mapping(base15, i);
                }
                let base = BankInfo {
                    bank: bank17.bank & !1,
                    kind: bank17.kind,
                    writable: bank17.writable,
                };
                for i in 0..2 {
                    slots[2 + i] = self.make_mapping(base, i);
                }
            }
            2 => {
                // 16 KB at $8000 (5115), 8 KB at $C000 (5116), 8 KB at
                // $E000 (5117). Register bit 0 is ignored for the 16 KB range.
                let base15 = BankInfo {
                    bank: bank15.bank & !1,
                    kind: bank15.kind,
                    writable: bank15.writable,
                };
                for i in 0..2 {
                    slots[i] = self.make_mapping(base15, i);
                }
                slots[2] = self.make_mapping(self.current_bank_info(0x5116), 0);
                slots[3] = self.make_mapping(
                    BankInfo {
                        bank: bank17.bank & 0x7F,
                        kind: bank17.kind,
                        writable: bank17.writable,
                    },
                    0,
                );
            }
            _ => {
                // Four independent 8 KB banks.
                slots[0] = self.make_mapping(self.current_bank_info(0x5114), 0);
                slots[1] = self.make_mapping(bank15, 0);
                slots[2] = self.make_mapping(self.current_bank_info(0x5116), 0);
                slots[3] = self.make_mapping(
                    BankInfo {
                        bank: bank17.bank & 0x7F,
                        kind: bank17.kind,
                        writable: bank17.writable,
                    },
                    0,
                );
            }
        }

        self.prg_slots = slots;
        for (i, slot) in self.prg_slots.iter().enumerate() {
            self.common.prg_map[i] = if slot.kind == PrgKind::Rom {
                slot.offset
            } else {
                0
            };
        }
    }

    fn switch_prg_bank(&mut self, reg: u16, value: u8) {
        self.prg_banks[(reg - 0x5113) as usize] = value;
        self.update_prg_banks();
    }

    fn read_prg_mapping(&self, mapping: PrgMapping, offset: usize) -> Option<u8> {
        match mapping.kind {
            PrgKind::Rom => {
                let len = self.common.prg_rom.len().max(1);
                Some(self.common.prg_rom[(mapping.offset + offset) % len])
            }
            PrgKind::WorkRam => {
                let len = self.work_real_size;
                if len == 0 {
                    None
                } else {
                    Some(self.work_ram[(mapping.offset + offset) % len])
                }
            }
            PrgKind::SaveRam => {
                let len = self.save_real_size;
                if len == 0 {
                    None
                } else {
                    Some(self.save_ram[(mapping.offset + offset) % len])
                }
            }
            PrgKind::Unmapped => None,
        }
    }

    fn write_prg_mapping(&mut self, mapping: PrgMapping, offset: usize, value: u8) {
        if !mapping.writable {
            return;
        }
        match mapping.kind {
            PrgKind::WorkRam if self.work_real_size != 0 => {
                let len = self.work_real_size;
                self.work_ram[(mapping.offset + offset) % len] = value;
            }
            PrgKind::SaveRam if self.save_real_size != 0 => {
                let len = self.save_real_size;
                self.save_ram[(mapping.offset + offset) % len] = value;
            }
            _ => {}
        }
    }

    fn read_prg(&self, addr: u16) -> Option<u8> {
        if (0x6000..=0x7FFF).contains(&addr) {
            return self.read_prg_mapping(self.prg_6000, (addr - 0x6000) as usize);
        }
        let slot = ((addr as usize - 0x8000) >> 13) & 3;
        self.read_prg_mapping(self.prg_slots[slot], addr as usize & 0x1FFF)
    }

    fn write_prg(&mut self, addr: u16, value: u8) {
        if (0x6000..=0x7FFF).contains(&addr) {
            let mapping = self.prg_6000;
            self.write_prg_mapping(mapping, (addr - 0x6000) as usize, value);
            return;
        }
        let slot = ((addr as usize - 0x8000) >> 13) & 3;
        let mapping = self.prg_slots[slot];
        self.write_prg_mapping(mapping, addr as usize & 0x1FFF, value);
    }

    // -- Nametables / ExRAM -------------------------------------------------

    fn nametable_id(&self, slot: usize) -> u8 {
        (self.nametable_mapping >> (slot * 2)) & 0x03
    }

    fn read_nametable_byte(&self, addr: u16) -> u8 {
        let addr = addr & 0x2FFF;
        let slot = ((addr as usize - 0x2000) >> 10) & 3;
        let offset = addr as usize & 0x3FF;
        let id = self.nametable_id(slot);
        match id {
            0 | 1 => self.common.nametables[id as usize * 0x400 + offset],
            2 if self.extended_ram_mode <= 1 => self.exram()[offset],
            2 => self.common.nametables[NT_EMPTY_INDEX as usize * 0x400 + offset],
            _ => self.common.nametables[NT_FILL_INDEX as usize * 0x400 + offset],
        }
    }

    fn write_nametable_byte(&mut self, addr: u16, value: u8) {
        let addr = addr & 0x2FFF;
        let slot = ((addr as usize - 0x2000) >> 10) & 3;
        let offset = addr as usize & 0x3FF;
        let id = self.nametable_id(slot);
        match id {
            0 | 1 => self.common.nametables[id as usize * 0x400 + offset] = value,
            2 if self.extended_ram_mode <= 1 => self.exram_mut()[offset] = value,
            2 => {
                self.common.nametables[NT_EMPTY_INDEX as usize * 0x400 + offset] = value
            }
            _ => self.common.nametables[NT_FILL_INDEX as usize * 0x400 + offset] = value,
        }
    }

    fn set_nametable_mapping(&mut self, value: u8) {
        self.nametable_mapping = value;
    }

    fn set_extended_ram_mode(&mut self, mode: u8) {
        self.extended_ram_mode = mode;
    }

    fn set_fill_mode_tile(&mut self, tile: u8) {
        self.fill_mode_tile = tile;
        let base = NT_FILL_INDEX as usize * 0x400;
        self.common.nametables[base..base + 32 * 30].fill(tile);
    }

    fn set_fill_mode_color(&mut self, color: u8) {
        self.fill_mode_color = color;
        let attribute = color | (color << 2) | (color << 4) | (color << 6);
        let base = NT_FILL_INDEX as usize * 0x400 + 32 * 30;
        self.common.nametables[base..base + 64].fill(attribute);
    }

    fn write_exram(&mut self, offset: usize, value: u8) {
        match self.extended_ram_mode {
            0 | 1 => {
                // Can only be written while the PPU is rendering; writes during
                // blanking store 0.
                let value = if self.ppu_in_frame { value } else { 0 };
                self.exram_mut()[offset & 0x3FF] = value;
            }
            2 => self.exram_mut()[offset & 0x3FF] = value,
            _ => {}
        }
    }

    // -- PPU scanline / frame tracking --------------------------------------

    fn detect_scanline_start(&mut self, addr: u16) {
        if (0x2000..=0x2FFF).contains(&addr) {
            if self.last_ppu_read_addr == addr {
                self.nt_read_counter = self.nt_read_counter.saturating_add(1);
            } else {
                self.nt_read_counter = 0;
            }

            if self.nt_read_counter >= 2 {
                self.sprite_tables_seen = [false; 2];
                if !self.ppu_in_frame && !self.need_in_frame {
                    self.need_in_frame = true;
                    self.scanline_counter = 0;
                } else {
                    self.scanline_counter = self.scanline_counter.wrapping_add(1);
                    if self.irq_counter_target == self.scanline_counter {
                        self.common.irq_pending = true;
                    }
                }
                self.split_tile_number = 0;
            }
        } else {
            self.nt_read_counter = 0;
        }
    }

    fn reset_frame_state_from_vector_read(&mut self) {
        self.ppu_in_frame = false;
        self.update_chr_banks(true);
        self.last_ppu_read_addr = 0;
        self.scanline_counter = 0;
        self.common.irq_pending = false;
    }

    // -- Register interface -------------------------------------------------

    fn write_mapper_register(&mut self, addr: u16, value: u8) {
        if (0x5113..=0x5117).contains(&addr) {
            self.switch_prg_bank(addr, value);
        } else if (0x5120..=0x512B).contains(&addr) {
            self.switch_chr_bank(addr, value);
        } else {
            match addr {
                0x5000..=0x5007 | 0x5010 | 0x5011 | 0x5015 => {
                    self.audio.write_register(addr, value)
                }
                0x5100 => {
                    self.prg_mode = value & 0x03;
                    self.update_prg_banks();
                }
                0x5101 => {
                    self.chr_mode = value & 0x03;
                    self.update_chr_banks(true);
                }
                0x5102 => {
                    self.prg_ram_protect1 = value & 0x03;
                    self.update_prg_banks();
                }
                0x5103 => {
                    self.prg_ram_protect2 = value & 0x03;
                    self.update_prg_banks();
                }
                0x5104 => self.set_extended_ram_mode(value & 0x03),
                0x5105 => self.set_nametable_mapping(value),
                0x5106 => self.set_fill_mode_tile(value),
                0x5107 => self.set_fill_mode_color(value & 0x03),
                0x5130 => self.chr_upper_bits = value & 0x03,
                0x5200 => {
                    self.vertical_split_enabled = value & 0x80 != 0;
                    self.vertical_split_right_side = value & 0x40 != 0;
                    self.vertical_split_delimiter_tile = value & 0x1F;
                }
                0x5201 => self.vertical_split_scroll = value,
                0x5202 => self.vertical_split_bank = value,
                0x5203 => self.irq_counter_target = value,
                0x5204 => self.irq_enabled = value & 0x80 != 0,
                0x5205 => self.multiplier_value1 = value,
                0x5206 => self.multiplier_value2 = value,
                _ => {}
            }
        }
    }

    fn read_mapper_register(&mut self, addr: u16) -> Option<u8> {
        match addr {
            0x5010 | 0x5015 => Some(self.audio.read_register(addr)),
            0x5204 => {
                let value = (self.ppu_in_frame as u8) << 6
                    | (self.common.irq_pending as u8) << 7;
                self.common.irq_pending = false;
                Some(value)
            }
            0x5205 => Some((self.multiplier_value1 as u16 * self.multiplier_value2 as u16) as u8),
            0x5206 => {
                Some(((self.multiplier_value1 as u16 * self.multiplier_value2 as u16) >> 8) as u8)
            }
            0x5C00..=0x5FFF => match self.extended_ram_mode {
                2 | 3 => Some(self.exram()[(addr as usize) & 0x3FF]),
                _ => None,
            },
            _ => None,
        }
    }

    /// Hook for the PPU owner: call for fully decoded writes to `$2000-$2007`.
    /// Mirrors Mesen's `MMC5MemoryHandler`. Safe to call repeatedly; PPU
    /// mirrors (`$2008-$3FFF`) must not be forwarded.
    pub fn ppu_register_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x2000 => self.set_ppu_ctrl(value),
            0x2001 => {
                if value & 0x18 == 0 {
                    // Rendering disabled: substitutions and in-frame detection
                    // stop; the scanline counter and pending IRQ are reset.
                    self.ppu_in_frame = false;
                    self.need_in_frame = false;
                    self.last_ppu_read_addr = 0;
                    self.scanline_counter = 0;
                    self.common.irq_pending = false;
                    self.update_chr_banks(true);
                }
            }
            _ => {}
        }
    }

    /// Hook for the PPU owner: fully decoded write to PPU `$2000`.
    pub fn set_ppu_ctrl(&mut self, value: u8) {
        self.ppu_ctrl_hook_used = true;
        let large = value & 0x20 != 0;
        if large != self.large_sprites {
            self.large_sprites = large;
            self.update_chr_banks(true);
        }
    }

    // -- Read/Chip interface ------------------------------------------------

    fn read_ppu(&mut self, full_addr: u16) -> Option<u8> {
        let addr = full_addr & 0x3FFF;
        let is_nt_fetch = (0x2000..=0x2FFF).contains(&addr) && (addr & 0x3FF) < 0x3C0;

        if is_nt_fetch {
            // Nametable data, not an attribute fetch.
            self.split_tile_number += 1;
            if self.ppu_in_frame {
                self.update_chr_banks(false);
            } else if self.need_in_frame {
                self.need_in_frame = false;
                self.ppu_in_frame = true;
                self.update_chr_banks(false);
            }
        }

        self.detect_scanline_start(addr);
        self.ppu_idle_counter = 3;
        self.last_ppu_read_addr = addr;

        // Fallback 8x16 sprite detection (only when the PPU hook is absent).
        // With 8x8 sprites all sprite fetches on a scanline use the single
        // sprite pattern table selected by $2000 bit 3. With 8x16 sprites the
        // tile's low bit selects the pattern table, so a scanline that fetches
        // 8x16 tiles of both parities touches both 4 KB halves. The B bank
        // registers are only meaningful in 8x16 mode, which keeps the
        // heuristic conservative.
        if !self.ppu_ctrl_hook_used
            && !self.large_sprites
            && self.chr_b_regs_written
            && self.ppu_in_frame
            && (32..40).contains(&self.split_tile_number)
            && addr < 0x2000
        {
            self.sprite_tables_seen[((addr >> 12) & 1) as usize] = true;
            if self.sprite_tables_seen[0] && self.sprite_tables_seen[1] {
                self.large_sprites = true;
                self.update_chr_banks(true);
            }
        }

        if self.extended_ram_mode <= 1 && self.ppu_in_frame {
            if self.vertical_split_enabled {
                let split_vertical_scroll =
                    (self.vertical_split_scroll as u32 + self.scanline_counter as u32) % 240;
                if addr >= 0x2000 {
                    if is_nt_fetch {
                        let tile_number = (self.split_tile_number + 2).rem_euclid(42);
                        let in_split = tile_number <= 32
                            && ((self.vertical_split_right_side
                                && tile_number >= self.vertical_split_delimiter_tile as i32)
                                || (!self.vertical_split_right_side
                                    && tile_number
                                        < self.vertical_split_delimiter_tile as i32));
                        if in_split {
                            self.split_in_split_region = true;
                            self.split_tile =
                                ((split_vertical_scroll & 0xF8) << 2) | tile_number as u32;
                            return Some(
                                self.exram()[(self.split_tile as usize) & 0x3FF],
                            );
                        } else {
                            self.split_in_split_region = false;
                        }
                    } else if self.split_in_split_region {
                        let offset = 0x3C0
                            | (((self.split_tile & 0x380) >> 4) as usize)
                            | (((self.split_tile & 0x1F) >> 2) as usize);
                        return Some(self.exram()[offset & 0x3FF]);
                    }
                } else if self.split_in_split_region {
                    // CHR tile fetches for split region.
                    let page_count = (self.common.chr.len() / 0x1000).max(1);
                    let bank = self.vertical_split_bank as usize % page_count;
                    let position = bank * 0x1000
                        + (((addr as usize & !0x07) | (split_vertical_scroll as usize & 0x07))
                            & 0xFFF);
                    return Some(self.chr_read_absolute(position));
                }
            }

            if self.extended_ram_mode == 1
                && (self.split_tile_number < 32 || self.split_tile_number >= 40)
            {
                if is_nt_fetch {
                    self.ex_attr_last_nametable_fetch = addr & 0x03FF;
                    self.ex_attr_last_fetch_counter = 3;
                } else if self.ex_attr_last_fetch_counter > 0 {
                    self.ex_attr_last_fetch_counter -= 1;
                    match self.ex_attr_last_fetch_counter {
                        2 => {
                            let value = self.exram()
                                [(self.ex_attr_last_nametable_fetch as usize) & 0x3FF];
                            let page_count = (self.common.chr.len() / 0x1000).max(1);
                            self.ex_attr_selected_chr_bank = ((((value as usize) & 0x3F)
                                | ((self.chr_upper_bits as usize) << 6))
                                % page_count) as u16;
                            let palette = (value & 0xC0) >> 6;
                            return Some(palette | (palette << 2) | (palette << 4) | (palette << 6));
                        }
                        1 | 0 => {
                            let position = self.ex_attr_selected_chr_bank as usize * 0x1000
                                + (addr as usize & 0xFFF);
                            return Some(self.chr_read_absolute(position));
                        }
                        _ => {}
                    }
                }
            }
        }

        match addr {
            0x0000..=0x1FFF => Some(self.common.read_chr(addr)),
            0x2000..=0x3EFF => Some(self.read_nametable_byte(addr)),
            _ => None,
        }
    }
}

impl Mapper for Mmc5 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }

    fn reset(&mut self) {
        // Mesen's MMC5::Reset only re-registers the PPU write handler; all
        // mapper state (banks, ExRAM, IRQ/audio) survives a soft reset. Power
        // on state is established by `new`/`power_on`.
    }

    fn ppu_register_write(&mut self, addr: u16, value: u8) {
        Mmc5::ppu_register_write(self, addr, value);
    }

    fn cpu_read(&mut self, addr: u16) -> Option<u8> {
        match addr {
            0xFFFA | 0xFFFB => {
                self.reset_frame_state_from_vector_read();
                self.read_prg(addr)
            }
            0x5C00..=0x5FFF => self.read_mapper_register(addr),
            0x5000..=0x5BFF => self.read_mapper_register(addr),
            0x6000..=0x7FFF => self.read_prg(addr),
            0x8000..=0xFFFF => {
                let value = self.read_prg(addr);
                if (0x8000..=0xBFFF).contains(&addr) && self.audio.pcm_read_mode {
                    if let Some(byte) = value {
                        self.audio.pcm_dac_write(byte);
                    }
                }
                value
            }
            _ => None,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x5000..=0x5206 => self.write_mapper_register(addr, value),
            0x5C00..=0x5FFF => self.write_exram((addr - 0x5C00) as usize, value),
            0x6000..=0xFFFF => self.write_prg(addr, value),
            _ => {}
        }
    }

    fn ppu_read(&mut self, addr: u16) -> Option<u8> {
        self.read_ppu(addr)
    }

    fn ppu_write(&mut self, addr: u16, value: u8) {
        let addr = addr & 0x3FFF;
        match addr {
            0x0000..=0x1FFF => self.common.write_chr(addr, value),
            0x2000..=0x3EFF => self.write_nametable_byte(addr, value),
            _ => {}
        }
    }

    fn cpu_cycle(&mut self) {
        let clock_rate = self.common.info.region.cpu_clock();
        self.audio.clock(clock_rate);

        if self.ppu_idle_counter != 0 {
            self.ppu_idle_counter -= 1;
            if self.ppu_idle_counter == 0 {
                // "The 'in-frame' flag is cleared when the PPU is no longer
                // rendering. This is detected when 3 CPU cycles pass without a
                // PPU read having occurred."
                self.ppu_in_frame = false;
                self.update_chr_banks(true);
            }
        }
    }

    fn irq_pending(&self) -> bool {
        (self.common.irq_pending && self.irq_enabled)
            || (self.audio.pcm_irq_trip && self.audio.pcm_irq_enabled)
    }

    fn expansion_audio_sample(&self) -> f32 {
        self.audio.sample()
    }

    fn battery(&self) -> Option<&[u8]> {
        if self.has_battery() && !self.save_ram.is_empty() {
            Some(&self.save_ram)
        } else {
            None
        }
    }

    fn set_battery(&mut self, data: &[u8]) {
        if self.has_battery() {
            self.save_ram.fill(0);
            let n = data.len().min(self.save_ram.len());
            self.save_ram[..n].copy_from_slice(&data[..n]);
        }
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);

        w.bytes(&self.work_ram);
        w.bytes(&self.save_ram);
        w.bool(self.exram_in_save);
        w.u32(self.exram_offset as u32);
        w.u32(self.work_real_size as u32);
        w.u32(self.save_real_size as u32);

        w.u8(self.prg_mode);
        for value in self.prg_banks {
            w.u8(value);
        }
        w.u8(self.prg_ram_protect1);
        w.u8(self.prg_ram_protect2);

        w.u8(self.chr_mode);
        w.u8(self.chr_upper_bits);
        for value in self.chr_banks {
            w.u16(value);
        }
        w.u16(self.last_chr_reg);
        w.bool(self.prev_chr_a);
        w.bool(self.large_sprites);
        w.bool(self.ppu_ctrl_hook_used);
        w.bool(self.chr_b_regs_written);
        w.bool(self.sprite_tables_seen[0]);
        w.bool(self.sprite_tables_seen[1]);

        w.u8(self.nametable_mapping);
        w.u8(self.extended_ram_mode);
        w.u8(self.fill_mode_tile);
        w.u8(self.fill_mode_color);

        w.u16(self.ex_attr_last_nametable_fetch);
        w.u8(self.ex_attr_last_fetch_counter as u8);
        w.u16(self.ex_attr_selected_chr_bank);

        w.bool(self.vertical_split_enabled);
        w.bool(self.vertical_split_right_side);
        w.u8(self.vertical_split_delimiter_tile);
        w.u8(self.vertical_split_scroll);
        w.u8(self.vertical_split_bank);
        w.bool(self.split_in_split_region);
        w.u32(self.split_vertical_scroll);
        w.u32(self.split_tile);
        w.i32(self.split_tile_number);

        w.u8(self.irq_counter_target);
        w.bool(self.irq_enabled);
        w.u8(self.scanline_counter);

        w.bool(self.need_in_frame);
        w.bool(self.ppu_in_frame);
        w.u8(self.ppu_idle_counter);
        w.u16(self.last_ppu_read_addr);
        w.u8(self.nt_read_counter);

        w.u8(self.multiplier_value1);
        w.u8(self.multiplier_value2);

        self.audio.save_state(w);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;

        let work_ram = r.bytes()?;
        if work_ram.len() != self.work_ram.len() {
            return Err("MMC5 work RAM size mismatch in save state".into());
        }
        self.work_ram.copy_from_slice(&work_ram);
        let save_ram = r.bytes()?;
        if save_ram.len() != self.save_ram.len() {
            return Err("MMC5 save RAM size mismatch in save state".into());
        }
        self.save_ram.copy_from_slice(&save_ram);
        let exram_in_save = r.bool()?;
        let exram_offset = r.u32()? as usize;
        let work_real = r.u32()? as usize;
        let save_real = r.u32()? as usize;
        if exram_in_save != self.exram_in_save
            || exram_offset != self.exram_offset
            || work_real != self.work_real_size
            || save_real != self.save_real_size
        {
            return Err("MMC5 RAM layout mismatch in save state".into());
        }

        self.prg_mode = r.u8()? & 0x03;
        for value in &mut self.prg_banks {
            *value = r.u8()?;
        }
        self.prg_ram_protect1 = r.u8()? & 0x03;
        self.prg_ram_protect2 = r.u8()? & 0x03;

        self.chr_mode = r.u8()? & 0x03;
        self.chr_upper_bits = r.u8()? & 0x03;
        for value in &mut self.chr_banks {
            *value = r.u16()?;
        }
        self.last_chr_reg = r.u16()?;
        self.prev_chr_a = r.bool()?;
        self.large_sprites = r.bool()?;
        self.ppu_ctrl_hook_used = r.bool()?;
        self.chr_b_regs_written = r.bool()?;
        self.sprite_tables_seen[0] = r.bool()?;
        self.sprite_tables_seen[1] = r.bool()?;

        self.nametable_mapping = r.u8()?;
        self.extended_ram_mode = r.u8()? & 0x03;
        self.fill_mode_tile = r.u8()?;
        self.fill_mode_color = r.u8()? & 0x03;

        self.ex_attr_last_nametable_fetch = r.u16()?;
        self.ex_attr_last_fetch_counter = r.u8()? as i8;
        self.ex_attr_selected_chr_bank = r.u16()?;

        self.vertical_split_enabled = r.bool()?;
        self.vertical_split_right_side = r.bool()?;
        self.vertical_split_delimiter_tile = r.u8()? & 0x1F;
        self.vertical_split_scroll = r.u8()?;
        self.vertical_split_bank = r.u8()?;
        self.split_in_split_region = r.bool()?;
        self.split_vertical_scroll = r.u32()?;
        self.split_tile = r.u32()?;
        self.split_tile_number = r.i32()?;

        self.irq_counter_target = r.u8()?;
        self.irq_enabled = r.bool()?;
        self.scanline_counter = r.u8()?;

        self.need_in_frame = r.bool()?;
        self.ppu_in_frame = r.bool()?;
        self.ppu_idle_counter = r.u8()?;
        self.last_ppu_read_addr = r.u16()?;
        self.nt_read_counter = r.u8()?;

        self.multiplier_value1 = r.u8()?;
        self.multiplier_value2 = r.u8()?;

        self.audio.load_state(r)?;

        // The generic mapper state does not preserve RAM mappings, so rebuild
        // them from the restored PRG registers.
        self.update_prg_banks();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

pub fn supports(id: u16) -> bool {
    matches!(id, 5 | 9 | 10)
}

pub fn create(id: u16, rom: RomData) -> Option<Box<dyn Mapper>> {
    match id {
        5 => Some(Box::new(Mmc5::new(rom))),
        9 | 10 => Some(Box::new(Mmc2::new(rom))),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{RomData, RomInfo};

    fn make_rom_with(
        mapper: u16,
        prg_8k_pages: usize,
        chr_1k_pages: usize,
        has_battery: bool,
        is_nes2: bool,
    ) -> RomData {
        let info = RomInfo {
            mapper,
            has_battery,
            is_nes2,
            prg_rom_size: prg_8k_pages * 0x2000,
            chr_rom_size: chr_1k_pages * 0x400,
            prg_ram_size: 0,
            prg_nvram_size: 0,
            ..Default::default()
        };
        let mut prg_rom = Vec::new();
        for page in 0..prg_8k_pages {
            prg_rom.extend(std::iter::repeat(page as u8).take(0x2000));
        }
        let mut chr_rom = Vec::new();
        for page in 0..chr_1k_pages {
            let mut bank = vec![page as u8; 0x400];
            bank[0] = page as u8;
            bank[1] = (page >> 8) as u8;
            chr_rom.extend(bank);
        }
        RomData::new(info, prg_rom, chr_rom)
    }

    fn make_rom(mapper: u16, prg_8k_pages: usize, chr_1k_pages: usize) -> RomData {
        make_rom_with(mapper, prg_8k_pages, chr_1k_pages, false, false)
    }

    fn mmc5(prg_8k_pages: usize, chr_1k_pages: usize) -> Mmc5 {
        Mmc5::new(make_rom(5, prg_8k_pages, chr_1k_pages))
    }

    fn read(mapper: &mut dyn Mapper, addr: u16) -> u8 {
        mapper.cpu_read(addr).expect("expected mapped read")
    }

    fn ppu_read(mapper: &mut dyn Mapper, addr: u16) -> u8 {
        mapper.ppu_read(addr).expect("expected mapped PPU read")
    }

    // -- MMC5 PRG banking ---------------------------------------------------

    #[test]
    fn mmc5_prg_modes() {
        let mut m = mmc5(32, 8);

        // Power-on defaults to PRG mode 3 with $5117 = $FF.
        assert_eq!(read(&mut m, 0xE000), 31);
        assert_eq!(read(&mut m, 0x8000), 0);

        // Bit 7 of $5114-$5116 selects ROM; 0 selects PRG RAM.
        m.cpu_write(0x5114, 0x81);
        m.cpu_write(0x5115, 0x82);
        m.cpu_write(0x5116, 0x83);
        m.cpu_write(0x5117, 0x84);
        assert_eq!(read(&mut m, 0x8000), 1);
        assert_eq!(read(&mut m, 0xA000), 2);
        assert_eq!(read(&mut m, 0xC000), 3);
        assert_eq!(read(&mut m, 0xE000), 4);

        // Mode 0: 32 KB, bottom two bits ignored.
        m.cpu_write(0x5100, 0);
        m.cpu_write(0x5117, 0x06);
        assert_eq!(read(&mut m, 0x8000), 4);
        assert_eq!(read(&mut m, 0xA000), 5);
        assert_eq!(read(&mut m, 0xC000), 6);
        assert_eq!(read(&mut m, 0xE000), 7);

        // Mode 1: 16 KB at $8000, 16 KB at $C000, bottom bit ignored.
        m.cpu_write(0x5100, 1);
        m.cpu_write(0x5115, 0x8B); // bit 0 is ignored
        m.cpu_write(0x5117, 0x8C);
        assert_eq!(read(&mut m, 0x8000), 10);
        assert_eq!(read(&mut m, 0xA000), 11);
        assert_eq!(read(&mut m, 0xC000), 12);
        assert_eq!(read(&mut m, 0xE000), 13);

        // Mode 2: 16 KB at $8000, 8 KB at $C000/$E000.
        m.cpu_write(0x5100, 2);
        m.cpu_write(0x5115, 0x8F); // bit 0 is ignored
        m.cpu_write(0x5116, 0x90);
        m.cpu_write(0x5117, 0x92);
        assert_eq!(read(&mut m, 0x8000), 14);
        assert_eq!(read(&mut m, 0xA000), 15);
        assert_eq!(read(&mut m, 0xC000), 16);
        assert_eq!(read(&mut m, 0xE000), 18);
    }

    #[test]
    fn mmc5_prg_ram_protection_and_banking() {
        let rom = make_rom_with(5, 8, 8, true, false);
        let mut m = Mmc5::new(rom);
        assert_eq!(m.battery().map(|b| b.len()), Some(0x10400));

        // 64 KB save RAM: bank 1 = offset $2000, bank 2 = offset $4000.
        m.cpu_write(0x5102, 2);
        m.cpu_write(0x5103, 1);
        m.cpu_write(0x5113, 1);
        m.cpu_write(0x6000, 0xAB);
        m.cpu_write(0x5113, 2);
        assert_eq!(read(&mut m, 0x6000), 0);
        m.cpu_write(0x5113, 1);
        assert_eq!(read(&mut m, 0x6000), 0xAB);

        // Disable write protection; writes are ignored but reads still work.
        m.cpu_write(0x5102, 0);
        m.cpu_write(0x5103, 0);
        m.cpu_write(0x6000, 0xCD);
        assert_eq!(read(&mut m, 0x6000), 0xAB);

        // Bank 4 is only accessible on the 64 KB superset via NES 2.0.
        let mut rom2 = make_rom_with(5, 8, 8, true, true);
        rom2.info.prg_nvram_size = 0x10000;
        let mut m2 = Mmc5::new(rom2);
        m2.cpu_write(0x5102, 2);
        m2.cpu_write(0x5103, 1);
        m2.cpu_write(0x5113, 4);
        m2.cpu_write(0x6000, 0x77);
        m2.cpu_write(0x5113, 5);
        assert_eq!(read(&mut m2, 0x6000), 0);
        m2.cpu_write(0x5113, 4);
        assert_eq!(read(&mut m2, 0x6000), 0x77);
    }

    // -- MMC5 CHR banking ---------------------------------------------------

    #[test]
    fn mmc5_chr_modes_and_upper_bits() {
        let mut m = mmc5(8, 1024);

        // Power-on CHR mode 0, bank 0.
        assert_eq!(ppu_read(&mut m, 0x0000), 0);

        // 1 KB mode.
        m.cpu_write(0x5101, 3);
        m.cpu_write(0x5120, 2);
        m.cpu_write(0x5127, 5);
        assert_eq!(ppu_read(&mut m, 0x0000), 2);
        assert_eq!(ppu_read(&mut m, 0x1C00), 5);

        // Upper CHR bank bits are applied on the next bank write.
        m.cpu_write(0x5130, 1);
        m.cpu_write(0x5120, 3);
        assert_eq!(ppu_read(&mut m, 0x0000), 3);
        assert_eq!(ppu_read(&mut m, 0x0001), 1); // bank $0103 => 1 KB page 259

        // 8 KB mode uses $5127, scaled by eight.
        m.cpu_write(0x5101, 0);
        m.cpu_write(0x5127, 1);
        assert_eq!(ppu_read(&mut m, 0x0000), 8);
        assert_eq!(ppu_read(&mut m, 0x1C00), 15);

        // 2 KB mode uses $5121/$5123/$5125/$5127.
        m.cpu_write(0x5101, 2);
        m.cpu_write(0x5121, 2);
        m.cpu_write(0x5123, 4);
        m.cpu_write(0x5125, 6);
        m.cpu_write(0x5127, 8);
        assert_eq!(ppu_read(&mut m, 0x0000), 4);
        assert_eq!(ppu_read(&mut m, 0x0800), 8);
        assert_eq!(ppu_read(&mut m, 0x1000), 12);
        assert_eq!(ppu_read(&mut m, 0x1800), 16);

        // 4 KB mode uses $5123 and $5127.
        m.cpu_write(0x5101, 1);
        m.cpu_write(0x5123, 3);
        m.cpu_write(0x5127, 5);
        assert_eq!(ppu_read(&mut m, 0x0000), 12);
        assert_eq!(ppu_read(&mut m, 0x1000), 20);
    }

    // -- ExRAM, nametable mapping and fill mode -----------------------------

    #[test]
    fn mmc5_exram_nametable_modes() {
        let mut m = mmc5(8, 8);

        // Mode 0: ExRAM is a single-screen nametable for all four slots, and
        // the PPU may read/write it.
        m.cpu_write(0x5104, 0);
        m.cpu_write(0x5105, 0xAA);
        m.ppu_write(0x2000, 0x55);
        assert_eq!(ppu_read(&mut m, 0x2000), 0x55);
        assert_eq!(ppu_read(&mut m, 0x2400), 0x55);
        assert_eq!(ppu_read(&mut m, 0x2800), 0x55);
        assert_eq!(ppu_read(&mut m, 0x2C00), 0x55);

        // CPU reads return open bus in modes 0/1. CPU writes during blanking
        // store zero instead of the written value.
        assert!(m.cpu_read(0x5C00).is_none());
        m.cpu_write(0x5C00, 0x99);
        assert_eq!(ppu_read(&mut m, 0x2000), 0x00);

        // Mode 2: ExRAM is readable/writable from the CPU but is no longer
        // usable as a nametable (reads as the empty page).
        m.cpu_write(0x5104, 2);
        m.cpu_write(0x5C00, 0x77);
        assert_eq!(read(&mut m, 0x5C00), 0x77);
        assert_eq!(ppu_read(&mut m, 0x2000), 0x00);
        m.ppu_write(0x2000, 0x33);
        assert_eq!(read(&mut m, 0x5C00), 0x77);

        // Mode 3 is read-only from the CPU.
        m.cpu_write(0x5104, 3);
        m.cpu_write(0x5C00, 0xEE);
        assert_eq!(read(&mut m, 0x5C00), 0x77);

        // Mode 1 maps ExRAM as a nametable again.
        m.cpu_write(0x5104, 1);
        assert_eq!(ppu_read(&mut m, 0x2000), 0x77);
        m.ppu_write(0x2400, 0x88);
        assert_eq!(ppu_read(&mut m, 0x2C00), 0x88);

        // Fill mode: tile and replicated palette.
        m.cpu_write(0x5104, 2);
        m.cpu_write(0x5105, 0xFF);
        m.cpu_write(0x5106, 0x42);
        m.cpu_write(0x5107, 0x02);
        assert_eq!(ppu_read(&mut m, 0x2000), 0x42);
        assert_eq!(ppu_read(&mut m, 0x2800), 0x42);
        assert_eq!(ppu_read(&mut m, 0x23C0), 0xAA);
        assert_eq!(ppu_read(&mut m, 0x2FFF), 0xAA);

        // Horizontal CIRAM mapping ($50) maps NT2/NT3 to CIRAM page 1.
        m.cpu_write(0x5105, 0x50);
        m.cpu_write(0x5104, 2);
        m.ppu_write(0x2400, 0x33);
        assert_eq!(ppu_read(&mut m, 0x2000), 0x33);
        m.ppu_write(0x2800, 0x44);
        assert_eq!(ppu_read(&mut m, 0x2C00), 0x44);
        assert_eq!(ppu_read(&mut m, 0x2000), 0x33);
    }

    #[test]
    fn mmc5_extended_attributes() {
        let mut m = mmc5(8, 1024);

        // Seed ExRAM byte 0 = palette 2, 4 KB CHR bank 5. Use mode 2 for the
        // CPU write, then switch to extended-attribute mode 1.
        m.cpu_write(0x5104, 2);
        m.cpu_write(0x5C00, 0x85);
        m.cpu_write(0x5104, 1);

        // Enter the PPU frame (three matching nametable reads, then the start
        // of the next line).
        for _ in 0..4 {
            let _ = m.ppu_read(0x2000);
        }

        // Nametable fetch latches the ExRAM index.
        let _ = m.ppu_read(0x2000);
        // Attribute fetch returns the palette replicated four times.
        assert_eq!(ppu_read(&mut m, 0x23C0), 0xAA);
        // Tile low/high fetches use 4 KB bank 5 (1 KB page 20).
        assert_eq!(ppu_read(&mut m, 0x0000), 20);
        assert_eq!(ppu_read(&mut m, 0x0008), 20);
    }

    #[test]
    fn mmc5_vertical_split() {
        let mut m = mmc5(8, 1024);

        // Seed ExRAM (mode 2 for CPU writes), then use it as the split
        // nametable (mode 0), with a normal CIRAM nametable elsewhere.
        m.cpu_write(0x5104, 2);
        m.cpu_write(0x5C03, 0xAB); // tile 3 / row 0
        m.cpu_write(0x5C04, 0xCD);
        m.cpu_write(0x5104, 0);
        m.ppu_write(0x2000, 0x11); // normal CIRAM page 0, offset 0

        // Left split at tile 10, vertical scroll 0, 4 KB CHR bank 2.
        m.cpu_write(0x5200, 0x80 | 10);
        m.cpu_write(0x5201, 0x00);
        m.cpu_write(0x5202, 0x02);

        enter_frame(&mut m); // scanline counter 1

        // Tile numbers (split_tile_number + 2) % 42 => 3, 4, ... while the
        // split is active. Alternate addresses so Mesen's scanline detector
        // does not reset the fetch counter.
        assert_eq!(ppu_read(&mut m, 0x2400), 0xAB);
        assert_eq!(ppu_read(&mut m, 0x2800), 0xCD);
        // Attribute/tile fetches in the split region are ExRAM too.
        let _ = m.ppu_read(0x2C00);
        let _ = m.ppu_read(0x2000);
        let _ = m.ppu_read(0x2400);
        let _ = m.ppu_read(0x2800);

        // CHR fetches for the split region bypass CHR banking: 4 KB bank 2
        // with the split row bits substituted (1 KB page 8 in our test ROM).
        assert_eq!(ppu_read(&mut m, 0x0010), 8);

        // After the delimiter (tile 10) the normal nametable is used again.
        let _ = m.ppu_read(0x2C00); // split_tile_number 7 => tile 9
        assert_eq!(ppu_read(&mut m, 0x2000), 0x11);
    }

    #[test]
    fn mmc5_large_sprite_chr_banks_via_hook() {
        let mut m = mmc5(8, 32);
        m.cpu_write(0x5101, 3); // 1 KB CHR
        m.set_ppu_ctrl(0x20); // 8x16 sprites
        m.cpu_write(0x5120, 1); // sprite banks ($5120-$5127)
        m.cpu_write(0x5128, 2); // background banks ($5128-$512B)

        // Outside a frame, the last-written bank register selects the $2007
        // mapping; $5128 was written last, so background bank 2 is used.
        assert_eq!(ppu_read(&mut m, 0x0000), 2);

        // During background fetches in a frame the B banks are used.
        enter_frame(&mut m);
        assert_eq!(ppu_read(&mut m, 0x0000), 2);

        // Sprite-phase fetches (fetch counter >= 32) use the A banks.
        m.split_tile_number = 32;
        m.ppu_in_frame = true;
        m.update_chr_banks(false);
        assert_eq!(ppu_read(&mut m, 0x0000), 1);

        // Going back to 8x8 sprites resets to the A banks everywhere.
        m.set_ppu_ctrl(0x00);
        m.split_tile_number = 0;
        assert_eq!(ppu_read(&mut m, 0x0000), 1);
    }

    #[test]
    fn mmc5_large_sprite_fallback_detection() {
        // 8x16 tiles of both parities in one sprite phase touch both 4 KB
        // pattern-table halves; with the B registers written this is enough to
        // switch to A/B bank selection without a PPU $2000 hook.
        let mut m = mmc5(8, 32);
        m.cpu_write(0x5128, 0); // B registers written
        enter_frame(&mut m);
        m.split_tile_number = 32;
        let _ = m.ppu_read(0x0010); // pattern table 0
        assert!(!m.large_sprites);
        let _ = m.ppu_read(0x1010); // pattern table 1
        assert!(m.large_sprites);

        // Without B register writes the fallback stays conservative.
        let mut m2 = mmc5(8, 32);
        enter_frame(&mut m2);
        m2.split_tile_number = 32;
        let _ = m2.ppu_read(0x0010);
        let _ = m2.ppu_read(0x1010);
        assert!(!m2.large_sprites);
    }

    // -- Multiplier ---------------------------------------------------------

    #[test]
    fn mmc5_multiplier() {
        let mut m = mmc5(8, 8);
        m.cpu_write(0x5205, 23);
        m.cpu_write(0x5206, 45);
        let product = 23u16 * 45;
        assert_eq!(read(&mut m, 0x5205), product as u8);
        assert_eq!(read(&mut m, 0x5206), (product >> 8) as u8);

        m.cpu_write(0x5205, 0xFF);
        m.cpu_write(0x5206, 0xFF);
        assert_eq!(read(&mut m, 0x5205), 0x01);
        assert_eq!(read(&mut m, 0x5206), 0xFE);
    }

    // -- Scanline IRQ -------------------------------------------------------

    fn enter_frame(m: &mut Mmc5) {
        for _ in 0..4 {
            let _ = m.ppu_read(0x2000);
        }
    }

    #[test]
    fn mmc5_scanline_irq() {
        let mut m = mmc5(8, 8);
        m.cpu_write(0x5203, 2);
        m.cpu_write(0x5204, 0x80);

        enter_frame(&mut m); // scanline counter = 1, in frame
        assert!(!m.irq_pending());
        let _ = m.ppu_read(0x2000); // counter 2
        assert!(m.irq_pending());

        let status = read(&mut m, 0x5204);
        assert_eq!(status & 0xC0, 0xC0); // in-frame + pending
        assert!(!m.irq_pending());

        // Pending is latched even when IRQs are disabled.
        m.cpu_write(0x5204, 0x00);
        m.cpu_write(0x5203, 3);
        let _ = m.ppu_read(0x2000); // counter 3
        assert!(!m.irq_pending());
        assert_eq!(read(&mut m, 0x5204) & 0x80, 0x80);

        // A later scanline latches again; re-enabling asserts the line.
        m.cpu_write(0x5203, 4);
        let _ = m.ppu_read(0x2000); // counter 4 -> pending
        assert!(!m.irq_pending());
        m.cpu_write(0x5204, 0x80);
        assert!(m.irq_pending());

        // Reading the reset vector clears pending and the in-frame state.
        assert!(m.cpu_read(0xFFFA).is_some());
        assert!(!m.irq_pending());
        let status = read(&mut m, 0x5204);
        assert_eq!(status & 0xC0, 0x00);
    }

    // -- Expansion audio ----------------------------------------------------

    #[test]
    fn mmc5_audio_status_and_pcm_irq() {
        let mut m = mmc5(8, 8);

        // Enable pulse 1 and give it a length counter value.
        m.cpu_write(0x5015, 0x01);
        m.cpu_write(0x5003, 0x08); // length index 1 => 254
        m.cpu_cycle(); // length counter reload happens on the next audio clock
        assert_eq!(read(&mut m, 0x5015) & 0x03, 0x01);
        m.cpu_write(0x5015, 0x00);
        assert_eq!(read(&mut m, 0x5015) & 0x03, 0x00);

        // PCM write mode: writing 0 trips the IRQ without changing the DAC.
        m.cpu_write(0x5010, 0x80);
        m.cpu_write(0x5011, 0x00);
        assert!(m.irq_pending());
        let status = read(&mut m, 0x5010);
        assert_eq!(status & 0x80, 0x80);
        assert_eq!(status & 0x01, 0x00);
        assert!(!m.irq_pending());

        // Reading $8000-$BFFF in PCM read mode acts as a DAC write.
        m.cpu_write(0x5010, 0x81);
        let _ = m.cpu_read(0x8000);
        m.cpu_write(0x5010, 0x80);
        m.cpu_write(0x5011, 0x20);
        assert!(!m.irq_pending());
    }

    // -- Save state ---------------------------------------------------------

    #[test]
    fn mmc5_save_state_round_trip() {
        let rom = make_rom_with(5, 16, 16, true, false);
        let mut a = Mmc5::new(rom.clone());
        a.cpu_write(0x5104, 2);
        a.cpu_write(0x5C00, 0x5A);
        a.cpu_write(0x5102, 2);
        a.cpu_write(0x5103, 1);
        a.cpu_write(0x5113, 1);
        a.cpu_write(0x6000, 0x99);
        a.cpu_write(0x5101, 3);
        a.cpu_write(0x5120, 7);
        a.cpu_write(0x5205, 11);
        a.cpu_write(0x5206, 13);
        a.cpu_write(0x5203, 9);
        a.cpu_write(0x5204, 0x80);

        let mut writer = StateWriter::new();
        a.save_state(&mut writer);
        let state = writer.into_inner();

        let mut b = Mmc5::new(rom);
        let mut reader = StateReader::new(&state);
        b.load_state(&mut reader).unwrap();

        assert_eq!(read(&mut b, 0x5C00), 0x5A);
        assert_eq!(ppu_read(&mut b, 0x0000), 7);
        assert_eq!(read(&mut b, 0x5205), (11 * 13) as u8);
        assert_eq!(read(&mut b, 0x5206), ((11 * 13) >> 8) as u8);
        b.cpu_write(0x5113, 1);
        assert_eq!(read(&mut b, 0x6000), 0x99);
    }

    // -- MMC2 / MMC4 --------------------------------------------------------

    #[test]
    fn mmc2_prg_and_chr_latches() {
        let mut m = Mmc2::new(make_rom(9, 16, 32));

        // $8000 is open bus until $A000 is written.
        assert!(m.cpu_read(0x8000).is_none());
        m.cpu_write(0xA000, 3);
        assert_eq!(read(&mut m, 0x8000), 3);
        // Last three 8 KB banks are fixed at $A000/$C000/$E000.
        assert_eq!(read(&mut m, 0xA000), 13);
        assert_eq!(read(&mut m, 0xC000), 14);
        assert_eq!(read(&mut m, 0xE000), 15);

        m.cpu_write(0xB000, 1);
        m.cpu_write(0xC000, 2);
        // Left latch starts at 1 => 4 KB bank 2 => 1 KB page 8.
        assert_eq!(ppu_read(&mut m, 0x0000), 8);
        m.ppu_read(0x0FD8); // latch 0, deferred update
        assert_eq!(ppu_read(&mut m, 0x0000), 4);
        m.ppu_read(0x0FE8); // latch 1
        assert_eq!(ppu_read(&mut m, 0x0000), 8);

        m.cpu_write(0xD000, 5);
        m.cpu_write(0xE000, 6);
        assert_eq!(ppu_read(&mut m, 0x1000), 24);
        m.ppu_read(0x1FD8); // right latch 0
        assert_eq!(ppu_read(&mut m, 0x1000), 20);
        m.ppu_read(0x1FE8); // right latch 1
        assert_eq!(ppu_read(&mut m, 0x1000), 24);

        m.cpu_write(0xF000, 0x01);
        assert_eq!(m.common().mirroring, Mirroring::Horizontal);
        m.cpu_write(0xF000, 0x00);
        assert_eq!(m.common().mirroring, Mirroring::Vertical);

        // $3000-$3EFF mirrors $2000-$2EFF.
        m.ppu_write(0x2000, 0x5A);
        assert_eq!(ppu_read(&mut m, 0x3000), 0x5A);
    }

    #[test]
    fn mmc4_prg_and_latch_ranges() {
        let mut m = Mmc2::new(make_rom(10, 16, 32));

        assert!(m.cpu_read(0x8000).is_none());
        assert!(m.cpu_read(0xA000).is_none());
        m.cpu_write(0xA000, 3); // 16 KB bank 3 => 8 KB pages 6/7
        assert_eq!(read(&mut m, 0x8000), 6);
        assert_eq!(read(&mut m, 0xA000), 7);
        // Fixed last 16 KB.
        assert_eq!(read(&mut m, 0xC000), 14);
        assert_eq!(read(&mut m, 0xE000), 15);

        m.cpu_write(0xB000, 1);
        m.cpu_write(0xC000, 2);
        assert_eq!(ppu_read(&mut m, 0x0000), 8);
        // MMC4 left latch decodes the full $0FD8-$0FDF range.
        m.ppu_read(0x0FD0);
        assert_eq!(ppu_read(&mut m, 0x0000), 8);
        m.ppu_read(0x0FD8);
        assert_eq!(ppu_read(&mut m, 0x0000), 4);
        m.ppu_read(0x0FE8);
        assert_eq!(ppu_read(&mut m, 0x0000), 8);

        // Right side range.
        m.cpu_write(0xD000, 5);
        m.cpu_write(0xE000, 6);
        assert_eq!(ppu_read(&mut m, 0x1000), 24);
        m.ppu_read(0x1FD8);
        assert_eq!(ppu_read(&mut m, 0x1000), 20);
        m.ppu_read(0x1FE8);
        assert_eq!(ppu_read(&mut m, 0x1000), 24);
    }

    #[test]
    fn mmc2_save_state_round_trip() {
        let rom = make_rom(9, 16, 32);
        let mut a = Mmc2::new(rom.clone());
        a.cpu_write(0xA000, 5);
        a.cpu_write(0xB000, 3);
        a.cpu_write(0xC000, 4);
        a.ppu_read(0x0FD8);

        let mut writer = StateWriter::new();
        a.save_state(&mut writer);
        let state = writer.into_inner();

        let mut b = Mmc2::new(rom);
        let mut reader = StateReader::new(&state);
        b.load_state(&mut reader).unwrap();

        assert_eq!(read(&mut b, 0x8000), 5);
        let _ = b.ppu_read(0x0000); // applies deferred latch update
        assert_eq!(ppu_read(&mut b, 0x0000), 12); // 4 KB bank 3
    }
}
