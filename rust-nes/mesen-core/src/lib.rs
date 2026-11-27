//! Rust port of the Mesen 0.9.9 NES emulator core.
//!
//! Original C++: Copyright (C) 2014-2020 Sour, GPL-3.0.
//! Rust port: GPL-3.0-or-later.

pub mod apu;
pub mod bus;
pub mod cartridge {
    pub use crate::rom::{Cartridge, RomError, RomResult};
}
pub mod cpu;
pub mod debugger;
pub mod emulator;
pub mod hd;
pub mod input;
pub mod mapper;
pub mod ppu;
pub mod pose;
pub mod rom;
pub mod state;
pub mod types;

pub use bus::Bus;
pub use cpu::{Cpu, CpuState};
pub use emulator::Emulator;
pub use rom::{Cartridge, RomError, RomResult};
pub use types::{
    buttons, flags, Mirroring, Region, RomInfo, SCREEN_HEIGHT, SCREEN_WIDTH,
};
