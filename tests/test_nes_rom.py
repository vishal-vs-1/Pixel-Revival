import hashlib

import pytest

from systems.nes.metadata import new_metadata, validate_metadata
from systems.nes.rom import MAX_ROM_BYTES, NesRomError, NesRomInspector


def cartridge(*, prg=1, chr_banks=0, flags6=0, flags7=0, byte8=0, byte9=0,
              region=0, misc=0, device=0, payload=None, tail=b""):
    header = b"NES\x1a" + bytes((prg, chr_banks, flags6, flags7, byte8, byte9,
                                0, 0, region, 0, misc, device))
    if payload is None:
        payload = bytes(prg * 16384 + chr_banks * 8192 + (512 if flags6 & 4 else 0))
    return header + payload + tail


def inspect(tmp_path, raw, name="Synthetic.nes"):
    path = tmp_path / name
    path.write_bytes(raw)
    return NesRomInspector().inspect(path)


def test_ines_trainer_ram_and_complete_file_identity(tmp_path):
    raw = cartridge(flags6=0xA7, flags7=0x30, byte9=1, tail=b"keep this")
    info = inspect(tmp_path, raw)
    assert (info.container, info.mapper, info.region, info.chr_kind) == ("ines", 0x3A, "pal", "ram")
    assert info.has_trainer and info.battery and info.mirroring == "vertical"
    assert info.prg_rom_bytes == 16384 and info.trailing_bytes == 9
    digest = hashlib.sha256(raw).digest()
    assert info.source_sha256 == digest.hex()
    assert info.id == hashlib.sha256(b"nes\0" + digest).hexdigest()
    assert inspect(tmp_path, raw, "Renamed.nes") == info
    assert inspect(tmp_path, raw[:-1] + b"?", "Changed.nes").id != info.id


def test_nes2_linear_and_exponential_forms(tmp_path):
    linear = cartridge(prg=1, chr_banks=1, flags6=0xB8, flags7=0x68, byte8=0xC2,
                       byte9=0x10, region=3, payload=bytes(16384 + 257 * 8192))
    info = inspect(tmp_path, linear)
    assert (info.container, info.mapper, info.submapper, info.region) == ("nes2", 0x26B, 12, "dendy")
    assert info.chr_rom_bytes == 257 * 8192 and info.mirroring == "four-screen"
    exp = cartridge(prg=(12 << 2) | 1, chr_banks=11 << 2, flags7=8, byte9=255,
                    payload=bytes(3 * 4096 + 2048), misc=1, tail=b"extra")
    info = inspect(tmp_path, exp)
    assert (info.prg_rom_bytes, info.chr_rom_bytes, info.misc_rom_count, info.trailing_bytes) == (12288, 2048, 1, 5)


@pytest.mark.parametrize("raw,match", [
    (b"FDS\x1a", "cartridge file"),
    (b"NES\x1a" + b"\0" * 11, "cartridge file"),
    (cartridge(flags6=4, payload=bytes(16384)), "Truncated"),
    (cartridge(prg=0), "nonempty PRG"),
    (cartridge(flags7=9), "Vs. System"),
    (cartridge(flags7=2), "PlayChoice"),
    (cartridge(flags7=8, device=5), "expansion device 5"),
    (cartridge(flags7=8, misc=2, tail=b"x"), "miscellaneous"),
    (cartridge(flags7=8, prg=255, byte9=15, payload=b""), "64 MiB"),
], ids=["fds", "short", "trainer", "prg-zero", "vs", "playchoice",
        "expansion", "misc", "huge-exponent"])
def test_reject_invalid_or_unsupported_files(tmp_path, raw, match):
    with pytest.raises(NesRomError, match=match):
        inspect(tmp_path, raw)


def test_oversize_rejected_from_file_length(tmp_path):
    path = tmp_path / "Large.nes"
    with path.open("wb") as stream:
        stream.write(cartridge())
        stream.truncate(MAX_ROM_BYTES + 1)
    with pytest.raises(NesRomError, match="64 MiB"):
        NesRomInspector().inspect(path)


def test_metadata_does_not_guess_runtime_or_palette(tmp_path):
    info = inspect(tmp_path, cartridge())
    game = new_metadata(info, "合成.nes", "ai")
    assert (game["system"], game["id"], game["name"]) == ("nes", info.id, "合成")
    assert game["runtime_status"] == "setup-needed" and game["palette_profile"] is None
    assert game["nes"]["chr_kind"] == "ram" and game["capture_policy"] == "disabled"
    assert validate_metadata(game, info.id) is game
    with pytest.raises(ValueError, match="cartridge or workspace"):
        validate_metadata({**game, "source_sha256": "a" * 64})
