//! Shared types used by the emulator core.
//!
//! This is a faithful Rust translation of the concepts in Mesen 0.9.9's
//! `Core/Types.h`, plus a few Rust-specific helpers.

/// NES CPU/PPU timing region.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Region {
    #[default]
    Ntsc,
    Pal,
    Dendy,
}

impl Region {
    /// CPU clock in Hz (master clock / 12 for NTSC/PAL, /15 for Dendy).
    pub const fn cpu_clock(self) -> u32 {
        match self {
            Region::Ntsc => 1_789_773,
            Region::Pal => 1_662_607,
            Region::Dendy => 1_773_448,
        }
    }

    /// PPU dots per second.
    pub const fn ppu_clock(self) -> u32 {
        self.cpu_clock() * 3
    }

    /// Number of PPU dots per CPU cycle. Always 3 on the NES.
    pub const fn ppu_divider(self) -> u32 {
        3
    }

    /// CPU master clock cycles per CPU cycle (Mesen `cpuDivider`).
    pub const fn cpu_master_clock_divider(self) -> u32 {
        match self {
            Region::Ntsc => 12,
            Region::Pal => 16,
            Region::Dendy => 15,
        }
    }

    /// Master clocks per PPU dot (Mesen `ppuDivider`).
    pub const fn ppu_master_clock_divider(self) -> u32 {
        match self {
            Region::Ntsc => 4,
            Region::Pal | Region::Dendy => 5,
        }
    }

    /// Mesen CPU `_startClockCount`, used to place memory accesses inside a
    /// CPU cycle.
    pub const fn cpu_start_clock_count(self) -> u32 {
        match self {
            Region::Ntsc => 6,
            Region::Pal => 8,
            Region::Dendy => 7,
        }
    }

    /// Mesen CPU `_endClockCount`.
    pub const fn cpu_end_clock_count(self) -> u32 {
        match self {
            Region::Ntsc => 6,
            Region::Pal => 8,
            Region::Dendy => 8,
        }
    }

    /// Scanlines per frame (including the post-render line).
    pub const fn scanlines_per_frame(self) -> u16 {
        match self {
            Region::Ntsc => 262,
            Region::Pal | Region::Dendy => 312,
        }
    }

    /// First scanline of vblank.
    pub const fn vblank_start(self) -> i16 {
        match self {
            Region::Ntsc => 241,
            Region::Pal | Region::Dendy => 241,
        }
    }

    pub const fn frame_rate(self) -> f64 {
        match self {
            Region::Ntsc => 60.0988,
            Region::Pal => 50.0070,
            Region::Dendy => 50.0070,
        }
    }
}

/// Nametable mirroring.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mirroring {
    #[default]
    Horizontal,
    Vertical,
    ScreenAOnly,
    ScreenBOnly,
    FourScreens,
}

impl Mirroring {
    pub fn from_ines(bits: u8) -> Self {
        if bits & 0x08 != 0 {
            Mirroring::FourScreens
        } else if bits & 0x01 != 0 {
            Mirroring::Vertical
        } else {
            Mirroring::Horizontal
        }
    }
}

/// Where a ROM came from / what it contains.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RomSystem {
    #[default]
    Nes,
    Fds,
    Nsf,
    Unif,
}

/// Information about the loaded cartridge.
#[derive(Clone, Debug)]
pub struct RomInfo {
    pub mapper: u16,
    pub submapper: u8,
    pub region: Region,
    pub mirroring: Mirroring,
    pub has_battery: bool,
    pub four_screen: bool,
    pub is_nes2: bool,
    pub system: RomSystem,
    pub prg_rom_size: usize,
    pub chr_rom_size: usize,
    pub prg_ram_size: usize,
    pub chr_ram_size: usize,
    pub prg_nvram_size: usize,
    pub chr_nvram_size: usize,
}

impl Default for RomInfo {
    fn default() -> Self {
        Self {
            mapper: 0,
            submapper: 0,
            region: Region::Ntsc,
            mirroring: Mirroring::Horizontal,
            has_battery: false,
            four_screen: false,
            is_nes2: false,
            system: RomSystem::Nes,
            prg_rom_size: 0,
            chr_rom_size: 0,
            prg_ram_size: 0,
            chr_ram_size: 0,
            prg_nvram_size: 0,
            chr_nvram_size: 0,
        }
    }
}

/// ROM contents handed to a mapper constructor.
#[derive(Clone, Debug, Default)]
pub struct RomData {
    pub info: RomInfo,
    pub prg_rom: Vec<u8>,
    pub chr_rom: Vec<u8>,
    pub trainer: Option<Vec<u8>>,
    /// Raw PRG-RAM/NVRAM restored from a .sav, if any.
    pub battery: Option<Vec<u8>>,
    /// Optional FDS BIOS image (8KB), supplied by the frontend.
    pub fds_bios: Option<Vec<u8>>,
}

impl RomData {
    pub fn new(info: RomInfo, prg_rom: Vec<u8>, chr_rom: Vec<u8>) -> Self {
        Self {
            info,
            prg_rom,
            chr_rom,
            trainer: None,
            battery: None,
            fds_bios: None,
        }
    }
}

pub const CPU_FREQ_NTSC: u32 = Region::Ntsc.cpu_clock();
pub const PPU_FREQ_NTSC: u32 = Region::Ntsc.ppu_clock();

pub const SCREEN_WIDTH: usize = 256;
pub const SCREEN_HEIGHT: usize = 240;
pub const SCREEN_PIXELS: usize = SCREEN_WIDTH * SCREEN_HEIGHT;

/// Classic 2C02 palette, 0x00RRGGBB.
pub const NES_PALETTE: [u32; 64] = [
    0x666666, 0x002A88, 0x1412A7, 0x3B00A4, 0x5C007E, 0x6E0040, 0x6C0600, 0x561D00,
    0x333500, 0x0B4800, 0x005200, 0x004F08, 0x00404D, 0x000000, 0x000000, 0x000000,
    0xADADAD, 0x155FD9, 0x4240FF, 0x7527FE, 0xA01ACC, 0xB71E7B, 0xB53120, 0x994E00,
    0x6B6D00, 0x388700, 0x0C9300, 0x008F32, 0x007C8D, 0x000000, 0x000000, 0x000000,
    0xFFFEFF, 0x64B0FF, 0x9290FF, 0xC676FF, 0xF36AFF, 0xFE6ECC, 0xFE8170, 0xEA9E22,
    0xBCBE00, 0x88D800, 0x5CE430, 0x45E082, 0x48CDDE, 0x4F4F4F, 0x000000, 0x000000,
    0xFFFEFF, 0xC0DFFF, 0xD3D2FF, 0xE8C8FF, 0xFBC2FF, 0xFEC4EA, 0xFECCC5, 0xF7D8A5,
    0xE4E594, 0xCFEF96, 0xBDF4AB, 0xB3F3CC, 0xB5EBF2, 0xB8B8B8, 0x000000, 0x000000,
];

/// Palette index -> XRGB pixel.
#[inline]
pub fn palette_color(index: u8) -> u32 {
    NES_PALETTE[(index & 0x3F) as usize]
}

/// Controller button bit mask. Matches Mesen's `ControllerButtons`.
pub mod buttons {
    pub const A: u8 = 0x01;
    pub const B: u8 = 0x02;
    pub const SELECT: u8 = 0x04;
    pub const START: u8 = 0x08;
    pub const UP: u8 = 0x10;
    pub const DOWN: u8 = 0x20;
    pub const LEFT: u8 = 0x40;
    pub const RIGHT: u8 = 0x80;
    pub const NONE: u8 = 0x00;
}

/// 6502 status flags (same bit layout as the real chip).
pub mod flags {
    pub const CARRY: u8 = 0x01;
    pub const ZERO: u8 = 0x02;
    pub const INTERRUPT: u8 = 0x04;
    pub const DECIMAL: u8 = 0x08;
    pub const BREAK: u8 = 0x10;
    pub const RESERVED: u8 = 0x20;
    pub const OVERFLOW: u8 = 0x40;
    pub const NEGATIVE: u8 = 0x80;
}

/// Kinds of memory accesses, used for debugging and for dummy reads/writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryAccessType {
    Read,
    Write,
    ExecOpCode,
    ExecOperand,
    PpuRenderingRead,
    DummyRead,
    DmcRead,
    DummyWrite,
}

/// IRQ sources, matching Mesen's `IRQSource`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrqSource {
    External = 1,
    FrameCounter = 2,
    Dmc = 4,
    FdsDisk = 8,
    Mapper = 16,
}
