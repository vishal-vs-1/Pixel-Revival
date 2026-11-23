//! Special cartridge formats: FDS, NSF, VS System.
//!
//! This is a faithful translation of the relevant parts of Mesen 0.9.9:
//! `Core/FDS.cpp/.h`, `Core/FdsAudio.h`, `Core/BaseFdsChannel.h`,
//! `Core/ModChannel.h`, `Core/FdsLoader.cpp`, `Core/NsfMapper.cpp/.h`,
//! `Core/NsfLoader.cpp`, `Core/NsfCart31.h`, `Core/VsSystem.h` and
//! `Core/VsControlManager.cpp`.
//!
//! Original C++: Copyright (C) 2014-2020 Sour, GPL-3.0.
//! Rust port: GPL-3.0-or-later.
//!
//! OWNER: mapper-special agent.
//!
//! Supported IDs: 20 (FDS), 31 (NSF player when the ROM system is NSF,
//! otherwise the iNES "NSF cart" bank mapper), 99 (VS System), plus the
//! upstream private aliases 65534 (NSF mapper) and 65535 (FDS mapper).

use super::{Mapper, MapperCommon};
use crate::state::{StateReader, StateResult, StateWriter};
use crate::types::{Mirroring, Region, RomData, RomSystem};
use std::any::Any;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

#[inline]
fn page_count(len: usize, page_size: usize) -> usize {
    (len / page_size).max(1)
}

#[inline]
fn page_offset(page: usize, page_size: usize, len: usize) -> usize {
    (page % page_count(len, page_size)) * page_size
}

fn load_u8_array<const N: usize>(r: &mut StateReader<'_>) -> StateResult<[u8; N]> {
    let data = r.fixed_bytes(N)?;
    let mut array = [0u8; N];
    array.copy_from_slice(&data);
    Ok(array)
}

// ===========================================================================
// FDS audio (FdsAudio.h, BaseFdsChannel.h, ModChannel.h)
// ===========================================================================

/// One of the two FDS envelope generators (`BaseFdsChannel`).
#[derive(Clone)]
struct BaseFdsChannel {
    speed: u8,
    gain: u8,
    envelope_off: bool,
    volume_increase: bool,
    frequency: u16,
    timer: u32,
    /// "Few FDS NSFs write to this register. The BIOS initializes this to $FF."
    master_speed: u8,
}

impl BaseFdsChannel {
    fn new() -> Self {
        Self {
            speed: 0,
            gain: 0,
            envelope_off: false,
            volume_increase: false,
            frequency: 0,
            timer: 0,
            master_speed: 0xFF,
        }
    }

    fn set_master_envelope_speed(&mut self, master_speed: u8) {
        self.master_speed = master_speed;
    }

    fn write_reg(&mut self, addr: u16, value: u8) {
        match addr & 0x03 {
            0 => {
                self.speed = value & 0x3F;
                self.volume_increase = (value & 0x40) == 0x40;
                self.envelope_off = (value & 0x80) == 0x80;

                // "Writing to this register immediately resets the clock timer
                // that ticks the volume envelope (delaying the next tick
                // slightly)."
                self.reset_timer();

                if self.envelope_off {
                    // Envelope is off, gain = speed.
                    self.gain = self.speed;
                }
            }
            2 => {
                self.frequency = (self.frequency & 0x0F00) | value as u16;
            }
            3 => {
                self.frequency = (self.frequency & 0x00FF) | (((value & 0x0F) as u16) << 8);
            }
            _ => {}
        }
    }

    fn tick_envelope(&mut self) -> bool {
        if !self.envelope_off && self.master_speed > 0 {
            self.timer = self.timer.wrapping_sub(1);
            if self.timer == 0 {
                self.reset_timer();

                if self.volume_increase && self.gain < 32 {
                    self.gain += 1;
                } else if !self.volume_increase && self.gain > 0 {
                    self.gain -= 1;
                }
                return true;
            }
        }
        false
    }

    fn gain(&self) -> u8 {
        self.gain
    }

    fn frequency(&self) -> u16 {
        self.frequency
    }

    fn reset_timer(&mut self) {
        self.timer = 8 * (self.speed as u32 + 1) * self.master_speed as u32;
    }

    fn save_state(&self, w: &mut StateWriter) {
        w.u8(self.speed);
        w.u8(self.gain);
        w.bool(self.envelope_off);
        w.bool(self.volume_increase);
        w.u16(self.frequency);
        w.u32(self.timer);
        w.u8(self.master_speed);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.speed = r.u8()?;
        self.gain = r.u8()?;
        self.envelope_off = r.bool()?;
        self.volume_increase = r.bool()?;
        self.frequency = r.u16()?;
        self.timer = r.u32()?;
        self.master_speed = r.u8()?;
        Ok(())
    }
}

/// The FDS modulator (`ModChannel`).
#[derive(Clone)]
struct ModChannel {
    base: BaseFdsChannel,
    counter: i8,
    modulation_disabled: bool,
    mod_table: [u8; 64],
    mod_table_position: u8,
    overflow_counter: u16,
    output: i32,
}

const MOD_RESET: i32 = 0xFF;
const MOD_LUT: [i32; 8] = [0, 1, 2, 4, MOD_RESET, -4, -2, -1];

impl ModChannel {
    fn new() -> Self {
        Self {
            base: BaseFdsChannel::new(),
            counter: 0,
            modulation_disabled: false,
            mod_table: [0; 64],
            mod_table_position: 0,
            overflow_counter: 0,
            output: 0,
        }
    }

    fn write_reg(&mut self, addr: u16, value: u8) {
        match addr {
            0x4084 | 0x4086 => self.base.write_reg(addr, value),
            0x4085 => self.update_counter((value & 0x7F) as i32),
            0x4087 => {
                self.base.write_reg(addr, value);
                self.modulation_disabled = (value & 0x80) == 0x80;
                if self.modulation_disabled {
                    self.overflow_counter = 0;
                }
            }
            _ => {}
        }
    }

    fn write_mod_table(&mut self, value: u8) {
        // "This register has no effect unless the mod unit is disabled via the
        // high bit of $4087."
        if self.modulation_disabled {
            self.mod_table[(self.mod_table_position & 0x3F) as usize] = value & 0x07;
            self.mod_table[((self.mod_table_position + 1) & 0x3F) as usize] = value & 0x07;
            self.mod_table_position = (self.mod_table_position + 2) & 0x3F;
        }
    }

    fn update_counter(&mut self, value: i32) {
        // C++ takes an int8_t parameter, so the value wraps before the
        // [-64, 63] normalization below.
        let mut counter = value as i16;
        if counter >= 64 {
            counter -= 128;
        } else if counter < -64 {
            counter += 128;
        }
        self.counter = counter as i8;
    }

    fn is_enabled(&self) -> bool {
        !self.modulation_disabled && self.base.frequency > 0
    }

    fn tick_modulator(&mut self) -> bool {
        if self.is_enabled() {
            let frequency = self.base.frequency;
            self.overflow_counter = self.overflow_counter.wrapping_add(frequency);

            if self.overflow_counter < frequency {
                // Overflowed, tick the modulator.
                let offset = MOD_LUT[(self.mod_table[self.mod_table_position as usize] & 0x07) as usize];
                let next = if offset == MOD_RESET {
                    0
                } else {
                    self.counter as i32 + offset
                };
                self.update_counter(next);

                self.mod_table_position = (self.mod_table_position + 1) & 0x3F;

                return true;
            }
        }
        false
    }

    fn update_output(&mut self, volume_pitch: u16) {
        // Code from NesDev Wiki.
        //  pitch   = $4082/4083 (12-bit unsigned pitch value)
        //  counter = $4085 (7-bit signed mod counter)
        //  gain    = $4084 (6-bit unsigned mod gain)
        //
        // 1. multiply counter by gain, lose lowest 4 bits of result but "round"
        //    in a strange way.
        let counter = self.counter as i32;
        let mut temp = counter * self.base.gain as i32;
        let mut remainder = temp & 0xF;
        temp >>= 4;
        if remainder > 0 && (temp & 0x80) == 0 {
            temp += if counter < 0 { -1 } else { 2 };
        }

        // 2. wrap if a certain range is exceeded.
        if temp >= 192 {
            temp -= 256;
        } else if temp < -64 {
            temp += 256;
        }

        // 3. multiply result by pitch, then round to nearest while dropping
        //    6 bits.
        temp = volume_pitch as i32 * temp;
        remainder = temp & 0x3F;
        temp >>= 6;
        if remainder >= 32 {
            temp += 1;
        }

        self.output = temp;
    }

    fn get_output(&self) -> i32 {
        if self.is_enabled() {
            self.output
        } else {
            0
        }
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.base.save_state(w);
        w.u8(self.counter as u8);
        w.bool(self.modulation_disabled);
        w.u8(self.mod_table_position);
        w.u16(self.overflow_counter);
        w.fixed_bytes(&self.mod_table);
        w.i32(self.output);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.base.load_state(r)?;
        self.counter = r.u8()? as i8;
        self.modulation_disabled = r.bool()?;
        self.mod_table_position = r.u8()?;
        self.overflow_counter = r.u16()?;
        self.mod_table = load_u8_array::<64>(r)?;
        self.output = r.i32()?;
        Ok(())
    }
}

/// FDS expansion audio (`FdsAudio.h`). Header-only in Mesen.
struct FdsAudio {
    wave_table: [u8; 64],
    wave_write_enabled: bool,

    volume: BaseFdsChannel,
    modulator: ModChannel,

    disable_envelopes: bool,
    halt_waveform: bool,

    master_volume: u8,

    wave_overflow_counter: u16,
    wave_position: u8,

    last_output: u8,
}

const WAVE_VOLUME_TABLE: [u32; 4] = [36, 24, 17, 14];

impl FdsAudio {
    fn new() -> Self {
        Self {
            wave_table: [0; 64],
            wave_write_enabled: false,
            volume: BaseFdsChannel::new(),
            modulator: ModChannel::new(),
            disable_envelopes: false,
            halt_waveform: false,
            master_volume: 0,
            wave_overflow_counter: 0,
            wave_position: 0,
            last_output: 0,
        }
    }

    fn read_register(&self, addr: u16) -> u8 {
        if addr <= 0x407F {
            // Open-bus bits (0xC0) are not available to the mapper; the bus
            // supplies open bus for the whole read when we return None.
            self.wave_table[(addr & 0x3F) as usize]
        } else if addr == 0x4090 {
            self.volume.gain()
        } else if addr == 0x4092 {
            self.modulator.base.gain()
        } else {
            0
        }
    }

    fn write_register(&mut self, addr: u16, value: u8) {
        if addr < 0x4040 {
            return;
        }
        if addr <= 0x407F {
            if self.wave_write_enabled {
                self.wave_table[(addr & 0x3F) as usize] = value & 0x3F;
            }
        } else {
            match addr {
                0x4080 | 0x4082 => self.volume.write_reg(addr, value),
                0x4083 => {
                    self.disable_envelopes = (value & 0x40) == 0x40;
                    self.halt_waveform = (value & 0x80) == 0x80;
                    if self.disable_envelopes {
                        self.volume.reset_timer();
                        self.modulator.base.reset_timer();
                    }
                    self.volume.write_reg(addr, value);
                }
                0x4084 | 0x4085 | 0x4086 | 0x4087 => self.modulator.write_reg(addr, value),
                0x4088 => self.modulator.write_mod_table(value),
                0x4089 => {
                    self.master_volume = value & 0x03;
                    self.wave_write_enabled = (value & 0x80) == 0x80;
                }
                0x408A => {
                    self.volume.set_master_envelope_speed(value);
                    self.modulator.base.set_master_envelope_speed(value);
                }
                _ => {}
            }
        }
    }

    fn clock(&mut self) {
        let frequency = self.volume.frequency();
        if !self.halt_waveform && !self.disable_envelopes {
            self.volume.tick_envelope();
            if self.modulator.base.tick_envelope() {
                self.modulator.update_output(frequency);
            }
        }

        if self.modulator.tick_modulator() {
            // Modulator was ticked, update wave pitch.
            self.modulator.update_output(frequency);
        }

        if self.halt_waveform {
            self.wave_position = 0;
            self.update_output();
        } else {
            self.update_output();

            let step = frequency as i32 + self.modulator.get_output();
            if step > 0 && !self.wave_write_enabled {
                self.wave_overflow_counter =
                    (self.wave_overflow_counter as i32).wrapping_add(step) as u16;
                if (self.wave_overflow_counter as i32) < step {
                    self.wave_position = (self.wave_position + 1) & 0x3F;
                }
            }
        }
    }

    fn update_output(&mut self) {
        let level = (self.volume.gain().min(32) as u32) * WAVE_VOLUME_TABLE[(self.master_volume & 3) as usize];
        let output_level = ((self.wave_table[self.wave_position as usize] as u32 * level) / 1152) as u8;

        // Mesen feeds APU deltas; the Rust APU contract asks for an
        // instantaneous level, so keep the last value (normalized to 0..1).
        self.last_output = output_level;
    }

    fn sample(&self) -> f32 {
        self.last_output as f32 / 63.0
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.volume.save_state(w);
        self.modulator.save_state(w);
        w.bool(self.wave_write_enabled);
        w.bool(self.disable_envelopes);
        w.bool(self.halt_waveform);
        w.u8(self.master_volume);
        w.u16(self.wave_overflow_counter);
        w.u8(self.wave_position);
        w.u8(self.last_output);
        w.fixed_bytes(&self.wave_table);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.volume.load_state(r)?;
        self.modulator.load_state(r)?;
        self.wave_write_enabled = r.bool()?;
        self.disable_envelopes = r.bool()?;
        self.halt_waveform = r.bool()?;
        self.master_volume = r.u8()?;
        self.wave_overflow_counter = r.u16()?;
        self.wave_position = r.u8()?;
        self.last_output = r.u8()?;
        self.wave_table = load_u8_array::<64>(r)?;
        Ok(())
    }
}

// ===========================================================================
// FDS disk system (FDS.cpp/.h, FdsLoader.cpp)
// ===========================================================================

const FDS_SIDE_CAPACITY: usize = 65500;
const FDS_NO_DISK: u32 = 0xFF;

/// Reproduce `FdsLoader::AddGaps`.
fn add_fds_gaps(disk_side: &mut Vec<u8>, read_buffer: &[u8]) {
    // Start image with 28300 bits of gap.
    disk_side.extend(std::iter::repeat(0).take(28300 / 8));

    let mut j = 0usize;
    while j < FDS_SIDE_CAPACITY {
        let block_type = read_buffer.get(j).copied().unwrap_or(0);
        let block_length = match block_type {
            1 => 56usize, // Disk header
            2 => 2,       // File count
            3 => 16,      // File header
            4 => {
                // File data: length is stored two bytes before the block
                // marker in the raw FDS image.
                let low = read_buffer.get(j.wrapping_sub(3)).copied().unwrap_or(0) as usize;
                let high = read_buffer.get(j.wrapping_sub(2)).copied().unwrap_or(0) as usize;
                1 + low + high * 0x100
            }
            // End parsing when we encounter an invalid block type (this is
            // what Nestopia appears to do).
            _ => return,
        };

        if block_type == 0 {
            disk_side.push(block_type);
        } else {
            disk_side.push(0x80);
            let end = j.saturating_add(block_length).min(read_buffer.len());
            disk_side.extend_from_slice(&read_buffer[j..end]);
            if end < j + block_length {
                // Truncated image: pad missing bytes rather than reading out
                // of bounds (Mesen would read unallocated memory here).
                disk_side.extend(std::iter::repeat(0).take(j + block_length - end));
            }

            // Fake CRC value.
            disk_side.push(0x4D);
            disk_side.push(0x62);

            // Insert 976 bits of gap after a block.
            disk_side.extend(std::iter::repeat(0).take(976 / 8));
        }

        j += block_length;
    }
}

/// Reproduce `FdsLoader::LoadDiskData` + `AddGaps`. The loader hands us the
/// concatenated side data (without a fwNES header per the port contract), but
/// we accept a fwNES header too.
fn load_fds_disk_sides(data: &[u8]) -> Vec<Vec<u8>> {
    let (number_of_sides, mut file_offset) = if data.len() >= 16 && &data[0..4] == b"FDS\x1a" {
        (data[4] as usize, 16usize)
    } else {
        (data.len() / FDS_SIDE_CAPACITY, 0usize)
    };

    let mut sides = Vec::with_capacity(number_of_sides);
    for _ in 0..number_of_sides {
        if file_offset >= data.len() {
            break;
        }
        let mut side = Vec::with_capacity(FDS_SIDE_CAPACITY);
        add_fds_gaps(&mut side, &data[file_offset..]);
        if side.len() < FDS_SIDE_CAPACITY {
            side.resize(FDS_SIDE_CAPACITY, 0);
        }
        sides.push(side);
        file_offset += FDS_SIDE_CAPACITY;
    }
    sides
}

/// Famicom Disk System mapper (mapper 20).
pub struct Fds {
    common: MapperCommon,
    bios: Option<Vec<u8>>,
    audio: FdsAudio,

    // Write registers.
    irq_reload_value: u16,
    irq_counter: u16,
    irq_enabled: bool,
    irq_repeat_enabled: bool,
    disk_reg_enabled: bool,
    sound_reg_enabled: bool,
    write_data_reg: u8,
    motor_on: bool,
    reset_transfer: bool,
    read_mode: bool,
    crc_control: bool,
    disk_ready: bool,
    disk_irq_enabled: bool,
    ext_con_write_reg: u8,

    // Read registers.
    bad_crc: bool,
    end_of_head: bool,
    read_data_reg: u8,

    // Internal values.
    disk_number: u32,
    disk_position: u32,
    delay: u32,
    crc_accumulator: u16,
    previous_crc_control_flag: bool,
    gap_ended: bool,
    scanning_disk: bool,
    transfer_complete: bool,

    /// The trait exposes a single IRQ line; Mesen keeps the FDS timer IRQ
    /// ("External") and the disk IRQ separate, so we do too and OR them.
    external_irq: bool,
    disk_irq: bool,

    disk_sides: Vec<Vec<u8>>,
    /// Original (as-loaded) side data, used to serialize only the bytes the
    /// game wrote back into the save state.
    org_disk_sides: Vec<Vec<u8>>,

    auto_insert: bool,
    disable_auto_insert: bool,
    auto_disk_eject_counter: i32,
    auto_disk_switch_counter: i32,
    restart_auto_insert_counter: i32,
    frame_count: u32,
    previous_frame: u32,
    last_disk_check_frame: i32,
    successive_checks: i32,
    previous_disk_number: u32,
}

impl Fds {
    fn new(mut rom: RomData) -> Self {
        let bios = match rom.fds_bios.take() {
            Some(data) if data.len() >= 0x2000 => Some(data[..0x2000].to_vec()),
            _ => None,
        };

        let mut common = MapperCommon::new(rom);

        // FDS has 32KB of work RAM at $6000-$DFFF and 8KB of CHR RAM.
        common.prg_ram.resize(0x8000, 0);
        common.prg_ram.truncate(0x8000);
        if common.chr.len() < 0x2000 {
            common.chr.resize(0x2000, 0);
        }
        common.chr_is_ram = true;
        common.mirroring = Mirroring::Vertical;

        let disk_sides = load_fds_disk_sides(&common.prg_rom);
        let org_disk_sides = disk_sides.clone();

        let disk_number = if disk_sides.is_empty() {
            FDS_NO_DISK
        } else {
            // Mesen's frontend inserts disk 0 on load when FdsAutoLoadDisk is
            // enabled (the default in the UI). The core has no disk UI yet, so
            // do it here to make the BIOS boot headlessly.
            0
        };

        Self {
            common,
            bios,
            audio: FdsAudio::new(),
            irq_reload_value: 0,
            irq_counter: 0,
            irq_enabled: false,
            irq_repeat_enabled: false,
            disk_reg_enabled: true,
            sound_reg_enabled: true,
            write_data_reg: 0,
            motor_on: false,
            reset_transfer: false,
            read_mode: false,
            crc_control: false,
            disk_ready: false,
            disk_irq_enabled: false,
            ext_con_write_reg: 0,
            bad_crc: false,
            end_of_head: false,
            read_data_reg: 0,
            disk_number,
            disk_position: 0,
            delay: 0,
            crc_accumulator: 0,
            previous_crc_control_flag: false,
            gap_ended: true,
            scanning_disk: false,
            transfer_complete: false,
            external_irq: false,
            disk_irq: false,
            disk_sides,
            org_disk_sides,
            auto_insert: true,
            disable_auto_insert: false,
            auto_disk_eject_counter: -1,
            auto_disk_switch_counter: -1,
            restart_auto_insert_counter: -1,
            frame_count: 0,
            previous_frame: 0,
            last_disk_check_frame: 0,
            successive_checks: 0,
            previous_disk_number: FDS_NO_DISK,
        }
    }

    /// True when no 8KB BIOS image was supplied. The mapper still constructs
    /// and CPU reads from $E000-$FFFF return 0xFF; the BIOS reset vector is
    /// meaningless, so the system cannot boot.
    pub fn bios_missing(&self) -> bool {
        self.bios.is_none()
    }

    pub fn bios_present(&self) -> bool {
        self.bios.is_some()
    }

    pub fn side_count(&self) -> usize {
        self.disk_sides.len()
    }

    pub fn current_disk(&self) -> u32 {
        self.disk_number
    }

    pub fn is_disk_inserted(&self) -> bool {
        self.disk_number != FDS_NO_DISK
    }

    /// Same semantics as Mesen's `InsertDisk`: only inserts when empty.
    pub fn insert_disk(&mut self, disk_number: u32) {
        let count = self.disk_sides.len();
        if self.disk_number == FDS_NO_DISK && count > 0 {
            self.disk_number = disk_number % count as u32;
        }
    }

    pub fn eject_disk(&mut self) {
        self.disk_number = FDS_NO_DISK;
    }

    pub fn set_auto_insert_disk(&mut self, enabled: bool) {
        self.auto_insert = enabled;
    }

    fn auto_insert_enabled(&self) -> bool {
        self.auto_insert && !self.disable_auto_insert && !self.disk_sides.is_empty()
    }

    fn read_bios(&self, addr: u16) -> u8 {
        match &self.bios {
            Some(bios) => bios[((addr - 0xE000) as usize) & 0x1FFF],
            None => 0xFF,
        }
    }

    fn read_disk_byte(&self) -> u8 {
        let index = self.disk_number as usize;
        match self.disk_sides.get(index) {
            Some(side) => side.get(self.disk_position as usize).copied().unwrap_or(0),
            None => 0,
        }
    }

    fn write_disk_byte(&mut self, value: u8) {
        let index = self.disk_number as usize;
        if index >= self.disk_sides.len() {
            return;
        }
        // Writes are delayed by two bytes relative to the head position in
        // Mesen (`_diskPosition - 2`). Guard against the power-on case where
        // the position is 0 or 1.
        if let Some(position) = self.disk_position.checked_sub(2) {
            if let Some(byte) = self.disk_sides[index].get_mut(position as usize) {
                *byte = value;
            }
        }
    }

    fn clock_irq(&mut self) {
        if self.irq_enabled {
            if self.irq_counter == 0 {
                self.external_irq = true;
                self.irq_counter = self.irq_reload_value;
                if !self.irq_repeat_enabled {
                    self.irq_enabled = false;
                }
            } else {
                self.irq_counter -= 1;
            }
        }
    }

    fn update_crc(&mut self, value: u8) {
        let mut n = 0x01u16;
        while n <= 0x80 {
            let carry = self.crc_accumulator & 1;
            self.crc_accumulator >>= 1;
            if carry != 0 {
                self.crc_accumulator ^= 0x8408;
            }

            if value & (n as u8) != 0 {
                self.crc_accumulator ^= 0x8000;
            }

            n <<= 1;
        }
    }

    fn process_auto_disk_insert(&mut self) {
        if !self.auto_insert_enabled() {
            return;
        }

        let current_frame = self.frame_count;
        if self.previous_frame == current_frame {
            return;
        }
        self.previous_frame = current_frame;

        if self.auto_disk_eject_counter > 0 {
            // After reading a disk, wait until this counter reaches 0 before
            // automatically ejecting the disk the next time $4032 is read.
            self.auto_disk_eject_counter -= 1;
        } else if self.auto_disk_switch_counter > 0 {
            // After ejecting the disk, wait a bit before inserting a new one.
            self.auto_disk_switch_counter -= 1;
            if self.auto_disk_switch_counter == 0 {
                // Mesen inserts a dummy disk 0 and lets the game select the
                // proper side through $E445. We cannot read CPU RAM from a
                // mapper, so advance to the next side instead: games that keep
                // polling $4032 will eventually reach the requested side.
                let count = self.disk_sides.len() as u32;
                let next = if self.previous_disk_number == FDS_NO_DISK {
                    0
                } else {
                    (self.previous_disk_number + 1) % count
                };
                self.disk_number = next;
                self.previous_disk_number = next;

                // Restart the process after 200 frames if the game hasn't
                // read the disk yet.
                self.restart_auto_insert_counter = 200;
            }
        } else if self.restart_auto_insert_counter > 0 {
            self.restart_auto_insert_counter -= 1;
            if self.restart_auto_insert_counter == 0 {
                // Wait a bit before ejecting the disk (eject in ~34 frames).
                self.auto_disk_eject_counter = 34;
                self.auto_disk_switch_counter = -1;
            }
        }
    }

    fn read_register(&mut self, addr: u16) -> Option<u8> {
        if self.sound_reg_enabled && addr >= 0x4040 {
            return Some(self.audio.read_register(addr));
        }

        if self.disk_reg_enabled && addr <= 0x4033 {
            match addr {
                0x4030 => {
                    let mut value = 0u8;
                    if self.external_irq {
                        value |= 0x01;
                    }
                    if self.transfer_complete {
                        value |= 0x02;
                    }
                    if self.bad_crc {
                        value |= 0x10;
                    }

                    self.transfer_complete = false;
                    self.external_irq = false;
                    self.disk_irq = false;
                    return Some(value);
                }
                0x4031 => {
                    self.transfer_complete = false;
                    self.disk_irq = false;
                    return Some(self.read_data_reg);
                }
                0x4032 => {
                    let inserted = self.is_disk_inserted();
                    let mut value = 0u8;
                    if !inserted {
                        value |= 0x01; // Disk not in drive
                    }
                    if !inserted || !self.scanning_disk {
                        value |= 0x02; // Disk not ready
                    }
                    if !inserted {
                        value |= 0x04; // Disk not writable
                    }

                    if self.auto_insert_enabled() {
                        let frame = self.frame_count as i32;
                        if frame - self.last_disk_check_frame < 100 {
                            self.successive_checks += 1;
                        } else {
                            self.successive_checks = 0;
                        }
                        self.last_disk_check_frame = frame;

                        if self.successive_checks > 20
                            && self.auto_disk_eject_counter == 0
                            && self.auto_disk_switch_counter == -1
                        {
                            // Game tried to check if a disk was inserted - this
                            // usually happens when the disk needs to change.
                            self.last_disk_check_frame = 0;
                            self.successive_checks = 0;
                            self.auto_disk_switch_counter = 77;
                            self.previous_disk_number = self.disk_number;
                            self.disk_number = FDS_NO_DISK;
                        }
                    }
                    return Some(value);
                }
                0x4033 => {
                    // Always return good battery.
                    return Some(self.ext_con_write_reg);
                }
                _ => return None,
            }
        }

        None
    }

    fn write_register(&mut self, addr: u16, value: u8) {
        if (!self.disk_reg_enabled && (0x4024..=0x4026).contains(&addr))
            || (!self.sound_reg_enabled && addr >= 0x4040)
        {
            return;
        }

        match addr {
            0x4020 => {
                self.irq_reload_value = (self.irq_reload_value & 0xFF00) | value as u16;
            }
            0x4021 => {
                self.irq_reload_value = (self.irq_reload_value & 0x00FF) | ((value as u16) << 8);
            }
            0x4022 => {
                self.irq_repeat_enabled = (value & 0x01) == 0x01;
                self.irq_enabled = (value & 0x02) == 0x02 && self.disk_reg_enabled;

                if self.irq_enabled {
                    self.irq_counter = self.irq_reload_value;
                } else {
                    self.external_irq = false;
                }
            }
            0x4023 => {
                self.disk_reg_enabled = (value & 0x01) == 0x01;
                self.sound_reg_enabled = (value & 0x02) == 0x02;

                if !self.disk_reg_enabled {
                    self.irq_enabled = false;
                    self.external_irq = false;
                    self.disk_irq = false;
                }
            }
            0x4024 => {
                self.write_data_reg = value;
                self.transfer_complete = false;
                self.disk_irq = false;
            }
            0x4025 => {
                self.motor_on = (value & 0x01) == 0x01;
                self.reset_transfer = (value & 0x02) == 0x02;
                self.read_mode = (value & 0x04) == 0x04;
                self.common.mirroring = if value & 0x08 != 0 {
                    Mirroring::Horizontal
                } else {
                    Mirroring::Vertical
                };
                self.crc_control = (value & 0x10) == 0x10;
                // Bit 6 is not used, always 1.
                self.disk_ready = (value & 0x40) == 0x40;
                self.disk_irq_enabled = (value & 0x80) == 0x80;

                // Writing to $4025 clears IRQ according to FCEUX, puNES and
                // Nintendulator. Fixes issues in some unlicensed games.
                self.disk_irq = false;
            }
            0x4026 => {
                self.ext_con_write_reg = value;
            }
            _ => {
                if addr >= 0x4040 {
                    self.audio.write_register(addr, value);
                }
            }
        }
    }
}

impl Mapper for Fds {
    fn common(&self) -> &MapperCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn Any> {
        Some(self)
    }

    fn bios_missing(&self) -> bool {
        Fds::bios_missing(self)
    }

    fn reset(&mut self) {
        // FDS::Reset only resets the auto-disk-insert state.
        self.auto_disk_eject_counter = -1;
        self.auto_disk_switch_counter = -1;
        self.restart_auto_insert_counter = -1;
        self.disable_auto_insert = false;
    }

    fn cpu_read(&mut self, addr: u16) -> Option<u8> {
        match addr {
            0x4020..=0x4092 => self.read_register(addr),
            0x6000..=0xDFFF => {
                let index = (addr as usize - 0x6000) % self.common.prg_ram.len().max(1);
                Some(self.common.prg_ram[index])
            }
            0xE000..=0xFFFF => {
                // $E445 is the BIOS routine that checks whether the required
                // disk is inserted. Mesen uses it to pick the matching side
                // from CPU RAM, which a mapper cannot read; at least stop the
                // auto-insert retry counters when the game reaches this check.
                if addr == 0xE445 && self.auto_insert_enabled() {
                    self.auto_disk_switch_counter = -1;
                    self.restart_auto_insert_counter = -1;
                }
                Some(self.read_bios(addr))
            }
            _ => None,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x4020..=0x4092 => self.write_register(addr, value),
            0x6000..=0xDFFF => {
                let index = (addr as usize - 0x6000) % self.common.prg_ram.len().max(1);
                self.common.prg_ram[index] = value;
            }
            _ => {}
        }
    }

    fn scanline(&mut self, scanline: i16) {
        if scanline == 0 {
            self.frame_count = self.frame_count.wrapping_add(1);
        }
    }

    fn cpu_cycle(&mut self) {
        self.process_auto_disk_insert();
        self.clock_irq();
        self.audio.clock();

        if self.disk_number == FDS_NO_DISK || !self.motor_on {
            // Disk has been ejected.
            self.end_of_head = true;
            self.scanning_disk = false;
            return;
        }

        if self.reset_transfer && !self.scanning_disk {
            return;
        }

        if self.end_of_head {
            self.delay = 50000;
            self.end_of_head = false;
            self.disk_position = 0;
            self.gap_ended = false;
            return;
        }

        if self.delay > 0 {
            self.delay -= 1;
            return;
        }

        if self.disk_number as usize >= self.disk_sides.len() {
            return;
        }

        self.scanning_disk = true;
        self.auto_disk_eject_counter = -1;
        self.auto_disk_switch_counter = -1;

        let mut need_irq = self.disk_irq_enabled;

        if self.read_mode {
            let disk_data = self.read_disk_byte();

            if !self.previous_crc_control_flag {
                self.update_crc(disk_data);
            }

            if !self.disk_ready {
                self.gap_ended = false;
                self.crc_accumulator = 0;
            } else if disk_data != 0 && !self.gap_ended {
                self.gap_ended = true;
                need_irq = false;
            }

            if self.gap_ended {
                self.transfer_complete = true;
                self.read_data_reg = disk_data;
                if need_irq {
                    self.disk_irq = true;
                }
            }
        } else {
            let mut disk_data = 0u8;

            if !self.crc_control {
                self.transfer_complete = true;
                disk_data = self.write_data_reg;
                if need_irq {
                    self.disk_irq = true;
                }
            }

            if !self.disk_ready {
                disk_data = 0x00;
            }

            if !self.crc_control {
                self.update_crc(disk_data);
            } else {
                if !self.previous_crc_control_flag {
                    // Finish CRC calculation.
                    self.update_crc(0x00);
                    self.update_crc(0x00);
                }
                disk_data = (self.crc_accumulator & 0xFF) as u8;
                self.crc_accumulator >>= 8;
            }

            self.write_disk_byte(disk_data);
            self.gap_ended = false;
        }

        self.previous_crc_control_flag = self.crc_control;

        self.disk_position += 1;
        let side_len = self.disk_sides[self.disk_number as usize].len();
        if self.disk_position as usize >= side_len {
            self.motor_on = false;

            // Wait a bit before ejecting the disk (eject in ~77 frames).
            self.auto_disk_eject_counter = 77;
        } else {
            self.delay = 150;
        }
    }

    fn irq_pending(&self) -> bool {
        self.external_irq || self.disk_irq
    }

    fn expansion_audio_sample(&self) -> f32 {
        self.audio.sample()
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);

        self.audio.save_state(w);

        w.u16(self.irq_reload_value);
        w.u16(self.irq_counter);
        w.bool(self.irq_enabled);
        w.bool(self.irq_repeat_enabled);
        w.bool(self.disk_reg_enabled);
        w.bool(self.sound_reg_enabled);
        w.u8(self.write_data_reg);
        w.bool(self.motor_on);
        w.bool(self.reset_transfer);
        w.bool(self.read_mode);
        w.bool(self.crc_control);
        w.bool(self.disk_ready);
        w.bool(self.disk_irq_enabled);
        w.u8(self.ext_con_write_reg);
        w.bool(self.bad_crc);
        w.bool(self.end_of_head);
        w.u8(self.read_data_reg);
        w.u32(self.disk_number);
        w.u32(self.disk_position);
        w.u32(self.delay);
        w.u16(self.crc_accumulator);
        w.bool(self.previous_crc_control_flag);
        w.bool(self.gap_ended);
        w.bool(self.scanning_disk);
        w.bool(self.transfer_complete);
        w.bool(self.external_irq);
        w.bool(self.disk_irq);
        w.bool(self.auto_insert);
        w.bool(self.disable_auto_insert);
        w.i32(self.auto_disk_eject_counter);
        w.i32(self.auto_disk_switch_counter);
        w.i32(self.restart_auto_insert_counter);
        w.u32(self.frame_count);
        w.u32(self.previous_frame);
        w.i32(self.last_disk_check_frame);
        w.i32(self.successive_checks);
        w.u32(self.previous_disk_number);

        // Only the bytes the game wrote to the disk are serialized (Mesen
        // saves an IPS patch of the modified sides).
        w.u32(self.disk_sides.len() as u32);
        for (side_index, side) in self.disk_sides.iter().enumerate() {
            let org = self.org_disk_sides.get(side_index);
            let mut changes: Vec<(u32, u8)> = Vec::new();
            for (offset, &value) in side.iter().enumerate() {
                let original = org
                    .and_then(|o| o.get(offset))
                    .copied()
                    .unwrap_or(0);
                if value != original {
                    changes.push((offset as u32, value));
                }
            }
            w.u32(changes.len() as u32);
            for (offset, value) in changes {
                w.u32(offset);
                w.u8(value);
            }
        }
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;

        self.audio.load_state(r)?;

        self.irq_reload_value = r.u16()?;
        self.irq_counter = r.u16()?;
        self.irq_enabled = r.bool()?;
        self.irq_repeat_enabled = r.bool()?;
        self.disk_reg_enabled = r.bool()?;
        self.sound_reg_enabled = r.bool()?;
        self.write_data_reg = r.u8()?;
        self.motor_on = r.bool()?;
        self.reset_transfer = r.bool()?;
        self.read_mode = r.bool()?;
        self.crc_control = r.bool()?;
        self.disk_ready = r.bool()?;
        self.disk_irq_enabled = r.bool()?;
        self.ext_con_write_reg = r.u8()?;
        self.bad_crc = r.bool()?;
        self.end_of_head = r.bool()?;
        self.read_data_reg = r.u8()?;
        self.disk_number = r.u32()?;
        self.disk_position = r.u32()?;
        self.delay = r.u32()?;
        self.crc_accumulator = r.u16()?;
        self.previous_crc_control_flag = r.bool()?;
        self.gap_ended = r.bool()?;
        self.scanning_disk = r.bool()?;
        self.transfer_complete = r.bool()?;
        self.external_irq = r.bool()?;
        self.disk_irq = r.bool()?;
        self.auto_insert = r.bool()?;
        self.disable_auto_insert = r.bool()?;
        self.auto_disk_eject_counter = r.i32()?;
        self.auto_disk_switch_counter = r.i32()?;
        self.restart_auto_insert_counter = r.i32()?;
        self.frame_count = r.u32()?;
        self.previous_frame = r.u32()?;
        self.last_disk_check_frame = r.i32()?;
        self.successive_checks = r.i32()?;
        self.previous_disk_number = r.u32()?;

        let side_count = r.u32()? as usize;
        if side_count != self.disk_sides.len() {
            return Err(format!(
                "FDS side count mismatch: state {} != cart {}",
                side_count,
                self.disk_sides.len()
            )
            .into());
        }

        for side_index in 0..side_count {
            match self.org_disk_sides.get(side_index) {
                Some(org) if org.len() == self.disk_sides[side_index].len() => {
                    self.disk_sides[side_index].copy_from_slice(org);
                }
                _ => self.disk_sides[side_index].fill(0),
            }

            let change_count = r.u32()? as usize;
            for _ in 0..change_count {
                let offset = r.u32()? as usize;
                let value = r.u8()?;
                if let Some(byte) = self.disk_sides[side_index].get_mut(offset) {
                    *byte = value;
                }
            }
        }

        // No open-bus / CPU RAM access from the mapper; make sure the head is
        // in a consistent state.
        if self.disk_number != FDS_NO_DISK && self.disk_number as usize >= self.disk_sides.len() {
            self.disk_number = FDS_NO_DISK;
        }
        Ok(())
    }
}

// ===========================================================================
// NSF player (NsfMapper.cpp/.h, NsfLoader.cpp, NsfPpu.cpp)
// ===========================================================================

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum NsfIrqType {
    Init = 0,
    Stop = 1,
    Play = 2,
    None = 0xFF,
}

impl NsfIrqType {
    fn from_u8(value: u8) -> Self {
        match value {
            0 => NsfIrqType::Init,
            1 => NsfIrqType::Stop,
            2 => NsfIrqType::Play,
            _ => NsfIrqType::None,
        }
    }
}

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum NsfModel {
    Ntsc = 0,
    Pal = 1,
    Dendy = 2,
}

#[derive(Clone)]
struct NsfHeader {
    total_songs: u8,
    starting_song: u8,
    load_address: u16,
    init_address: u16,
    play_address: u16,
    play_speed_ntsc: u16,
    play_speed_pal: u16,
    flags: u8,
    sound_chips: u8,
    bank_setup: [u8; 8],
    track_length: [i32; 256],
    track_fade: [i32; 256],
}

impl NsfHeader {
    fn empty() -> Self {
        Self {
            total_songs: 1,
            starting_song: 1,
            load_address: 0x8000,
            init_address: 0,
            play_address: 0,
            play_speed_ntsc: 16639,
            play_speed_pal: 19997,
            flags: 0,
            sound_chips: 0,
            bank_setup: [0; 8],
            track_length: [-1; 256],
            track_fade: [-1; 256],
        }
    }
}

fn read_u16_le(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

/// Parse the 0x80-byte NSF header and build the 4KB-aligned image Mesen feeds
/// to the mapper (`NsfLoader::LoadRom` / `InitializeFromHeader`).
fn parse_nsf(rom: &RomData) -> (NsfHeader, Vec<u8>) {
    let data = &rom.prg_rom;
    let mut header = NsfHeader::empty();

    let has_header = data.len() >= 0x80 && &data[0..5] == b"NESM\x1a";
    let payload_start = if has_header { 0x80usize } else { 0usize };

    if has_header {
        header.total_songs = data[6];
        header.starting_song = data[7];
        header.load_address = read_u16_le(data, 8);
        header.init_address = read_u16_le(data, 10);
        header.play_address = read_u16_le(data, 12);
        header.play_speed_ntsc = read_u16_le(data, 0x6E);
        header.bank_setup.copy_from_slice(&data[0x70..0x78]);
        header.play_speed_pal = read_u16_le(data, 0x78);
        header.flags = data[0x7A];
        header.sound_chips = data[0x7B];
    }

    if header.play_speed_ntsc == 0 {
        header.play_speed_ntsc = 16639;
    }
    if header.play_speed_pal == 0 {
        header.play_speed_pal = 19997;
    }
    if header.play_speed_ntsc == 16666 || header.play_speed_ntsc == 16667 {
        header.play_speed_ntsc = 16639;
    }
    if header.play_speed_pal == 20000 {
        header.play_speed_pal = 19997;
    }
    if header.starting_song > header.total_songs || header.starting_song == 0 {
        header.starting_song = 1;
    }

    // Pad the start of the image so the first block starts at a multiple of
    // 4KB, exactly like NsfLoader::LoadRom.
    let padding = if has_header {
        (header.load_address as usize) % 4096
    } else {
        0
    };
    let mut image = vec![0u8; padding];
    image.extend_from_slice(&data[payload_start..]);
    if image.len() % 0x1000 != 0 {
        image.resize(image.len() + (0x1000 - image.len() % 0x1000), 0);
    }
    if image.is_empty() {
        image.resize(0x1000, 0);
    }

    (header, image)
}

/// A 4KB PRG window target for the NSF mapper.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NsfBank {
    Ram(u8),
    Rom(usize),
    Unmapped,
}

fn save_nsf_bank(w: &mut StateWriter, bank: NsfBank) {
    match bank {
        NsfBank::Ram(page) => {
            w.u8(0);
            w.u32(page as u32);
        }
        NsfBank::Rom(page) => {
            w.u8(1);
            w.u32(page as u32);
        }
        NsfBank::Unmapped => {
            w.u8(2);
            w.u32(0);
        }
    }
}

fn load_nsf_bank(r: &mut StateReader<'_>) -> StateResult<NsfBank> {
    let kind = r.u8()?;
    let page = r.u32()? as usize;
    Ok(match kind {
        0 => NsfBank::Ram(page as u8),
        1 => NsfBank::Rom(page),
        _ => NsfBank::Unmapped,
    })
}

/// Quietust's NSF BIOS (from Nintendulator), `$3F00-$3FFF`.
const NSF_BIOS: [u8; 256] = [
    0xFF, 0xFF, 0xFF, 0x78, 0xA2, 0xFF, 0x8E, 0x17, 0x40, 0xE8, 0x20, 0x30, 0x3F, 0x8E, 0x00, 0x20,
    0x8E, 0x01, 0x20, 0x8E, 0x12, 0x3E, 0x58, 0x4C, 0x17, 0x3F, 0x48, 0x8A, 0x48, 0x98, 0x48, 0xAE,
    0x12, 0x3E, 0xF0, 0x59, 0xCA, 0xF0, 0xDC, 0x20, 0xF9, 0x3F, 0x68, 0xA8, 0x68, 0xAA, 0x68, 0x40,
    0x8E, 0x15, 0x40, 0xAD, 0x13, 0x3E, 0x4A, 0x90, 0x09, 0x8E, 0x02, 0x90, 0x8E, 0x02, 0xA0, 0x8E,
    0x02, 0xB0, 0x4A, 0x90, 0x0D, 0xA0, 0x20, 0x8C, 0x10, 0x90, 0x8E, 0x30, 0x90, 0xC8, 0xC0, 0x26,
    0xD0, 0xF5, 0x4A, 0x90, 0x0B, 0xA0, 0x80, 0x8C, 0x83, 0x40, 0x8C, 0x87, 0x40, 0x8C, 0x89, 0x40,
    0x4A, 0x90, 0x03, 0x8E, 0x15, 0x50, 0x4A, 0x90, 0x08, 0xCA, 0x8E, 0x00, 0xF8, 0xE8, 0x8E, 0x00,
    0x48, 0x4A, 0x90, 0x08, 0xA0, 0x07, 0x8C, 0x00, 0xC0, 0x8C, 0x00, 0xE0, 0x60, 0x20, 0x30, 0x3F,
    0x8A, 0xCA, 0x9A, 0x8E, 0xF7, 0x5F, 0xCA, 0x8E, 0xF6, 0x5F, 0xA2, 0x7F, 0x85, 0x00, 0x86, 0x01,
    0xA8, 0xA2, 0x27, 0x91, 0x00, 0xC8, 0xD0, 0xFB, 0xCA, 0x30, 0x0A, 0xC6, 0x01, 0xE0, 0x07, 0xD0,
    0xF2, 0x86, 0x01, 0xF0, 0xEE, 0xA2, 0x14, 0xCA, 0x9D, 0x00, 0x40, 0xD0, 0xFA, 0xA2, 0x07, 0xBD,
    0x08, 0x3E, 0x9D, 0xF8, 0x5F, 0xCA, 0x10, 0xF7, 0xA0, 0x0F, 0x8C, 0x15, 0x40, 0xAD, 0x13, 0x3E,
    0x29, 0x04, 0xF0, 0x10, 0xAD, 0x0E, 0x3E, 0xEA, 0xEA, 0x8D, 0xF6, 0x5F, 0xAD, 0x0F, 0x3E, 0xEA,
    0xEA, 0x8D, 0xF7, 0x5F, 0xAE, 0x11, 0x3E, 0xBD, 0x04, 0x3E, 0x8D, 0x10, 0x3E, 0xBD, 0x06, 0x3E,
    0x8D, 0x11, 0x3E, 0x8C, 0x12, 0x3E, 0xAD, 0x12, 0x3E, 0x58, 0xAD, 0x10, 0x3E, 0x20, 0xF6, 0x3F,
    0x8D, 0x13, 0x3E, 0x4C, 0x17, 0x3F, 0x6C, 0x00, 0x3E, 0x6C, 0x02, 0x3E, 0x03, 0x3F, 0x1A, 0x3F,
];

/// NSF player mapper (the upstream `NsfMapper`). The Rust loader contract
/// stores the whole NSF file (including the 0x80-byte header) in
/// `RomData.prg_rom`; we parse it here and build a 4KB-aligned image.
pub struct NsfMapper {
    common: MapperCommon,
    header: NsfHeader,
    image: Vec<u8>,

    banks_8000: [NsfBank; 8],
    bank_writable: [bool; 8],
    bank_6000: NsfBank,
    bank_7000: NsfBank,
    bank_6000_writable: bool,
    bank_7000_writable: bool,

    model: NsfModel,
    need_init: bool,
    irq_enabled: bool,
    irq_reload_value: u32,
    irq_counter: u32,
    irq_status: NsfIrqType,
    mmc5_multiplier_values: [u8; 2],

    track_end_counter: i32,
    track_fade_counter: i32,
    track_ended: bool,

    has_bank_switching: bool,

    ntsc_speed: u16,
    pal_speed: u16,
    dendy_speed: u16,

    song_number: u8,

    fds_audio: FdsAudio,
}

impl NsfMapper {
    fn new(rom: RomData) -> Self {
        let (header, image) = parse_nsf(&rom);

        let model = if header.flags & 0x01 != 0 {
            NsfModel::Pal
        } else {
            match rom.info.region {
                Region::Pal => NsfModel::Pal,
                Region::Dendy => NsfModel::Dendy,
                Region::Ntsc => NsfModel::Ntsc,
            }
        };

        let mut common = MapperCommon::new(rom);
        if common.chr.len() < 0x2000 {
            common.chr.resize(0x2000, 0);
        }
        common.chr_is_ram = true;
        common.init_default_maps();

        let mut mapper = Self {
            common,
            header,
            image,
            banks_8000: [NsfBank::Unmapped; 8],
            bank_writable: [false; 8],
            bank_6000: NsfBank::Ram(0),
            bank_7000: NsfBank::Ram(1),
            bank_6000_writable: true,
            bank_7000_writable: true,
            model,
            need_init: false,
            irq_enabled: false,
            irq_reload_value: 0,
            irq_counter: 0,
            irq_status: NsfIrqType::None,
            mmc5_multiplier_values: [0; 2],
            track_end_counter: -1,
            track_fade_counter: -1,
            track_ended: false,
            has_bank_switching: false,
            ntsc_speed: 0,
            pal_speed: 0,
            dendy_speed: 0,
            song_number: 0,
            fds_audio: FdsAudio::new(),
        };

        mapper.has_bank_switching = mapper.header.bank_setup.iter().any(|&bank| bank != 0);
        if !mapper.has_bank_switching {
            // Update bank config to allow the BIOS to select the right banks
            // on init.
            let start_bank = (mapper.header.load_address / 0x1000) as i32;
            let page_total = page_count(mapper.image.len(), 0x1000) as i32;
            for i in 0..page_total {
                if start_bank + i > 0x0F {
                    break;
                }
                if start_bank + i - 8 >= 0 {
                    mapper.header.bank_setup[(start_bank + i - 8) as usize] = i as u8;
                }
            }
        }

        mapper.ntsc_speed = (mapper.header.play_speed_ntsc as f64 * (1_789_773.0 / 1_000_000.0)) as u16;
        mapper.pal_speed = (mapper.header.play_speed_pal as f64 * (1_662_607.0 / 1_000_000.0)) as u16;
        mapper.dendy_speed = (mapper.header.play_speed_pal as f64 * (1_773_448.0 / 1_000_000.0)) as u16;

        mapper.reset_player();
        mapper
    }

    fn image_pages(&self) -> usize {
        page_count(self.image.len(), 0x1000)
    }

    fn rom_page(&self, page: usize) -> NsfBank {
        NsfBank::Rom(page % self.image_pages())
    }

    fn reset_player(&mut self) {
        self.song_number = self.header.starting_song.saturating_sub(1);
        self.need_init = true;
        self.irq_enabled = false;
        self.irq_counter = 0;
        self.irq_reload_value = 0;
        self.irq_status = NsfIrqType::None;
        self.mmc5_multiplier_values = [0; 2];
        self.track_end_counter = -1;
        self.track_fade_counter = -1;
        self.track_ended = false;
        self.banks_8000 = [NsfBank::Unmapped; 8];
        self.bank_writable = [false; 8];
        self.bank_6000 = NsfBank::Ram(0);
        self.bank_7000 = NsfBank::Ram(1);
        self.bank_6000_writable = true;
        self.bank_7000_writable = true;

        self.internal_select_track(self.song_number, false);
        self.need_init = true;
    }

    pub fn select_track(&mut self, track_number: u8) {
        if track_number < self.header.total_songs {
            self.internal_select_track(track_number, true);
        }
    }

    pub fn current_track(&self) -> u8 {
        self.song_number
    }

    pub fn track_count(&self) -> u8 {
        self.header.total_songs
    }

    pub fn init_address(&self) -> u16 {
        self.header.init_address
    }

    pub fn play_address(&self) -> u16 {
        self.header.play_address
    }

    pub fn load_address(&self) -> u16 {
        self.header.load_address
    }

    pub fn sound_chips(&self) -> u8 {
        self.header.sound_chips
    }

    /// Upstream `SetNesModel`: changing the model adjusts the PLAY IRQ reload
    /// value to follow the tempo.
    pub fn set_nes_model(&mut self, region: Region) {
        let model = match region {
            Region::Ntsc => NsfModel::Ntsc,
            Region::Pal => NsfModel::Pal,
            Region::Dendy => NsfModel::Dendy,
        };
        if model != self.model {
            self.irq_reload_value = match model {
                NsfModel::Ntsc => self.ntsc_speed as u32,
                NsfModel::Pal => self.pal_speed as u32,
                NsfModel::Dendy => self.dendy_speed as u32,
            };
            self.model = model;
        }
    }

    fn clock_rate(&self) -> u32 {
        if self.header.flags & 0x01 != 0 {
            1_662_607
        } else {
            1_789_773
        }
    }

    fn speed_low(&self) -> u8 {
        (match self.model {
            NsfModel::Ntsc => self.ntsc_speed & 0xFF,
            NsfModel::Pal => self.pal_speed & 0xFF,
            NsfModel::Dendy => self.dendy_speed & 0xFF,
        }) as u8
    }

    fn speed_high(&self) -> u8 {
        (match self.model {
            NsfModel::Ntsc => (self.ntsc_speed >> 8) & 0xFF,
            NsfModel::Pal => (self.pal_speed >> 8) & 0xFF,
            NsfModel::Dendy => (self.dendy_speed >> 8) & 0xFF,
        }) as u8
    }

    fn set_track_timers(&mut self, track_number: u8) {
        let clock_rate = self.clock_rate() as i64;
        let track = track_number as usize;

        if self.header.track_length[track] >= 0 {
            self.track_end_counter = (self.header.track_length[track] as f64 / 1000.0
                * self.clock_rate() as f64) as i32;
        } else if self.header.total_songs > 1 {
            // Only apply a maximum duration to multi-track NSFs. The default
            // move-to-next-track time in Mesen is 120 seconds; subtract one
            // second for the fade.
            self.track_end_counter = (119 * clock_rate) as i32;
        } else {
            self.track_end_counter = -1;
        }

        if self.header.track_fade[track] >= 0 {
            self.track_fade_counter = (self.header.track_fade[track] as f64 / 1000.0
                * self.clock_rate() as f64) as i32;
        } else {
            // Default to a 1 second fade if none is specified.
            self.track_fade_counter = clock_rate as i32;
        }

        self.track_ended = false;
    }

    fn trigger_irq(&mut self, irq_type: NsfIrqType) {
        if irq_type == NsfIrqType::Init {
            self.track_ended = false;
        }
        self.irq_status = irq_type;
    }

    fn clear_irq(&mut self) {
        self.irq_status = NsfIrqType::None;
    }

    fn internal_select_track(&mut self, track_number: u8, request_reset: bool) {
        self.song_number = track_number;
        if request_reset {
            // Mesen soft-resets the console here so that a track change always
            // works, even when a tune keeps interrupts disabled. The mapper
            // cannot reset the CPU; we re-arm the init IRQ instead. The
            // frontend should call Emulator::reset() after select_track().
            self.irq_enabled = false;
            self.irq_counter = 0;
            self.irq_status = NsfIrqType::None;
            self.need_init = true;
        }

        self.set_track_timers(track_number);
        self.trigger_irq(NsfIrqType::Init);
    }

    fn clock_length_and_fade_counters(&mut self) {
        if self.track_end_counter > 0 {
            self.track_end_counter -= 1;
            if self.track_end_counter == 0 {
                self.track_ended = true;
            }
        }

        // Mesen also ends the track through the sound mixer's mute-frame
        // counter (NsfAutoDetectSilenceDelay). The mapper has no access to the
        // mixer/APU, so only explicit/known track lengths are honored here.

        if self.track_ended {
            if self.track_fade_counter > 0 {
                // The mixer fade ratio cannot be applied from the mapper; the
                // fade counter still controls when the next track starts.
                self.track_fade_counter -= 1;
            }

            if self.track_fade_counter <= 0 {
                self.select_next_track();
            }
        }
    }

    fn select_next_track(&mut self) {
        if self.header.total_songs == 0 {
            return;
        }
        if self.header.total_songs > 1 {
            self.song_number = (self.song_number + 1) % self.header.total_songs;
        }
        self.internal_select_track(self.song_number, true);
        self.track_ended = false;
    }

    fn read_bank(&self, bank: NsfBank, addr: u16) -> Option<u8> {
        let low = (addr & 0x0FFF) as usize;
        match bank {
            NsfBank::Ram(page) => {
                let offset = ((page as usize & 1) * 0x1000 + low) % self.common.prg_ram.len().max(1);
                Some(self.common.prg_ram[offset])
            }
            NsfBank::Rom(page) => {
                let offset = page * 0x1000 + low;
                self.image.get(offset).copied()
            }
            NsfBank::Unmapped => None,
        }
    }

    fn write_bank(&mut self, bank: NsfBank, writable: bool, addr: u16, value: u8) {
        let low = (addr & 0x0FFF) as usize;
        match bank {
            NsfBank::Ram(page) => {
                let offset = ((page as usize & 1) * 0x1000 + low) % self.common.prg_ram.len().max(1);
                self.common.prg_ram[offset] = value;
            }
            NsfBank::Rom(page) => {
                if writable {
                    let offset = page * 0x1000 + low;
                    if let Some(byte) = self.image.get_mut(offset) {
                        *byte = value;
                    }
                }
            }
            NsfBank::Unmapped => {}
        }
    }

    fn map_6000_bank(&mut self, high: bool, value: u8) {
        let is_ram_value = value == 0xFF || value == 0xFE;
        let (bank, writable) = if is_ram_value {
            if !self.has_bank_switching && self.header.load_address < 0x7000 {
                // Load address = 0x6000, put bank 0 at $6000.
                (self.rom_page((value & 0x01) as usize), true)
            } else if !self.has_bank_switching
                && high
                && self.header.load_address < 0x8000
            {
                // Load address = 0x7000, put bank 0 at $7000.
                (self.rom_page(0), true)
            } else {
                // Set RAM at $6000/7000 (default behavior).
                (NsfBank::Ram(value & 0x01), true)
            }
        } else {
            (self.rom_page(value as usize), true)
        };

        if high {
            self.bank_7000 = bank;
            self.bank_7000_writable = writable;
        } else {
            self.bank_6000 = bank;
            self.bank_6000_writable = writable;
        }
    }

    fn map_8000_bank(&mut self, slot: usize, value: u8) {
        self.banks_8000[slot & 7] = self.rom_page(value as usize);
        self.bank_writable[slot & 7] =
            (self.header.sound_chips & 0x04) != 0 && slot < 6;
    }

    fn read_register(&mut self, addr: u16) -> Option<u8> {
        if (self.header.sound_chips & 0x04) != 0 && (0x4040..=0x4092).contains(&addr) {
            return Some(self.fds_audio.read_register(addr));
        }
        if (self.header.sound_chips & 0x08) != 0 && (0x5205..=0x5206).contains(&addr) {
            let product = self.mmc5_multiplier_values[0] as u16
                * self.mmc5_multiplier_values[1] as u16;
            return Some(if addr == 0x5205 {
                (product & 0xFF) as u8
            } else {
                (product >> 8) as u8
            });
        }

        match addr {
            0x3E00 => Some((self.header.init_address & 0xFF) as u8),
            0x3E01 => Some((self.header.init_address >> 8) as u8),
            0x3E02 => Some((self.header.play_address & 0xFF) as u8),
            0x3E03 => Some((self.header.play_address >> 8) as u8),

            0x3E04 | 0x3E05 => Some(self.speed_low()),
            0x3E06 | 0x3E07 => Some(self.speed_high()),

            0x3E08..=0x3E0F => Some(self.header.bank_setup[(addr & 0x07) as usize]),

            0x3E10 => Some(self.song_number),
            0x3E11 => Some(if self.model == NsfModel::Pal { 0x01 } else { 0x00 }),
            0x3E12 => {
                let result = self.irq_status;
                self.clear_irq();
                Some(result as u8)
            }
            0x3E13 => Some(self.header.sound_chips & 0x3F),
            _ => None,
        }
    }

    fn write_register(&mut self, addr: u16, value: u8) {
        if (self.header.sound_chips & 0x04) != 0 && (0x4040..=0x4092).contains(&addr) {
            self.fds_audio.write_register(addr, value);
            return;
        }
        if (self.header.sound_chips & 0x08) != 0 && (0x5205..=0x5206).contains(&addr) {
            self.mmc5_multiplier_values[(addr - 0x5205) as usize] = value;
            return;
        }

        match addr {
            0x3E10 => {
                self.irq_reload_value = (self.irq_reload_value & 0xFF00) | value as u32;
            }
            0x3E11 => {
                self.irq_reload_value = (self.irq_reload_value & 0x00FF) | ((value as u32) << 8);
            }
            0x3E12 => {
                self.irq_counter = self.irq_reload_value * 5;
                self.irq_enabled = value > 0;
            }
            0x3E13 => {
                self.irq_counter = self.irq_reload_value;
            }
            0x5FF6 => self.map_6000_bank(false, value),
            0x5FF7 => self.map_6000_bank(true, value),
            0x5FF8..=0x5FFF => self.map_8000_bank((addr & 0x07) as usize, value),
            // VRC6 ($9000-$B002), VRC7 ($9010/$9030), MMC5 audio
            // ($5000-$5015) and Namco/Sunsoft registers are not implemented
            // here (see the final report); writes are ignored.
            _ => {}
        }
    }
}

impl Mapper for NsfMapper {
    fn common(&self) -> &MapperCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn Any> {
        Some(self)
    }

    fn reset(&mut self) {
        self.reset_player();
    }

    fn cpu_read(&mut self, addr: u16) -> Option<u8> {
        match addr {
            // The NSF BIOS uses $3E00-$3E13 as its parameter block. In Mesen
            // the mapper overrides the PPU register range for these addresses.
            0x3E00..=0x3E13 => self.read_register(addr),
            0x3F00..=0x3FFF => Some(NSF_BIOS[(addr - 0x3F00) as usize]),
            0x4040..=0x4092 => self.read_register(addr),
            0x4800..=0x4FFF => self.read_register(addr),
            0x5205..=0x5206 => self.read_register(addr),
            0x6000..=0x6FFF => self.read_bank(self.bank_6000, addr),
            0x7000..=0x7FFF => self.read_bank(self.bank_7000, addr),
            0x8000..=0xFFFF => {
                // Reset/IRQ vectors have priority over the banked PRG data,
                // like BaseMapper register ranges.
                if addr >= 0xFFFC {
                    return Some(NSF_BIOS[(addr & 0xFF) as usize]);
                }
                let slot = ((addr - 0x8000) >> 12) as usize;
                self.read_bank(self.banks_8000[slot], addr)
            }
            _ => None,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x3E10..=0x3E13 => self.write_register(addr, value),
            0x3F00..=0x3FFF => {}
            0x4040..=0x4092 => {
                if self.header.sound_chips & 0x04 != 0 {
                    self.fds_audio.write_register(addr, value);
                }
            }
            0x5000..=0x5015 => {}
            0x5205..=0x5206 => self.write_register(addr, value),
            0x5FF6..=0x5FFF => self.write_register(addr, value),
            0x6000..=0x6FFF => {
                self.write_bank(self.bank_6000, self.bank_6000_writable, addr, value)
            }
            0x7000..=0x7FFF => {
                self.write_bank(self.bank_7000, self.bank_7000_writable, addr, value)
            }
            0x8000..=0xFFFF => {
                let slot = ((addr - 0x8000) >> 12) as usize;
                let bank = self.banks_8000[slot];
                let writable = self.bank_writable[slot];
                self.write_bank(bank, writable, addr, value);
            }
            _ => {}
        }
    }

    fn cpu_cycle(&mut self) {
        if self.need_init {
            self.trigger_irq(NsfIrqType::Init);
            self.need_init = false;
        }

        if self.irq_enabled {
            self.irq_counter = self.irq_counter.wrapping_sub(1);
            if self.irq_counter == 0 {
                self.irq_counter = self.irq_reload_value;
                self.trigger_irq(NsfIrqType::Play);
            }
        }

        self.clock_length_and_fade_counters();

        if self.header.sound_chips & 0x04 != 0 {
            self.fds_audio.clock();
        }
    }

    fn irq_pending(&self) -> bool {
        self.irq_status != NsfIrqType::None
    }

    fn expansion_audio_sample(&self) -> f32 {
        if self.header.sound_chips & 0x04 != 0 {
            self.fds_audio.sample()
        } else {
            0.0
        }
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);

        w.u8(self.model as u8);
        w.bool(self.need_init);
        w.bool(self.irq_enabled);
        w.u32(self.irq_reload_value);
        w.u32(self.irq_counter);
        w.u8(self.irq_status as u8);
        w.u8(self.mmc5_multiplier_values[0]);
        w.u8(self.mmc5_multiplier_values[1]);
        w.i32(self.track_end_counter);
        w.i32(self.track_fade_counter);
        w.bool(self.track_ended);
        w.bool(self.has_bank_switching);
        w.u16(self.ntsc_speed);
        w.u16(self.pal_speed);
        w.u16(self.dendy_speed);
        w.u8(self.song_number);

        for slot in 0..8 {
            save_nsf_bank(w, self.banks_8000[slot]);
            w.bool(self.bank_writable[slot]);
        }
        save_nsf_bank(w, self.bank_6000);
        w.bool(self.bank_6000_writable);
        save_nsf_bank(w, self.bank_7000);
        w.bool(self.bank_7000_writable);

        self.fds_audio.save_state(w);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;

        self.model = match r.u8()? {
            1 => NsfModel::Pal,
            2 => NsfModel::Dendy,
            _ => NsfModel::Ntsc,
        };
        self.need_init = r.bool()?;
        self.irq_enabled = r.bool()?;
        self.irq_reload_value = r.u32()?;
        self.irq_counter = r.u32()?;
        self.irq_status = NsfIrqType::from_u8(r.u8()?);
        self.mmc5_multiplier_values[0] = r.u8()?;
        self.mmc5_multiplier_values[1] = r.u8()?;
        self.track_end_counter = r.i32()?;
        self.track_fade_counter = r.i32()?;
        self.track_ended = r.bool()?;
        self.has_bank_switching = r.bool()?;
        self.ntsc_speed = r.u16()?;
        self.pal_speed = r.u16()?;
        self.dendy_speed = r.u16()?;
        self.song_number = r.u8()?;

        for slot in 0..8 {
            self.banks_8000[slot] = load_nsf_bank(r)?;
            self.bank_writable[slot] = r.bool()?;
        }
        self.bank_6000 = load_nsf_bank(r)?;
        self.bank_6000_writable = r.bool()?;
        self.bank_7000 = load_nsf_bank(r)?;
        self.bank_7000_writable = r.bool()?;

        self.fds_audio.load_state(r)?;
        Ok(())
    }
}

// ===========================================================================
// iNES mapper 31 (NsfCart31.h)
// ===========================================================================

/// Simple 4KB bank mapper used by iNES files with mapper 31 (not the NSF
/// player). Upstream `NsfCart31`: eight independent 4KB windows at
/// $8000/$9000/.../$F000 selected by writes to $5000-$5FFF.
pub struct NsfCart31 {
    common: MapperCommon,
    /// Byte offsets of the eight 4KB PRG windows; `None` is open bus, matching
    /// BaseMapper's unmapped state.
    banks: [Option<usize>; 8],
}

impl NsfCart31 {
    fn new(rom: RomData) -> Self {
        let mut common = MapperCommon::new(rom);
        if common.chr.len() < 0x2000 {
            common.chr.resize(0x2000, 0);
        }
        // SelectCHRPage(0, 0).
        let chr_len = common.chr.len();
        for slot in 0..8 {
            common.chr_map[slot] = (slot * 0x400) % chr_len.max(1);
        }
        let mut mapper = Self {
            common,
            banks: [None; 8],
        };
        mapper.map_page(7, 0xFF);
        mapper
    }

    fn map_page(&mut self, slot: usize, page: u8) {
        let len = self.common.prg_rom.len();
        self.banks[slot & 7] = Some(page_offset(page as usize, 0x1000, len));
    }

    fn read_prg(&self, addr: u16) -> Option<u8> {
        let slot = ((addr as usize - 0x8000) >> 12) & 7;
        let bank = self.banks[slot]?;
        let len = self.common.prg_rom.len().max(1);
        Some(self.common.prg_rom[(bank + (addr as usize & 0x0FFF)) % len])
    }
}

impl Mapper for NsfCart31 {
    fn common(&self) -> &MapperCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }

    fn reset(&mut self) {
        // NsfCart31 does not override BaseMapper::Reset: bank latches survive
        // a soft reset.
    }

    fn cpu_read(&mut self, addr: u16) -> Option<u8> {
        match addr {
            0x6000..=0x7FFF => Some(self.common.read_prg_ram(addr)),
            0x8000..=0xFFFF => self.read_prg(addr),
            _ => None,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x5000..=0x5FFF => self.map_page((addr & 0x07) as usize, value),
            0x6000..=0x7FFF => self.common.write_prg_ram(addr, value),
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
        let len = self.common.prg_ram.len().min(data.len());
        self.common.prg_ram[..len].copy_from_slice(&data[..len]);
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);
        for bank in self.banks {
            match bank {
                Some(offset) => {
                    w.bool(true);
                    w.u32(offset as u32);
                }
                None => {
                    w.bool(false);
                    w.u32(0);
                }
            }
        }
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        for bank in &mut self.banks {
            let present = r.bool()?;
            let offset = r.u32()? as usize;
            *bank = if present { Some(offset) } else { None };
        }
        Ok(())
    }
}

// ===========================================================================
// VS System (VsSystem.h, VsControlManager.cpp)
// ===========================================================================

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VsSystemType {
    Default = 0,
    RbiBaseballProtection = 1,
    TkoBoxingProtection = 2,
    SuperXeviousProtection = 3,
    IceClimberProtection = 4,
    VsDualSystem = 5,
    RaidOnBungelingBayProtection = 6,
}

impl VsSystemType {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => VsSystemType::RbiBaseballProtection,
            2 => VsSystemType::TkoBoxingProtection,
            3 => VsSystemType::SuperXeviousProtection,
            4 => VsSystemType::IceClimberProtection,
            5 => VsSystemType::VsDualSystem,
            6 => VsSystemType::RaidOnBungelingBayProtection,
            _ => VsSystemType::Default,
        }
    }
}

const VS_PROTECTION_DATA: [[u8; 32]; 3] = [
    [
        0xFF, 0xBF, 0xB7, 0x97, 0x97, 0x17, 0x57, 0x4F, 0x6F, 0x6B, 0xEB, 0xA9, 0xB1, 0x90, 0x94,
        0x14, 0x56, 0x4E, 0x6F, 0x6B, 0xEB, 0xA9, 0xB1, 0x90, 0xD4, 0x5C, 0x3E, 0x26, 0x87, 0x83,
        0x13, 0x00,
    ],
    [
        0x00, 0x00, 0x00, 0x00, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x6F, 0x00, 0x00, 0x00, 0x00, 0x94,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00,
    ],
    [
        0x05, 0x01, 0x89, 0x37, 0x05, 0x00, 0xD1, 0x3E, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00,
    ],
];

/// VS System mapper (mapper 99).
///
/// Mesen splits the VS hardware between `VsSystem` (PRG/CHR banking) and
/// `VsControlManager` (DIP switches, coin/service inputs, $4016 writes,
/// protection). The Rust bus has no VS control manager, so the mapper
/// implements the register behaviour it can see and exposes helpers the
/// integration layer can call. See the final report for the required bus
/// routing changes.
pub struct VsSystem {
    common: MapperCommon,
    prg_chr_select_bit: u8,
    protection_counter: u32,
    refresh_state: bool,
    dip_switches: u32,
    vs_type: VsSystemType,
}

impl VsSystem {
    fn new(rom: RomData) -> Self {
        let info = rom.info.clone();
        let prg_size = rom.prg_rom.len();
        let mut common = MapperCommon::new(rom);
        // VS systems use 2KB of work RAM; battery-backed boards use the
        // standard 8KB save RAM instead (BaseMapper's default).
        if info.has_battery {
            common.prg_ram.resize(0x2000, 0);
        } else {
            common.prg_ram.resize(0x800, 0);
        }

        let vs_type = if !info.is_nes2 && prg_size >= 0x10000 {
            VsSystemType::VsDualSystem
        } else {
            VsSystemType::Default
        };

        let mut mapper = Self {
            common,
            prg_chr_select_bit: 0,
            protection_counter: 0,
            refresh_state: false,
            dip_switches: 0,
            vs_type,
        };
        mapper.apply_initial_mapping();
        mapper
    }

    fn apply_initial_mapping(&mut self) {
        let prg_size = self.common.prg_rom.len();

        if prg_size == 0xC000 && self.vs_type == VsSystemType::VsDualSystem {
            // 48KB == unpadded dual-system ROM. This is the master console.
            // Mesen leaves slot 0 (the slave's bank) unmapped; we map bank 0
            // so a single-console run still has valid code at $8000.
            self.map_prg_8k_page(0, 0);
            self.map_prg_8k_page(1, 0);
            self.map_prg_8k_page(2, 1);
            self.map_prg_8k_page(3, 2);
        } else {
            for slot in 0..4 {
                self.map_prg_8k_page(slot, slot);
            }
        }

        self.select_chr_page(0);
    }

    fn map_prg_8k_page(&mut self, slot: usize, page: usize) {
        let len = self.common.prg_rom.len();
        self.common.map_prg_8k(slot, page_offset(page, 0x2000, len));
    }

    fn select_chr_page(&mut self, page: usize) {
        let chr_len = self.common.chr.len().max(1);
        let base = page_offset(page, 0x2000, chr_len);
        for slot in 0..8 {
            self.common.chr_map[slot] = (base + slot * 0x400) % chr_len;
        }
    }

    pub fn set_dip_switches(&mut self, value: u32) {
        self.dip_switches = value & 0xFF;
    }

    pub fn dip_switches(&self) -> u32 {
        self.dip_switches
    }

    pub fn set_vs_type(&mut self, vs_type: VsSystemType) {
        self.vs_type = vs_type;
        self.apply_initial_mapping();
        self.apply_select_bit();
    }

    pub fn vs_type(&self) -> VsSystemType {
        self.vs_type
    }

    /// Read one of the VS controller ports, merging the DIP switches (and,
    /// for a real frontend, the controller bits supplied by the caller).
    /// Mirrors `VsControlManager::ReadRAM`.
    pub fn read_port(&mut self, addr: u16, controller_value: u8) -> u8 {
        match addr {
            0x4016 => {
                let mut value = controller_value & 0x65;
                if self.dip_switches & 0x01 != 0 {
                    value |= 0x08;
                }
                if self.dip_switches & 0x02 != 0 {
                    value |= 0x10;
                }
                value
            }
            0x4017 => {
                let mut value = controller_value & 0x01;
                value |= (self.dip_switches as u8) & 0xFC;
                value
            }
            _ => 0,
        }
    }

    /// Handles the `$4016` write performed by the VS control manager: the
    /// refresh bit and the PRG/CHR select bit live in that latch.
    pub fn write_control(&mut self, value: u8) {
        self.refresh_state = (value & 0x01) == 0x01;
        self.update_prg_chr_select_bit((value >> 2) & 0x01);
    }

    pub fn set_prg_chr_select_bit(&mut self, bit: u8) {
        self.update_prg_chr_select_bit(bit & 1);
    }

    fn apply_select_bit(&mut self) {
        let bit = self.prg_chr_select_bit;

        if self.vs_type == VsSystemType::Default && self.common.prg_rom.len() > 0x8000 {
            // "Note: In case of games with 40KiB PRG-ROM (as found in VS
            // Gumshoe), the above bit additionally changes 8KiB PRG-ROM at
            // $8000-$9FFF."
            self.map_prg_8k_page(0, (bit as usize) << 2);
        }

        self.select_chr_page(bit as usize);
    }

    fn update_prg_chr_select_bit(&mut self, bit: u8) {
        if self.prg_chr_select_bit == bit {
            return;
        }
        self.prg_chr_select_bit = bit;
        self.apply_select_bit();
    }
}

impl Mapper for VsSystem {
    fn common(&self) -> &MapperCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut MapperCommon {
        &mut self.common
    }

    fn read_input_port(&mut self, addr: u16, controller_bits: u8) -> Option<u8> {
        Some(VsSystem::read_port(self, addr, controller_bits))
    }

    fn write_input_port(&mut self, value: u8) {
        self.write_control(value);
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn Any> {
        Some(self)
    }

    fn reset(&mut self) {
        // BaseMapper::Reset is empty; VsControlManager resets the protection
        // counter. The PRG/CHR select latch survives a soft reset.
        self.protection_counter = 0;
        self.apply_initial_mapping();
        self.apply_select_bit();
    }

    fn cpu_read(&mut self, addr: u16) -> Option<u8> {
        match addr {
            0x4016 | 0x4017 => Some(self.read_port(addr, 0)),
            0x5E00 => {
                self.protection_counter = 0;
                Some(0)
            }
            0x5E01 => {
                let value = match self.vs_type {
                    VsSystemType::TkoBoxingProtection => {
                        VS_PROTECTION_DATA[0][(self.protection_counter & 0x1F) as usize]
                    }
                    VsSystemType::RbiBaseballProtection => {
                        VS_PROTECTION_DATA[1][(self.protection_counter & 0x1F) as usize]
                    }
                    _ => 0,
                };
                self.protection_counter = self.protection_counter.wrapping_add(1);
                Some(value)
            }
            0x6000..=0x7FFF => {
                let index = (addr as usize - 0x6000) % self.common.prg_ram.len().max(1);
                Some(self.common.prg_ram[index])
            }
            0x8000..=0xFFFF => Some(self.common.read_prg_rom(addr)),
            _ => {
                if self.vs_type == VsSystemType::SuperXeviousProtection
                    && (0x4020..=0x5FFF).contains(&addr)
                {
                    let value =
                        VS_PROTECTION_DATA[2][(self.protection_counter & 0x1F) as usize];
                    self.protection_counter = self.protection_counter.wrapping_add(1);
                    Some(value)
                } else {
                    None
                }
            }
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            // The bus currently routes $4016 to the controller strobe, so
            // this is only reached if integration forwards it (see report).
            0x4016 => self.write_control(value),
            0x6000..=0x7FFF => {
                let index = (addr as usize - 0x6000) % self.common.prg_ram.len().max(1);
                self.common.prg_ram[index] = value;
            }
            // VRAM writes are handled by the default CHR/nametable mapping.
            0x8000..=0xFFFF => {}
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
        let len = self.common.prg_ram.len().min(data.len());
        self.common.prg_ram[..len].copy_from_slice(&data[..len]);
    }

    fn save_state(&self, w: &mut StateWriter) {
        self.common.save_state(w);
        w.u8(self.prg_chr_select_bit);
        w.u32(self.protection_counter);
        w.bool(self.refresh_state);
        w.u32(self.dip_switches);
        w.u8(self.vs_type as u8);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.common.load_state(r)?;
        self.prg_chr_select_bit = r.u8()?;
        self.protection_counter = r.u32()?;
        self.refresh_state = r.bool()?;
        self.dip_switches = r.u32()?;
        self.vs_type = VsSystemType::from_u8(r.u8()?);
        Ok(())
    }
}

// ===========================================================================
// Group dispatch
// ===========================================================================

/// IDs handled by this file. 65534/65535 are the private IDs Mesen uses for
/// NSF and FDS; accepting them keeps the loader contract forgiving.
pub fn supports(id: u16) -> bool {
    matches!(id, 20 | 31 | 99 | 65534 | 65535)
}

pub fn create(id: u16, rom: RomData) -> Option<Box<dyn Mapper>> {
    match id {
        20 | 65535 => Some(Box::new(Fds::new(rom))),
        31 => {
            if rom.info.system == RomSystem::Nsf {
                Some(Box::new(NsfMapper::new(rom)))
            } else {
                Some(Box::new(NsfCart31::new(rom)))
            }
        }
        65534 => Some(Box::new(NsfMapper::new(rom))),
        99 => Some(Box::new(VsSystem::new(rom))),
        _ => None,
    }
}

// ===========================================================================
// Unit tests (pure mapper behaviour, no CPU/PPU/APU required)
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::RomInfo;

    fn make_rom(mapper: u16, system: RomSystem, prg: Vec<u8>, chr: Vec<u8>) -> RomData {
        let mut info = RomInfo::default();
        info.mapper = mapper;
        info.system = system;
        info.prg_rom_size = prg.len();
        info.chr_rom_size = chr.len();
        RomData::new(info, prg, chr)
    }

    fn raw_fds_side() -> Vec<u8> {
        let mut side = vec![0u8; FDS_SIDE_CAPACITY];
        side[0] = 1; // disk header block
        side[1] = b'V';
        side[2] = b'S';
        side[3] = b'X';
        side
    }

    #[test]
    fn fds_side_parses_with_gaps() {
        let sides = load_fds_disk_sides(&raw_fds_side());
        assert_eq!(sides.len(), 1);
        assert_eq!(sides[0].len(), FDS_SIDE_CAPACITY);
        let start = 28300 / 8;
        assert_eq!(sides[0][start], 0x80);
        assert_eq!(sides[0][start + 1], 1);
        assert_eq!(sides[0][start + 2], b'V');
    }

    #[test]
    fn fds_without_header_parses_sides() {
        let sides = load_fds_disk_sides(&raw_fds_side());
        assert_eq!(sides.len(), 1);
    }

    #[test]
    fn fds_bios_and_ram_mapping() {
        let mut rom = make_rom(20, RomSystem::Fds, raw_fds_side(), Vec::new());
        rom.fds_bios = Some(vec![0xAB; 0x2000]);
        let mut mapper = Fds::new(rom);

        assert!(mapper.bios_present());
        assert_eq!(mapper.side_count(), 1);
        assert_eq!(mapper.current_disk(), 0);

        assert_eq!(mapper.cpu_read(0xE000), Some(0xAB));
        assert_eq!(mapper.cpu_read(0xFFFC), Some(0xAB));
        // The FDS work RAM covers $6000-$DFFF (32KB).
        assert_eq!(mapper.cpu_read(0x6000), Some(0));
        assert_eq!(mapper.cpu_read(0x8000), Some(0));
        assert_eq!(mapper.cpu_read(0xDFFF), Some(0));

        mapper.cpu_write(0x6000, 0x42);
        assert_eq!(mapper.cpu_read(0x6000), Some(0x42));

        // Disk status: no transfer yet, disk inserted but not scanning.
        assert_eq!(mapper.cpu_read(0x4032), Some(0x02));
        assert_eq!(mapper.cpu_read(0x4030), Some(0x00));
    }

    #[test]
    fn fds_without_bios_does_not_panic() {
        let rom = make_rom(20, RomSystem::Fds, raw_fds_side(), Vec::new());
        let mut mapper = Fds::new(rom);
        assert!(mapper.bios_missing());
        assert_eq!(mapper.cpu_read(0xE000), Some(0xFF));
        assert_eq!(mapper.cpu_read(0xFFFC), Some(0xFF));
    }

    fn nsf_file() -> Vec<u8> {
        let mut data = vec![0u8; 0x80 + 0x1000];
        data[0..5].copy_from_slice(b"NESM\x1a");
        data[5] = 1;
        data[6] = 2; // total songs
        data[7] = 1; // starting song
        data[8..10].copy_from_slice(&0x8000u16.to_le_bytes());
        data[10..12].copy_from_slice(&0x8123u16.to_le_bytes());
        data[12..14].copy_from_slice(&0x8456u16.to_le_bytes());
        data[0x6E..0x70].copy_from_slice(&16639u16.to_le_bytes());
        data[0x78..0x7A].copy_from_slice(&19997u16.to_le_bytes());
        data[0x7A] = 0;
        data[0x7B] = 0;
        data[0x80] = 0x11;
        data[0x81] = 0x22;
        data
    }

    #[test]
    fn nsf_header_and_bank_mapping() {
        let rom = make_rom(31, RomSystem::Nsf, nsf_file(), Vec::new());
        let mut mapper = NsfMapper::new(rom);

        assert_eq!(mapper.init_address(), 0x8123);
        assert_eq!(mapper.play_address(), 0x8456);
        assert_eq!(mapper.current_track(), 0);
        assert_eq!(mapper.track_count(), 2);

        assert_eq!(mapper.cpu_read(0x3E00), Some(0x23));
        assert_eq!(mapper.cpu_read(0x3E01), Some(0x81));
        assert_eq!(mapper.cpu_read(0x3E02), Some(0x56));
        assert_eq!(mapper.cpu_read(0x3E03), Some(0x84));
        assert_eq!(mapper.cpu_read(0x3E10), Some(0));
        assert_eq!(mapper.cpu_read(0xFFFC), Some(0x03));
        assert_eq!(mapper.cpu_read(0xFFFD), Some(0x3F));
        assert_eq!(mapper.cpu_read(0x3F03), Some(0x78));

        // $5FF8 selects the bank for $8000-$8FFF.
        mapper.cpu_write(0x5FF8, 0);
        assert_eq!(mapper.cpu_read(0x8000), Some(0x11));
        assert_eq!(mapper.cpu_read(0x8001), Some(0x22));

        // $6000-$7FFF defaults to RAM (FE -> page 0, FF -> page 1).
        mapper.cpu_write(0x6000, 0x5A);
        assert_eq!(mapper.cpu_read(0x6000), Some(0x5A));
        mapper.cpu_write(0x5FF6, 0xFE);
        mapper.cpu_write(0x5FF7, 0xFF);
        assert_eq!(mapper.cpu_read(0x6000), Some(0x5A));
        mapper.cpu_write(0x7000, 0x77);
        assert_eq!(mapper.cpu_read(0x7000), Some(0x77));
    }

    #[test]
    fn nsf_irq_init_and_play() {
        let rom = make_rom(31, RomSystem::Nsf, nsf_file(), Vec::new());
        let mut mapper = NsfMapper::new(rom);

        // Selecting the starting track arms the init IRQ immediately.
        assert!(mapper.irq_pending());
        mapper.cpu_cycle();
        assert_eq!(mapper.cpu_read(0x3E12), Some(NsfIrqType::Init as u8));
        assert!(!mapper.irq_pending());

        // One cycle consumes the pending need_init flag; then program a short
        // PLAY period (reload = 0x10, counter = reload * 5).
        mapper.cpu_cycle();
        mapper.cpu_read(0x3E12);
        mapper.cpu_write(0x3E10, 0x10);
        mapper.cpu_write(0x3E11, 0x00);
        mapper.cpu_write(0x3E12, 1);
        for _ in 0..(0x10 * 5) {
            mapper.cpu_cycle();
        }
        assert!(mapper.irq_pending());
        assert_eq!(mapper.cpu_read(0x3E12), Some(NsfIrqType::Play as u8));
    }

    #[test]
    fn fds_save_state_roundtrip() {
        let mut rom = make_rom(20, RomSystem::Fds, raw_fds_side(), Vec::new());
        rom.fds_bios = Some(vec![0xCD; 0x2000]);
        let mut mapper = Fds::new(rom);
        mapper.cpu_write(0x4020, 0x34);
        mapper.cpu_write(0x4021, 0x12);
        mapper.cpu_write(0x6000, 0x99);

        let mut w = StateWriter::new();
        mapper.save_state(&mut w);
        let state = w.into_inner();

        mapper.cpu_write(0x4020, 0x00);
        mapper.cpu_write(0x4021, 0x00);
        mapper.cpu_write(0x6000, 0x00);

        let mut r = StateReader::new(&state);
        mapper.load_state(&mut r).unwrap();
        assert_eq!(mapper.irq_reload_value, 0x1234);
        assert_eq!(mapper.cpu_read(0x6000), Some(0x99));
    }

    #[test]
    fn nsf_save_state_roundtrip() {
        let rom = make_rom(31, RomSystem::Nsf, nsf_file(), Vec::new());
        let mut mapper = NsfMapper::new(rom);
        mapper.cpu_write(0x5FF8, 0);
        mapper.cpu_write(0x5FF9, 1);
        mapper.cpu_write(0x7000, 0x42);

        let mut w = StateWriter::new();
        mapper.save_state(&mut w);
        let state = w.into_inner();

        mapper.cpu_write(0x5FF8, 1);
        mapper.cpu_write(0x7000, 0x00);

        let mut r = StateReader::new(&state);
        mapper.load_state(&mut r).unwrap();
        assert_eq!(mapper.cpu_read(0x8000), Some(0x11));
        assert_eq!(mapper.cpu_read(0x7000), Some(0x42));
    }

    #[test]
    fn nsf_cart31_has_eight_4k_windows() {
        let mut prg = vec![0x00; 0x4000];
        prg[0x1000..0x2000].fill(0x11);
        prg[0x2000..0x3000].fill(0x22);
        prg[0x3000..0x4000].fill(0x33);
        let rom = make_rom(31, RomSystem::Nes, prg, Vec::new());
        let mut mapper = NsfCart31::new(rom);

        // InitMapper maps $F000 to the last bank.
        assert_eq!(mapper.cpu_read(0xF000), Some(0x33));
        mapper.cpu_write(0x5FF8, 1);
        mapper.cpu_write(0x5FF9, 2);
        assert_eq!(mapper.cpu_read(0x8000), Some(0x11));
        assert_eq!(mapper.cpu_read(0x9000), Some(0x22));
        // Unwritten windows are open bus, like BaseMapper.
        assert_eq!(mapper.cpu_read(0xA000), None);
    }

    #[test]
    fn vs_dip_switches_and_ram() {
        let prg = vec![0xEA; 0x8000];
        let mut chr = vec![0x11; 0x2000];
        chr.extend(vec![0x22; 0x2000]);
        let rom = make_rom(99, RomSystem::Nes, prg, chr);
        let mut mapper = VsSystem::new(rom);

        mapper.set_dip_switches(0xFF);
        assert_eq!(mapper.read_port(0x4016, 0), 0x18);
        assert_eq!(mapper.read_port(0x4017, 0), 0xFC);
        assert_eq!(mapper.cpu_read(0x4016), Some(0x18));
        assert_eq!(mapper.cpu_read(0x4017), Some(0xFC));

        mapper.cpu_write(0x6000, 0x42);
        assert_eq!(mapper.cpu_read(0x6000), Some(0x42));
        // VS work RAM is 2KB, mirrored over $6000-$7FFF.
        assert_eq!(mapper.cpu_read(0x6800), Some(0x42));

        assert_eq!(mapper.ppu_read(0x0000), Some(0x11));
        mapper.write_control(0x04); // PRG/CHR select bit = 1
        assert_eq!(mapper.ppu_read(0x0000), Some(0x22));
    }

    #[test]
    fn vs_protection_tables() {
        let rom = make_rom(99, RomSystem::Nes, vec![0u8; 0x8000], Vec::new());
        let mut mapper = VsSystem::new(rom);
        assert_eq!(mapper.vs_type(), VsSystemType::Default);
        mapper.set_vs_type(VsSystemType::TkoBoxingProtection);
        assert_eq!(mapper.cpu_read(0x5E00), Some(0));
        assert_eq!(mapper.cpu_read(0x5E01), Some(0xFF));
        mapper.set_vs_type(VsSystemType::SuperXeviousProtection);
        assert_eq!(mapper.cpu_read(0x5E00), Some(0));
        assert_eq!(mapper.cpu_read(0x5000), Some(0x05));
        assert_eq!(mapper.cpu_read(0x5001), Some(0x01));
    }

    #[test]
    fn group_dispatch_ids() {
        assert!(supports(20));
        assert!(supports(31));
        assert!(supports(99));
        assert!(!supports(0));
    }
}
