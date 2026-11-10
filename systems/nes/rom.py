"""Read iNES/NES 2.0 cartridge metadata without loading emulator code."""

import hashlib
import os
import stat
from dataclasses import dataclass
from pathlib import Path

MAX_ROM_BYTES = 64 * 1024 * 1024
HEADER_BYTES = 16
TRAINER_BYTES = 512


class NesRomError(ValueError):
    pass


def nes_game_id(source_digest: bytes) -> str:
    if len(source_digest) != 32:
        raise ValueError("NES source digest must be SHA-256")
    return hashlib.sha256(b"nes\0" + source_digest).hexdigest()


@dataclass(frozen=True)
class NesRomInfo:
    container: str
    mapper: int
    submapper: int
    region: str
    prg_rom_bytes: int
    chr_rom_bytes: int
    has_trainer: bool
    battery: bool
    mirroring: str
    expansion_device: int
    trailing_bytes: int
    misc_rom_count: int
    warnings: tuple[str, ...]
    file_bytes: int
    source_sha256: str
    id: str

    @property
    def chr_kind(self) -> str:
        return "rom" if self.chr_rom_bytes else "ram"


def _nes2_size(low: int, high: int, bank_size: int) -> int:
    if high != 15:
        return ((high << 8) | low) * bank_size
    exponent, multiplier = divmod(low, 4)
    # Exponent form gives a byte count, with no PRG/CHR bank multiplier.
    if exponent >= MAX_ROM_BYTES.bit_length():
        raise NesRomError("NES 2.0 ROM size exceeds the 64 MiB limit.")
    return (1 << exponent) * (2 * multiplier + 1)


def _header_fields(header: bytes, file_bytes: int) -> dict:
    if len(header) != HEADER_BYTES or header[:4] != b"NES\x1a":
        raise NesRomError("Select an iNES or NES 2.0 cartridge file. FDS, NSF and archives are unsupported.")
    flags6, flags7 = header[6:8]
    version = flags7 & 0x0C
    nes2 = version == 8
    archaic = version not in (0, 8)
    if not archaic and flags7 & 3:
        console = ("NES", "Vs. System", "PlayChoice-10", "extended console")[flags7 & 3]
        raise NesRomError(f"Unsupported NES console mode: {console}.")
    mapper = (flags6 >> 4) | (0 if archaic else flags7 & 0xF0)
    expansion = header[15] & 0x3F if nes2 else 0
    if expansion not in (0, 1):
        raise NesRomError(f"NES expansion device {expansion} is unsupported by the standard-pad player.")
    if nes2:
        mapper |= (header[8] & 15) << 8
        prg = _nes2_size(header[4], header[9] & 15, 16384)
        chr_rom = _nes2_size(header[5], header[9] >> 4, 8192)
        region = ("ntsc", "pal", "multi", "dendy")[header[12] & 3]
    else:
        prg, chr_rom = header[4] * 16384, header[5] * 8192
        region = "unknown" if archaic else ("pal" if header[9] & 1 else "ntsc")
    if not prg:
        raise NesRomError("The NES header must declare nonempty PRG ROM.")
    trainer = bool(flags6 & 4)
    required = HEADER_BYTES + (TRAINER_BYTES if trainer else 0) + prg + chr_rom
    if required > MAX_ROM_BYTES:
        raise NesRomError("Declared cartridge data exceeds the 64 MiB limit.")
    if required > file_bytes:
        raise NesRomError("Truncated cartridge: declared trainer, PRG or CHR data is missing.")
    trailing = file_bytes - required
    misc = header[14] & 3 if nes2 else 0
    if trailing < misc:
        raise NesRomError("Truncated cartridge: declared miscellaneous ROM data is missing.")
    warnings = []
    if archaic:
        warnings.append("Archaic iNES header: only the low mapper nibble is interpreted.")
    if trailing:
        warnings.append(f"Preserved {trailing} trailing cartridge bytes.")
    return dict(container="nes2" if nes2 else "ines", mapper=mapper,
                submapper=header[8] >> 4 if nes2 else 0, region=region,
                prg_rom_bytes=prg, chr_rom_bytes=chr_rom, has_trainer=trainer,
                battery=bool(flags6 & 2), mirroring="four-screen" if flags6 & 8 else
                ("vertical" if flags6 & 1 else "horizontal"), expansion_device=expansion,
                trailing_bytes=trailing, misc_rom_count=misc, warnings=tuple(warnings))


class NesRomInspector:
    def inspect(self, path: Path) -> NesRomInfo:
        path = Path(path)
        if not path.is_file():
            raise NesRomError("Select a regular NES cartridge file.")
        with path.open("rb") as stream:
            info = os.fstat(stream.fileno())
            if not stat.S_ISREG(info.st_mode):
                raise NesRomError("Select a regular NES cartridge file.")
            size = info.st_size
            if size > MAX_ROM_BYTES:
                raise NesRomError("NES cartridge exceeds the 64 MiB limit.")
            header = stream.read(HEADER_BYTES)
            fields = _header_fields(header, size)
            digest = hashlib.sha256(header)
            remaining = size - len(header)
            while remaining:
                chunk = stream.read(min(remaining, 1024 * 1024))
                if not chunk:
                    raise NesRomError("The NES cartridge changed while it was being read.")
                digest.update(chunk)
                remaining -= len(chunk)
            after = os.fstat(stream.fileno())
            if stream.read(1) or (info.st_size, info.st_mtime_ns) != (after.st_size, after.st_mtime_ns):
                raise NesRomError("The NES cartridge changed while it was being read.")
        return NesRomInfo(**fields, file_bytes=size, source_sha256=digest.hexdigest(),
                          id=nes_game_id(digest.digest()))
