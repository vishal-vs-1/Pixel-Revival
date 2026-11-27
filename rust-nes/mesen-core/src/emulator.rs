//! Top-level emulator: CPU + bus + save states.
//!
//! This is the stable, UI-agnostic entry point. Frontends should normally need
//! only the methods on `Emulator` (video, audio, input, battery, state, region)
//! and a `Cartridge`. The `debug_*` methods expose the internals for debuggers,
//! test harnesses and trace tools.

use crate::bus::Bus;
use crate::cartridge::Cartridge;
use crate::cpu::{Cpu, CpuState};
use crate::state::{StateError, StateReader, StateResult, StateWriter};
use crate::types::{MemoryAccessType, Region, RomInfo};

const STATE_MAGIC: &[u8; 8] = b"MESENRS1";
const STATE_VERSION: u32 = 1;

pub struct Emulator {
    pub(crate) cpu: Cpu,
    pub(crate) bus: Bus,
    sample_rate: u32,
}

impl Emulator {
    pub const SCREEN_WIDTH: usize = crate::types::SCREEN_WIDTH;
    pub const SCREEN_HEIGHT: usize = crate::types::SCREEN_HEIGHT;

    pub fn new(cart: Cartridge, sample_rate: u32) -> Self {
        let region = cart.info().region;
        let mut emu = Self {
            cpu: Cpu::new(),
            bus: Bus::new(cart, region, sample_rate),
            sample_rate,
        };
        emu.reset();
        emu
    }

    pub fn region(&self) -> Region {
        self.bus.region()
    }

    pub fn set_region(&mut self, region: Region) {
        self.bus.set_region(region);
    }

    pub fn reset(&mut self) {
        self.bus.reset();
        self.cpu.reset(&mut self.bus);
    }

    /// Run until the PPU completes a frame, returning its framebuffer.
    pub fn run_frame(&mut self) -> &[u32] {
        self.bus.ppu.take_frame_ready();
        loop {
            self.cpu.step(&mut self.bus);
            if self.bus.ppu.take_frame_ready() {
                break;
            }
        }
        self.bus.ppu.framebuffer()
    }

    /// Run one CPU instruction (mostly for tests/debugging).
    pub fn step_instruction(&mut self) -> u32 {
        self.cpu.step(&mut self.bus)
    }

    pub fn set_controller(&mut self, port: usize, buttons: u8) {
        self.bus.set_controller(port, buttons);
    }

    /// Latest completed 256×240 XRGB frame.
    pub fn framebuffer(&self) -> &[u32] {
        self.bus.ppu.framebuffer()
    }

    /// VENDORED-DELTA (pose recording): observer snapshot for the Python
    /// player-pose detector.
    pub fn observation(&self) -> crate::ppu::PpuObservation {
        self.bus.ppu.observation()
    }

    pub fn rom_info(&self) -> &RomInfo {
        self.bus.cart.info()
    }

    pub fn mapper_id(&self) -> u16 {
        self.bus.cart.mapper_id()
    }

    /// Battery-backed cartridge RAM, if this board has any.
    pub fn battery(&self) -> Option<&[u8]> {
        self.bus.cart.battery()
    }

    pub fn set_battery(&mut self, data: &[u8]) {
        self.bus.cart.set_battery(data);
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn set_sample_rate(&mut self, rate: u32) {
        self.sample_rate = rate;
        self.bus.apu.set_sample_rate(rate);
    }

    /// Frame tiles for HD compositing (see `hd` module). Call after
    /// `run_frame`; the log drains on read. Empty unless `set_hd` armed it.
    pub fn take_hd_pixels(&mut self, out: &mut Vec<crate::hd::HdPixel>) {
        self.bus.ppu.take_hd_pixels(out);
    }

    /// Arm (`true`) or disarm the per-pixel HD tile log. `is_ram` is fixed
    /// per cartridge: CHR RAM tiles are reported as misses in v1.
    pub fn set_hd(&mut self, active: bool) {
        let is_ram = self.bus.cart.info().chr_ram_size > 0;
        self.bus.ppu.set_hd(active, is_ram);
    }

    /// Debug probe: (armed, chr_ram, pending_records) without draining.
    /// Lets Python tell "not armed" apart from "armed but no pixels".
    pub fn hd_debug(&self) -> (bool, bool, usize) {
        self.bus.ppu.hd_debug()
    }

    pub fn frame_count(&self) -> u64 {
        self.bus.ppu.frame_count()
    }

    /// Debug/testing access to the CPU state (copy).
    pub fn cpu_state(&self) -> CpuState {
        self.cpu.state
    }

    pub fn pc(&self) -> u16 {
        self.cpu.pc()
    }

    pub fn set_pc(&mut self, pc: u16) {
        self.cpu.state.pc = pc;
    }

    pub fn total_cycles(&self) -> u64 {
        self.cpu.total_cycles
    }

    /// Non-clocking CPU-memory read for debuggers/validation tools. Avoid
    /// using this in a frontend's normal frame loop (it can have side effects
    /// on I/O registers).
    pub fn read_debug(&mut self, addr: u16) -> u8 {
        self.bus.read(addr, MemoryAccessType::DummyRead)
    }

    /// Side-effect-free RAM/cartridge read for disassembly and tracing.
    pub fn peek(&mut self, addr: u16) -> u8 {
        self.bus.peek(addr)
    }

    pub fn ram(&self) -> &[u8; 0x800] {
        &self.bus.ram
    }

    /// Read PPU memory (CHR/nametables) without going through the CPU queue.
    pub fn ppu_read_debug(&mut self, addr: u16) -> u8 {
        self.bus.cart.ppu_read(addr).unwrap_or(0)
    }

    pub fn pose_arm(&mut self, active: bool) { self.bus.ppu.pose_arm(active); }

    pub fn pose_cells(&self) -> impl Iterator<Item = &crate::pose::PoseCell> {
        self.bus.ppu.pose_cells()
    }

    pub fn pose_camera(&self) -> Option<(u16, u16)> { self.bus.ppu.pose_camera() }

    /// Low-level access for a debugger or trace tool. Normal frontends should
    /// not need these.
    pub fn debug_cpu(&self) -> &Cpu {
        &self.cpu
    }

    pub fn debug_cpu_mut(&mut self) -> &mut Cpu {
        &mut self.cpu
    }

    pub fn debug_bus(&self) -> &Bus {
        &self.bus
    }

    pub fn debug_bus_mut(&mut self) -> &mut Bus {
        &mut self.bus
    }

    pub fn drain_audio(&mut self, out: &mut Vec<f32>) {
        self.bus.drain_audio(out);
    }

    pub fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.fixed_bytes(STATE_MAGIC);
        w.u32(STATE_VERSION);
        self.cpu.save_state(&mut w);
        self.bus.ppu.save_state(&mut w);
        self.bus.apu.save_state(&mut w);
        self.bus.save_state(&mut w);
        let mapper = self.bus.cart.save_state();
        w.bytes(&mapper);
        w.into_inner()
    }

    pub fn load_state(&mut self, data: &[u8]) -> StateResult<()> {
        let mut r = StateReader::new(data);
        let magic = r.fixed_bytes(8)?;
        if magic != STATE_MAGIC {
            return Err(StateError("not a Mesen-RS save state".into()));
        }
        let version = r.u32()?;
        if version != STATE_VERSION {
            return Err(StateError(format!(
                "unsupported save-state version {}",
                version
            )));
        }
        self.cpu.load_state(&mut r)?;
        self.bus.ppu.load_state(&mut r)?;
        self.bus.apu.load_state(&mut r)?;
        self.bus.load_state(&mut r)?;
        let mapper = r.bytes()?;
        self.bus.cart.load_state(&mapper)?;
        Ok(())
    }
}
