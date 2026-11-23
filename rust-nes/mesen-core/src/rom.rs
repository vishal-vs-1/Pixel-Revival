//! ROM loading and cartridge wrapper.
//!
//! Rust port of Mesen 0.9.9's `RomLoader.cpp`, `iNesLoader.cpp`,
//! `NESHeader.cpp`, `UnifLoader.cpp`/`UnifBoards.h`, `NsfLoader.cpp`,
//! `NsfeLoader.h` and `FdsLoader.cpp`.
//!
//! Original C++: Copyright (C) 2014-2020 Sour, GPL-3.0.
//! Rust port: GPL-3.0-or-later.
//!
//! Supported container formats: iNES 1.0, archaic iNES, NES 2.0, UNIF,
//! NSF, NSFe and fwNES/raw FDS images. The mapper is only constructed by
//! [`Cartridge`]; [`parse_rom`] is a pure container parser and is also used by
//! unit tests.
//!
//! ## Special-format byte layouts (loader -> mapper contract)
//!
//! ### NSF
//! `RomData.prg_rom` is the **entire NSF file**, starting with the 0x80-byte
//! NSF header (`NESM\x1a`). `chr_rom` is empty; the mapper uses 8KB CHR RAM.
//! `info.mapper = 31`, `info.system = Nsf`. The header can be decoded with
//! [`parse_nsf_header`].
//!
//! ### NSFe
//! NSFe has no plain NSF header, so the loader synthesises a canonical
//! 0x80-byte NSF header from the `INFO`/`BANK` chunks followed by the raw
//! `DATA` chunk bytes. The resulting `prg_rom` therefore has the same shape as
//! a plain NSF image and can be consumed by the same mapper. (Mesen keeps the
//! parsed header in `RomData.Info.NsfInfo` instead; the Rust `RomInfo` has no
//! NSF fields, hence the synthesis.)
//!
//! ### FDS
//! `RomData.prg_rom` is the concatenation of the raw fwNES disk sides with the
//! 16-byte fwNES file header removed: exactly `sides * 65500` bytes, no gap
//! bytes, no per-side header other than the FDS block stream that is part of
//! the side itself. This is byte-for-byte what Mesen's
//! `FdsLoader::LoadDiskData` consumes after skipping the fwNES header; the
//! mapper can slice sides with `side * 65500`. `RomData.fds_bios` carries the
//! optional 8KB BIOS and is never folded into `prg_rom`.

use crate::mapper::{create_mapper, Mapper, MapperCommon};
use crate::state::{StateReader, StateResult, StateWriter};
use crate::types::{Mirroring, Region, RomData, RomInfo, RomSystem};

/// Size of an iNES/NES 2.0 header.
const INES_HEADER_SIZE: usize = 16;
/// Size of the optional trainer stored between the header and PRG ROM.
const TRAINER_SIZE: usize = 512;
/// Size of the fwNES file header.
const FDS_HEADER_SIZE: usize = 16;
/// Size of one raw FDS disk side (both in fwNES files and headerless dumps).
const FDS_SIDE_SIZE: usize = 65500;
/// Size of a plain NSF header.
const NSF_HEADER_SIZE: usize = 0x80;
/// Mapper IDs prescribed by PORTING.md §5.
const NSF_MAPPER_ID: u16 = 31;
const FDS_MAPPER_ID: u16 = 20;
const VS_MAPPER_ID: u16 = 99;

/// Raw FDS disk image magic (no fwNES header). Byte 0 is the block type of the
/// disk-header block that every side starts with.
const FDS_RAW_MAGIC: &[u8; 15] = b"\x01*NINTENDO-HVC*";

#[derive(Clone, Debug)]
pub struct RomError(pub String);

impl std::fmt::Display for RomError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for RomError {}

impl From<&str> for RomError {
    fn from(value: &str) -> Self {
        RomError(value.to_string())
    }
}

impl From<String> for RomError {
    fn from(value: String) -> Self {
        RomError(value)
    }
}

pub type RomResult<T> = Result<T, RomError>;

/// A loaded cartridge plus its mapper.
pub struct Cartridge {
    mapper: Box<dyn Mapper>,
}

impl Cartridge {
    pub fn from_rom_data(data: RomData) -> RomResult<Self> {
        let id = data.info.mapper;
        // Mesen's BaseMapper copies the trainer into work/save RAM before
        // LoadBattery() runs, so a battery image wins over trainer bytes.
        // MapperCommon already loaded `battery`; keep copies so we can restore
        // that ordering after applying the trainer.
        let trainer = data.trainer.clone();
        let battery = data.battery.clone();
        let mut mapper = create_mapper(data)
            .ok_or_else(|| RomError(format!("unsupported mapper: {}", id)))?;
        apply_trainer(mapper.common_mut(), trainer.as_deref());
        if let Some(battery) = battery.as_deref() {
            let common = mapper.common_mut();
            if !common.prg_ram.is_empty() {
                let len = battery.len().min(common.prg_ram.len());
                common.prg_ram[..len].copy_from_slice(&battery[..len]);
            }
        }
        Ok(Self { mapper })
    }

    pub fn from_bytes(data: &[u8]) -> RomResult<Self> {
        let rom = parse_rom(data)?;
        Self::from_rom_data(rom)
    }

    /// Load with an optional FDS BIOS image (8KB).
    pub fn from_bytes_with_bios(data: &[u8], fds_bios: Option<&[u8]>) -> RomResult<Self> {
        let mut rom = parse_rom(data)?;
        if let Some(bios) = fds_bios {
            if bios.len() < 0x2000 {
                return Err(RomError(
                    "FDS BIOS must be 8192 bytes (got a shorter image)".into(),
                ));
            }
            rom.fds_bios = Some(bios[..0x2000].to_vec());
        }
        Self::from_rom_data(rom)
    }

    pub fn mapper(&self) -> &dyn Mapper {
        self.mapper.as_ref()
    }

    pub fn mapper_mut(&mut self) -> &mut dyn Mapper {
        self.mapper.as_mut()
    }

    pub fn mapper_id(&self) -> u16 {
        self.mapper.mapper_id()
    }

    pub fn info(&self) -> &RomInfo {
        &self.mapper.common().info
    }

    pub fn common(&self) -> &MapperCommon {
        self.mapper.common()
    }

    pub fn reset(&mut self) {
        self.mapper.reset();
    }

    #[inline]
    pub fn cpu_read(&mut self, addr: u16) -> Option<u8> {
        self.mapper.cpu_read(addr)
    }

    #[inline]
    pub fn cpu_write(&mut self, addr: u16, value: u8) {
        self.mapper.cpu_write(addr, value);
    }

    #[inline]
    pub fn ppu_read(&mut self, addr: u16) -> Option<u8> {
        self.mapper.ppu_read(addr)
    }

    #[inline]
    pub fn ppu_write(&mut self, addr: u16, value: u8) {
        self.mapper.ppu_write(addr, value);
    }

    #[inline]
    pub fn scanline(&mut self, scanline: i16) {
        self.mapper.scanline(scanline);
    }

    #[inline]
    pub fn cpu_cycle(&mut self) {
        self.mapper.cpu_cycle();
    }

    #[inline]
    pub fn irq_pending(&self) -> bool {
        self.mapper.irq_pending()
    }

    #[inline]
    pub fn expansion_audio_sample(&self) -> f32 {
        self.mapper.expansion_audio_sample()
    }

    pub fn battery(&self) -> Option<&[u8]> {
        self.mapper.battery()
    }

    pub fn set_battery(&mut self, data: &[u8]) {
        self.mapper.set_battery(data);
    }

    pub fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        self.mapper.save_state(&mut w);
        w.into_inner()
    }

    pub fn load_state(&mut self, data: &[u8]) -> StateResult<()> {
        let mut r = StateReader::new(data);
        self.mapper.load_state(&mut r)
    }
}

/// Mesen copies the 512-byte trainer to `$7000-$71FF` (`work_ram + 0x1000`,
/// or save RAM when the work RAM is smaller) before the mapper is initialised.
/// `MapperCommon` exposes one 8KB PRG-RAM window at `$6000-$7FFF`, so the
/// trainer lands at offset 0x1000 of that buffer.
fn apply_trainer(common: &mut MapperCommon, trainer: Option<&[u8]>) {
    if let Some(trainer) = trainer {
        if trainer.len() == TRAINER_SIZE && common.prg_ram.len() >= 0x2000 {
            common.prg_ram[0x1000..0x1000 + TRAINER_SIZE].copy_from_slice(trainer);
        }
    }
}

/// Detect and parse a ROM container. The returned [`RomData`] is not yet
/// validated against the mapper factory; use [`Cartridge::from_bytes`] for
/// that.
pub fn parse_rom(data: &[u8]) -> RomResult<RomData> {
    if data.len() < 4 {
        return Err(RomError(
            "file too small to be a ROM (need at least 4 bytes)".into(),
        ));
    }

    if data.starts_with(b"NES\x1A") {
        parse_ines(data)
    } else if data.starts_with(b"FDS\x1A")
        || (data.len() >= FDS_RAW_MAGIC.len() && &data[..FDS_RAW_MAGIC.len()] == FDS_RAW_MAGIC)
    {
        parse_fds(data)
    } else if data.starts_with(b"NESM\x1A") {
        parse_nsf(data)
    } else if data.starts_with(b"NSFE") {
        parse_nsfe(data)
    } else if data.starts_with(b"UNIF") {
        parse_unif(data)
    } else {
        Err(RomError(format!(
            "unrecognized ROM format (magic {:02X?}; expected iNES, NES 2.0, UNIF, NSF, NSFe or FDS)",
            &data[..4]
        )))
    }
}

// ---------------------------------------------------------------------------
// iNES 1.0 / archaic iNES / NES 2.0
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeaderVersion {
    /// iNES 1.0 (byte 7 bits 2-3 == 0).
    INes,
    /// NES 2.0 (byte 7 bits 2-3 == 2).
    Nes20,
    /// Archaic/old iNES (byte 7 bits 2-3 == 1 or 3). Only byte 6 is trusted.
    Archaic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConsoleType {
    Nes,
    VsSystem,
    PlayChoice,
}

impl HeaderVersion {
    fn detect(byte7: u8) -> Self {
        match byte7 & 0x0C {
            0x08 => HeaderVersion::Nes20,
            0x00 => HeaderVersion::INes,
            _ => HeaderVersion::Archaic,
        }
    }
}

/// NES 2.0 size encoding. `high` is byte 9's low nibble (PRG) or high nibble
/// (CHR); `unit_shift` is 14 for PRG (16KB) and 13 for CHR (8KB).
///
/// An exponent-multiplier value of 0x0F in `high` selects the MSB/LSB notation
/// described by the NES 2.0 spec; Mesen's `NESHeader::GetSizeValue` is the
/// reference for the arithmetic.
fn nes2_rom_size(count: u8, high: u8, unit_shift: u32) -> RomResult<usize> {
    if high == 0x0F {
        // Byte 4/5: bits 0-5 exponent, bits 6-7 multiplier selector.
        let exponent = u32::from(count >> 2).min(60);
        let multiplier = u64::from(count & 0x03);
        let bytes = (multiplier * 2 + 1)
            .checked_shl(exponent)
            .ok_or_else(|| RomError("NES 2.0: PRG/CHR size overflows 64 bits".into()))?;
        if bytes > u64::from(u32::MAX) {
            return Err(RomError(
                "NES 2.0: PRG/CHR size is too large (over 4GB)".into(),
            ));
        }
        Ok(bytes as usize)
    } else {
        let pages = ((usize::from(high)) << 8) | usize::from(count);
        pages
            .checked_shl(unit_shift)
            .ok_or_else(|| RomError("NES 2.0: PRG/CHR size overflows".into()))
    }
}

/// NES 2.0 RAM/NVRAM shift notation: 0 means "none", otherwise
/// `128 * 2^(value-1)` bytes (equivalent to `64 << value`).
fn nes2_ram_size(value: u8) -> usize {
    if value == 0 {
        0
    } else {
        64usize << value
    }
}

fn parse_ines(data: &[u8]) -> RomResult<RomData> {
    if data.len() < INES_HEADER_SIZE {
        return Err(RomError(format!(
            "iNES: truncated header ({} of {} bytes)",
            data.len(),
            INES_HEADER_SIZE
        )));
    }

    let header = &data[..INES_HEADER_SIZE];
    let prg_count = header[4];
    let chr_count = header[5];
    let byte6 = header[6];
    let byte7 = header[7];
    let byte8 = header[8];
    let byte9 = header[9];
    let byte10 = header[10];
    let byte11 = header[11];
    let byte12 = header[12];
    let byte13 = header[13];

    let version = HeaderVersion::detect(byte7);
    let is_nes2 = version == HeaderVersion::Nes20;

    // Console type. Archaic headers have no console bits (Mesen's
    // NESHeader::GetGameSystem() only inspects them for iNES/NES 2.0).
    let console = match version {
        HeaderVersion::Nes20 => match byte7 & 0x03 {
            0x01 => ConsoleType::VsSystem,
            0x02 => ConsoleType::PlayChoice,
            0x03 => match byte13 {
                0x01 => ConsoleType::VsSystem,
                0x02 => ConsoleType::PlayChoice,
                _ => ConsoleType::Nes,
            },
            _ => ConsoleType::Nes,
        },
        HeaderVersion::INes => {
            if byte7 & 0x01 != 0 {
                ConsoleType::VsSystem
            } else if byte7 & 0x02 != 0 {
                ConsoleType::PlayChoice
            } else {
                ConsoleType::Nes
            }
        }
        HeaderVersion::Archaic => ConsoleType::Nes,
    };

    // Mapper ID: NES 2.0 uses byte 8's low nibble as bits 8-11; iNES uses
    // byte 7's high nibble; archaic headers only have byte 6's high nibble.
    let header_mapper = match version {
        HeaderVersion::Nes20 => {
            (u16::from(byte8 & 0x0F) << 8)
                | u16::from(byte7 & 0xF0)
                | u16::from(byte6 >> 4)
        }
        HeaderVersion::INes => u16::from(byte7 & 0xF0) | u16::from(byte6 >> 4),
        HeaderVersion::Archaic => u16::from(byte6 >> 4),
    };
    // PORTING.md §5: VS System cartridges are exposed as mapper 99. The
    // original mapper bits are not preserved (the Vs mapper decides its own
    // variant from PRG/CHR sizes and the NES 2.0 VS type byte, which the Rust
    // RomInfo cannot currently carry).
    let mapper = if console == ConsoleType::VsSystem {
        VS_MAPPER_ID
    } else {
        header_mapper
    };
    let submapper = if is_nes2 && console != ConsoleType::VsSystem {
        byte8 >> 4
    } else {
        0
    };

    // Mesen can override these sizes from its game database. This port has no
    // database, so the header is authoritative (the fallback used by
    // iNesLoader::LoadRom when the DB has no entry).
    let (prg_rom_size, chr_rom_size) = match version {
        HeaderVersion::Nes20 => (
            nes2_rom_size(prg_count, byte9 & 0x0F, 14)?,
            nes2_rom_size(chr_count, byte9 >> 4, 13)?,
        ),
        // Mesen treats a PRG count of 0 in a non-NES 2.0 header as 256 banks
        // (4MB), not as an empty ROM.
        HeaderVersion::INes | HeaderVersion::Archaic => {
            let prg = if prg_count == 0 {
                256 * 0x4000
            } else {
                usize::from(prg_count) * 0x4000
            };
            (prg, usize::from(chr_count) * 0x2000)
        }
    };

    let (prg_ram_size, prg_nvram_size, mut chr_ram_size, chr_nvram_size) = if is_nes2 {
        (
            nes2_ram_size(byte10 & 0x0F),
            nes2_ram_size(byte10 >> 4),
            nes2_ram_size(byte11 & 0x0F),
            nes2_ram_size(byte11 >> 4),
        )
    } else {
        // iNES 1.0 has no size fields. Mesen's NESHeader reports -1 and
        // BaseMapper falls back to: 8KB save RAM for battery carts (no work
        // RAM), or 8KB work RAM for battery-less carts (no save RAM). This is
        // also what mapper defaults (e.g. MMC1 SOROM detection) expect.
        if byte6 & 0x02 != 0 {
            (0, 0x2000, 0, 0)
        } else {
            (0x2000, 0, 0, 0)
        }
    };

    // An iNES/NES 2.0 image with no CHR ROM must have CHR RAM; default to 8KB
    // when the header does not specify a usable size (MapperCommon enforces
    // this too, but keeping info accurate avoids surprising mapper code).
    if chr_rom_size == 0 && chr_ram_size == 0 {
        chr_ram_size = 0x2000;
    }

    let has_trainer = byte6 & 0x04 != 0;
    let mut offset = INES_HEADER_SIZE;
    let trainer = if has_trainer {
        if data.len() < INES_HEADER_SIZE + TRAINER_SIZE {
            return Err(RomError(format!(
                "iNES: truncated trainer (header declares a 512-byte trainer, file provides {} bytes)",
                data.len().saturating_sub(INES_HEADER_SIZE)
            )));
        }
        let trainer = data[INES_HEADER_SIZE..INES_HEADER_SIZE + TRAINER_SIZE].to_vec();
        offset += TRAINER_SIZE;
        Some(trainer)
    } else {
        None
    };

    let total_size = prg_rom_size
        .checked_add(chr_rom_size)
        .ok_or_else(|| RomError("iNES: PRG+CHR size overflows".into()))?;
    let available = data.len().saturating_sub(offset);
    if total_size > available {
        return Err(RomError(format!(
            "iNES: truncated ROM: header declares {} bytes of PRG+CHR, file provides {} bytes after the header{}",
            total_size,
            available,
            if has_trainer { " and trainer" } else { "" }
        )));
    }

    let prg_rom = data[offset..offset + prg_rom_size].to_vec();
    offset += prg_rom_size;
    let chr_rom = data[offset..offset + chr_rom_size].to_vec();

    let four_screen = byte6 & 0x08 != 0;
    let mirroring = if four_screen {
        Mirroring::FourScreens
    } else {
        Mirroring::from_ines(byte6)
    };

    // Region: NES 2.0 uses byte 12; iNES 1.0 only has byte 9 bit 0. Archaic
    // headers are untrusted beyond byte 6 (Mesen leaves them Unknown/NTSC).
    let region = match console {
        ConsoleType::VsSystem | ConsoleType::PlayChoice => Region::Ntsc,
        ConsoleType::Nes => match version {
            HeaderVersion::Nes20 => match byte12 & 0x03 {
                0x01 => Region::Pal,
                0x03 => Region::Dendy,
                _ => Region::Ntsc,
            },
            HeaderVersion::INes => {
                if byte9 & 0x01 != 0 {
                    Region::Pal
                } else {
                    Region::Ntsc
                }
            }
            HeaderVersion::Archaic => Region::Ntsc,
        },
    };

    let info = RomInfo {
        mapper,
        submapper,
        region,
        mirroring,
        has_battery: byte6 & 0x02 != 0,
        four_screen,
        is_nes2,
        system: RomSystem::Nes,
        prg_rom_size,
        chr_rom_size,
        prg_ram_size,
        chr_ram_size,
        prg_nvram_size,
        chr_nvram_size,
    };

    let mut rom = RomData::new(info, prg_rom, chr_rom);
    rom.trainer = trainer;
    Ok(rom)
}

// ---------------------------------------------------------------------------
// UNIF
// ---------------------------------------------------------------------------

/// Map a UNIF board name to an iNES mapper ID. The table is a direct port of
/// Mesen's `UnifLoader::_boardMappings` (including the `NES-`, `UNL-`,
/// `HVC-`, `BTL-` and `BMC-` prefix stripping performed by
/// `UnifLoader::GetMapperID`).
///
/// Returns `None` for unknown boards and for boards that Mesen implements
/// with a private `UnifBoards` mapper ID (32768+). Those private IDs have no
/// iNES equivalent in this port, so the loader reports them as unsupported
/// instead of inventing an ID.
fn unif_board_mapper(board: &str) -> Option<u16> {
    let name = ["NES-", "UNL-", "HVC-", "BTL-", "BMC-"]
        .iter()
        .find_map(|prefix| board.strip_prefix(prefix))
        .unwrap_or(board);

    Some(match name {
        "11160" => 299,
        "12-IN-1" => 331,
        "190in1" => 300,
        "22211" => 132,
        "411120-C" => 287,
        "42in1ResetSwitch" => 226,
        "43272" => 227,
        "603-5052" => 238,
        "64in1NoRepeat" => 314,
        "70in1" | "70in1B" => 236,
        "810544-C-A1" => 261,
        "8157" => 301,
        "8237" => 215,
        "830118C" => 348,
        "830425C-4391T" => 320,
        "8-IN-1" => 333,
        "A65AS" => 285,
        "ANROM" => 7,
        "AX5705" => 530,
        "BB" => 108,
        "BS-5" => 286,
        "CITYFIGHT" => 266,
        "CHINA_ER_SAN2" => 19,
        "CNROM" => 3,
        "COOLBOY" => 268,
        "CPROM" => 13,
        "D1038" => 59,
        "DANCE2000" => 518,
        "DREAMTECH01" => 521,
        "DRAGONFIGHTER" => 292,
        "DRIPGAME" => 284,
        "EDU2000" => 329,
        "EH8813A" => 519,
        "EKROM" | "ELROM" | "ETROM" | "EWROM" => 5,
        "F-15" => 259,
        "FARID_SLROM_8-IN-1" => 323,
        "FARID_UNROM_8-IN-1" => 324,
        "FK23C" | "FK23CA" => 176,
        "FS304" => 162,
        "G-146" => 349,
        "GK-192" => 58,
        "GS-2004" => 283,
        "H2288" => 123,
        "HKROM" => 4,
        "HP898F" => 319,
        "HPxx" | "HP2018A" => 260,
        "K-3046" => 336,
        "KOF97" => 263,
        "KONAMI-QTAI" => 190,
        "KS7012" => 346,
        "KS7013B" => 312,
        "KS7016" => 306,
        "KS7017" => 303,
        "KS7031" => 305,
        "KS7032" => 142,
        "KS7037" => 307,
        "KS7057" => 302,
        "LH10" => 522,
        "LH32" => 125,
        "LH51" => 309,
        "MALISB" => 325,
        "MHROM" => 66,
        "N625092" => 221,
        "NROM" | "NROM-128" | "NROM-256" | "RROM" | "RROM-128" => 0,
        "NTBROM" => 68,
        "NTD-03" => 290,
        "NovelDiamond9999999in1" => 201,
        "RESET-TXROM" => 313,
        "RET-CUFROM" => 29,
        "RT-01" => 328,
        "SA-002" => 136,
        "SA-0036" => 149,
        "SA-0037" => 148,
        "SA-009" => 160,
        "SA-016-1M" => 146,
        "SA-72007" => 145,
        "SA-72008" => 133,
        "SA-9602B" => 513,
        "SA-NROM" => 143,
        "SAROM" | "SBROM" | "SCROM" | "SEROM" | "SGROM" | "SKROM" | "SL1ROM"
        | "SLROM" | "SNROM" | "SOROM" => 1,
        "SC-127" => 35,
        "SHERO" => 262,
        "SL12" => 116,
        "SL1632" => 14,
        "SMB2J" => 304,
        "SUNSOFT_UNROM" => 93,
        "Sachen-74LS374N" => 150,
        "Sachen-74LS374NA" => 243,
        "Sachen-8259A" => 141,
        "Sachen-8259B" => 138,
        "Sachen-8259C" => 139,
        "Sachen-8259D" => 137,
        "Super24in1SC03" => 176,
        "SuperHIK8in1" => 45,
        "Supervision16in1" => 53,
        "T-230" => 529,
        "T-262" => 265,
        "TBROM" | "TEROM" | "TFROM" | "TGROM" | "TKROM" | "TKSROM" | "TLROM"
        | "TLSROM" | "TQROM" | "TR1ROM" | "TSROM" | "TVROM" => 4,
        "TC-U01-1.5M" => 147,
        "TEK90" => 90,
        "TF1201" => 298,
        "UNROM" | "UOROM" => 2,
        "UNROM-512-8" | "UNROM-512-16" | "UNROM-512-32" => 30,
        "VRC7" => 85,
        "WAIXING-FW01" => 227,
        "WS" => 332,
        "YOKO" => 264,
        "158B" => 258,
        "60311C" => 289,
        "80013-B" => 274,
        _ => return None,
    })
}

fn read_le_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn read_le_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

/// True if `pos` looks like the start of a well-formed UNIF chunk. Used only
/// to decide whether bytes between a chunk and the next 4-byte boundary are
/// padding (some writers pad chunks, the original spec does not).
fn unif_chunk_header_ok(data: &[u8], pos: usize) -> bool {
    let Some(header_end) = pos.checked_add(8) else {
        return false;
    };
    if header_end > data.len() {
        return false;
    }
    if !data[pos..pos + 4]
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric())
    {
        return false;
    }
    let length = read_le_u32(data, pos + 4) as usize;
    header_end
        .checked_add(length)
        .is_some_and(|end| end <= data.len())
}

fn fourcc_string(id: &[u8]) -> String {
    id.iter()
        .map(|&b| {
            if b.is_ascii_graphic() {
                char::from(b)
            } else {
                '?'
            }
        })
        .collect()
}

/// Read a UNIF NUL-terminated name, ignoring spaces (Mesen's `ReadString`).
fn unif_read_name(payload: &[u8]) -> String {
    let mut name = String::new();
    for &byte in payload {
        if byte == 0 {
            break;
        }
        if byte != b' ' {
            name.push(char::from(byte));
        }
    }
    name
}

fn parse_unif(data: &[u8]) -> RomResult<RomData> {
    // "UNIF" + 4-byte version + 24 reserved bytes.
    const UNIF_HEADER_SIZE: usize = 32;
    if data.len() < UNIF_HEADER_SIZE {
        return Err(RomError(format!(
            "UNIF: truncated header ({} of {} bytes)",
            data.len(),
            UNIF_HEADER_SIZE
        )));
    }

    let mut prg_chunks: [Vec<u8>; 16] = std::array::from_fn(|_| Vec::new());
    let mut chr_chunks: [Vec<u8>; 16] = std::array::from_fn(|_| Vec::new());
    let mut mapper_name: Option<String> = None;
    let mut mirroring = Mirroring::Horizontal;
    let mut region = Region::Ntsc;
    let mut has_battery = false;

    let mut pos = UNIF_HEADER_SIZE;
    while pos < data.len() {
        if data.len() - pos < 8 {
            // Trailing alignment bytes / junk; Mesen stops here as well.
            break;
        }

        let id = &data[pos..pos + 4];
        if !id.iter().all(|byte| byte.is_ascii_alphanumeric()) {
            break;
        }
        let length = read_le_u32(data, pos + 4) as usize;
        let payload_start = pos + 8;
        let Some(chunk_end) = payload_start.checked_add(length) else {
            return Err(RomError("UNIF: chunk length overflows".into()));
        };
        if chunk_end > data.len() {
            return Err(RomError(format!(
                "UNIF: truncated '{}' chunk (declares {} bytes, {} available)",
                fourcc_string(id),
                length,
                data.len() - payload_start
            )));
        }
        let payload = &data[payload_start..chunk_end];

        match id {
            b"MAPR" => {
                let name = unif_read_name(payload);
                if name.is_empty() {
                    return Err(RomError("UNIF: empty MAPR board name".into()));
                }
                mapper_name = Some(name);
            }
            b"TVCI" => {
                if let Some(&value) = payload.first() {
                    region = if value == 1 { Region::Pal } else { Region::Ntsc };
                }
            }
            b"BATR" => {
                if let Some(&value) = payload.first() {
                    has_battery = value != 0;
                }
            }
            b"MIRR" => {
                if let Some(&value) = payload.first() {
                    mirroring = match value {
                        1 => Mirroring::Vertical,
                        2 => Mirroring::ScreenAOnly,
                        3 => Mirroring::ScreenBOnly,
                        4 => Mirroring::FourScreens,
                        _ => Mirroring::Horizontal,
                    };
                }
            }
            _ => {
                if id.starts_with(b"PRG") || id.starts_with(b"CHR") {
                    if let Some(index) = char::from(id[3]).to_digit(16) {
                        let index = index as usize;
                        if id.starts_with(b"PRG") {
                            prg_chunks[index].extend_from_slice(payload);
                        } else {
                            chr_chunks[index].extend_from_slice(payload);
                        }
                    }
                }
                // CTRL and unknown chunks (PCKn, CCKn, NAME, WRTR, READ,
                // DINF, VROR, ...) are intentionally ignored.
            }
        }

        // Advance past the chunk, tolerating both padded and unpadded files.
        let mut next = chunk_end;
        if next % 4 != 0 {
            let aligned = next.saturating_add(3) & !3;
            if !unif_chunk_header_ok(data, next)
                && aligned <= data.len()
                && unif_chunk_header_ok(data, aligned)
            {
                next = aligned;
            }
        }
        pos = next;
    }

    let board = mapper_name.ok_or_else(|| RomError("UNIF: missing MAPR chunk".into()))?;
    let mapper = unif_board_mapper(&board).ok_or_else(|| {
        RomError(format!(
            "UNIF: unsupported board '{}' (no iNES mapper equivalent in this build)",
            board
        ))
    })?;

    let mut prg_rom = Vec::new();
    for chunk in prg_chunks {
        prg_rom.extend_from_slice(&chunk);
    }
    let mut chr_rom = Vec::new();
    for chunk in chr_chunks {
        chr_rom.extend_from_slice(&chunk);
    }

    if prg_rom.is_empty() {
        return Err(RomError(format!(
            "UNIF: board '{}' has no PRG ROM data",
            board
        )));
    }

    let prg_rom_size = prg_rom.len();
    let chr_rom_size = chr_rom.len();
    let chr_ram_size = if chr_rom_size == 0 { 0x2000 } else { 0 };
    let four_screen = mirroring == Mirroring::FourScreens;

    let info = RomInfo {
        mapper,
        submapper: 0,
        region,
        mirroring,
        has_battery,
        four_screen,
        is_nes2: false,
        system: RomSystem::Unif,
        prg_rom_size,
        chr_rom_size,
        // UNIF has no RAM-size chunks; use the same BaseMapper fallback as
        // iNES 1.0: 8KB save RAM for battery boards, 8KB work RAM otherwise.
        prg_ram_size: if has_battery { 0 } else { 0x2000 },
        chr_ram_size,
        prg_nvram_size: if has_battery { 0x2000 } else { 0 },
        chr_nvram_size: 0,
    };

    Ok(RomData::new(info, prg_rom, chr_rom))
}

// ---------------------------------------------------------------------------
// NSF / NSFe
// ---------------------------------------------------------------------------

/// Decoded plain-NSF header fields. The raw header remains available in
/// `RomData.prg_rom[0..0x80]` for mappers that prefer to read it directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NsfHeaderInfo {
    pub version: u8,
    pub total_songs: u8,
    /// 1-based song number (as stored in the NSF header).
    pub starting_song: u8,
    pub load_address: u16,
    pub init_address: u16,
    pub play_address: u16,
    pub play_speed_ntsc: u16,
    pub play_speed_pal: u16,
    pub bank_setup: [u8; 8],
    /// Bit 0: PAL when set, NTSC when clear. Bit 1: dual-compatible.
    pub flags: u8,
    pub sound_chips: u8,
}

/// Decode the 0x80-byte plain NSF header at the start of `data`.
pub fn parse_nsf_header(data: &[u8]) -> Option<NsfHeaderInfo> {
    if data.len() < NSF_HEADER_SIZE || &data[..5] != b"NESM\x1A" {
        return None;
    }
    let mut bank_setup = [0u8; 8];
    bank_setup.copy_from_slice(&data[0x70..0x78]);
    Some(NsfHeaderInfo {
        version: data[5],
        total_songs: data[6],
        starting_song: data[7],
        load_address: read_le_u16(data, 0x08),
        init_address: read_le_u16(data, 0x0A),
        play_address: read_le_u16(data, 0x0C),
        play_speed_ntsc: read_le_u16(data, 0x6E),
        bank_setup,
        play_speed_pal: read_le_u16(data, 0x78),
        flags: data[0x7A],
        sound_chips: data[0x7B],
    })
}

fn nsf_region(flags: u8) -> Region {
    // Mesen: Flags == 0x01 means PAL, everything else defaults to NTSC.
    if flags == 0x01 {
        Region::Pal
    } else {
        Region::Ntsc
    }
}

fn nsf_info(prg_rom_size: usize, region: Region) -> RomInfo {
    RomInfo {
        mapper: NSF_MAPPER_ID,
        submapper: 0,
        region,
        mirroring: Mirroring::Horizontal,
        has_battery: false,
        four_screen: false,
        is_nes2: false,
        system: RomSystem::Nsf,
        prg_rom_size,
        chr_rom_size: 0,
        // Mesen's NsfMapper exposes 16KB of work RAM (GetWorkRamSize() =
        // 0x4000) for the player, and 8KB CHR RAM (there is no CHR ROM).
        prg_ram_size: 0x4000,
        chr_ram_size: 0x2000,
        prg_nvram_size: 0,
        chr_nvram_size: 0,
    }
}

fn parse_nsf(data: &[u8]) -> RomResult<RomData> {
    let header = parse_nsf_header(data).ok_or_else(|| {
        RomError(format!(
            "NSF: truncated header ({} of {} bytes)",
            data.len(),
            NSF_HEADER_SIZE
        ))
    })?;

    if header.total_songs == 0 {
        return Err(RomError("NSF: header declares zero songs".into()));
    }
    if header.load_address < 0x6000 {
        return Err(RomError(format!(
            "NSF: invalid load address ${:04X} (must be at least $6000)",
            header.load_address
        )));
    }

    let info = nsf_info(data.len(), nsf_region(header.flags));
    // PORTING.md §5: the whole file (header included) is the mapper's PRG ROM.
    Ok(RomData::new(info, data.to_vec(), Vec::new()))
}

#[derive(Clone, Copy, Debug)]
struct NsfeInfoChunk {
    load_address: u16,
    init_address: u16,
    play_address: u16,
    flags: u8,
    sound_chips: u8,
    total_songs: u8,
    /// 0-based in NSFe (converted to the 1-based NSF representation).
    starting_song: u8,
}

/// Parse an NSFe file into a canonical NSF-shaped image: a synthesised
/// 0x80-byte header followed by the `DATA` chunk. Optional chunks are
/// ignored; unknown *required* (uppercase) chunks reject the file, matching
/// Mesen's `NsfeLoader`.
fn parse_nsfe(data: &[u8]) -> RomResult<RomData> {
    let mut pos = 4usize; // after "NSFE"
    let mut info: Option<NsfeInfoChunk> = None;
    let mut bank_setup = [0u8; 8];
    let mut prg_data = Vec::new();
    let mut saw_data = false;

    while data.len() - pos >= 8 {
        let size = read_le_u32(data, pos) as usize;
        let id = [data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]];

        // NSFe spec (and Mesen's NsfeLoader): the length field is the payload
        // length and excludes the 8-byte chunk header.
        let payload_start = pos + 8;
        let Some(next) = payload_start.checked_add(size) else {
            return Err(RomError("NSFe: chunk length overflows".into()));
        };
        if next > data.len() {
            return Err(RomError(format!(
                "NSFe: truncated '{}' chunk (declares {} bytes, {} available)",
                fourcc_string(&id),
                size,
                data.len() - payload_start
            )));
        }

        let payload = &data[payload_start..next];
        match &id {
            b"INFO" => {
                if payload.len() < 10 {
                    return Err(RomError(format!(
                        "NSFe: INFO chunk too short ({} bytes, expected 10)",
                        payload.len()
                    )));
                }
                info = Some(NsfeInfoChunk {
                    load_address: read_le_u16(payload, 0),
                    init_address: read_le_u16(payload, 2),
                    play_address: read_le_u16(payload, 4),
                    flags: payload[6],
                    sound_chips: payload[7],
                    total_songs: payload[8],
                    starting_song: payload[9],
                });
            }
            b"BANK" => {
                let len = payload.len().min(bank_setup.len());
                bank_setup[..len].copy_from_slice(&payload[..len]);
            }
            b"DATA" => {
                prg_data.extend_from_slice(payload);
                saw_data = true;
            }
            b"NEND" => {
                break;
            }
            _ => {
                if id[0].is_ascii_uppercase() {
                    return Err(RomError(format!(
                        "NSFe: unsupported required chunk '{}'",
                        fourcc_string(&id)
                    )));
                }
                // Optional chunks (plst, time, fade, tlbl, auth, text, ...)
                // are ignored: they only affect the player UI/timing.
            }
        }
        pos = next;
    }

    let info = info.ok_or_else(|| RomError("NSFe: missing INFO chunk".into()))?;
    // A zero-length DATA chunk is legal in NSFe (the player will simply have
    // no code to run); only a completely absent DATA chunk is an error.
    if !saw_data {
        return Err(RomError("NSFe: missing DATA chunk".into()));
    }
    if info.total_songs == 0 {
        return Err(RomError("NSFe: header declares zero songs".into()));
    }
    if info.load_address < 0x6000 {
        return Err(RomError(format!(
            "NSFe: invalid load address ${:04X} (must be at least $6000)",
            info.load_address
        )));
    }

    // Convert NSFe's 0-based starting song to NSF's 1-based field, clamping
    // out-of-range values the same way NsfLoader::InitializeFromHeader does.
    let starting_song = if usize::from(info.starting_song) + 1 > usize::from(info.total_songs) {
        1
    } else {
        info.starting_song + 1
    };

    let mut header = [0u8; NSF_HEADER_SIZE];
    header[..5].copy_from_slice(b"NESM\x1A");
    header[5] = 1; // version
    header[6] = info.total_songs;
    header[7] = starting_song;
    header[0x08..0x0A].copy_from_slice(&info.load_address.to_le_bytes());
    header[0x0A..0x0C].copy_from_slice(&info.init_address.to_le_bytes());
    header[0x0C..0x0E].copy_from_slice(&info.play_address.to_le_bytes());
    // Mesen's NsfeLoader uses the same defaults as a plain NSF without speed
    // fields: 16639us NTSC, 19997us PAL.
    header[0x6E..0x70].copy_from_slice(&16639u16.to_le_bytes());
    header[0x70..0x78].copy_from_slice(&bank_setup);
    header[0x78..0x7A].copy_from_slice(&19997u16.to_le_bytes());
    header[0x7A] = info.flags;
    header[0x7B] = info.sound_chips;

    let mut prg_rom = Vec::with_capacity(NSF_HEADER_SIZE + prg_data.len());
    prg_rom.extend_from_slice(&header);
    prg_rom.extend_from_slice(&prg_data);

    let info = nsf_info(prg_rom.len(), nsf_region(info.flags));
    Ok(RomData::new(info, prg_rom, Vec::new()))
}

// ---------------------------------------------------------------------------
// FDS
// ---------------------------------------------------------------------------

fn parse_fds(data: &[u8]) -> RomResult<RomData> {
    let (sides, side_count) = if data.starts_with(b"FDS\x1A") {
        if data.len() < FDS_HEADER_SIZE {
            return Err(RomError(format!(
                "FDS: truncated fwNES header ({} of {} bytes)",
                data.len(),
                FDS_HEADER_SIZE
            )));
        }
        let side_count = usize::from(data[4]);
        if side_count == 0 {
            return Err(RomError("FDS: fwNES header declares zero sides".into()));
        }
        let expected = FDS_HEADER_SIZE
            .checked_add(side_count * FDS_SIDE_SIZE)
            .ok_or_else(|| RomError("FDS: side count overflows".into()))?;
        if data.len() < expected {
            return Err(RomError(format!(
                "FDS: truncated image: header declares {} side(s) ({} bytes of side data), file provides {} bytes",
                side_count,
                side_count * FDS_SIDE_SIZE,
                data.len().saturating_sub(FDS_HEADER_SIZE)
            )));
        }
        (
            &data[FDS_HEADER_SIZE..FDS_HEADER_SIZE + side_count * FDS_SIDE_SIZE],
            side_count,
        )
    } else {
        if data.len() < FDS_RAW_MAGIC.len() || &data[..FDS_RAW_MAGIC.len()] != FDS_RAW_MAGIC {
            return Err(RomError(
                "FDS: image is neither fwNES (\"FDS\\x1a\") nor a raw NINTENDO-HVC disk".into(),
            ));
        }
        if data.len() < FDS_SIDE_SIZE {
            return Err(RomError(format!(
                "FDS: raw image is too small ({} bytes, one side is {} bytes)",
                data.len(),
                FDS_SIDE_SIZE
            )));
        }
        if data.len() % FDS_SIDE_SIZE != 0 {
            return Err(RomError(format!(
                "FDS: raw image size {} is not a multiple of the {} byte side size ({} trailing bytes)",
                data.len(),
                FDS_SIDE_SIZE,
                data.len() % FDS_SIDE_SIZE
            )));
        }
        (data, data.len() / FDS_SIDE_SIZE)
    };

    let prg_rom_size = sides.len();
    debug_assert_eq!(prg_rom_size, side_count * FDS_SIDE_SIZE);

    let info = RomInfo {
        mapper: FDS_MAPPER_ID,
        submapper: 0,
        region: Region::Ntsc,
        // Mesen's FdsLoader sets Vertical mirroring for the FDS mapper.
        mirroring: Mirroring::Vertical,
        // The FDS saves its modified disk sides (Mesen uses an IPS battery
        // file); mark the cart as battery-backed so the frontend persists it.
        has_battery: true,
        four_screen: false,
        is_nes2: false,
        system: RomSystem::Fds,
        prg_rom_size,
        chr_rom_size: 0,
        // The FDS has 32KB of work RAM at $6000-$DFFF and 8KB CHR RAM.
        prg_ram_size: 0x8000,
        chr_ram_size: 0x2000,
        prg_nvram_size: 0,
        chr_nvram_size: 0,
    };

    Ok(RomData::new(info, sides.to_vec(), Vec::new()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn header_only(prg_pages: u8, chr_pages: u8, flags6: u8, flags7: u8) -> Vec<u8> {
        let mut data = vec![0u8; INES_HEADER_SIZE];
        data[..4].copy_from_slice(b"NES\x1A");
        data[4] = prg_pages;
        data[5] = chr_pages;
        data[6] = flags6;
        data[7] = flags7;
        data
    }

    fn build_ines(prg: &[u8], chr: &[u8], flags6: u8, flags7: u8) -> Vec<u8> {
        assert_eq!(prg.len() % 0x4000, 0);
        assert_eq!(chr.len() % 0x2000, 0);
        let mut data = header_only(
            (prg.len() / 0x4000) as u8,
            (chr.len() / 0x2000) as u8,
            flags6,
            flags7,
        );
        data.extend_from_slice(prg);
        data.extend_from_slice(chr);
        data
    }

    #[test]
    fn ines_nrom_mapper_mirroring_and_sizes() {
        let data = build_ines(&vec![0xAA; 0x8000], &vec![0x55; 0x2000], 0x01, 0x00);
        let rom = parse_rom(&data).expect("valid iNES");

        assert_eq!(rom.info.mapper, 0);
        assert_eq!(rom.info.submapper, 0);
        assert_eq!(rom.info.mirroring, Mirroring::Vertical);
        assert!(!rom.info.four_screen);
        assert!(!rom.info.is_nes2);
        assert!(!rom.info.has_battery);
        assert_eq!(rom.info.system, RomSystem::Nes);
        assert_eq!(rom.info.region, Region::Ntsc);
        assert_eq!(rom.info.prg_rom_size, 0x8000);
        assert_eq!(rom.info.chr_rom_size, 0x2000);
        assert_eq!(rom.info.prg_ram_size, 0x2000);
        assert_eq!(rom.info.chr_ram_size, 0);
        assert_eq!(rom.prg_rom, vec![0xAA; 0x8000]);
        assert_eq!(rom.chr_rom, vec![0x55; 0x2000]);
        assert!(rom.trainer.is_none());
    }

    #[test]
    fn ines_battery_and_four_screen() {
        let data = build_ines(&vec![0u8; 0x4000], &vec![0u8; 0x2000], 0x0B, 0x00);
        let rom = parse_rom(&data).unwrap();
        assert!(rom.info.has_battery);
        assert_eq!(rom.info.prg_ram_size, 0);
        assert_eq!(rom.info.prg_nvram_size, 0x2000);
        assert!(rom.info.four_screen);
        assert_eq!(rom.info.mirroring, Mirroring::FourScreens);
    }

    #[test]
    fn ines_chr_ram_defaults_to_8k() {
        let data = build_ines(&vec![0u8; 0x4000], &[], 0x00, 0x00);
        let rom = parse_rom(&data).unwrap();
        assert_eq!(rom.info.chr_rom_size, 0);
        assert_eq!(rom.info.chr_ram_size, 0x2000);
        assert!(rom.chr_rom.is_empty());
    }

    #[test]
    fn ines_archaic_header_uses_only_byte6_mapper() {
        // Byte 7 bits 2-3 == 0b01 => archaic; byte 8+ is ignored.
        let mut data = build_ines(&vec![0u8; 0x8000], &vec![0u8; 0x2000], 0x31, 0x04);
        data[8..16].fill(0xFF);

        let rom = parse_rom(&data).unwrap();
        assert_eq!(rom.info.mapper, 3);
        assert!(!rom.info.is_nes2);
        assert_eq!(rom.info.mirroring, Mirroring::Vertical);
        assert_eq!(rom.info.region, Region::Ntsc);
        assert_eq!(rom.info.prg_ram_size, 0x2000);
        assert_eq!(rom.info.prg_nvram_size, 0);
    }

    #[test]
    fn ines_trainer_is_attached_and_applied_at_7000() {
        let trainer = [0x5Au8; TRAINER_SIZE];
        let mut data = header_only(2, 1, 0x04, 0x00);
        data.extend_from_slice(&trainer);
        data.extend(vec![0u8; 0x8000]);
        data.extend(vec![0u8; 0x2000]);

        let rom = parse_rom(&data).unwrap();
        assert_eq!(rom.trainer.as_deref(), Some(&trainer[..]));

        let mut common =
            MapperCommon::new(RomData::new(RomInfo::default(), vec![0u8; 0x8000], Vec::new()));
        apply_trainer(&mut common, rom.trainer.as_deref());
        assert!(common.prg_ram[0x1000..0x1200].iter().all(|&b| b == 0x5A));
        assert!(common.prg_ram[..0x1000].iter().all(|&b| b == 0));
    }

    #[test]
    fn cartridge_applies_trainer_to_prg_ram() {
        let mut data = header_only(2, 1, 0x04, 0x00);
        data.extend_from_slice(&[0x5Au8; TRAINER_SIZE]);
        data.extend_from_slice(&vec![0u8; 0x8000]);
        data.extend_from_slice(&vec![0u8; 0x2000]);

        // Mapper 0 is a mandatory target of the port; this also exercises the
        // full parse_rom -> create_mapper -> apply_trainer path. Skip in a
        // build where the mapper groups have not registered NROM.
        if !crate::mapper::is_supported(0) {
            return;
        }
        let cart = Cartridge::from_bytes(&data).expect("NROM cartridge");
        assert_eq!(cart.mapper_id(), 0);
        assert_eq!(
            &cart.common().prg_ram[0x1000..0x1000 + TRAINER_SIZE],
            &[0x5Au8; TRAINER_SIZE][..]
        );
    }

    #[test]
    fn ines_truncated_trainer_is_rejected() {
        let mut data = header_only(1, 0, 0x04, 0x00);
        data.extend(vec![0u8; 100]);
        let err = parse_rom(&data).unwrap_err();
        assert!(err.0.contains("trainer"), "unexpected error: {err}");
    }

    #[test]
    fn ines_truncated_prg_is_rejected() {
        let mut data = header_only(2, 1, 0x00, 0x00);
        data.extend(vec![0u8; 0x4000]); // only half the declared PRG
        let err = parse_rom(&data).unwrap_err();
        assert!(err.0.contains("truncated"), "unexpected error: {err}");
    }

    #[test]
    fn ines_region_byte_and_vs_mapper() {
        let data = build_ines(&vec![0u8; 0x4000], &[], 0x00, 0x00);
        let mut pal = data.clone();
        pal[9] = 0x01;
        assert_eq!(parse_rom(&pal).unwrap().info.region, Region::Pal);

        // Byte 7 bit 0 selects VS System; PORTING.md maps it to mapper 99.
        let mut vs = data;
        vs[7] = 0x01;
        let rom = parse_rom(&vs).unwrap();
        assert_eq!(rom.info.mapper, VS_MAPPER_ID);
        assert_eq!(rom.info.system, RomSystem::Nes);
        assert_eq!(rom.info.region, Region::Ntsc);
    }

    #[test]
    fn nes20_mapper_submapper_and_ram_sizes() {
        let mut data = header_only(2, 1, 0x42, 0x08);
        data[8] = 0x30; // mapper 4 (byte 6 high nibble), submapper 3
        data[10] = 0x47; // PRG-RAM shift 7 (8KB), PRG-NVRAM shift 4 (1KB)
        data[11] = 0x25; // CHR-RAM shift 5 (2KB), CHR-NVRAM shift 2 (256B)
        data[12] = 0x01; // PAL
        data.extend(vec![0u8; 0x8000]);
        data.extend(vec![0u8; 0x2000]);

        let rom = parse_rom(&data).unwrap();
        assert!(rom.info.is_nes2);
        assert_eq!(rom.info.mapper, 4);
        assert_eq!(rom.info.submapper, 3);
        assert_eq!(rom.info.region, Region::Pal);
        assert!(rom.info.has_battery);
        assert_eq!(rom.info.prg_rom_size, 0x8000);
        assert_eq!(rom.info.chr_rom_size, 0x2000);
        assert_eq!(rom.info.prg_ram_size, 0x2000);
        assert_eq!(rom.info.prg_nvram_size, 0x0400);
        assert_eq!(rom.info.chr_ram_size, 0x0800);
        assert_eq!(rom.info.chr_nvram_size, 0x0100);
    }

    #[test]
    fn nes20_region_and_zero_ram_shifts() {
        let mut data = header_only(1, 0, 0x00, 0x08);
        data[9] = 0x00;
        data[10] = 0x00;
        data[11] = 0x00;
        data[12] = 0x03; // Dendy
        data.extend(vec![0u8; 0x4000]);

        let rom = parse_rom(&data).unwrap();
        assert_eq!(rom.info.mapper, 0);
        assert_eq!(rom.info.region, Region::Dendy);
        assert_eq!(rom.info.prg_ram_size, 0);
        assert_eq!(rom.info.prg_nvram_size, 0);
        // No CHR ROM and no CHR-RAM shift: fall back to the mandatory 8KB.
        assert_eq!(rom.info.chr_rom_size, 0);
        assert_eq!(rom.info.chr_ram_size, 0x2000);
    }

    #[test]
    fn nes20_exponent_multiplier_sizes() {
        let mut data = header_only(0, 0, 0x00, 0x08);
        // PRG: exponent 4, multiplier selector 1 => (1*2+1) << 4 = 48 bytes.
        data[4] = (4 << 2) | 1;
        // CHR: exponent 3, multiplier selector 0 => 1 << 3 = 8 bytes.
        data[5] = 3 << 2;
        data[9] = 0xFF; // exponent/multiplier mode for both PRG and CHR
        data.extend(std::iter::repeat(0x11).take(48));
        data.extend(std::iter::repeat(0x22).take(8));

        let rom = parse_rom(&data).unwrap();
        assert_eq!(rom.info.prg_rom_size, 48);
        assert_eq!(rom.info.chr_rom_size, 8);
        assert_eq!(rom.prg_rom.len(), 48);
        assert_eq!(rom.chr_rom.len(), 8);
    }

    // ---- UNIF ----

    fn push_unif_chunk(out: &mut Vec<u8>, id: &[u8; 4], payload: &[u8], pad: bool) {
        out.extend_from_slice(id);
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(payload);
        if pad {
            while out.len() % 4 != 0 {
                out.push(0);
            }
        }
    }

    fn unif_file(board: &str, prg: &[u8], chr: &[u8], pad: bool) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(b"UNIF");
        data.extend_from_slice(&[0u8; 28]);
        let mut name = board.as_bytes().to_vec();
        name.push(0);
        push_unif_chunk(&mut data, b"MAPR", &name, pad);
        push_unif_chunk(&mut data, b"PRG0", prg, pad);
        if !chr.is_empty() {
            push_unif_chunk(&mut data, b"CHR0", chr, pad);
        }
        data
    }

    #[test]
    fn unif_board_mapping_chunks_and_padding() {
        let prg0 = vec![0x11u8; 0x4000];
        let prg1 = vec![0x22u8; 0x2000];
        let chr0 = vec![0x33u8; 0x2000];
        let mut data = unif_file("NES-NROM-256", &prg0, &chr0, true);
        push_unif_chunk(&mut data, b"PRG1", &prg1, true);
        push_unif_chunk(&mut data, b"BATR", &[1], true);
        push_unif_chunk(&mut data, b"MIRR", &[1], true);
        push_unif_chunk(&mut data, b"TVCI", &[1], true);
        // Unknown/ignored chunk, still with valid framing.
        push_unif_chunk(&mut data, b"NAME", b"some name", true);

        let rom = parse_rom(&data).unwrap();
        assert_eq!(rom.info.mapper, 0);
        assert_eq!(rom.info.system, RomSystem::Unif);
        assert_eq!(rom.info.prg_rom_size, 0x6000);
        assert_eq!(rom.info.chr_rom_size, 0x2000);
        assert_eq!(rom.info.prg_ram_size, 0);
        assert_eq!(rom.info.prg_nvram_size, 0x2000);
        assert_eq!(rom.info.chr_ram_size, 0);
        assert!(rom.info.has_battery);
        assert_eq!(rom.info.mirroring, Mirroring::Vertical);
        assert!(!rom.info.four_screen);
        assert_eq!(rom.info.region, Region::Pal);
        assert_eq!(&rom.prg_rom[..0x4000], &prg0[..]);
        assert_eq!(&rom.prg_rom[0x4000..], &prg1[..]);
        assert_eq!(rom.chr_rom, chr0);
    }

    #[test]
    fn unif_unpadded_chunks_are_accepted() {
        let data = unif_file("NROM", &vec![0x44u8; 0x4000], &[], false);
        let rom = parse_rom(&data).unwrap();
        assert_eq!(rom.info.mapper, 0);
        assert_eq!(rom.prg_rom.len(), 0x4000);
        assert_eq!(rom.info.chr_ram_size, 0x2000);
    }

    #[test]
    fn unif_board_table_common_entries() {
        let cases: &[(&str, Option<u16>)] = &[
            ("NROM", Some(0)),
            ("NES-NROM-128", Some(0)),
            ("SNROM", Some(1)),
            ("HVC-SLROM", Some(1)),
            ("UNROM", Some(2)),
            ("UNL-UOROM", Some(2)),
            ("CNROM", Some(3)),
            ("TKROM", Some(4)),
            ("EWROM", Some(5)),
            ("ANROM", Some(7)),
            ("VRC7", Some(85)),
            ("MHROM", Some(66)),
            ("COOLBOY", Some(268)),
            ("BMC-GS-2004", Some(283)),
            ("SA-9602B", Some(513)),
            ("MARIO1-MALEE2", None),
            ("GS-2013", None),
            ("NOT-A-BOARD", None),
        ];
        for &(name, expected) in cases {
            assert_eq!(unif_board_mapper(name), expected, "board {name}");
        }
    }

    #[test]
    fn unif_prefix_stripping_and_known_private_mapping() {
        // UNL- prefix is stripped before the table lookup.
        let data = unif_file("UNL-UNROM-512-8", &vec![0u8; 0x8000], &[], true);
        assert_eq!(parse_rom(&data).unwrap().info.mapper, 30);

        // COOLBOY has a concrete iNES mapper number (268).
        let data = unif_file("COOLBOY", &vec![0u8; 0x8000], &[], true);
        assert_eq!(parse_rom(&data).unwrap().info.mapper, 268);
    }

    #[test]
    fn unif_unknown_or_private_board_is_rejected() {
        for board in ["NO-SUCH-BOARD", "MARIO1-MALEE2", "DANCE"] {
            let data = unif_file(board, &vec![0u8; 0x4000], &[], true);
            let err = parse_rom(&data).unwrap_err();
            assert!(
                err.0.contains("unsupported board") && err.0.contains(board),
                "unexpected error for {board}: {err}"
            );
        }
    }

    #[test]
    fn unif_missing_mapper_and_truncated_chunk_are_rejected() {
        let mut no_mapr = Vec::new();
        no_mapr.extend_from_slice(b"UNIF");
        no_mapr.extend_from_slice(&[0u8; 28]);
        push_unif_chunk(&mut no_mapr, b"PRG0", &[0u8; 0x4000], true);
        let err = parse_rom(&no_mapr).unwrap_err();
        assert!(err.0.contains("MAPR"), "unexpected error: {err}");

        let mut truncated = unif_file("NROM", &vec![0u8; 0x4000], &[], true);
        truncated.extend_from_slice(b"PRG1");
        truncated.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        let err = parse_rom(&truncated).unwrap_err();
        assert!(err.0.contains("truncated"), "unexpected error: {err}");
    }

    // ---- NSF / NSFe ----

    fn make_nsf(total: u8, load: u16, init: u16, play: u16, flags: u8, start: u8) -> Vec<u8> {
        let mut data = vec![0u8; NSF_HEADER_SIZE];
        data[..5].copy_from_slice(b"NESM\x1A");
        data[5] = 1;
        data[6] = total;
        data[7] = start;
        data[8..0x0A].copy_from_slice(&load.to_le_bytes());
        data[0x0A..0x0C].copy_from_slice(&init.to_le_bytes());
        data[0x0C..0x0E].copy_from_slice(&play.to_le_bytes());
        data[0x6E..0x70].copy_from_slice(&16639u16.to_le_bytes());
        data[0x70..0x78].copy_from_slice(&[0, 1, 2, 3, 4, 5, 6, 7]);
        data[0x78..0x7A].copy_from_slice(&19997u16.to_le_bytes());
        data[0x7A] = flags;
        data[0x7B] = 0x01; // VRC6
        data.extend(vec![0xABu8; 0x400]);
        data
    }

    #[test]
    fn nsf_keeps_whole_file_in_prg_rom() {
        let data = make_nsf(3, 0x8000, 0x8000, 0x8003, 0x01, 2);
        let rom = parse_rom(&data).unwrap();

        assert_eq!(rom.info.mapper, NSF_MAPPER_ID);
        assert_eq!(rom.info.system, RomSystem::Nsf);
        assert_eq!(rom.info.region, Region::Pal);
        assert_eq!(rom.info.prg_rom_size, data.len());
        assert_eq!(rom.info.chr_rom_size, 0);
        assert_eq!(rom.info.chr_ram_size, 0x2000);
        assert_eq!(rom.info.prg_ram_size, 0x4000);
        assert_eq!(rom.prg_rom, data);
        assert!(rom.chr_rom.is_empty());

        let header = parse_nsf_header(&rom.prg_rom).expect("NSF header");
        assert_eq!(header.total_songs, 3);
        assert_eq!(header.starting_song, 2);
        assert_eq!(header.load_address, 0x8000);
        assert_eq!(header.init_address, 0x8000);
        assert_eq!(header.play_address, 0x8003);
        assert_eq!(header.sound_chips, 0x01);
        assert_eq!(header.bank_setup, [0, 1, 2, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn nsf_rejects_bad_headers() {
        let zero_songs = make_nsf(0, 0x8000, 0x8000, 0x8003, 0, 1);
        assert!(parse_rom(&zero_songs).is_err());

        let bad_load = make_nsf(1, 0x5FFF, 0x8000, 0x8003, 0, 1);
        assert!(parse_rom(&bad_load).is_err());

        let mut truncated = make_nsf(1, 0x8000, 0x8000, 0x8003, 0, 1);
        truncated.truncate(NSF_HEADER_SIZE - 1);
        let err = parse_rom(&truncated).unwrap_err();
        assert!(err.0.contains("truncated"), "unexpected error: {err}");
    }

    fn push_nsfe_chunk(out: &mut Vec<u8>, id: &[u8; 4], payload: &[u8]) {
        // NSFe: the length is the payload length, excluding the 8-byte header.
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(id);
        out.extend_from_slice(payload);
    }

    #[allow(clippy::too_many_arguments)]
    fn nsfe_info(
        load: u16,
        init: u16,
        play: u16,
        flags: u8,
        chips: u8,
        total: u8,
        start: u8,
    ) -> [u8; 10] {
        let mut p = [0u8; 10];
        p[0..2].copy_from_slice(&load.to_le_bytes());
        p[2..4].copy_from_slice(&init.to_le_bytes());
        p[4..6].copy_from_slice(&play.to_le_bytes());
        p[6] = flags;
        p[7] = chips;
        p[8] = total;
        p[9] = start;
        p
    }

    #[test]
    fn nsfe_is_converted_to_canonical_nsf_image() {
        let mut data = Vec::new();
        data.extend_from_slice(b"NSFE");
        let info = nsfe_info(0x8000, 0x8010, 0x8020, 0, 0x02, 2, 0);
        push_nsfe_chunk(&mut data, b"INFO", &info);
        push_nsfe_chunk(&mut data, b"BANK", &[8, 9, 10, 11, 12, 13, 14, 15]);
        push_nsfe_chunk(&mut data, b"auth", b"Title\0Artist\0Copyright\0Ripper\0");
        push_nsfe_chunk(&mut data, b"DATA", &[0xCD; 0x100]);
        push_nsfe_chunk(&mut data, b"NEND", &[]);

        let rom = parse_rom(&data).unwrap();
        assert_eq!(rom.info.mapper, NSF_MAPPER_ID);
        assert_eq!(rom.info.system, RomSystem::Nsf);
        assert_eq!(rom.info.prg_rom_size, NSF_HEADER_SIZE + 0x100);
        assert_eq!(&rom.prg_rom[..5], b"NESM\x1A");
        assert_eq!(rom.prg_rom[6], 2);
        assert_eq!(rom.prg_rom[7], 1); // NSFe 0-based -> NSF 1-based
        assert_eq!(&rom.prg_rom[0x08..0x0A], &0x8000u16.to_le_bytes());
        assert_eq!(&rom.prg_rom[0x0A..0x0C], &0x8010u16.to_le_bytes());
        assert_eq!(&rom.prg_rom[0x0C..0x0E], &0x8020u16.to_le_bytes());
        assert_eq!(&rom.prg_rom[0x70..0x78], &[8, 9, 10, 11, 12, 13, 14, 15]);
        assert_eq!(rom.prg_rom[0x7B], 0x02);
        assert_eq!(&rom.prg_rom[NSF_HEADER_SIZE..], &[0xCD; 0x100]);
    }

    #[test]
    fn nsfe_rejects_missing_info_data_and_required_chunks() {
        let mut missing_info = Vec::new();
        missing_info.extend_from_slice(b"NSFE");
        push_nsfe_chunk(&mut missing_info, b"DATA", &[1, 2, 3]);
        push_nsfe_chunk(&mut missing_info, b"NEND", &[]);
        assert!(parse_rom(&missing_info).unwrap_err().0.contains("INFO"));

        let mut missing_data = Vec::new();
        missing_data.extend_from_slice(b"NSFE");
        push_nsfe_chunk(
            &mut missing_data,
            b"INFO",
            &nsfe_info(0x8000, 0x8000, 0x8003, 0, 0, 1, 0),
        );
        push_nsfe_chunk(&mut missing_data, b"NEND", &[]);
        assert!(parse_rom(&missing_data).unwrap_err().0.contains("DATA"));

        let mut required = Vec::new();
        required.extend_from_slice(b"NSFE");
        push_nsfe_chunk(
            &mut required,
            b"INFO",
            &nsfe_info(0x8000, 0x8000, 0x8003, 0, 0, 1, 0),
        );
        push_nsfe_chunk(&mut required, b"ZZZZ", &[0]);
        push_nsfe_chunk(&mut required, b"DATA", &[1]);
        let err = parse_rom(&required).unwrap_err();
        assert!(err.0.contains("required chunk"), "unexpected error: {err}");

        let mut truncated = Vec::new();
        truncated.extend_from_slice(b"NSFE");
        truncated.extend_from_slice(&1000u32.to_le_bytes());
        truncated.extend_from_slice(b"INFO");
        truncated.extend_from_slice(&[0u8; 10]);
        let err = parse_rom(&truncated).unwrap_err();
        assert!(err.0.contains("truncated"), "unexpected error: {err}");
    }

    // ---- FDS ----

    fn make_fds(sides: usize) -> Vec<u8> {
        let mut data = vec![0u8; FDS_HEADER_SIZE];
        data[..4].copy_from_slice(b"FDS\x1A");
        data[4] = sides as u8;
        data.extend(vec![0x11u8; FDS_SIDE_SIZE * sides]);
        data
    }

    #[test]
    fn fds_fwnes_header_is_stripped_and_sides_concatenated() {
        let data = make_fds(2);
        let rom = parse_rom(&data).unwrap();

        assert_eq!(rom.info.mapper, FDS_MAPPER_ID);
        assert_eq!(rom.info.system, RomSystem::Fds);
        assert_eq!(rom.info.mirroring, Mirroring::Vertical);
        assert!(rom.info.has_battery);
        assert_eq!(rom.info.prg_rom_size, 2 * FDS_SIDE_SIZE);
        assert_eq!(rom.info.chr_rom_size, 0);
        assert_eq!(rom.info.chr_ram_size, 0x2000);
        assert_eq!(rom.info.prg_ram_size, 0x8000);
        assert_eq!(rom.prg_rom.len(), 2 * FDS_SIDE_SIZE);
        assert_eq!(&rom.prg_rom[..], &data[FDS_HEADER_SIZE..]);
        assert!(rom.chr_rom.is_empty());
        assert!(rom.trainer.is_none());
    }

    #[test]
    fn fds_rejects_truncated_and_zero_side_images() {
        let mut truncated = make_fds(2);
        truncated.truncate(FDS_HEADER_SIZE + FDS_SIDE_SIZE);
        let err = parse_rom(&truncated).unwrap_err();
        assert!(err.0.contains("truncated"), "unexpected error: {err}");

        let mut zero_sides = vec![0u8; FDS_HEADER_SIZE];
        zero_sides[..4].copy_from_slice(b"FDS\x1A");
        let err = parse_rom(&zero_sides).unwrap_err();
        assert!(err.0.contains("zero sides"), "unexpected error: {err}");
    }

    #[test]
    fn fds_raw_nintendo_hvc_image_is_accepted() {
        let mut data = vec![0u8; FDS_SIDE_SIZE];
        data[..FDS_RAW_MAGIC.len()].copy_from_slice(FDS_RAW_MAGIC);

        let rom = parse_rom(&data).unwrap();
        assert_eq!(rom.info.mapper, FDS_MAPPER_ID);
        assert_eq!(rom.info.system, RomSystem::Fds);
        assert_eq!(rom.prg_rom, data);

        let mut not_multiple = data;
        not_multiple.push(0);
        let err = parse_rom(&not_multiple).unwrap_err();
        assert!(err.0.contains("multiple"), "unexpected error: {err}");
    }

    // ---- misc ----

    #[test]
    fn unrecognized_and_tiny_files_are_rejected() {
        assert!(parse_rom(b"NOPE").unwrap_err().0.contains("unrecognized"));
        assert!(parse_rom(b"").unwrap_err().0.contains("too small"));
        assert!(parse_rom(b"NES\x1A").unwrap_err().0.contains("truncated"));
    }

    #[test]
    fn nsf_header_parser_rejects_wrong_magic() {
        let mut data = make_nsf(1, 0x8000, 0x8000, 0x8003, 0, 1);
        data[0] = b'X';
        assert!(parse_nsf_header(&data).is_none());
        assert!(parse_nsf_header(&data[..0x7F]).is_none());
    }
}
