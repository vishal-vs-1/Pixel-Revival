//! NES APU (2A03): pulse x2, triangle, noise, DMC, frame sequencer and mixer.
//!
//! This is a direct translation of Mesen 0.9.9's APU implementation:
//! `Core/APU.cpp`, `Core/APU.h`, `Core/ApuEnvelope.h`, `Core/ApuLengthCounter.h`,
//! `Core/ApuFrameCounter.h`, `Core/BaseApuChannel.h`, `Core/SquareChannel.h`,
//! `Core/TriangleChannel.h`, `Core/NoiseChannel.h`,
//! `Core/DeltaModulationChannel.cpp/.h` and the non-linear mixer from
//! `Core/SoundMixer.cpp`.
//!
//! Original C++: Copyright (C) 2014-2020 Sour, GPL-3.0.
//! Rust port: GPL-3.0-or-later.
//!
//! Timing model
//! ------------
//! `Bus::tick` calls [`Apu::tick`] once per CPU cycle, so unlike upstream this
//! port clocks the frame sequencer and every channel timer one CPU cycle at a
//! time instead of running them in chunks to a target cycle. The observable
//! ordering matches Mesen's `APU::Run`:
//!
//! 1. advance the frame sequencer one cycle (fires quarter/half-frame clocks),
//! 2. apply pending length-counter reloads (`ReloadCounter`),
//! 3. advance each channel timer by one cycle and clock it when it expires.
//!
//! Audio is decimated with a box filter driven by a fractional accumulator at
//! `sample_rate`, then DC-blocked (37 Hz one-pole high-pass) and clamped to
//! `[-1, 1]`.
//!
//! Known limitation: DMC CPU-DMA stalls are not modelled. This layer fetches
//! DMC sample bytes itself from the cartridge/RAM; the memory reader completes
//! in zero cycles (the fill is serviced on the APU cycle where it becomes
//! possible). The CPU therefore never loses cycles to DMC DMA, so the
//! sprite/DMC DMA interaction tests cannot pass, and the bus interface does not
//! need to change. Everything observable through $4015 (bytes remaining, IRQ,
//! rates, looping) is implemented. A mapper's expansion audio is sampled once
//! per CPU cycle and mixed in at the output sampling point.

use crate::cartridge::Cartridge;
use crate::state::{StateReader, StateResult, StateWriter};
use crate::types::Region;

// ---------------------------------------------------------------------------
// Tables (Mesen Core/ApuLengthCounter.h, SquareChannel.h, TriangleChannel.h,
// NoiseChannel.h, DeltaModulationChannel.cpp, ApuFrameCounter.h)
// ---------------------------------------------------------------------------

/// Length-counter lookup, indexed by the 5-bit length index.
const LENGTH_TABLE: [u8; 32] = [
    10, 254, 20, 2, 40, 4, 80, 6, 160, 8, 60, 10, 14, 12, 26, 14, 12, 16, 24, 18, 48, 20, 96, 22,
    192, 24, 72, 26, 16, 28, 32, 30,
];

/// Pulse duty sequences.
const DUTY_TABLE: [[u8; 8]; 4] = [
    [0, 0, 0, 0, 0, 0, 0, 1],
    [0, 0, 0, 0, 0, 0, 1, 1],
    [0, 0, 0, 0, 1, 1, 1, 1],
    [1, 1, 1, 1, 1, 1, 0, 0],
];

/// Triangle 32-step sequence.
const TRIANGLE_TABLE: [u8; 32] = [
    15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11,
    12, 13, 14, 15,
];

const NOISE_PERIOD_NTSC: [u16; 16] = [
    4, 8, 16, 32, 64, 96, 128, 160, 202, 254, 380, 508, 762, 1016, 2034, 4068,
];
const NOISE_PERIOD_PAL: [u16; 16] = [
    4, 8, 14, 30, 60, 88, 118, 148, 188, 236, 354, 472, 708, 944, 1890, 3778,
];

const DMC_PERIOD_NTSC: [u16; 16] = [
    428, 380, 340, 320, 286, 254, 226, 214, 190, 160, 142, 128, 106, 84, 72, 54,
];
const DMC_PERIOD_PAL: [u16; 16] = [
    398, 354, 316, 298, 276, 236, 210, 198, 176, 148, 132, 118, 98, 78, 66, 50,
];

#[inline]
fn noise_periods(region: Region) -> &'static [u16; 16] {
    match region {
        Region::Pal => &NOISE_PERIOD_PAL,
        _ => &NOISE_PERIOD_NTSC,
    }
}

#[inline]
fn dmc_periods(region: Region) -> &'static [u16; 16] {
    match region {
        Region::Pal => &DMC_PERIOD_PAL,
        _ => &DMC_PERIOD_NTSC,
    }
}

/// Frame sequencer step boundaries (CPU cycles), indexed by
/// `[step_mode][step]` where step_mode 0 is 4-step and 1 is 5-step.
/// Dendy uses NTSC APU timings.
#[inline]
fn frame_step_cycles(region: Region) -> &'static [[i32; 6]; 2] {
    const NTSC: [[i32; 6]; 2] = [
        [7457, 14913, 22371, 29828, 29829, 29830],
        [7457, 14913, 22371, 29829, 37281, 37282],
    ];
    const PAL: [[i32; 6]; 2] = [
        [8313, 16627, 24939, 33252, 33253, 33254],
        [8313, 16627, 24939, 33253, 41565, 41566],
    ];
    match region {
        Region::Pal => &PAL,
        _ => &NTSC,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FrameType {
    None,
    Quarter,
    Half,
}

/// Frame clock events for both modes. Steps 0/2 are quarter frames, 1/4 half
/// frames, 3/5 do nothing (step 4 of 5-step mode is the half frame).
const FRAME_TYPES: [[FrameType; 6]; 2] = [
    [
        FrameType::Quarter,
        FrameType::Half,
        FrameType::Quarter,
        FrameType::None,
        FrameType::Half,
        FrameType::None,
    ],
    [
        FrameType::Quarter,
        FrameType::Half,
        FrameType::Quarter,
        FrameType::None,
        FrameType::Half,
        FrameType::None,
    ],
];

// ---------------------------------------------------------------------------
// Mixer constants
// ---------------------------------------------------------------------------

/// Mesen's mixer constants (`SoundMixer::GetOutputVolume`). Raw mixer output
/// spans roughly `0.0..=5000.0` with all channels at maximum.
const MIXER_SQUARE_SCALE: f64 = 477_600.0;
const MIXER_TND_SCALE: f64 = 818_350.0;
const MIXER_TND_DIVISOR: f64 = 24_329.0;

/// Raw mixer full-scale (~5000) mapped to just under 0 dBFS so the mixer does
/// not clip before expansion audio is added.
const NES_MIX_SCALE: f64 = 1.0 / 6144.0;

/// Gain applied to `Cartridge::expansion_audio_sample()` (contract: -1.0..=1.0).
/// 0.5 is in the same ballpark as Mesen's VRC6/MMC5/FDS gains relative to the
/// 2A03 mixer.
const EXPANSION_GAIN: f32 = 0.5;

/// One-pole DC blocker cutoff (the NES output stage has a ~37 Hz high-pass).
const DC_CUTOFF_HZ: f64 = 37.0;

/// Mesen's `SilenceTriangleHighFreq` option is off by default; keep that
/// behaviour (triangle keeps clocking, even at ultrasonic periods).
const SILENCE_TRIANGLE_HIGH_FREQ: bool = false;

// ---------------------------------------------------------------------------
// Length counter (ApuLengthCounter.h)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Default)]
struct LengthCounter {
    enabled: bool,
    halt: bool,
    /// Value latched by a $4003/$4007/$400B/$400F write (`_newHaltValue`).
    new_halt: bool,
    value: u8,
    reload_value: u8,
    previous_value: u8,
}

impl LengthCounter {
    /// `ApuLengthCounter::Reset`. `is_triangle` mirrors the upstream behaviour
    /// where a soft reset leaves the triangle's length counter/halt untouched.
    fn reset(&mut self, is_triangle: bool) {
        self.enabled = false;
        if !is_triangle {
            self.halt = false;
            self.value = 0;
            self.new_halt = false;
            self.reload_value = 0;
            self.previous_value = 0;
        }
    }

    /// `InitializeLengthCounter`.
    fn init_halt(&mut self, halt: bool) {
        self.new_halt = halt;
    }

    /// `LoadLengthCounter`.
    fn load_length(&mut self, index: u8) {
        if self.enabled {
            self.reload_value = LENGTH_TABLE[(index & 0x1F) as usize];
            self.previous_value = self.value;
        }
    }

    /// `ReloadCounter`, called after the frame sequencer runs so a length
    /// counter clocked on the same cycle takes precedence (len_reload_timing).
    fn reload(&mut self) {
        if self.reload_value != 0 {
            if self.value == self.previous_value {
                self.value = self.reload_value;
            }
            self.reload_value = 0;
        }
        self.halt = self.new_halt;
    }

    fn tick(&mut self) {
        if self.value > 0 && !self.halt {
            self.value -= 1;
        }
    }

    fn set_enabled(&mut self, enabled: bool) {
        if !enabled {
            self.value = 0;
        }
        self.enabled = enabled;
    }

    fn status(&self) -> bool {
        self.value > 0
    }

    fn save_state(&self, w: &mut StateWriter) {
        w.bool(self.enabled);
        w.bool(self.halt);
        w.bool(self.new_halt);
        w.u8(self.value);
        w.u8(self.reload_value);
        w.u8(self.previous_value);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.enabled = r.bool()?;
        self.halt = r.bool()?;
        self.new_halt = r.bool()?;
        self.value = r.u8()?;
        self.reload_value = r.u8()?;
        self.previous_value = r.u8()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Envelope (ApuEnvelope.h)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Default)]
struct Envelope {
    constant_volume: bool,
    volume: u8,
    start: bool,
    divider: i8,
    counter: u8,
}

impl Envelope {
    fn reset(&mut self) {
        *self = Self::default();
    }

    /// `InitializeEnvelope`.
    fn init(&mut self, reg_value: u8) {
        self.constant_volume = reg_value & 0x10 == 0x10;
        self.volume = reg_value & 0x0F;
    }

    /// `ResetEnvelope` (the envelope start flag).
    fn restart(&mut self) {
        self.start = true;
    }

    /// `GetVolume`: the envelope is muted while the length counter is zero.
    fn current_volume(&self, length_active: bool) -> u8 {
        if length_active {
            if self.constant_volume {
                self.volume
            } else {
                self.counter
            }
        } else {
            0
        }
    }

    /// `TickEnvelope`, clocked on quarter frames.
    fn tick(&mut self, length_halt: bool) {
        if !self.start {
            self.divider -= 1;
            if self.divider < 0 {
                self.divider = self.volume as i8;
                if self.counter > 0 {
                    self.counter -= 1;
                } else if length_halt {
                    self.counter = 15;
                }
            }
        } else {
            self.start = false;
            self.counter = 15;
            self.divider = self.volume as i8;
        }
    }

    fn save_state(&self, w: &mut StateWriter) {
        w.bool(self.constant_volume);
        w.u8(self.volume);
        w.bool(self.start);
        w.u8(self.divider as u8);
        w.u8(self.counter);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.constant_volume = r.bool()?;
        self.volume = r.u8()?;
        self.start = r.bool()?;
        self.divider = r.u8()? as i8;
        self.counter = r.u8()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Pulse channels (SquareChannel.h)
// ---------------------------------------------------------------------------

struct Pulse {
    is_channel1: bool,

    // BaseApuChannel
    timer: u16,
    /// Timer reload. Effective clock interval is `period + 1` CPU cycles.
    period: u16,
    output: u8,

    length: LengthCounter,
    envelope: Envelope,

    duty: u8,
    duty_pos: u8,
    real_period: u16,

    sweep_enabled: bool,
    sweep_period: u8,
    sweep_negate: bool,
    sweep_shift: u8,
    reload_sweep: bool,
    sweep_divider: u8,
    sweep_target_period: u32,
}

impl Pulse {
    fn new(is_channel1: bool) -> Self {
        Self {
            is_channel1,
            timer: 0,
            period: 0,
            output: 0,
            length: LengthCounter::default(),
            envelope: Envelope::default(),
            duty: 0,
            duty_pos: 0,
            real_period: 0,
            sweep_enabled: false,
            sweep_period: 0,
            sweep_negate: false,
            sweep_shift: 0,
            reload_sweep: false,
            sweep_divider: 0,
            sweep_target_period: 0,
        }
    }

    /// `SquareChannel::Reset`.
    fn reset(&mut self) {
        self.timer = 0;
        self.period = 0;
        self.output = 0;
        self.length.reset(false);
        self.envelope.reset();
        self.duty = 0;
        self.duty_pos = 0;
        self.real_period = 0;
        self.sweep_enabled = false;
        self.sweep_period = 0;
        self.sweep_negate = false;
        self.sweep_shift = 0;
        self.reload_sweep = false;
        self.sweep_divider = 0;
        self.sweep_target_period = 0;
        self.update_target_period();
    }

    fn update_target_period(&mut self) {
        let shift_result = (self.real_period >> self.sweep_shift) as i32;
        if self.sweep_negate {
            let mut target = (self.real_period as i32 - shift_result) as u32;
            if self.is_channel1 {
                // Pulse 1 subtracts one extra from the shifted value.
                target = target.wrapping_sub(1);
            }
            self.sweep_target_period = target;
        } else {
            self.sweep_target_period = self.real_period as u32 + shift_result as u32;
        }
    }

    fn set_period(&mut self, new_period: u16) {
        self.real_period = new_period;
        self.period = self.real_period.wrapping_mul(2).wrapping_add(1);
        self.update_target_period();
    }

    fn init_sweep(&mut self, reg_value: u8) {
        self.sweep_enabled = reg_value & 0x80 == 0x80;
        self.sweep_negate = reg_value & 0x08 == 0x08;
        // The divider's period is set to P + 1.
        self.sweep_period = ((reg_value & 0x70) >> 4) + 1;
        self.sweep_shift = reg_value & 0x07;
        self.update_target_period();
        // Side effect: sets the reload flag.
        self.reload_sweep = true;
    }

    fn is_muted(&self) -> bool {
        // A period < 8, either explicit or via a sweep update, silences the
        // pulse; an overflow target also mutes it (except with negate).
        self.real_period < 8 || (!self.sweep_negate && self.sweep_target_period > 0x7FF)
    }

    fn update_output(&mut self) {
        if self.is_muted() {
            self.output = 0;
        } else {
            self.output = DUTY_TABLE[(self.duty & 3) as usize][(self.duty_pos & 7) as usize]
                * self.envelope.current_volume(self.length.status());
        }
    }

    /// `BaseApuChannel::Run` for one CPU cycle; returns true when the channel
    /// timer expires and `clock` must be called.
    #[inline]
    fn tick_timer(&mut self) -> bool {
        if self.timer == 0 {
            self.timer = self.period;
            true
        } else {
            self.timer -= 1;
            false
        }
    }

    fn clock(&mut self) {
        self.duty_pos = (self.duty_pos.wrapping_sub(1)) & 0x07;
        self.update_output();
    }

    /// `ApuLengthCounter::ReloadCounter`.
    fn reload_counters(&mut self) {
        self.length.reload();
    }

    fn write(&mut self, offset: u16, value: u8) {
        match offset & 0x03 {
            0 => {
                // $4000 / $4004
                self.length.init_halt(value & 0x20 == 0x20);
                self.envelope.init(value);
                self.duty = (value & 0xC0) >> 6;
            }
            1 => {
                // $4001 / $4005
                self.init_sweep(value);
            }
            2 => {
                // $4002 / $4006
                self.set_period((self.real_period & 0x0700) | value as u16);
            }
            3 => {
                // $4003 / $4007
                self.length.load_length(value >> 3);
                self.set_period((self.real_period & 0x00FF) | (((value & 0x07) as u16) << 8));
                // The sequencer restarts at the first value of the sequence.
                self.duty_pos = 0;
                self.envelope.restart();
            }
            _ => {}
        }
        self.update_output();
    }

    /// `ApuEnvelope::TickEnvelope` plus length counter clocking for half frames.
    fn tick_envelope(&mut self) {
        self.envelope.tick(self.length.halt);
    }

    fn tick_length_counter(&mut self) {
        self.length.tick();
    }

    /// `SquareChannel::TickSweep`.
    fn tick_sweep(&mut self) {
        self.sweep_divider = self.sweep_divider.wrapping_sub(1);
        if self.sweep_divider == 0 {
            if self.sweep_shift > 0
                && self.sweep_enabled
                && self.real_period >= 8
                && self.sweep_target_period <= 0x7FF
            {
                self.set_period(self.sweep_target_period as u16);
            }
            self.sweep_divider = self.sweep_period;
        }

        if self.reload_sweep {
            self.sweep_divider = self.sweep_period;
            self.reload_sweep = false;
        }
    }

    fn status(&self) -> bool {
        self.length.status()
    }

    fn save_state(&self, w: &mut StateWriter) {
        w.u16(self.timer);
        w.u16(self.period);
        w.u8(self.output);
        self.length.save_state(w);
        self.envelope.save_state(w);
        w.u8(self.duty);
        w.u8(self.duty_pos);
        w.u16(self.real_period);
        w.bool(self.sweep_enabled);
        w.u8(self.sweep_period);
        w.bool(self.sweep_negate);
        w.u8(self.sweep_shift);
        w.bool(self.reload_sweep);
        w.u8(self.sweep_divider);
        w.u32(self.sweep_target_period);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.timer = r.u16()?;
        self.period = r.u16()?;
        self.output = r.u8()?;
        self.length.load_state(r)?;
        self.envelope.load_state(r)?;
        self.duty = r.u8()?;
        self.duty_pos = r.u8()?;
        self.real_period = r.u16()?;
        self.sweep_enabled = r.bool()?;
        self.sweep_period = r.u8()?;
        self.sweep_negate = r.bool()?;
        self.sweep_shift = r.u8()?;
        self.reload_sweep = r.bool()?;
        self.sweep_divider = r.u8()?;
        self.sweep_target_period = r.u32()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Triangle channel (TriangleChannel.h)
// ---------------------------------------------------------------------------

struct Triangle {
    timer: u16,
    period: u16,
    output: u8,

    length: LengthCounter,
    linear_counter: u8,
    linear_counter_reload: u8,
    linear_reload_flag: bool,
    linear_control_flag: bool,
    sequence_position: u8,
}

impl Triangle {
    fn new() -> Self {
        Self {
            timer: 0,
            period: 0,
            output: 0,
            length: LengthCounter::default(),
            linear_counter: 0,
            linear_counter_reload: 0,
            linear_reload_flag: false,
            linear_control_flag: false,
            sequence_position: 0,
        }
    }

    fn reset(&mut self) {
        self.timer = 0;
        self.period = 0;
        self.output = 0;
        self.length.reset(true);
        self.linear_counter = 0;
        self.linear_counter_reload = 0;
        self.linear_reload_flag = false;
        self.linear_control_flag = false;
        self.sequence_position = 0;
    }

    #[inline]
    fn tick_timer(&mut self) -> bool {
        if self.timer == 0 {
            self.timer = self.period;
            true
        } else {
            self.timer -= 1;
            false
        }
    }

    fn clock(&mut self) {
        // The sequencer only runs while both counters are non-zero.
        if self.length.value > 0 && self.linear_counter > 0 {
            self.sequence_position = (self.sequence_position + 1) & 0x1F;
            if self.period >= 2 || !SILENCE_TRIANGLE_HIGH_FREQ {
                self.output = TRIANGLE_TABLE[self.sequence_position as usize];
            }
        }
    }

    fn write(&mut self, offset: u16, value: u8) {
        match offset & 0x03 {
            0 => {
                // $4008
                self.linear_control_flag = value & 0x80 == 0x80;
                self.linear_counter_reload = value & 0x7F;
                self.length.init_halt(self.linear_control_flag);
            }
            2 => {
                // $400A
                self.period = (self.period & !0x00FF) | value as u16;
            }
            3 => {
                // $400B
                self.length.load_length(value >> 3);
                self.period = (self.period & !0xFF00) | (((value & 0x07) as u16) << 8);
                // Side effect: sets the linear counter reload flag.
                self.linear_reload_flag = true;
            }
            _ => {}
        }
    }

    fn tick_linear_counter(&mut self) {
        if self.linear_reload_flag {
            self.linear_counter = self.linear_counter_reload;
        } else if self.linear_counter > 0 {
            self.linear_counter -= 1;
        }

        if !self.linear_control_flag {
            self.linear_reload_flag = false;
        }
    }

    fn reload_counters(&mut self) {
        self.length.reload();
    }

    fn tick_length_counter(&mut self) {
        self.length.tick();
    }

    fn status(&self) -> bool {
        self.length.status()
    }

    fn save_state(&self, w: &mut StateWriter) {
        w.u16(self.timer);
        w.u16(self.period);
        w.u8(self.output);
        self.length.save_state(w);
        w.u8(self.linear_counter);
        w.u8(self.linear_counter_reload);
        w.bool(self.linear_reload_flag);
        w.bool(self.linear_control_flag);
        w.u8(self.sequence_position);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.timer = r.u16()?;
        self.period = r.u16()?;
        self.output = r.u8()?;
        self.length.load_state(r)?;
        self.linear_counter = r.u8()?;
        self.linear_counter_reload = r.u8()?;
        self.linear_reload_flag = r.bool()?;
        self.linear_control_flag = r.bool()?;
        self.sequence_position = r.u8()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Noise channel (NoiseChannel.h)
// ---------------------------------------------------------------------------

struct Noise {
    timer: u16,
    period: u16,
    output: u8,

    length: LengthCounter,
    envelope: Envelope,

    shift_register: u16,
    mode_flag: bool,
    rate_index: u8,
}

impl Noise {
    fn new(region: Region) -> Self {
        Self {
            timer: 0,
            period: noise_periods(region)[0] - 1,
            output: 0,
            length: LengthCounter::default(),
            envelope: Envelope::default(),
            shift_register: 1,
            mode_flag: false,
            rate_index: 0,
        }
    }

    fn reset(&mut self, region: Region) {
        self.timer = 0;
        self.period = noise_periods(region)[0] - 1;
        self.output = 0;
        self.length.reset(false);
        self.envelope.reset();
        self.shift_register = 1;
        self.mode_flag = false;
        self.rate_index = 0;
    }

    fn apply_region(&mut self, region: Region) {
        self.period = noise_periods(region)[self.rate_index as usize] - 1;
    }

    #[inline]
    fn tick_timer(&mut self) -> bool {
        if self.timer == 0 {
            self.timer = self.period;
            true
        } else {
            self.timer -= 1;
            false
        }
    }

    fn clock(&mut self) {
        // Feedback is bit 0 XOR bit 6 (short mode) or bit 1 (long mode).
        let feedback = (self.shift_register & 0x01)
            ^ ((self.shift_register >> if self.mode_flag { 6 } else { 1 }) & 0x01);
        self.shift_register >>= 1;
        self.shift_register |= feedback << 14;

        if self.shift_register & 0x01 == 0x01 {
            self.output = 0;
        } else {
            self.output = self.envelope.current_volume(self.length.status());
        }
    }

    fn write(&mut self, offset: u16, value: u8, region: Region) {
        match offset & 0x03 {
            0 => {
                // $400C
                self.length.init_halt(value & 0x20 == 0x20);
                self.envelope.init(value);
            }
            2 => {
                // $400E
                self.rate_index = value & 0x0F;
                self.period = noise_periods(region)[self.rate_index as usize] - 1;
                self.mode_flag = value & 0x80 == 0x80;
            }
            3 => {
                // $400F
                self.length.load_length(value >> 3);
                self.envelope.restart();
            }
            _ => {}
        }
    }

    fn reload_counters(&mut self) {
        self.length.reload();
    }

    fn tick_envelope(&mut self) {
        self.envelope.tick(self.length.halt);
    }

    fn tick_length_counter(&mut self) {
        self.length.tick();
    }

    fn status(&self) -> bool {
        self.length.status()
    }

    fn save_state(&self, w: &mut StateWriter) {
        w.u16(self.timer);
        w.u16(self.period);
        w.u8(self.output);
        self.length.save_state(w);
        self.envelope.save_state(w);
        w.u16(self.shift_register);
        w.bool(self.mode_flag);
        w.u8(self.rate_index);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.timer = r.u16()?;
        self.period = r.u16()?;
        self.output = r.u8()?;
        self.length.load_state(r)?;
        self.envelope.load_state(r)?;
        self.shift_register = r.u16()?;
        self.mode_flag = r.bool()?;
        self.rate_index = r.u8()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Delta modulation channel (DeltaModulationChannel.cpp)
// ---------------------------------------------------------------------------

struct Dmc {
    timer: u16,
    period: u16,
    output_level: u8,

    sample_addr: u16,
    sample_length: u16,
    irq_enabled: bool,
    loop_flag: bool,

    current_addr: u16,
    bytes_remaining: u16,
    read_buffer: u8,
    buffer_empty: bool,

    shift_register: u8,
    bits_remaining: u8,
    silence_flag: bool,
    rate_index: u8,

    /// DMC interrupt latch (cleared by $4015 writes, $4010 with IRQ disabled).
    irq: bool,
}

impl Dmc {
    fn new(region: Region) -> Self {
        Self {
            timer: dmc_periods(region)[0] - 1,
            period: dmc_periods(region)[0] - 1,
            output_level: 0,
            // Power-on sample address/length (not affected by reset).
            sample_addr: 0xC000,
            sample_length: 1,
            irq_enabled: false,
            loop_flag: false,
            current_addr: 0,
            bytes_remaining: 0,
            read_buffer: 0,
            buffer_empty: true,
            shift_register: 0,
            bits_remaining: 8,
            silence_flag: true,
            rate_index: 0,
            irq: false,
        }
    }

    /// `DeltaModulationChannel::Reset` (always the soft-reset variant: sample
    /// address/length survive a reset, matching the hardware).
    fn reset(&mut self, region: Region) {
        self.output_level = 0;
        self.irq_enabled = false;
        self.loop_flag = false;
        self.current_addr = 0;
        self.bytes_remaining = 0;
        self.read_buffer = 0;
        self.buffer_empty = true;
        self.shift_register = 0;
        self.bits_remaining = 8;
        self.silence_flag = true;
        self.rate_index = 0;
        self.irq = false;

        self.period = dmc_periods(region)[0] - 1;
        // Make sure the DMC doesn't tick on the first cycle.
        self.timer = self.period;
    }

    fn apply_region(&mut self, region: Region) {
        self.period = dmc_periods(region)[self.rate_index as usize] - 1;
    }

    fn init_sample(&mut self) {
        self.current_addr = self.sample_addr;
        self.bytes_remaining = self.sample_length;
    }

    /// Fill the one-byte sample buffer if it is empty and bytes remain.
    ///
    /// This layer has no CPU/DMA stall, so the memory read completes
    /// immediately (on the CPU cycle the fill is serviced). This is the
    /// behaviour observable through $4015; the CPU-cycle distortion of DMC DMA
    /// is not modelled.
    fn service_fetch(&mut self, cart: &mut Cartridge, ram: &[u8]) {
        if !self.buffer_empty || self.bytes_remaining == 0 {
            return;
        }
        self.fetch_sample(cart, ram);
    }

    /// Read one sample byte. DMC addresses in $8000-$FFFF come from the
    /// cartridge; $0000-$1FFF mirrors the 2KB CPU RAM. Other ranges would need
    /// the full bus but cannot occur because sample addresses are forced into
    /// $C000-$FFC0.
    fn fetch_sample(&mut self, cart: &mut Cartridge, ram: &[u8]) {
        if self.bytes_remaining == 0 {
            return;
        }

        let addr = self.current_addr;
        let value = if addr >= 0x8000 {
            cart.cpu_read(addr).unwrap_or(0)
        } else if addr < 0x2000 {
            ram[(addr as usize) & 0x7FF]
        } else {
            0
        };

        self.read_buffer = value;
        self.buffer_empty = false;

        // The address is incremented; $FFFF wraps to $8000.
        self.current_addr = self.current_addr.wrapping_add(1);
        if self.current_addr == 0 {
            self.current_addr = 0x8000;
        }

        self.bytes_remaining -= 1;
        if self.bytes_remaining == 0 {
            if self.loop_flag {
                // A looped sample never sets the IRQ flag.
                self.init_sample();
            } else if self.irq_enabled {
                self.irq = true;
            }
        }
    }

    /// Per-cycle bookkeeping. The buffer is normally filled from `clock` (when
    /// a byte is consumed) or on the first cycle after a $4015 enable.
    fn tick_control(&mut self, cart: &mut Cartridge, ram: &[u8]) {
        self.service_fetch(cart, ram);
    }

    #[inline]
    fn tick_timer(&mut self) -> bool {
        if self.timer == 0 {
            self.timer = self.period;
            true
        } else {
            self.timer -= 1;
            false
        }
    }

    fn clock(&mut self) {
        if self.bits_remaining == 0 {
            self.bits_remaining = 8;
        }
        if !self.silence_flag {
            if self.shift_register & 0x01 == 0x01 {
                if self.output_level <= 125 {
                    self.output_level += 2;
                }
            } else if self.output_level >= 2 {
                self.output_level -= 2;
            }
            self.shift_register >>= 1;
        }

        self.bits_remaining -= 1;
        if self.bits_remaining == 0 {
            self.bits_remaining = 8;
            if self.buffer_empty {
                self.silence_flag = true;
            } else {
                self.silence_flag = false;
                self.shift_register = self.read_buffer;
                self.buffer_empty = true;
            }
        }
    }

    fn write(&mut self, offset: u16, value: u8, region: Region) {
        match offset & 0x03 {
            0 => {
                // $4010
                self.irq_enabled = value & 0x80 == 0x80;
                self.loop_flag = value & 0x40 == 0x40;
                self.rate_index = value & 0x0F;
                self.period = dmc_periods(region)[self.rate_index as usize] - 1;
                if !self.irq_enabled {
                    self.irq = false;
                }
            }
            1 => {
                // $4011: output level applies immediately.
                self.output_level = value & 0x7F;
            }
            2 => {
                // $4012: sample address = $C000 | (value << 6).
                self.sample_addr = 0xC000 | ((value as u16) << 6);
            }
            3 => {
                // $4013: sample length = (value << 4) | 1 bytes.
                self.sample_length = ((value as u16) << 4) | 0x0001;
            }
            _ => {}
        }
    }

    fn set_enabled(&mut self, enabled: bool) {
        if !enabled {
            self.bytes_remaining = 0;
        } else if self.bytes_remaining == 0 {
            // The sample is restarted; the buffer is filled on the next APU
            // cycle (this layer cannot read memory during a register write).
            self.init_sample();
        }
    }

    fn status(&self) -> bool {
        self.bytes_remaining > 0
    }

    fn save_state(&self, w: &mut StateWriter) {
        w.u16(self.timer);
        w.u16(self.period);
        w.u8(self.output_level);
        w.u16(self.sample_addr);
        w.u16(self.sample_length);
        w.bool(self.irq_enabled);
        w.bool(self.loop_flag);
        w.u16(self.current_addr);
        w.u16(self.bytes_remaining);
        w.u8(self.read_buffer);
        w.bool(self.buffer_empty);
        w.u8(self.shift_register);
        w.u8(self.bits_remaining);
        w.bool(self.silence_flag);
        w.u8(self.rate_index);
        w.bool(self.irq);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.timer = r.u16()?;
        self.period = r.u16()?;
        self.output_level = r.u8()?;
        self.sample_addr = r.u16()?;
        self.sample_length = r.u16()?;
        self.irq_enabled = r.bool()?;
        self.loop_flag = r.bool()?;
        self.current_addr = r.u16()?;
        self.bytes_remaining = r.u16()?;
        self.read_buffer = r.u8()?;
        self.buffer_empty = r.bool()?;
        self.shift_register = r.u8()?;
        self.bits_remaining = r.u8()?;
        self.silence_flag = r.bool()?;
        self.rate_index = r.u8()?;
        self.irq = r.bool()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Frame sequencer (ApuFrameCounter.h)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct FrameCounter {
    step_mode: u8, // 0: 4-step, 1: 5-step
    current_step: u8,
    previous_cycle: i32,
    inhibit_irq: bool,
    block_tick: u8,
    new_value: i16, // -1: no pending write
    write_delay: i8,
}

impl FrameCounter {
    fn new() -> Self {
        let mut fc = Self {
            step_mode: 0,
            current_step: 0,
            previous_cycle: 0,
            inhibit_irq: false,
            block_tick: 0,
            new_value: -1,
            write_delay: -1,
        };
        fc.reset();
        fc
    }

    /// `ApuFrameCounter::Reset`: a soft reset keeps the current $4017 mode but
    /// emulates a $00/$80 write 3 cycles later.
    fn reset(&mut self) {
        self.previous_cycle = 0;
        self.current_step = 0;
        self.new_value = if self.step_mode != 0 { 0x80 } else { 0x00 };
        self.write_delay = 3;
        self.inhibit_irq = false;
        self.block_tick = 0;
    }

    /// Advance one CPU cycle. Returns `(set_frame_irq, frame_clock_event)`.
    fn tick(&mut self, region: Region) -> (bool, FrameType) {
        let mut set_irq = false;
        let mut frame_type = FrameType::None;

        self.previous_cycle += 1;
        let steps = frame_step_cycles(region);
        let target = steps[self.step_mode as usize][self.current_step as usize];

        if self.previous_cycle >= target {
            // The frame IRQ is raised on the last three cycles of 4-step mode.
            if !self.inhibit_irq && self.step_mode == 0 && self.current_step >= 3 {
                set_irq = true;
            }

            let ty = FRAME_TYPES[self.step_mode as usize][self.current_step as usize];
            if ty != FrameType::None && self.block_tick == 0 {
                frame_type = ty;
                // Block $4017-triggered clocks for this and the next cycle.
                self.block_tick = 2;
            }

            self.previous_cycle = target;
            self.current_step += 1;
            if self.current_step == 6 {
                self.current_step = 0;
                self.previous_cycle = 0;
            }
        }

        if self.new_value >= 0 {
            self.write_delay -= 1;
            if self.write_delay == 0 {
                // Apply the new value after the appropriate delay.
                self.step_mode = if self.new_value & 0x80 != 0 { 1 } else { 0 };
                self.write_delay = -1;
                self.current_step = 0;
                self.previous_cycle = 0;
                self.new_value = -1;

                if self.step_mode == 1 && self.block_tick == 0 {
                    // Bit 7 immediately clocks quarter and half frame units.
                    frame_type = FrameType::Half;
                    self.block_tick = 2;
                }
            }
        }

        if self.block_tick > 0 {
            self.block_tick -= 1;
        }

        (set_irq, frame_type)
    }

    /// `ApuFrameCounter::WriteRAM` ($4017). `odd_cycle` is the CPU cycle parity.
    fn write(&mut self, value: u8, odd_cycle: bool) {
        self.new_value = value as i16;
        self.write_delay = if odd_cycle { 4 } else { 3 };
        self.inhibit_irq = value & 0x40 == 0x40;
    }

    fn save_state(&self, w: &mut StateWriter) {
        w.u8(self.step_mode);
        w.u8(self.current_step);
        w.i32(self.previous_cycle);
        w.bool(self.inhibit_irq);
        w.u8(self.block_tick);
        w.i16(self.new_value);
        w.u8(self.write_delay as u8);
    }

    fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.step_mode = r.u8()?;
        self.current_step = r.u8()?;
        self.previous_cycle = r.i32()?;
        self.inhibit_irq = r.bool()?;
        self.block_tick = r.u8()?;
        self.new_value = r.i16()?;
        self.write_delay = r.u8()? as i8;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// APU
// ---------------------------------------------------------------------------

pub struct Apu {
    sample_rate: u32,
    master_volume: f32,
    region: Region,
    region_synced: bool,
    /// Number of APU (CPU) cycles elapsed; used for the $4017 write-delay
    /// parity and preserved across save states so timing stays consistent.
    total_cycles: u64,

    pulse1: Pulse,
    pulse2: Pulse,
    triangle: Triangle,
    noise: Noise,
    dmc: Dmc,
    frame_counter: FrameCounter,

    frame_irq: bool,

    // Resampler / output stage.
    sample_phase: f64,
    samples_per_cycle: f64,
    mix_accum: f64,
    expansion_accum: f64,
    sample_cycles: u32,
    dc_x: f32,
    dc_y: f32,
    dc_coeff: f32,
    samples: Vec<f32>,
}

impl Apu {
    pub fn new(sample_rate: u32) -> Self {
        let mut apu = Self {
            sample_rate: sample_rate.max(1),
            master_volume: 1.0,
            region: Region::Ntsc,
            region_synced: false,
            total_cycles: 0,
            pulse1: Pulse::new(true),
            pulse2: Pulse::new(false),
            triangle: Triangle::new(),
            noise: Noise::new(Region::Ntsc),
            dmc: Dmc::new(Region::Ntsc),
            frame_counter: FrameCounter::new(),
            frame_irq: false,
            sample_phase: 0.0,
            samples_per_cycle: 0.0,
            mix_accum: 0.0,
            expansion_accum: 0.0,
            sample_cycles: 0,
            dc_x: 0.0,
            dc_y: 0.0,
            dc_coeff: 0.0,
            samples: Vec::new(),
        };
        apu.update_timing();
        apu
    }

    /// Soft reset, matching `APU::Reset(true)` + the channel resets. The
    /// $4017 step mode is retained and the frame sequencer starts as if $00
    /// (or $80 in 5-step mode) were written, three cycles later.
    pub fn reset(&mut self) {
        self.pulse1.reset();
        self.pulse2.reset();
        self.triangle.reset();
        self.noise.reset(self.region);
        self.dmc.reset(self.region);
        self.frame_counter.reset();
        self.frame_irq = false;
        self.total_cycles = 0;

        self.sample_phase = 0.0;
        self.mix_accum = 0.0;
        self.expansion_accum = 0.0;
        self.sample_cycles = 0;
        self.dc_x = 0.0;
        self.dc_y = 0.0;
        // Drop any undrained output, like Mesen's mixer reset.
        self.samples.clear();
        self.region_synced = false;
    }

    pub fn set_sample_rate(&mut self, rate: u32) {
        self.sample_rate = rate.max(1);
        self.update_timing();
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Master output volume, 0.0..=2.0 (default 1.0).
    pub fn master_volume(&self) -> f32 {
        self.master_volume
    }

    pub fn set_master_volume(&mut self, volume: f32) {
        self.master_volume = volume.clamp(0.0, 2.0);
    }

    /// Override the timing region (normally taken from the cartridge on the
    /// first tick). Dendy uses NTSC APU tables.
    pub fn set_region(&mut self, region: Region) {
        if self.region == region {
            return;
        }
        self.region = region;
        self.noise.apply_region(region);
        self.dmc.apply_region(region);
        self.update_timing();
    }

    fn update_timing(&mut self) {
        let cpu_clock = self.region.cpu_clock().max(1) as f64;
        // A sample rate above the CPU clock cannot produce more than one sample
        // per cycle; clamping keeps the resampler well behaved.
        self.samples_per_cycle = (self.sample_rate as f64 / cpu_clock).clamp(0.0, 1.0);
        let rate = self.sample_rate.max(1) as f64;
        self.dc_coeff = (-2.0 * std::f64::consts::PI * DC_CUTOFF_HZ / rate).exp() as f32;
    }

    /// Called once per CPU cycle.
    pub fn tick(&mut self, cart: &mut Cartridge, ram: &[u8]) {
        if !self.region_synced {
            let region = cart.info().region;
            self.region = region;
            self.noise.apply_region(region);
            self.dmc.apply_region(region);
            self.update_timing();
            self.region_synced = true;
        }

        self.total_cycles = self.total_cycles.wrapping_add(1);

        // 1. Frame sequencer.
        let (set_frame_irq, frame_type) = self.frame_counter.tick(self.region);
        if set_frame_irq {
            self.frame_irq = true;
        }
        match frame_type {
            FrameType::Quarter => self.clock_quarter_frame(),
            FrameType::Half => self.clock_half_frame(),
            FrameType::None => {}
        }

        // 2. Reload counters latched by $4003/$4007/$400B/$400F writes. This is
        //    done after the frame sequencer so a length clock wins on the same
        //    cycle (len_reload_timing).
        self.pulse1.reload_counters();
        self.pulse2.reload_counters();
        self.triangle.reload_counters();
        self.noise.reload_counters();

        // 3. Channel timers, one CPU cycle.
        if self.pulse1.tick_timer() {
            self.pulse1.clock();
        }
        if self.pulse2.tick_timer() {
            self.pulse2.clock();
        }
        if self.triangle.tick_timer() {
            self.triangle.clock();
        }
        if self.noise.tick_timer() {
            self.noise.clock();
        }

        self.dmc.tick_control(cart, ram);
        if self.dmc.tick_timer() {
            self.dmc.clock();
            // A completed byte emptied the buffer: fill it on this same cycle
            // (zero-cycle memory reader, see the module docs).
            self.dmc.tick_control(cart, ram);
        }

        // 4. Mix at the output sampling point. Mappers clock their expansion
        //    chips in Cartridge::cpu_cycle(), before this call.
        let expansion = cart.expansion_audio_sample();
        self.accumulate_output(expansion);
    }

    fn clock_quarter_frame(&mut self) {
        self.pulse1.tick_envelope();
        self.pulse2.tick_envelope();
        self.triangle.tick_linear_counter();
        self.noise.tick_envelope();
    }

    fn clock_half_frame(&mut self) {
        self.clock_quarter_frame();

        // Length counters and pulse sweep units are clocked on half frames.
        self.pulse1.tick_length_counter();
        self.pulse2.tick_length_counter();
        self.triangle.tick_length_counter();
        self.noise.tick_length_counter();

        self.pulse1.tick_sweep();
        self.pulse2.tick_sweep();
    }

    /// Apply length-counter reloads latched on a previous cycle. Mesen's
    /// register handlers call `APU::Run()` (which does this) before touching
    /// any state, so register accesses see the reloaded counters.
    fn flush_reloads(&mut self) {
        self.pulse1.reload_counters();
        self.pulse2.reload_counters();
        self.triangle.reload_counters();
        self.noise.reload_counters();
    }

    pub fn write(&mut self, addr: u16, value: u8) {
        match addr {
            0x4000..=0x4013 | 0x4015 | 0x4017 => self.flush_reloads(),
            _ => {}
        }

        match addr {
            0x4000..=0x4003 => self.pulse1.write(addr - 0x4000, value),
            0x4004..=0x4007 => self.pulse2.write(addr - 0x4004, value),
            0x4008..=0x400B => self.triangle.write(addr - 0x4008, value),
            0x400C..=0x400F => self.noise.write(addr - 0x400C, value, self.region),
            0x4010..=0x4013 => self.dmc.write(addr - 0x4010, value, self.region),
            0x4015 => {
                // Writing $4015 clears the DMC interrupt flag. This happens
                // before enabling the DMC (enabling can start a new sample).
                self.dmc.irq = false;

                self.pulse1.length.set_enabled(value & 0x01 != 0);
                self.pulse2.length.set_enabled(value & 0x02 != 0);
                self.triangle.length.set_enabled(value & 0x04 != 0);
                self.noise.length.set_enabled(value & 0x08 != 0);
                self.dmc.set_enabled(value & 0x10 != 0);
            }
            0x4017 => {
                // Bit 6 inhibits and acknowledges the frame interrupt.
                self.frame_counter
                    .write(value, self.total_cycles & 1 != 0);
                if self.frame_counter.inhibit_irq {
                    self.frame_irq = false;
                }
            }
            _ => {}
        }
    }

    /// $4015 read. Clears the frame-counter interrupt flag.
    pub fn read_status(&mut self) -> u8 {
        self.flush_reloads();

        let mut status = 0u8;
        if self.pulse1.status() {
            status |= 0x01;
        }
        if self.pulse2.status() {
            status |= 0x02;
        }
        if self.triangle.status() {
            status |= 0x04;
        }
        if self.noise.status() {
            status |= 0x08;
        }
        if self.dmc.status() {
            status |= 0x10;
        }
        // Bit 5 is open bus on hardware; the bus layer is responsible for
        // merging it (Mesen's APU also returns 0 here).
        if self.frame_irq {
            status |= 0x40;
        }
        if self.dmc.irq {
            status |= 0x80;
        }

        // Reading $4015 clears the frame counter interrupt flag (only that one).
        self.frame_irq = false;
        status
    }

    /// Level of the APU IRQ lines (frame counter or DMC).
    pub fn irq_pending(&self) -> bool {
        self.frame_irq || self.dmc.irq
    }

    /// Non-linear 2A03 mixer (`SoundMixer::GetOutputVolume`), raw units.
    #[inline]
    fn mixer_level(&self) -> f64 {
        let square_output = (self.pulse1.output + self.pulse2.output) as f64;
        let square_volume = if square_output > 0.0 {
            MIXER_SQUARE_SCALE / (8128.0 / square_output + 100.0)
        } else {
            0.0
        };

        let tnd_output = 3.0 * self.triangle.output as f64
            + 2.0 * self.noise.output as f64
            + self.dmc.output_level as f64;
        let tnd_volume = if tnd_output > 0.0 {
            MIXER_TND_SCALE / (MIXER_TND_DIVISOR / tnd_output + 100.0)
        } else {
            0.0
        };

        square_volume + tnd_volume
    }

    /// Fractional-accumulator box filter, DC blocker and master volume.
    fn accumulate_output(&mut self, expansion: f32) {
        self.mix_accum += self.mixer_level();
        self.expansion_accum += expansion as f64;
        self.sample_cycles += 1;
        self.sample_phase += self.samples_per_cycle;

        if self.sample_phase < 1.0 {
            return;
        }
        self.sample_phase -= 1.0;

        let cycles = self.sample_cycles.max(1) as f64;
        let nes = (self.mix_accum / cycles) * NES_MIX_SCALE;
        let exp = (self.expansion_accum / cycles) as f32 * EXPANSION_GAIN;
        self.mix_accum = 0.0;
        self.expansion_accum = 0.0;
        self.sample_cycles = 0;

        let x = ((nes as f32) + exp) * self.master_volume;
        // First-order DC blocker (~37 Hz high-pass).
        let y = x - self.dc_x + self.dc_coeff * self.dc_y;
        self.dc_x = x;
        self.dc_y = y;

        let out = if y.is_finite() {
            y.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        self.samples.push(out);
    }

    /// Append all mixed mono samples at `sample_rate`.
    pub fn drain_samples(&mut self, out: &mut Vec<f32>) {
        out.append(&mut self.samples);
    }

    pub fn save_state(&self, w: &mut StateWriter) {
        w.u32(self.sample_rate);
        w.f32(self.master_volume);
        w.u8(match self.region {
            Region::Ntsc => 0,
            Region::Pal => 1,
            Region::Dendy => 2,
        });
        w.bool(self.region_synced);
        w.u64(self.total_cycles);

        self.frame_counter.save_state(w);
        w.bool(self.frame_irq);

        self.pulse1.save_state(w);
        self.pulse2.save_state(w);
        self.triangle.save_state(w);
        self.noise.save_state(w);
        self.dmc.save_state(w);

        // Resampler/output stage.
        w.f64(self.sample_phase);
        w.f64(self.mix_accum);
        w.f64(self.expansion_accum);
        w.u32(self.sample_cycles);
        w.f32(self.dc_x);
        w.f32(self.dc_y);

        w.u32(self.samples.len() as u32);
        for sample in &self.samples {
            w.f32(*sample);
        }
    }

    pub fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.sample_rate = r.u32()?.max(1);
        self.master_volume = r.f32()?.clamp(0.0, 2.0);
        self.region = match r.u8()? {
            1 => Region::Pal,
            2 => Region::Dendy,
            _ => Region::Ntsc,
        };
        self.region_synced = r.bool()?;
        self.total_cycles = r.u64()?;

        self.frame_counter.load_state(r)?;
        self.frame_irq = r.bool()?;

        self.pulse1.load_state(r)?;
        self.pulse2.load_state(r)?;
        self.triangle.load_state(r)?;
        self.noise.load_state(r)?;
        self.dmc.load_state(r)?;

        self.sample_phase = r.f64()?;
        self.mix_accum = r.f64()?;
        self.expansion_accum = r.f64()?;
        self.sample_cycles = r.u32()?;
        self.dc_x = r.f32()?;
        self.dc_y = r.f32()?;

        let sample_count = r.u32()? as usize;
        // Guard against absurd/corrupt lengths: StateReader will error anyway,
        // but pre-allocating must not trust the file blindly.
        let mut samples = Vec::with_capacity(sample_count.min(1 << 20));
        for _ in 0..sample_count {
            samples.push(r.f32()?);
        }
        self.samples = samples;

        self.update_timing();
        Ok(())
    }
}

impl Default for Apu {
    fn default() -> Self {
        Self::new(44_100)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal NROM cartridge for tests. `$C000` maps to PRG offset 0 (16KB
    /// NROM mirrors), so DMC sample addresses in `$C000-$FFC0` are usable.
    fn test_cart() -> Cartridge {
        let mut data = vec![0u8; 16 + 0x4000 + 0x2000];
        data[0..4].copy_from_slice(b"NES\x1A");
        data[4] = 1; // 16KB PRG
        data[5] = 1; // 8KB CHR
        // Pattern in PRG; $C000+0x40 = 0xA5 for the DMC fetch test.
        for (i, byte) in data[16..16 + 0x4000].iter_mut().enumerate() {
            *byte = (i as u8) ^ 0x5A;
        }
        data[16 + 0x40] = 0xA5;
        Cartridge::from_bytes(&data).expect("test ROM")
    }

    fn tick_n(apu: &mut Apu, cart: &mut Cartridge, ram: &[u8], cycles: usize) {
        for _ in 0..cycles {
            apu.tick(cart, ram);
        }
    }

    fn save(apu: &Apu) -> Vec<u8> {
        let mut w = StateWriter::new();
        apu.save_state(&mut w);
        w.into_inner()
    }

    #[test]
    fn length_counter_table_matches_hardware() {
        assert_eq!(
            LENGTH_TABLE,
            [
                10, 254, 20, 2, 40, 4, 80, 6, 160, 8, 60, 10, 14, 12, 26, 14, 12, 16, 24, 18, 48,
                20, 96, 22, 192, 24, 72, 26, 16, 28, 32, 30
            ]
        );

        let mut lc = LengthCounter::default();
        // Disabled counters ignore loads.
        lc.load_length(0);
        lc.reload();
        assert_eq!(lc.value, 0);

        lc.set_enabled(true);
        lc.load_length(1);
        lc.reload();
        assert_eq!(lc.value, 254);
        assert!(lc.status());

        // Halt prevents decrementing.
        lc.halt = true;
        for _ in 0..10 {
            lc.tick();
        }
        assert_eq!(lc.value, 254);
        lc.halt = false;
        lc.tick();
        assert_eq!(lc.value, 253);

        // Disabling zeroes the counter immediately.
        lc.set_enabled(false);
        assert_eq!(lc.value, 0);
        assert!(!lc.status());

        // A write whose length index is 0 loads 10.
        let mut lc2 = LengthCounter::default();
        lc2.set_enabled(true);
        lc2.load_length(0);
        lc2.reload();
        assert_eq!(lc2.value, 10);
    }

    #[test]
    fn envelope_clocking() {
        let mut env = Envelope::default();
        env.init(0x02); // envelope mode, volume/period 2
        env.restart();
        assert_eq!(env.current_volume(true), 0);

        env.tick(false); // start flag: counter = 15, divider = 2
        assert_eq!(env.counter, 15);
        assert_eq!(env.divider, 2);

        env.tick(false); // divider 1
        assert_eq!(env.divider, 1);
        env.tick(false); // divider 0
        assert_eq!(env.divider, 0);
        env.tick(false); // underflow: counter 14, divider 2
        assert_eq!(env.counter, 14);
        assert_eq!(env.divider, 2);

        // At counter 0 the envelope either stops or loops to 15 with halt.
        env.counter = 0;
        env.divider = 0;
        env.tick(false);
        assert_eq!(env.counter, 0);
        env.divider = 0;
        env.tick(true);
        assert_eq!(env.counter, 15);

        // Constant volume ignores the decay counter but is gated by length.
        let mut env2 = Envelope::default();
        env2.init(0x1A); // constant volume, 10
        assert_eq!(env2.current_volume(true), 10);
        assert_eq!(env2.current_volume(false), 0);
    }

    #[test]
    fn pulse_timer_duty_and_sweep_mute() {
        let mut p = Pulse::new(true);
        p.length.set_enabled(true);
        p.write(0, 0x80 | 0x10 | 0x0F); // duty 2, constant volume 15
        p.write(2, 0x00); // real period 0
        p.write(3, 0x00); // length index 0 (10), duty position reset
        p.reload_counters(); // apply the latched length reload
        assert_eq!(p.length.value, 10);
        assert_eq!(p.output, 0);
        // Period < 8 mutes the channel unconditionally.
        assert!(p.is_muted());
        p.update_output();
        assert_eq!(p.output, 0);

        // A valid period (8 => timer reload 17, clock every 18 cycles).
        p.set_period(8);
        assert_eq!(p.period, 17);
        assert!(!p.is_muted());
        p.update_output();
        assert_eq!(p.output, 0); // duty 2, position 0 is 0

        // Duty sequence walks backwards through the 8-entry pattern.
        p.clock();
        assert_eq!(p.duty_pos, 7);
        assert_eq!(p.output, 15);
        p.clock();
        assert_eq!(p.duty_pos, 6);
        assert_eq!(p.output, 15);
        p.clock();
        assert_eq!(p.duty_pos, 5);
        assert_eq!(p.output, 15);
        p.clock();
        assert_eq!(p.duty_pos, 4);
        assert_eq!(p.output, 15);
        p.clock();
        assert_eq!(p.duty_pos, 3);
        assert_eq!(p.output, 0); // duty 2 is zero for positions 0..=3

        // Timer math: reload countdown is period + 1 cycles.
        p.timer = 0;
        assert!(p.tick_timer());
        assert_eq!(p.timer, 17);
        for _ in 0..17 {
            assert!(!p.tick_timer());
        }
        assert!(p.tick_timer());
        assert_eq!(p.timer, 17);

        // Non-negated sweep overflow target also mutes.
        p.set_period(0x7FF);
        p.init_sweep(0x81); // enabled, shift 1 -> target 0xBFE
        assert!(p.sweep_target_period > 0x7FF);
        p.update_output();
        assert!(p.is_muted());
        assert_eq!(p.output, 0);

        // Negated sweep is allowed to overflow without muting on target alone.
        p.init_sweep(0x89); // enabled, negate, shift 1
        assert!(!p.is_muted());

        // A short period arriving through the sweep also mutes.
        p.init_sweep(0x00);
        p.set_period(4);
        assert!(p.is_muted());
    }

    #[test]
    fn noise_lfsr_short_mode() {
        let mut n = Noise::new(Region::Ntsc);
        n.length.set_enabled(true);
        n.write(0, 0x1F, Region::Ntsc); // constant volume 15
        n.write(2, 0x80 | 0x00, Region::Ntsc); // short mode, rate 0
        n.write(3, 0x08, Region::Ntsc); // length index 1
        n.reload_counters();
        assert_eq!(n.shift_register, 1);

        n.clock(); // bit0=1 ^ bit6=0 -> feedback 1 -> 0x4000
        assert_eq!(n.shift_register, 0x4000);
        assert_eq!(n.output, 15); // shift bit0 == 0 -> audible

        n.clock(); // bit0=0 ^ bit6=1 -> feedback 1 -> 0x2000
        assert_eq!(n.shift_register, 0x2000);
        n.clock(); // 0x1000
        assert_eq!(n.shift_register, 0x1000);
        n.clock(); // 0x0800
        assert_eq!(n.shift_register, 0x0800);

        // Eventually the LFSR bit0 becomes 1, muting the channel.
        let mut saw_mute = false;
        for _ in 0..64 {
            n.clock();
            if n.output == 0 && n.shift_register & 1 == 1 {
                saw_mute = true;
                break;
            }
        }
        assert!(saw_mute);
    }

    #[test]
    fn status_4015_and_length_enable() {
        let mut apu = Apu::new(44_100);
        let mut cart = test_cart();
        let ram = [0u8; 0x800];

        assert_eq!(apu.read_status() & 0x1F, 0);

        // Enable pulse1 only; loads happen only while enabled.
        apu.write(0x4015, 0x01);
        apu.write(0x4003, 0x08); // length index 1 -> 254
        tick_n(&mut apu, &mut cart, &ram, 1); // apply the latched reload
        assert_eq!(apu.pulse1.length.value, 254);
        assert_eq!(apu.read_status() & 0x01, 0x01);

        // Square 2 is disabled: a $4007 write is ignored.
        apu.write(0x4007, 0x08);
        assert_eq!(apu.pulse2.length.value, 0);
        assert_eq!(apu.read_status() & 0x02, 0);

        // Writing $4015 clears the DMC IRQ latch; test that separately below.
        // Disabling zeroes length counters immediately.
        apu.write(0x4015, 0x00);
        assert_eq!(apu.pulse1.length.value, 0);
        assert_eq!(apu.read_status() & 0x1F, 0);

        // A length write while disabled does nothing.
        apu.write(0x4003, 0x08);
        apu.write(0x4007, 0x08);
        assert_eq!(apu.pulse1.length.value, 0);
        assert_eq!(apu.pulse2.length.value, 0);

        // Enabling and loading the triangle / noise reflects in status bits.
        apu.write(0x4015, 0x0C);
        apu.write(0x400B, 0x08);
        apu.write(0x400F, 0x08);
        tick_n(&mut apu, &mut cart, &ram, 1); // apply length reloads
        assert_eq!(apu.read_status() & 0x0C, 0x0C);

        // Halt bit (envelope loop) prevents length clocking on half frames.
        apu.write(0x4015, 0x01);
        apu.write(0x4000, 0x20 | 0x10 | 0x0F);
        apu.write(0x4003, 0x08); // 254
        tick_n(&mut apu, &mut cart, &ram, 1);
        assert_eq!(apu.pulse1.length.value, 254);
        let before = apu.pulse1.length.value;
        tick_n(&mut apu, &mut cart, &ram, 14_999);
        assert_eq!(apu.pulse1.length.value, before);
        assert_eq!(apu.read_status() & 0x01, 0x01);
    }

    #[test]
    fn frame_sequencer_irq_and_length_clocking() {
        let mut apu = Apu::new(44_100);
        let mut cart = test_cart();
        let ram = [0u8; 0x800];

        // Power-on behaves as a $00 write 3 cycles in. No IRQ before the last
        // step of the 4-step sequence (29828 + 3 cycles).
        tick_n(&mut apu, &mut cart, &ram, 29_000);
        assert!(!apu.irq_pending());
        tick_n(&mut apu, &mut cart, &ram, 831);
        assert!(apu.irq_pending());

        let status = apu.read_status();
        assert_eq!(status & 0x40, 0x40);
        assert!(!apu.irq_pending(), "reading $4015 clears the frame IRQ");

        // Disable IRQ via $4017 bit 6: no IRQ for a whole 4-step frame.
        apu.write(0x4017, 0x40);
        tick_n(&mut apu, &mut cart, &ram, 30_000);
        assert!(!apu.irq_pending());

        // An already pending frame IRQ is acknowledged by a bit-6 $4017 write.
        apu.write(0x4017, 0x00);
        tick_n(&mut apu, &mut cart, &ram, 30_000);
        assert!(apu.irq_pending());
        apu.write(0x4017, 0x40);
        assert!(!apu.irq_pending());

        // 5-step mode never raises the frame IRQ.
        apu.write(0x4017, 0x80);
        tick_n(&mut apu, &mut cart, &ram, 40_000);
        assert!(!apu.irq_pending());
        assert_eq!(apu.frame_counter.step_mode, 1);

        // Sequencer half frame decrements a non-halted length counter.
        let mut apu2 = Apu::new(44_100);
        apu2.write(0x4015, 0x01);
        apu2.write(0x4000, 0x10); // constant volume, halt off
        apu2.write(0x4003, 0x08); // 254
        tick_n(&mut apu2, &mut cart, &ram, 14_915);
        assert_eq!(apu2.pulse1.length.value, 254);
        tick_n(&mut apu2, &mut cart, &ram, 1); // first half frame (14913 + 3)
        assert_eq!(apu2.pulse1.length.value, 253);
    }

    #[test]
    fn dmc_irq_status_and_memory_fetch() {
        let mut apu = Apu::new(44_100);
        let mut cart = test_cart();
        let ram = [0u8; 0x800];

        // One byte sample at $C040 (PRG value 0xA5), fastest rate.
        apu.write(0x4010, 0x80 | 0x0F); // IRQ enabled, loop off, rate 15
        apu.write(0x4012, 0x01); // $C040
        apu.write(0x4013, 0x00); // 1 byte
        apu.write(0x4015, 0x10);

        tick_n(&mut apu, &mut cart, &ram, 12);
        assert!(apu.dmc.irq);
        assert!(apu.irq_pending());
        assert_eq!(apu.dmc.read_buffer, 0xA5);
        assert_eq!(apu.dmc.bytes_remaining, 0);
        assert_eq!(apu.dmc.current_addr, 0xC041);
        assert_eq!(apu.read_status() & 0x90, 0x80); // DMC IRQ, no bytes left

        // Reading $4015 does not clear the DMC IRQ.
        assert!(apu.irq_pending());

        // Writing $4015 clears it, but re-enabling with zero bytes restarts the
        // sample: check immediately after the write.
        apu.write(0x4015, 0x10);
        assert!(!apu.irq_pending());

        // Re-run and complete. The previously fetched byte is still buffered
        // and the DMC timer keeps its phase, so allow a full rate period(s).
        tick_n(&mut apu, &mut cart, &ram, 1_200);
        assert!(apu.irq_pending());
        apu.write(0x4010, 0x0F);
        assert!(!apu.irq_pending());

        // RAM mirror fetch ($0000-$1FFF) is supported by the fetch routine.
        let mut ram2 = [0u8; 0x800];
        ram2[0x123] = 0x42;
        let mut dmc = Dmc::new(Region::Ntsc);
        dmc.current_addr = 0x0123;
        dmc.bytes_remaining = 1;
        dmc.buffer_empty = true;
        dmc.fetch_sample(&mut cart, &ram2);
        assert_eq!(dmc.read_buffer, 0x42);
        assert_eq!(dmc.current_addr, 0x0124);

        // Address wrap: $FFFF -> $8000.
        let mut dmc2 = Dmc::new(Region::Ntsc);
        dmc2.current_addr = 0xFFFF;
        dmc2.bytes_remaining = 1;
        dmc2.buffer_empty = true;
        dmc2.fetch_sample(&mut cart, &ram);
        assert_eq!(dmc2.current_addr, 0x8000);

        // $4011 writes apply immediately to the output level.
        let mut apu3 = Apu::new(44_100);
        apu3.write(0x4011, 0x55);
        assert_eq!(apu3.dmc.output_level, 0x55);
        apu3.write(0x4011, 0xFF);
        assert_eq!(apu3.dmc.output_level, 0x7F);
    }

    #[test]
    fn dmc_delta_modulation_and_loop() {
        let mut d = Dmc::new(Region::Ntsc);
        d.output_level = 64;
        d.silence_flag = false;
        d.shift_register = 0xFF;
        d.buffer_empty = true; // no refill available
        for _ in 0..8 {
            d.clock();
        }
        // Eight 1-bits: +2 each, capped at 125.
        assert_eq!(d.output_level, 80);
        assert!(d.silence_flag);
        assert_eq!(d.bits_remaining, 8);

        // A silent DMC holds its level.
        let before = d.output_level;
        d.clock();
        assert_eq!(d.output_level, before);

        // Loop restarts the sample address/length when the last byte is fetched.
        let mut d2 = Dmc::new(Region::Ntsc);
        d2.sample_addr = 0xC000;
        d2.sample_length = 1;
        d2.loop_flag = true;
        d2.init_sample();
        d2.bytes_remaining = 1;
        d2.irq_enabled = true;
        let mut cart = test_cart();
        let ram = [0u8; 0x800];
        d2.fetch_sample(&mut cart, &ram);
        assert_eq!(d2.bytes_remaining, 1); // restarted
        assert_eq!(d2.current_addr, 0xC000);
        assert!(!d2.irq, "looped samples never set the IRQ flag");
    }

    #[test]
    fn sample_rate_and_output_stability() {
        let mut apu = Apu::new(44_100);
        assert_eq!(apu.sample_rate(), 44_100);
        let mut cart = test_cart();
        let ram = [0u8; 0x800];

        // Loud output on all channels.
        apu.write(0x4015, 0x0F);
        apu.write(0x4000, 0xBF); // pulse1 duty 2, constant 15
        apu.write(0x4002, 0x40);
        apu.write(0x4003, 0x08);
        apu.write(0x4004, 0xBF); // pulse2
        apu.write(0x4006, 0x30);
        apu.write(0x4007, 0x08);
        apu.write(0x4008, 0x81); // triangle linear control
        apu.write(0x400A, 0x20);
        apu.write(0x400B, 0x08);
        apu.write(0x400C, 0x1F); // noise constant 15
        apu.write(0x400E, 0x04);
        apu.write(0x400F, 0x08);

        tick_n(&mut apu, &mut cart, &ram, 178_977); // ~0.1 s
        let mut out = Vec::new();
        apu.drain_samples(&mut out);

        let expected = 44_100u64 * 178_977u64 / 1_789_773u64;
        assert!(
            (out.len() as i64 - expected as i64).abs() <= 2,
            "got {} samples, expected about {}",
            out.len(),
            expected
        );
        assert!(out.iter().all(|s| s.is_finite()));
        assert!(out.iter().all(|s| s.abs() <= 1.0));
        assert!(out.iter().any(|s| s.abs() > 0.05), "output should not be silent");

        // drain_samples empties the buffer.
        let mut out2 = Vec::new();
        apu.drain_samples(&mut out2);
        assert!(out2.is_empty());

        // Sample rate changes are honoured by the fractional accumulator.
        apu.set_sample_rate(48_000);
        assert_eq!(apu.sample_rate(), 48_000);
        tick_n(&mut apu, &mut cart, &ram, 178_977);
        let mut out3 = Vec::new();
        apu.drain_samples(&mut out3);
        assert!((out3.len() as i64 - 4800).abs() <= 2);
    }

    #[test]
    fn save_load_round_trip() {
        let mut apu = Apu::new(44_100);
        let mut cart = test_cart();
        let ram = [0u8; 0x800];

        apu.write(0x4015, 0x1F);
        apu.write(0x4000, 0xBF);
        apu.write(0x4001, 0x89);
        apu.write(0x4002, 0x40);
        apu.write(0x4003, 0x08);
        apu.write(0x4008, 0x81);
        apu.write(0x400A, 0x20);
        apu.write(0x400B, 0x18);
        apu.write(0x400C, 0x1F);
        apu.write(0x400E, 0x84);
        apu.write(0x400F, 0x28);
        apu.write(0x4010, 0x8F);
        apu.write(0x4012, 0x02);
        apu.write(0x4013, 0x08);

        tick_n(&mut apu, &mut cart, &ram, 50_000);
        let saved = save(&apu);

        tick_n(&mut apu, &mut cart, &ram, 12_345);
        let mut reader = StateReader::new(&saved);
        apu.load_state(&mut reader).expect("load state");
        assert_eq!(reader.remaining(), 0, "load must consume the whole state");

        let resaved = save(&apu);
        assert_eq!(saved, resaved, "state must round-trip exactly");

        // The restored machine keeps producing sane audio.
        tick_n(&mut apu, &mut cart, &ram, 100_000);
        let mut out = Vec::new();
        apu.drain_samples(&mut out);
        assert!(!out.is_empty());
        assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
    }

    #[test]
    fn reset_behavior() {
        let mut apu = Apu::new(44_100);
        let mut cart = test_cart();
        let ram = [0u8; 0x800];

        apu.write(0x4015, 0x01);
        apu.write(0x4000, 0xBF);
        apu.write(0x4002, 0x40);
        apu.write(0x4003, 0x08);
        tick_n(&mut apu, &mut cart, &ram, 30_000);
        assert!(apu.irq_pending());

        apu.reset();
        assert!(!apu.irq_pending());
        assert_eq!(apu.read_status() & 0x1F, 0);
        assert_eq!(apu.frame_counter.step_mode, 0);

        // $4017 mode survives a soft reset (but the sequencer restarts).
        apu.write(0x4017, 0x80);
        tick_n(&mut apu, &mut cart, &ram, 4);
        assert_eq!(apu.frame_counter.step_mode, 1);
        apu.reset();
        assert_eq!(apu.frame_counter.step_mode, 1);
        tick_n(&mut apu, &mut cart, &ram, 40_000);
        assert!(!apu.irq_pending());
        assert_eq!(apu.frame_counter.step_mode, 1);
    }
}
