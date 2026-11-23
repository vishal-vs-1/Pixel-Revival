//! System bus: CPU memory map, PPU/APU/cartridge/controller routing, timing.

use crate::apu::Apu;
use crate::cartridge::Cartridge;
use crate::input::Controller;
use crate::ppu::Ppu;
use crate::state::{StateReader, StateResult, StateWriter};
use crate::types::{MemoryAccessType, Region, RomSystem};

pub struct Bus {
    pub ram: [u8; 0x800],
    pub ppu: Ppu,
    pub apu: Apu,
    pub cart: Cartridge,
    pub controller1: Controller,
    pub controller2: Controller,
    pub open_bus: u8,
    /// Extra cycles (DMA) that the CPU must account for after the current
    /// instruction. The bus has not ticked these yet.
    pub stall_cycles: u32,
    pub master_cycles: u64,
    /// Master-clock phase within the current PPU dot (0..ppu_divider).
    ppu_clock_phase: u32,
    region: Region,
}

impl Bus {
    pub fn new(cart: Cartridge, region: Region, sample_rate: u32) -> Self {
        Self {
            ram: [0; 0x800],
            ppu: Ppu::new(region),
            apu: Apu::new(sample_rate),
            cart,
            controller1: Controller::new(),
            controller2: Controller::new(),
            open_bus: 0,
            stall_cycles: 0,
            master_cycles: 0,
            ppu_clock_phase: 0,
            region,
        }
    }

    pub fn region(&self) -> Region {
        self.region
    }

    /// Mesen starts the PPU one master clock behind the CPU and gives the CPU
    /// `cpuDivider` extra clocks before the eight reset cycles. Reproduce that
    /// sub-dot phase so NMI/vblank edges land on the same PPU dot.
    pub fn align_ppu_after_reset(&mut self) {
        let extra = self.region.cpu_master_clock_divider().saturating_sub(1);
        self.advance_ppu(extra);
    }

    /// Change the console region at runtime (frontend `--pal` override).
    pub fn set_region(&mut self, region: Region) {
        self.region = region;
        self.ppu.set_region(region);
        self.apu.set_region(region);
    }

    pub fn reset(&mut self) {
        // Soft reset: RAM (CPU and cartridge) intentionally survives, matching
        // Mesen's Console::ResetComponents(false).
        self.ppu.reset();
        self.apu.reset();
        self.cart.reset();
        self.controller1 = Controller::new();
        self.controller2 = Controller::new();
        self.open_bus = 0;
        self.stall_cycles = 0;
        self.master_cycles = 0;
        self.ppu_clock_phase = 0;
    }

    #[inline]
    pub fn take_stall_cycles(&mut self) -> u32 {
        std::mem::take(&mut self.stall_cycles)
    }

    pub fn read(&mut self, addr: u16, _access: MemoryAccessType) -> u8 {
        let nsf_bios = self.cart.info().system == RomSystem::Nsf;
        let value = match addr {
            0x0000..=0x1FFF => self.ram[(addr as usize) & 0x7FF],
            0x3E00..=0x3FFF if nsf_bios => {
                self.cart.cpu_read(addr).unwrap_or(self.open_bus)
            }
            0x2000..=0x3FFF => {
                let reg = 0x2000 + ((addr as usize) & 7) as u16;
                self.ppu.read_register(reg, &mut self.cart)
            }
            0x4015 => self.apu.read_status(),
            0x4016 => {
                let controller = self.controller1.read();
                match self.cart.mapper_mut().read_input_port(0x4016, controller) {
                    Some(value) => value,
                    None => (self.open_bus & 0xFE) | controller,
                }
            }
            0x4017 => {
                let controller = self.controller2.read();
                match self.cart.mapper_mut().read_input_port(0x4017, controller) {
                    Some(value) => value,
                    None => (self.open_bus & 0xFE) | controller,
                }
            }
            0x4018..=0x401F => self.open_bus,
            _ => {
                if addr >= 0x4020 {
                    self.cart.cpu_read(addr).unwrap_or(self.open_bus)
                } else {
                    self.open_bus
                }
            }
        };
        self.open_bus = value;
        value
    }

    pub fn write(&mut self, addr: u16, value: u8, _access: MemoryAccessType) {
        self.open_bus = value;
        let nsf_bios = self.cart.info().system == RomSystem::Nsf;
        match addr {
            0x0000..=0x1FFF => self.ram[(addr as usize) & 0x7FF] = value,
            0x3E00..=0x3FFF if nsf_bios => self.cart.cpu_write(addr, value),
            0x2000..=0x3FFF => {
                let reg = 0x2000 + ((addr as usize) & 7) as u16;
                self.ppu.write_register(reg, value, &mut self.cart);
            }
            0x4000..=0x4013 | 0x4015 | 0x4017 => self.apu.write(addr, value),
            0x4014 => {
                let page = (value as u16) << 8;
                let mut data = [0u8; 256];
                for (i, byte) in data.iter_mut().enumerate() {
                    *byte = self.read(page + i as u16, MemoryAccessType::DummyRead);
                }
                self.ppu.oam_dma(&data);
                // 513 cycles + 1 on odd CPU cycles.
                self.stall_cycles = self.stall_cycles.wrapping_add(
                    513 + (self.master_cycles as u32 & 1),
                );
            }
            0x4016 => {
                self.controller1.write_strobe(value);
                self.controller2.write_strobe(value);
                self.cart.mapper_mut().write_input_port(value);
            }
            0x4018..=0x401F => {}
            _ => {
                if addr >= 0x4020 {
                    self.cart.cpu_write(addr, value);
                }
            }
        }
    }

    /// Side-effect-free read for disassembly/tracing. PPU/APU/controller I/O
    /// returns the current open-bus value; cartridge reads are delegated (most
    /// mapper reads are pure, but debug-only use is expected).
    pub fn peek(&mut self, addr: u16) -> u8 {
        let nsf_bios = self.cart.info().system == RomSystem::Nsf;
        match addr {
            0x0000..=0x1FFF => self.ram[(addr as usize) & 0x7FF],
            0x3E00..=0x3FFF if nsf_bios => self.cart.cpu_read(addr).unwrap_or(self.open_bus),
            0x4020..=0xFFFF => self.cart.cpu_read(addr).unwrap_or(self.open_bus),
            _ => self.open_bus,
        }
    }

    /// Advance the whole machine by `cycles` CPU cycles (used for reset and
    /// stalls where there is no distinct memory access).
    pub fn tick(&mut self, cycles: u32) {
        for _ in 0..cycles {
            self.begin_access(true);
            self.end_access(true);
        }
    }

    /// First half of a CPU cycle, before the memory access. Mesen places read
    /// accesses at master clock 5/12 (write: 7/12), which matters for NMI
    /// suppression and MMC3 A12 timing.
    pub fn begin_access(&mut self, for_read: bool) {
        self.master_cycles = self.master_cycles.wrapping_add(1);
        let start = self.region.cpu_start_clock_count();
        let clocks = if for_read { start - 1 } else { start + 1 };
        self.advance_ppu(clocks);
        self.cart.cpu_cycle();
        self.apu.tick(&mut self.cart, &self.ram);
    }

    /// Second half of a CPU cycle, after the memory access.
    pub fn end_access(&mut self, for_read: bool) {
        let end = self.region.cpu_end_clock_count();
        let clocks = if for_read { end + 1 } else { end - 1 };
        self.advance_ppu(clocks);
    }

    #[inline]
    fn advance_ppu(&mut self, master_clocks: u32) {
        self.ppu_clock_phase += master_clocks;
        let divider = self.region.ppu_master_clock_divider();
        while self.ppu_clock_phase >= divider {
            self.ppu_clock_phase -= divider;
            if let Some(scanline) = self.ppu.tick(&mut self.cart) {
                self.cart.scanline(scanline);
            }
        }
    }

    #[inline]
    pub fn take_nmi(&mut self) -> bool {
        self.ppu.take_nmi()
    }

    #[inline]
    pub fn irq_pending(&self) -> bool {
        self.apu.irq_pending() || self.cart.irq_pending()
    }

    pub fn set_controller(&mut self, port: usize, buttons: u8) {
        if port == 0 {
            self.controller1.set_buttons(buttons);
        } else {
            self.controller2.set_buttons(buttons);
        }
    }

    pub fn drain_audio(&mut self, out: &mut Vec<f32>) {
        self.apu.drain_samples(out);
    }

    pub fn save_state(&self, w: &mut StateWriter) {
        w.fixed_bytes(&self.ram);
        self.controller1.save_state(w);
        self.controller2.save_state(w);
        w.u8(self.open_bus);
        w.u32(self.stall_cycles);
        w.u64(self.master_cycles);
        w.u32(self.ppu_clock_phase);
    }

    pub fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        let ram = r.fixed_bytes(0x800)?;
        self.ram.copy_from_slice(&ram);
        self.controller1.load_state(r)?;
        self.controller2.load_state(r)?;
        self.open_bus = r.u8()?;
        self.stall_cycles = r.u32()?;
        self.master_cycles = r.u64()?;
        self.ppu_clock_phase = r.u32()?;
        Ok(())
    }
}
