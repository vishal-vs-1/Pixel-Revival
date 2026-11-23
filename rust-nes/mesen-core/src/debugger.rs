//! 6502/NES disassembler and debugger helpers.
//!
//! Ported from Mesen 0.9.9's `Core/Disassembler.cpp`, `Core/DisassemblyInfo.cpp`
//! and `Core/TraceLogger.cpp` (original copyright (C) 2014-2020 Sour,
//! GPL-3.0).  This module deliberately has no dependency on a debugger UI or a
//! memory manager: callers provide a side-effect-free byte peek through a
//! callback.
//!
//! Formatting follows Mesen:
//! * zero-page operands are printed as `$XX` (or `$00XX` when extended),
//! * absolute operands are printed as `$XXXX`,
//! * unofficial opcodes carry Mesen's `*` suffix (`SLO* $1234,X`),
//! * branches show the resolved target address, not the raw displacement.
//!
//! Unlike upstream's fixed-width `DisassemblyInfo::ToString`, the text returned
//! by [`Disassembler::disassemble`] has no trailing padding space.  The trace
//! formatter supplies spacing through its format string.

use std::collections::VecDeque;
use std::io;
use std::path::Path;

use crate::types::MemoryAccessType;

/// 6502 addressing modes, in the same order as Mesen's `AddrMode` enum.
///
/// The order is significant: Mesen uses `mode <= AddrMode::Abs` to decide
/// whether an instruction has a meaningful "memory value" and
/// `mode > AddrMode::Abs` to decide whether it has an effective address.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum AddrMode {
    #[default]
    None,
    Acc,
    Imp,
    Imm,
    Rel,
    Zero,
    Abs,
    ZeroX,
    ZeroY,
    Ind,
    IndX,
    IndY,
    IndYW,
    AbsX,
    AbsXW,
    AbsY,
    AbsYW,
}

impl AddrMode {
    /// Operand size in bytes, including the opcode.
    pub const fn size(self) -> u8 {
        match self {
            AddrMode::Abs
            | AddrMode::AbsX
            | AddrMode::AbsXW
            | AddrMode::AbsY
            | AddrMode::AbsYW
            | AddrMode::Ind => 3,

            AddrMode::Imm
            | AddrMode::IndX
            | AddrMode::IndY
            | AddrMode::IndYW
            | AddrMode::Rel
            | AddrMode::Zero
            | AddrMode::ZeroX
            | AddrMode::ZeroY => 2,

            AddrMode::None | AddrMode::Acc | AddrMode::Imp => 1,
        }
    }

    /// True when the operand is a zero-page address (and can be extended to
    /// four hex digits in traces).
    pub const fn is_zero_page(self) -> bool {
        matches!(
            self,
            AddrMode::Zero
                | AddrMode::ZeroX
                | AddrMode::ZeroY
                | AddrMode::IndX
                | AddrMode::IndY
                | AddrMode::IndYW
        )
    }
}

/// Static information about one 6502 opcode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpcodeInfo {
    /// Mesen's mnemonic, without the unofficial `*` marker.
    pub mnemonic: &'static str,
    pub mode: AddrMode,
    /// Instruction length in bytes (opcode + operand).
    pub size: u8,
    /// Mesen marks unofficial/undocumented opcodes with a `*` in disassembly.
    pub unofficial: bool,
}

// Mesen's opcode names, exactly as listed in Disassembler::BuildOpCodeTables.
#[rustfmt::skip]
const OPCODE_NAMES: [&str; 256] = [
    "BRK", "ORA", "STP", "SLO", "NOP", "ORA", "ASL", "SLO", "PHP", "ORA", "ASL", "ANC", "NOP", "ORA", "ASL", "SLO", // 00
    "BPL", "ORA", "STP", "SLO", "NOP", "ORA", "ASL", "SLO", "CLC", "ORA", "NOP", "SLO", "NOP", "ORA", "ASL", "SLO", // 10
    "JSR", "AND", "STP", "RLA", "BIT", "AND", "ROL", "RLA", "PLP", "AND", "ROL", "ANC", "BIT", "AND", "ROL", "RLA", // 20
    "BMI", "AND", "STP", "RLA", "NOP", "AND", "ROL", "RLA", "SEC", "AND", "NOP", "RLA", "NOP", "AND", "ROL", "RLA", // 30
    "RTI", "EOR", "STP", "SRE", "NOP", "EOR", "LSR", "SRE", "PHA", "EOR", "LSR", "ALR", "JMP", "EOR", "LSR", "SRE", // 40
    "BVC", "EOR", "STP", "SRE", "NOP", "EOR", "LSR", "SRE", "CLI", "EOR", "NOP", "SRE", "NOP", "EOR", "LSR", "SRE", // 50
    "RTS", "ADC", "STP", "RRA", "NOP", "ADC", "ROR", "RRA", "PLA", "ADC", "ROR", "ARR", "JMP", "ADC", "ROR", "RRA", // 60
    "BVS", "ADC", "STP", "RRA", "NOP", "ADC", "ROR", "RRA", "SEI", "ADC", "NOP", "RRA", "NOP", "ADC", "ROR", "RRA", // 70
    "NOP", "STA", "NOP", "SAX", "STY", "STA", "STX", "SAX", "DEY", "NOP", "TXA", "XAA", "STY", "STA", "STX", "SAX", // 80
    "BCC", "STA", "STP", "AHX", "STY", "STA", "STX", "SAX", "TYA", "STA", "TXS", "TAS", "SHY", "STA", "SHX", "AXA", // 90
    "LDY", "LDA", "LDX", "LAX", "LDY", "LDA", "LDX", "LAX", "TAY", "LDA", "TAX", "LAX", "LDY", "LDA", "LDX", "LAX", // A0
    "BCS", "LDA", "STP", "LAX", "LDY", "LDA", "LDX", "LAX", "CLV", "LDA", "TSX", "LAS", "LDY", "LDA", "LDX", "LAX", // B0
    "CPY", "CMP", "NOP", "DCP", "CPY", "CMP", "DEC", "DCP", "INY", "CMP", "DEX", "AXS", "CPY", "CMP", "DEC", "DCP", // C0
    "BNE", "CMP", "STP", "DCP", "NOP", "CMP", "DEC", "DCP", "CLD", "CMP", "NOP", "DCP", "NOP", "CMP", "DEC", "DCP", // D0
    "CPX", "SBC", "NOP", "ISC", "CPX", "SBC", "INC", "ISC", "INX", "SBC", "NOP", "SBC", "CPX", "SBC", "INC", "ISC", // E0
    "BEQ", "SBC", "STP", "ISC", "NOP", "SBC", "INC", "ISC", "SED", "SBC", "NOP", "ISC", "NOP", "SBC", "INC", "ISC", // F0
];

// Mesen's IsUnofficialCode table.
#[rustfmt::skip]
const OPCODE_UNOFFICIAL: [bool; 256] = [
    false, false, true,  true,  true,  false, false, true,  false, false, false, true,  true,  false, false, true,  // 00
    false, false, true,  true,  true,  false, false, true,  false, false, true,  true,  true,  false, false, true,  // 10
    false, false, true,  true,  false, false, false, true,  false, false, false, true,  false, false, false, true,  // 20
    false, false, true,  true,  true,  false, false, true,  false, false, true,  true,  true,  false, false, true,  // 30
    false, false, true,  true,  true,  false, false, true,  false, false, false, true,  false, false, false, true,  // 40
    false, false, true,  true,  true,  false, false, true,  false, false, true,  true,  true,  false, false, true,  // 50
    false, false, true,  true,  true,  false, false, true,  false, false, false, true,  false, false, false, true,  // 60
    false, false, true,  true,  true,  false, false, true,  false, false, true,  true,  true,  false, false, true,  // 70
    true,  false, true,  true,  false, false, false, true,  false, true,  false, true,  false, false, false, true,  // 80
    false, false, true,  true,  false, false, false, true,  false, false, false, true,  true,  false, true,  true,  // 90
    false, false, false, true,  false, false, false, true,  false, false, false, true,  false, false, false, true,  // A0
    false, false, true,  true,  false, false, false, true,  false, false, false, true,  false, false, false, true,  // B0
    false, false, true,  true,  false, false, false, true,  false, false, false, true,  false, false, false, true,  // C0
    false, false, true,  true,  true,  false, false, true,  false, false, true,  true,  true,  false, false, true,  // D0
    false, false, true,  true,  false, false, false, true,  false, false, false, true,  false, false, false, true,  // E0
    false, false, true,  true,  true,  false, false, true,  false, false, true,  true,  true,  false, false, true,  // F0
];

// Mesen's OPMode table.
#[rustfmt::skip]
const OPCODE_MODES: [AddrMode; 256] = [
    AddrMode::Imp,  AddrMode::IndX, AddrMode::None, AddrMode::IndX, AddrMode::Zero, AddrMode::Zero, AddrMode::Zero, AddrMode::Zero, AddrMode::Imp,  AddrMode::Imm,  AddrMode::Acc,  AddrMode::Imm,  AddrMode::Abs,  AddrMode::Abs,  AddrMode::Abs,  AddrMode::Abs,  // 00
    AddrMode::Rel,  AddrMode::IndY, AddrMode::None, AddrMode::IndYW, AddrMode::ZeroX, AddrMode::ZeroX, AddrMode::ZeroX, AddrMode::ZeroX, AddrMode::Imp,  AddrMode::AbsY, AddrMode::Imp,  AddrMode::AbsYW, AddrMode::AbsX, AddrMode::AbsX, AddrMode::AbsXW, AddrMode::AbsXW, // 10
    AddrMode::Abs,  AddrMode::IndX, AddrMode::None, AddrMode::IndX, AddrMode::Zero, AddrMode::Zero, AddrMode::Zero, AddrMode::Zero, AddrMode::Imp,  AddrMode::Imm,  AddrMode::Acc,  AddrMode::Imm,  AddrMode::Abs,  AddrMode::Abs,  AddrMode::Abs,  AddrMode::Abs,  // 20
    AddrMode::Rel,  AddrMode::IndY, AddrMode::None, AddrMode::IndYW, AddrMode::ZeroX, AddrMode::ZeroX, AddrMode::ZeroX, AddrMode::ZeroX, AddrMode::Imp,  AddrMode::AbsY, AddrMode::Imp,  AddrMode::AbsYW, AddrMode::AbsX, AddrMode::AbsX, AddrMode::AbsXW, AddrMode::AbsXW, // 30
    AddrMode::Imp,  AddrMode::IndX, AddrMode::None, AddrMode::IndX, AddrMode::Zero, AddrMode::Zero, AddrMode::Zero, AddrMode::Zero, AddrMode::Imp,  AddrMode::Imm,  AddrMode::Acc,  AddrMode::Imm,  AddrMode::Abs,  AddrMode::Abs,  AddrMode::Abs,  AddrMode::Abs,  // 40
    AddrMode::Rel,  AddrMode::IndY, AddrMode::None, AddrMode::IndYW, AddrMode::ZeroX, AddrMode::ZeroX, AddrMode::ZeroX, AddrMode::ZeroX, AddrMode::Imp,  AddrMode::AbsY, AddrMode::Imp,  AddrMode::AbsYW, AddrMode::AbsX, AddrMode::AbsX, AddrMode::AbsXW, AddrMode::AbsXW, // 50
    AddrMode::Imp,  AddrMode::IndX, AddrMode::None, AddrMode::IndX, AddrMode::Zero, AddrMode::Zero, AddrMode::Zero, AddrMode::Zero, AddrMode::Imp,  AddrMode::Imm,  AddrMode::Acc,  AddrMode::Imm,  AddrMode::Ind,  AddrMode::Abs,  AddrMode::Abs,  AddrMode::Abs,  // 60
    AddrMode::Rel,  AddrMode::IndY, AddrMode::None, AddrMode::IndYW, AddrMode::ZeroX, AddrMode::ZeroX, AddrMode::ZeroX, AddrMode::ZeroX, AddrMode::Imp,  AddrMode::AbsY, AddrMode::Imp,  AddrMode::AbsYW, AddrMode::AbsX, AddrMode::AbsX, AddrMode::AbsXW, AddrMode::AbsXW, // 70
    AddrMode::Imm,  AddrMode::IndX, AddrMode::Imm,  AddrMode::IndX, AddrMode::Zero, AddrMode::Zero, AddrMode::Zero, AddrMode::Zero, AddrMode::Imp,  AddrMode::Imm,  AddrMode::Imp,  AddrMode::Imm,  AddrMode::Abs,  AddrMode::Abs,  AddrMode::Abs,  AddrMode::Abs,  // 80
    AddrMode::Rel,  AddrMode::IndYW, AddrMode::None, AddrMode::IndYW, AddrMode::ZeroX, AddrMode::ZeroX, AddrMode::ZeroY, AddrMode::ZeroY, AddrMode::Imp,  AddrMode::AbsYW, AddrMode::Imp,  AddrMode::AbsYW, AddrMode::AbsXW, AddrMode::AbsXW, AddrMode::AbsYW, AddrMode::AbsYW, // 90
    AddrMode::Imm,  AddrMode::IndX, AddrMode::Imm,  AddrMode::IndX, AddrMode::Zero, AddrMode::Zero, AddrMode::Zero, AddrMode::Zero, AddrMode::Imp,  AddrMode::Imm,  AddrMode::Imp,  AddrMode::Imm,  AddrMode::Abs,  AddrMode::Abs,  AddrMode::Abs,  AddrMode::Abs,  // A0
    AddrMode::Rel,  AddrMode::IndY, AddrMode::None, AddrMode::IndY, AddrMode::ZeroX, AddrMode::ZeroX, AddrMode::ZeroY, AddrMode::ZeroY, AddrMode::Imp,  AddrMode::AbsY, AddrMode::Imp,  AddrMode::AbsY,  AddrMode::AbsX,  AddrMode::AbsX,  AddrMode::AbsY,  AddrMode::AbsY,  // B0
    AddrMode::Imm,  AddrMode::IndX, AddrMode::Imm,  AddrMode::IndX, AddrMode::Zero, AddrMode::Zero, AddrMode::Zero, AddrMode::Zero, AddrMode::Imp,  AddrMode::Imm,  AddrMode::Imp,  AddrMode::Imm,  AddrMode::Abs,  AddrMode::Abs,  AddrMode::Abs,  AddrMode::Abs,  // C0
    AddrMode::Rel,  AddrMode::IndY, AddrMode::None, AddrMode::IndYW, AddrMode::ZeroX, AddrMode::ZeroX, AddrMode::ZeroX, AddrMode::ZeroX, AddrMode::Imp,  AddrMode::AbsY, AddrMode::Imp,  AddrMode::AbsYW, AddrMode::AbsX, AddrMode::AbsX, AddrMode::AbsXW, AddrMode::AbsXW, // D0
    AddrMode::Imm,  AddrMode::IndX, AddrMode::Imm,  AddrMode::IndX, AddrMode::Zero, AddrMode::Zero, AddrMode::Zero, AddrMode::Zero, AddrMode::Imp,  AddrMode::Imm,  AddrMode::Imp,  AddrMode::Imm,  AddrMode::Abs,  AddrMode::Abs,  AddrMode::Abs,  AddrMode::Abs,  // E0
    AddrMode::Rel,  AddrMode::IndY, AddrMode::None, AddrMode::IndYW, AddrMode::ZeroX, AddrMode::ZeroX, AddrMode::ZeroX, AddrMode::ZeroX, AddrMode::Imp,  AddrMode::AbsY, AddrMode::Imp,  AddrMode::AbsYW, AddrMode::AbsX, AddrMode::AbsX, AddrMode::AbsXW, AddrMode::AbsXW, // F0
];

const fn build_opcode_table() -> [OpcodeInfo; 256] {
    let mut table = [OpcodeInfo {
        mnemonic: "",
        mode: AddrMode::None,
        size: 1,
        unofficial: false,
    }; 256];

    let mut i = 0;
    while i < 256 {
        table[i] = OpcodeInfo {
            mnemonic: OPCODE_NAMES[i],
            mode: OPCODE_MODES[i],
            size: OPCODE_MODES[i].size(),
            unofficial: OPCODE_UNOFFICIAL[i],
        };
        i += 1;
    }
    table
}

/// Complete NES 6502 opcode table (official and unofficial), in opcode order.
pub const OPCODES: [OpcodeInfo; 256] = build_opcode_table();

/// Look up static information for an opcode.
#[inline]
pub fn opcode_info(opcode: u8) -> &'static OpcodeInfo {
    &OPCODES[opcode as usize]
}

/// Options that affect disassembly text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisassemblyOptions {
    /// Lower-case mnemonic only (Mesen's "Display opcodes in lower case").
    pub lowercase: bool,
    /// Print zero-page operands with four hex digits (`$0010`).
    pub extend_zero_page: bool,
    /// Append Mesen's `*` marker to unofficial opcodes.
    pub mark_unofficial: bool,
}

impl Default for DisassemblyOptions {
    fn default() -> Self {
        Self {
            lowercase: false,
            extend_zero_page: false,
            mark_unofficial: true,
        }
    }
}

/// One decoded instruction.
///
/// Mirrors Mesen's `DisassemblyInfo`: the first three bytes of the
/// instruction are cached so trace formatting does not need to re-read memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisassemblyInfo {
    pub opcode: u8,
    pub bytes: [u8; 3],
    /// Instruction length: 1, 2 or 3 (0 for a default-constructed value).
    pub size: u8,
    pub mode: AddrMode,
    pub mnemonic: &'static str,
    pub unofficial: bool,
    is_sub_entry_point: bool,
}

impl Default for DisassemblyInfo {
    fn default() -> Self {
        Self {
            opcode: 0,
            bytes: [0; 3],
            size: 0,
            mode: AddrMode::None,
            mnemonic: "",
            unofficial: false,
            is_sub_entry_point: false,
        }
    }
}

impl DisassemblyInfo {
    /// Decode the instruction at `addr`.
    ///
    /// `read` is called with [`MemoryAccessType::Read`] for the opcode and each
    /// operand byte.  It should be a side-effect-free peek.
    pub fn new(addr: u16, read: &mut dyn FnMut(u16, MemoryAccessType) -> u8) -> Self {
        Self::new_with_entry_point(addr, read, false)
    }

    /// Same as [`DisassemblyInfo::new`], with sub-entry-point tracking used by
    /// Mesen's debugger to annotate JSR targets.
    pub fn new_with_entry_point(
        addr: u16,
        read: &mut dyn FnMut(u16, MemoryAccessType) -> u8,
        is_sub_entry_point: bool,
    ) -> Self {
        let opcode = read(addr, MemoryAccessType::Read);
        let info = &OPCODES[opcode as usize];

        let mut bytes = [0u8; 3];
        bytes[0] = opcode;
        for (i, byte) in bytes.iter_mut().enumerate().take(info.size as usize).skip(1) {
            *byte = read(addr.wrapping_add(i as u16), MemoryAccessType::Read);
        }

        Self {
            opcode,
            bytes,
            size: info.size,
            mode: info.mode,
            mnemonic: info.mnemonic,
            unofficial: info.unofficial,
            is_sub_entry_point,
        }
    }

    /// Decode an instruction from an in-memory byte slice.
    ///
    /// Returns `None` if the slice is too short to contain the full
    /// instruction (the opcode byte alone is not enough for a 2/3-byte
    /// instruction).
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let opcode = *bytes.first()?;
        let info = &OPCODES[opcode as usize];
        let size = info.size as usize;
        if bytes.len() < size {
            return None;
        }

        let mut raw = [0u8; 3];
        raw[..size].copy_from_slice(&bytes[..size]);
        Some(Self {
            opcode,
            bytes: raw,
            size: info.size,
            mode: info.mode,
            mnemonic: info.mnemonic,
            unofficial: info.unofficial,
            is_sub_entry_point: false,
        })
    }

    #[inline]
    pub fn opcode_info(&self) -> &'static OpcodeInfo {
        &OPCODES[self.opcode as usize]
    }

    #[inline]
    pub fn is_unofficial(&self) -> bool {
        self.unofficial
    }

    #[inline]
    pub fn is_sub_entry_point(&self) -> bool {
        self.is_sub_entry_point
    }

    pub fn set_sub_entry_point(&mut self) {
        self.is_sub_entry_point = true;
    }

    /// RTS/RTI end a Mesen debugger "sub" block.
    #[inline]
    pub fn is_sub_exit_point(&self) -> bool {
        self.opcode == 0x40 || self.opcode == 0x60
    }

    /// Combine the operand bytes into a 16-bit value.  For relative branches
    /// this is the resolved target address (`memory_addr + 2 + displacement`).
    pub fn operand_address(&self, memory_addr: u16) -> u16 {
        let operand = match self.size {
            2 => self.bytes[1] as u16,
            3 => self.bytes[1] as u16 | ((self.bytes[2] as u16) << 8),
            _ => 0,
        };

        if self.mode == AddrMode::Rel {
            memory_addr
                .wrapping_add(2)
                .wrapping_add(self.bytes[1] as i8 as i16 as u16)
        } else {
            operand
        }
    }

    /// Effective address for indexed/indirect modes, matching Mesen's
    /// `DisassemblyInfo::GetEffectiveAddress`.
    ///
    /// Returns `None` for modes that do not have one (immediate, absolute,
    /// implied, accumulator, ...).  Zero-page pointer reads wrap inside page
    /// zero, and `JMP ($xxFF)` reads the high byte from `$xx00`, like the CPU.
    pub fn effective_address(
        &self,
        x: u8,
        y: u8,
        read: &mut dyn FnMut(u16, MemoryAccessType) -> u8,
    ) -> Option<u16> {
        match self.mode {
            AddrMode::ZeroX => Some(self.bytes[1].wrapping_add(x) as u16),
            AddrMode::ZeroY => Some(self.bytes[1].wrapping_add(y) as u16),

            AddrMode::IndX => {
                let zero_addr = self.bytes[1].wrapping_add(x);
                let lo = read(zero_addr as u16, MemoryAccessType::Read) as u16;
                let hi = read(zero_addr.wrapping_add(1) as u16, MemoryAccessType::Read) as u16;
                Some(lo | (hi << 8))
            }

            AddrMode::IndY | AddrMode::IndYW => {
                let zero_addr = self.bytes[1];
                let lo = read(zero_addr as u16, MemoryAccessType::Read) as u16;
                let hi = read(zero_addr.wrapping_add(1) as u16, MemoryAccessType::Read) as u16;
                Some((lo | (hi << 8)).wrapping_add(y as u16))
            }

            AddrMode::Ind => Some(read_indirect_word(self.operand_word(), read)),

            AddrMode::AbsX | AddrMode::AbsXW => {
                Some(self.operand_word().wrapping_add(x as u16))
            }
            AddrMode::AbsY | AddrMode::AbsYW => {
                Some(self.operand_word().wrapping_add(y as u16))
            }

            AddrMode::None
            | AddrMode::Acc
            | AddrMode::Imp
            | AddrMode::Imm
            | AddrMode::Rel
            | AddrMode::Zero
            | AddrMode::Abs => None,
        }
    }

    /// Jump destination for branches, `JMP` and `JSR`; `None` for other
    /// instructions.
    pub fn jump_destination(
        &self,
        pc: u16,
        read: &mut dyn FnMut(u16, MemoryAccessType) -> u8,
    ) -> Option<u16> {
        if !Disassembler::is_jump(self.opcode) {
            return None;
        }

        match self.mode {
            AddrMode::Rel | AddrMode::Abs => Some(self.operand_address(pc)),
            AddrMode::Ind => Some(read_indirect_word(self.operand_word(), read)),
            _ => None,
        }
    }

    /// Byte read at the instruction's memory operand, used by the trace
    /// logger's `[MemoryValue]` column.
    ///
    /// Mirrors Mesen's `GetMemoryValue`: immediate and implied instructions do
    /// not have a memory operand.
    pub fn memory_value(
        &self,
        pc: u16,
        x: u8,
        y: u8,
        read: &mut dyn FnMut(u16, MemoryAccessType) -> u8,
    ) -> Option<u8> {
        let address = if (self.mode as u8) <= (AddrMode::Abs as u8) {
            match self.mode {
                AddrMode::Rel | AddrMode::Abs | AddrMode::Zero => Some(self.operand_address(pc)),
                _ => None,
            }
        } else {
            self.effective_address(x, y, read)
        };

        address.map(|addr| read(addr, MemoryAccessType::Read))
    }

    /// Mesen's `GetByteCode`: `$A9 $42` (or `$6C $34 $12`).
    pub fn byte_code_string(&self) -> String {
        let mut out = String::with_capacity(1 + self.size as usize * 4);
        for (i, byte) in self.bytes.iter().enumerate().take(self.size as usize) {
            if i != 0 {
                out.push(' ');
            }
            out.push('$');
            push_hex(&mut out, *byte as u16, 2);
        }
        out
    }

    /// Compact byte code without separators, useful for CSV traces: `A942`.
    pub fn byte_code_compact(&self) -> String {
        let mut out = String::with_capacity(self.size as usize * 2);
        for byte in self.bytes.iter().take(self.size as usize) {
            push_hex(&mut out, *byte as u16, 2);
        }
        out
    }

    /// Format the instruction using default options.
    pub fn text(&self, addr: u16) -> String {
        self.text_with_options(addr, &DisassemblyOptions::default())
    }

    /// Format the instruction with explicit options.
    pub fn text_with_options(&self, addr: u16, options: &DisassemblyOptions) -> String {
        self.text_with_optional_labels(addr, options, None)
    }

    /// Format the instruction, replacing operand addresses with labels returned
    /// by `labels`.
    pub fn text_with_labels<F>(
        &self,
        addr: u16,
        options: &DisassemblyOptions,
        labels: F,
    ) -> String
    where
        F: Fn(u16) -> Option<String>,
    {
        let label_ref: &dyn Fn(u16) -> Option<String> = &labels;
        self.text_with_optional_labels(addr, options, Some(label_ref))
    }

    /// Format the instruction with an optional label resolver.
    pub fn text_with_optional_labels(
        &self,
        addr: u16,
        options: &DisassemblyOptions,
        labels: Option<&dyn Fn(u16) -> Option<String>>,
    ) -> String {
        let mut out = String::with_capacity(16);
        self.write_text(&mut out, addr, options, labels);
        out
    }

    fn write_text(
        &self,
        out: &mut String,
        addr: u16,
        options: &DisassemblyOptions,
        labels: Option<&dyn Fn(u16) -> Option<String>>,
    ) {
        if options.lowercase {
            out.push_str(&self.mnemonic.to_ascii_lowercase());
        } else {
            out.push_str(self.mnemonic);
        }
        if options.mark_unofficial && self.unofficial {
            out.push('*');
        }

        // Mesen's OPName strings include a trailing space before the operand.
        if !matches!(self.mode, AddrMode::None | AddrMode::Imp) {
            out.push(' ');
        }

        let operand = self.operand_address(addr);
        // Mesen only asks the label manager for modes with a real operand
        // (never immediate/implied/accumulator).
        let label = if matches!(
            self.mode,
            AddrMode::None | AddrMode::Acc | AddrMode::Imp | AddrMode::Imm
        ) {
            None
        } else {
            labels.and_then(|lookup| lookup(operand))
        };

        match self.mode {
            AddrMode::None | AddrMode::Imp => {}

            AddrMode::Acc => out.push('A'),

            AddrMode::Imm => {
                out.push_str("#$");
                push_hex(out, self.bytes[1] as u16, 2);
            }

            AddrMode::Ind => {
                out.push('(');
                push_operand(out, operand, label, 4);
                out.push(')');
            }

            AddrMode::IndX => {
                out.push('(');
                push_operand(out, operand, label, zp_width(options));
                out.push_str(",X)");
            }

            AddrMode::IndY | AddrMode::IndYW => {
                out.push('(');
                push_operand(out, operand, label, zp_width(options));
                out.push_str("),Y");
            }

            AddrMode::Abs | AddrMode::Rel => push_operand(out, operand, label, 4),

            AddrMode::Zero => push_operand(out, operand, label, zp_width(options)),

            AddrMode::AbsX | AddrMode::AbsXW => {
                push_operand(out, operand, label, 4);
                out.push_str(",X");
            }

            AddrMode::ZeroX => {
                push_operand(out, operand, label, zp_width(options));
                out.push_str(",X");
            }

            AddrMode::AbsY | AddrMode::AbsYW => {
                push_operand(out, operand, label, 4);
                out.push_str(",Y");
            }

            AddrMode::ZeroY => {
                push_operand(out, operand, label, zp_width(options));
                out.push_str(",Y");
            }
        }
    }

    /// Mesen's `GetEffectiveAddressString`: `""` or `" @ $1234"`/`" @ Label"`.
    pub fn effective_address_string(
        &self,
        x: u8,
        y: u8,
        read: &mut dyn FnMut(u16, MemoryAccessType) -> u8,
        labels: Option<&dyn Fn(u16) -> Option<String>>,
    ) -> String {
        if (self.mode as u8) <= (AddrMode::Abs as u8) {
            return String::new();
        }

        let Some(effective) = self.effective_address(x, y, read) else {
            return String::new();
        };

        let mut out = String::from(" @ ");
        if let Some(label) = labels.and_then(|lookup| lookup(effective)) {
            out.push_str(&label);
        } else if matches!(self.mode, AddrMode::ZeroX | AddrMode::ZeroY) {
            out.push('$');
            push_hex(&mut out, effective as u8 as u16, 2);
        } else {
            out.push('$');
            push_hex(&mut out, effective, 4);
        }
        out
    }

    fn operand_word(&self) -> u16 {
        match self.size {
            2 => self.bytes[1] as u16,
            3 => self.bytes[1] as u16 | ((self.bytes[2] as u16) << 8),
            _ => 0,
        }
    }
}

/// Read a 16-bit pointer with the 6502 `JMP ($xxFF)` page-wrap bug, matching
/// Mesen's `DisassemblyInfo::GetIndirectJumpDestination`.
pub fn read_indirect_word(
    addr: u16,
    read: &mut dyn FnMut(u16, MemoryAccessType) -> u8,
) -> u16 {
    if (addr & 0xFF) == 0xFF {
        let lo = read(addr, MemoryAccessType::Read) as u16;
        let hi = read(addr & 0xFF00, MemoryAccessType::Read) as u16;
        lo | (hi << 8)
    } else {
        let lo = read(addr, MemoryAccessType::Read) as u16;
        let hi = read(addr.wrapping_add(1), MemoryAccessType::Read) as u16;
        lo | (hi << 8)
    }
}

/// NES 6502 disassembler.
///
/// This is a stateless facade over [`OpcodeInfo`]/[`DisassemblyInfo`]; Mesen's
/// cache and label/code-data-logger integration are intentionally not included
/// (they belong to a debugger UI).
#[derive(Clone, Copy, Debug, Default)]
pub struct Disassembler;

impl Disassembler {
    pub const fn new() -> Self {
        Self
    }

    /// Disassemble one instruction at `addr`. Returns `(text, length)`.
    ///
    /// `read` must be a side-effect-free peek; it is called once per byte with
    /// [`MemoryAccessType::Read`].
    pub fn disassemble(
        addr: u16,
        read: &mut dyn FnMut(u16, MemoryAccessType) -> u8,
    ) -> (String, u8) {
        let info = DisassemblyInfo::new(addr, read);
        (info.text(addr), info.size)
    }

    /// Disassemble with explicit text options.
    pub fn disassemble_with_options(
        addr: u16,
        read: &mut dyn FnMut(u16, MemoryAccessType) -> u8,
        options: &DisassemblyOptions,
    ) -> (String, u8) {
        let info = DisassemblyInfo::new(addr, read);
        (info.text_with_options(addr, options), info.size)
    }

    /// Disassemble with explicit options and a label resolver.
    pub fn disassemble_with_labels<F>(
        addr: u16,
        read: &mut dyn FnMut(u16, MemoryAccessType) -> u8,
        options: &DisassemblyOptions,
        labels: F,
    ) -> (String, u8)
    where
        F: Fn(u16) -> Option<String>,
    {
        let info = DisassemblyInfo::new(addr, read);
        (info.text_with_labels(addr, options, labels), info.size)
    }

    /// Decode the full [`DisassemblyInfo`] for `addr`.
    pub fn disassemble_info(
        addr: u16,
        read: &mut dyn FnMut(u16, MemoryAccessType) -> u8,
    ) -> DisassemblyInfo {
        DisassemblyInfo::new(addr, read)
    }

    #[inline]
    pub fn opcode_info(opcode: u8) -> &'static OpcodeInfo {
        opcode_info(opcode)
    }

    /// Mesen's `IsUnofficialOpCode`.
    #[inline]
    pub fn is_unofficial(opcode: u8) -> bool {
        OPCODES[opcode as usize].unofficial
    }

    /// Mesen's `IsJump`: branches, JMP (indirect too) and JSR.
    #[inline]
    pub fn is_jump(opcode: u8) -> bool {
        matches!(
            opcode,
            0x10 | 0x30
                | 0x50
                | 0x70
                | 0x90
                | 0xB0
                | 0xD0
                | 0xF0
                | 0x4C
                | 0x6C
                | 0x20
        )
    }

    /// Mesen's `IsUnconditionalJump`: RTI/RTS/JMP/JMP indirect/JSR.
    #[inline]
    pub fn is_unconditional_jump(opcode: u8) -> bool {
        matches!(opcode, 0x40 | 0x60 | 0x6C | 0x4C | 0x20)
    }

    #[inline]
    pub fn size(opcode: u8) -> u8 {
        OPCODES[opcode as usize].size
    }
}

fn zp_width(options: &DisassemblyOptions) -> usize {
    if options.extend_zero_page {
        4
    } else {
        2
    }
}

fn push_hex(out: &mut String, value: u16, digits: usize) {
    use std::fmt::Write;
    if digits == 2 {
        // `write!` on a String cannot fail.
        let _ = write!(out, "{:02X}", value as u8);
    } else {
        let _ = write!(out, "{:04X}", value);
    }
}

fn push_operand(out: &mut String, value: u16, label: Option<String>, digits: usize) {
    if let Some(label) = label {
        out.push_str(&label);
    } else {
        out.push('$');
        push_hex(out, value, digits);
    }
}

/// Format a CPU state as a Mesen-style register component.
pub fn format_registers(pc: u16, a: u8, x: u8, y: u8, sp: u8, ps: u8) -> String {
    format!(
        "PC:{:04X} A:{:02X} X:{:02X} Y:{:02X} SP:{:02X} PS:{:02X}",
        pc, a, x, y, sp, ps
    )
}

/// Format an address as `$XXXX` (Mesen debugger convention).
pub fn format_address(addr: u16) -> String {
    format!("${:04X}", addr)
}

/// Format the status register as Mesen trace-logger flags:
/// active flags are `N V - - D I Z C`, inactive flags are lower-case when
/// `min_width >= 8`.
pub fn format_status_flags(ps: u8, min_width: usize) -> String {
    const ACTIVE: [char; 8] = ['N', 'V', '-', '-', 'D', 'I', 'Z', 'C'];
    const INACTIVE: [char; 8] = ['n', 'v', '-', '-', 'd', 'i', 'z', 'c'];

    let mut flags = String::with_capacity(8);
    let mut value = ps;
    for i in 0..8 {
        if (value & 0x80) != 0 {
            flags.push(ACTIVE[i]);
        } else if min_width >= 8 {
            flags.push(INACTIVE[i]);
        }
        value <<= 1;
    }
    flags
}

/// CPU/PPU snapshot needed to format a trace line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TraceContext {
    pub pc: u16,
    pub a: u8,
    pub x: u8,
    pub y: u8,
    pub sp: u8,
    pub ps: u8,
    pub cycle_count: u64,
    pub ppu_cycle: u32,
    pub ppu_scanline: i16,
    pub frame_count: u32,
}

impl TraceContext {
    pub fn new(pc: u16, a: u8, x: u8, y: u8, sp: u8, ps: u8) -> Self {
        Self {
            pc,
            a,
            x,
            y,
            sp,
            ps,
            cycle_count: 0,
            ppu_cycle: 0,
            ppu_scanline: 0,
            frame_count: 0,
        }
    }

    pub fn with_ppu_state(mut self, scanline: i16, cycle: u32, frame_count: u32) -> Self {
        self.ppu_scanline = scanline;
        self.ppu_cycle = cycle;
        self.frame_count = frame_count;
        self
    }

    pub fn with_cycle_count(mut self, cycle_count: u64) -> Self {
        self.cycle_count = cycle_count;
        self
    }

    /// Register portion in the same format as [`format_registers`].
    pub fn registers_string(&self) -> String {
        format_registers(self.pc, self.a, self.x, self.y, self.sp, self.ps)
    }
}

/// Default execution-trace format, inspired by Mesen's trace logger defaults.
pub const DEFAULT_TRACE_FORMAT: &str =
    "[PC,4h] [ByteCode] [Disassembly] A:[A,2h] X:[X,2h] Y:[Y,2h] SP:[SP,2h] P:[P,2h]";

/// CSV header emitted by [`format_trace_csv`].
pub const TRACE_CSV_HEADER: &str = "PC,ByteCode,Disassembly,A,X,Y,SP,PS,CycleCount";

#[derive(Clone, Debug, PartialEq, Eq)]
enum TraceField {
    ByteCode,
    Disassembly,
    EffectiveAddress,
    MemoryValue,
    Align,
    Pc,
    A,
    X,
    Y,
    Sp,
    Ps,
    Cycle,
    Scanline,
    FrameCount,
    CycleCount,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TraceRowPart {
    field: Option<TraceField>,
    text: String,
    display_in_hex: bool,
    min_width: usize,
}

impl TraceRowPart {
    fn text(text: String) -> Self {
        Self {
            field: None,
            text,
            display_in_hex: false,
            min_width: 0,
        }
    }

    fn field(field: TraceField, display_in_hex: bool, min_width: usize) -> Self {
        Self {
            field: Some(field),
            text: String::new(),
            display_in_hex,
            min_width,
        }
    }
}

/// Mesen `TraceLogger`-style line formatter.
///
/// The format string supports the same tags as Mesen's trace logger:
/// `[ByteCode]`, `[Disassembly]`, `[EffectiveAddress]`, `[MemoryValue]`,
/// `[Align]`, `[PC]`, `[A]`, `[X]`, `[Y]`, `[SP]`, `[P]`, `[Cycle]`,
/// `[Scanline]`, `[FrameCount]` and `[CycleCount]`, each optionally followed by
/// `,<minWidth>` and `h` for hexadecimal (`[PC,4h]`).
///
/// Tags are matched case-insensitively.  Unknown tags render as
/// `[Invalid tag]`, like Mesen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceFormatter {
    parts: Vec<TraceRowPart>,
    /// Text options used for the `[Disassembly]` field.
    pub options: DisassemblyOptions,
    /// Indent code by `0xFF - SP` (Mesen's `IndentCode` option).
    pub indent_code: bool,
    /// Terminate lines with CRLF instead of LF.
    pub use_windows_eol: bool,
}

impl Default for TraceFormatter {
    fn default() -> Self {
        Self::with_format(DEFAULT_TRACE_FORMAT)
    }
}

impl TraceFormatter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a formatter from a Mesen-style format string.
    pub fn with_format(format: &str) -> Self {
        Self {
            parts: parse_trace_format(format),
            options: DisassemblyOptions::default(),
            indent_code: false,
            use_windows_eol: false,
        }
    }

    /// Parse `format` and replace the current layout.
    pub fn set_format(&mut self, format: &str) {
        self.parts = parse_trace_format(format);
    }

    /// Line ending used by [`TraceFormatter::format_line_eol`].
    pub fn eol(&self) -> &'static str {
        if self.use_windows_eol {
            "\r\n"
        } else {
            "\n"
        }
    }

    /// Format one instruction without a line ending.
    pub fn format_line(
        &self,
        ctx: &TraceContext,
        info: &DisassemblyInfo,
        read: &mut dyn FnMut(u16, MemoryAccessType) -> u8,
    ) -> String {
        self.format_line_with_labels(ctx, info, read, None)
    }

    /// Format one instruction with an optional label resolver, without a line
    /// ending.
    pub fn format_line_with_labels(
        &self,
        ctx: &TraceContext,
        info: &DisassemblyInfo,
        read: &mut dyn FnMut(u16, MemoryAccessType) -> u8,
        labels: Option<&dyn Fn(u16) -> Option<String>>,
    ) -> String {
        let mut output = String::with_capacity(64);

        for part in &self.parts {
            let Some(field) = &part.field else {
                output.push_str(&part.text);
                continue;
            };

            match field {
                TraceField::ByteCode => {
                    let mut byte_code = info.byte_code_string();
                    if !part.display_in_hex {
                        byte_code.retain(|c| c != '$');
                    }
                    write_padded(&mut output, &byte_code, part.min_width);
                }

                TraceField::Disassembly => {
                    let mut text = String::new();
                    if self.indent_code {
                        let indent = 0xFFu8.wrapping_sub(ctx.sp) as usize;
                        text.extend(std::iter::repeat(' ').take(indent));
                    }
                    text.push_str(&info.text_with_optional_labels(
                        ctx.pc,
                        &self.options,
                        labels,
                    ));
                    write_padded(&mut output, &text, part.min_width);
                }

                TraceField::EffectiveAddress => {
                    let text = info.effective_address_string(ctx.x, ctx.y, read, labels);
                    write_padded(&mut output, &text, part.min_width);
                }

                TraceField::MemoryValue => {
                    if let Some(value) = info.memory_value(ctx.pc, ctx.x, ctx.y, read) {
                        output.push_str(if part.display_in_hex { "= $" } else { "= " });
                        let text = if part.display_in_hex {
                            hex_u8(value)
                        } else {
                            value.to_string()
                        };
                        write_padded(&mut output, &text, part.min_width);
                    }
                }

                TraceField::Align => {
                    if output.len() < part.min_width {
                        output.extend(std::iter::repeat(' ').take(part.min_width - output.len()));
                    }
                }

                TraceField::Pc => {
                    write_number(&mut output, ctx.pc as u64, part, 4);
                }
                TraceField::A => write_number(&mut output, ctx.a as u64, part, 2),
                TraceField::X => write_number(&mut output, ctx.x as u64, part, 2),
                TraceField::Y => write_number(&mut output, ctx.y as u64, part, 2),
                TraceField::Sp => write_number(&mut output, ctx.sp as u64, part, 2),

                TraceField::Ps => {
                    if part.display_in_hex {
                        write_padded(&mut output, &hex_u8(ctx.ps), part.min_width);
                    } else {
                        let flags = format_status_flags(ctx.ps, part.min_width);
                        write_padded(&mut output, &flags, part.min_width);
                    }
                }

                TraceField::Cycle => {
                    let text = if part.display_in_hex {
                        hex_u32(ctx.ppu_cycle)
                    } else {
                        ctx.ppu_cycle.to_string()
                    };
                    write_padded(&mut output, &text, part.min_width);
                }

                TraceField::Scanline => {
                    let text = if part.display_in_hex {
                        hex_u32(ctx.ppu_scanline as i32 as u32)
                    } else {
                        ctx.ppu_scanline.to_string()
                    };
                    write_padded(&mut output, &text, part.min_width);
                }

                TraceField::FrameCount => {
                    let text = if part.display_in_hex {
                        hex_u32(ctx.frame_count)
                    } else {
                        ctx.frame_count.to_string()
                    };
                    write_padded(&mut output, &text, part.min_width);
                }

                TraceField::CycleCount => {
                    let text = if part.display_in_hex {
                        hex_u64(ctx.cycle_count)
                    } else {
                        ctx.cycle_count.to_string()
                    };
                    write_padded(&mut output, &text, part.min_width);
                }
            }
        }

        output
    }

    /// Format one instruction and append the configured line ending.
    pub fn format_line_eol(
        &self,
        ctx: &TraceContext,
        info: &DisassemblyInfo,
        read: &mut dyn FnMut(u16, MemoryAccessType) -> u8,
    ) -> String {
        let mut output = self.format_line(ctx, info, read);
        output.push_str(self.eol());
        output
    }

    /// Disassemble at `ctx.pc` and format one line (no line ending).
    pub fn format_next(
        &self,
        ctx: &TraceContext,
        read: &mut dyn FnMut(u16, MemoryAccessType) -> u8,
    ) -> String {
        let info = DisassemblyInfo::new(ctx.pc, read);
        self.format_line(ctx, &info, read)
    }

    /// Disassemble at `ctx.pc` and format one line with line ending.
    pub fn format_next_eol(
        &self,
        ctx: &TraceContext,
        read: &mut dyn FnMut(u16, MemoryAccessType) -> u8,
    ) -> String {
        let info = DisassemblyInfo::new(ctx.pc, read);
        self.format_line_eol(ctx, &info, read)
    }
}

fn write_padded(output: &mut String, value: &str, min_width: usize) {
    output.push_str(value);
    if min_width > value.len() {
        output.extend(std::iter::repeat(' ').take(min_width - value.len()));
    }
}

fn write_number(output: &mut String, value: u64, part: &TraceRowPart, hex_width: usize) {
    let text = if part.display_in_hex {
        format!("{:0width$X}", value, width = hex_width)
    } else {
        value.to_string()
    };
    write_padded(output, &text, part.min_width);
}

fn hex_u8(value: u8) -> String {
    format!("{:02X}", value)
}

/// Mesen's `HexUtilities::ToHex(uint32_t)`: minimal width, but 2/4/6/8 digits
/// for the corresponding magnitude.
fn hex_u32(value: u32) -> String {
    if value > 0xFF_FFFF {
        format!("{:08X}", value)
    } else if value <= 0xFF {
        format!("{:02X}", value)
    } else if value <= 0xFFFF {
        format!("{:04X}", value)
    } else {
        format!("{:06X}", value)
    }
}

/// Mesen's `HexUtilities::ToHex(uint64_t)`: no leading zeroes (zero renders as
/// an empty string).
fn hex_u64(value: u64) -> String {
    if value == 0 {
        String::new()
    } else {
        format!("{:X}", value)
    }
}

fn parse_trace_format(format: &str) -> Vec<TraceRowPart> {
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut index = 0usize;

    while index < format.len() {
        let rest = &format[index..];
        let ch = rest.chars().next().expect("index is on a char boundary");

        if ch == '[' {
            if let Some(relative_end) = rest[1..].find(']') {
                let end = index + 1 + relative_end;
                if !text.is_empty() {
                    parts.push(TraceRowPart::text(std::mem::take(&mut text)));
                }
                parts.push(parse_trace_tag(&format[index + 1..end]));
                index = end + 1;
                continue;
            }
        }

        text.push(ch);
        index += ch.len_utf8();
    }

    if !text.is_empty() {
        parts.push(TraceRowPart::text(text));
    }

    parts
}

fn parse_trace_tag(tag: &str) -> TraceRowPart {
    let inner = tag.trim();
    let (name, spec) = match inner.split_once(',') {
        Some((name, spec)) => (name.trim(), Some(spec.trim())),
        None => (inner, None),
    };

    let mut min_width = 0usize;
    let mut display_in_hex = false;

    if let Some(spec) = spec {
        let digits_end = spec
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(spec.len());
        if digits_end > 0 {
            min_width = spec[..digits_end].parse::<usize>().unwrap_or(0);
        }
        let remainder = spec[digits_end..].trim();
        if remainder.eq_ignore_ascii_case("h") {
            display_in_hex = true;
        } else if !remainder.is_empty() {
            return TraceRowPart::text("[Invalid tag]".to_string());
        }
    }

    let field = match name.to_ascii_lowercase().as_str() {
        "bytecode" => Some(TraceField::ByteCode),
        "disassembly" => Some(TraceField::Disassembly),
        "effectiveaddress" => Some(TraceField::EffectiveAddress),
        "memoryvalue" => Some(TraceField::MemoryValue),
        "align" => Some(TraceField::Align),
        "pc" => Some(TraceField::Pc),
        "a" => Some(TraceField::A),
        "x" => Some(TraceField::X),
        "y" => Some(TraceField::Y),
        "sp" => Some(TraceField::Sp),
        "p" | "ps" => Some(TraceField::Ps),
        "cycle" => Some(TraceField::Cycle),
        "scanline" => Some(TraceField::Scanline),
        "framecount" => Some(TraceField::FrameCount),
        "cyclecount" => Some(TraceField::CycleCount),
        _ => None,
    };

    match field {
        Some(field) => TraceRowPart::field(field, display_in_hex, min_width),
        None => TraceRowPart::text("[Invalid tag]".to_string()),
    }
}

fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r') {
        let mut escaped = String::with_capacity(value.len() + 2);
        escaped.push('"');
        for ch in value.chars() {
            if ch == '"' {
                escaped.push('"');
            }
            escaped.push(ch);
        }
        escaped.push('"');
        escaped
    } else {
        value.to_string()
    }
}

/// Format a CSV trace row (no line ending).
///
/// The disassembly column is quoted when it contains a comma, as every indexed
/// operand does.
pub fn format_trace_csv(ctx: &TraceContext, info: &DisassemblyInfo) -> String {
    format!(
        "{:04X},{},{},{:02X},{:02X},{:02X},{:02X},{:02X},{}",
        ctx.pc,
        info.byte_code_compact(),
        csv_escape(&info.text(ctx.pc)),
        ctx.a,
        ctx.x,
        ctx.y,
        ctx.sp,
        ctx.ps,
        ctx.cycle_count
    )
}

/// How [`TraceLogger`] renders rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TraceLogFormat {
    /// Use the [`TraceFormatter`] format string.
    #[default]
    Text,
    /// Emit fixed CSV rows (see [`format_trace_csv`]).
    Csv,
}

/// Number of rows Mesen keeps in its in-memory execution trace.
pub const DEFAULT_TRACE_LOG_CAPACITY: usize = 30_000;

/// Simple execution trace recorder.
///
/// It keeps the most recent `capacity` rows in memory (0 means unlimited) and
/// can write them to a file with [`TraceLogger::save`].  There is no
/// background I/O and no debugger integration; call [`TraceLogger::log`] from
/// the CPU/step loop, passing a side-effect-free peek.
#[derive(Clone, Debug)]
pub struct TraceLogger {
    formatter: TraceFormatter,
    format: TraceLogFormat,
    lines: VecDeque<String>,
    capacity: usize,
}

impl Default for TraceLogger {
    fn default() -> Self {
        Self {
            formatter: TraceFormatter::default(),
            format: TraceLogFormat::Text,
            lines: VecDeque::new(),
            capacity: DEFAULT_TRACE_LOG_CAPACITY,
        }
    }
}

impl TraceLogger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Header line for CSV output (not written automatically).
    pub fn csv_header() -> &'static str {
        TRACE_CSV_HEADER
    }

    pub fn with_capacity(capacity: usize) -> Self {
        let mut logger = Self::default();
        logger.set_capacity(capacity);
        logger
    }

    pub fn with_formatter(formatter: TraceFormatter) -> Self {
        Self {
            formatter,
            ..Self::default()
        }
    }

    pub fn with_format(format: TraceLogFormat) -> Self {
        Self {
            format,
            ..Self::default()
        }
    }

    pub fn formatter(&self) -> &TraceFormatter {
        &self.formatter
    }

    pub fn formatter_mut(&mut self) -> &mut TraceFormatter {
        &mut self.formatter
    }

    pub fn format(&self) -> TraceLogFormat {
        self.format
    }

    pub fn set_format(&mut self, format: TraceLogFormat) {
        self.format = format;
    }

    /// Maximum number of retained lines; 0 means unlimited.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity;
        if capacity > 0 {
            while self.lines.len() > capacity {
                self.lines.pop_front();
            }
        }
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Record one instruction.  Returns the stored line.
    pub fn log(
        &mut self,
        ctx: &TraceContext,
        info: &DisassemblyInfo,
        read: &mut dyn FnMut(u16, MemoryAccessType) -> u8,
    ) -> &str {
        let line = match self.format {
            TraceLogFormat::Text => self.formatter.format_line_eol(ctx, info, read),
            TraceLogFormat::Csv => {
                let mut line = format_trace_csv(ctx, info);
                line.push_str(self.formatter.eol());
                line
            }
        };
        self.push_line(line)
    }

    /// Disassemble at `ctx.pc` and record the resulting instruction.
    pub fn log_next(
        &mut self,
        ctx: &TraceContext,
        read: &mut dyn FnMut(u16, MemoryAccessType) -> u8,
    ) -> &str {
        let info = DisassemblyInfo::new(ctx.pc, read);
        self.log(ctx, &info, read)
    }

    fn push_line(&mut self, line: String) -> &str {
        self.lines.push_back(line);
        if self.capacity > 0 {
            while self.lines.len() > self.capacity {
                self.lines.pop_front();
            }
        }
        self.lines.back().map(String::as_str).unwrap_or("")
    }

    pub fn clear(&mut self) {
        self.lines.clear();
    }

    /// Iterate over stored lines (each includes its line ending).
    pub fn lines(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().map(String::as_str)
    }

    /// Concatenate all stored lines.
    pub fn text(&self) -> String {
        self.lines.iter().map(String::as_str).collect()
    }

    /// Write the trace to `path` using `std::fs`.
    pub fn save<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        std::fs::write(path, self.text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disassemble_bytes(addr: u16, bytes: &[u8]) -> (String, u8) {
        let mut read = |a: u16, access: MemoryAccessType| {
            assert_eq!(access, MemoryAccessType::Read);
            let offset = a.wrapping_sub(addr) as usize;
            bytes.get(offset).copied().unwrap_or(0)
        };
        Disassembler::disassemble(addr, &mut read)
    }

    fn info_from(bytes: &[u8]) -> DisassemblyInfo {
        DisassemblyInfo::from_bytes(bytes).expect("complete instruction")
    }

    fn read_from_ram(ram: &[u8]) -> impl FnMut(u16, MemoryAccessType) -> u8 + '_ {
        move |addr, access| {
            assert_eq!(access, MemoryAccessType::Read);
            ram[addr as usize]
        }
    }

    fn read_code(base: u16, bytes: &[u8]) -> impl FnMut(u16, MemoryAccessType) -> u8 + '_ {
        move |addr, access| {
            assert_eq!(access, MemoryAccessType::Read);
            let offset = addr.wrapping_sub(base) as usize;
            bytes.get(offset).copied().unwrap_or(0)
        }
    }

    #[test]
    fn official_addressing_modes() {
        let cases: &[(u16, &[u8], &str, u8)] = &[
            (0xC000, &[0xEA], "NOP", 1),
            (0xC000, &[0x00], "BRK", 1),
            (0xC000, &[0x48], "PHA", 1),
            (0xC000, &[0x0A], "ASL A", 1),
            (0xC000, &[0xA9, 0x42], "LDA #$42", 2),
            (0xC000, &[0xA5, 0x10], "LDA $10", 2),
            (0xC000, &[0xB5, 0x10], "LDA $10,X", 2),
            (0xC000, &[0xB6, 0x10], "LDX $10,Y", 2),
            (0xC000, &[0xAD, 0x34, 0x12], "LDA $1234", 3),
            (0xC000, &[0xBD, 0x34, 0x12], "LDA $1234,X", 3),
            (0xC000, &[0xB9, 0x34, 0x12], "LDA $1234,Y", 3),
            (0xC000, &[0x6C, 0x34, 0x12], "JMP ($1234)", 3),
            (0xC000, &[0xA1, 0x10], "LDA ($10,X)", 2),
            (0xC000, &[0xB1, 0x10], "LDA ($10),Y", 2),
            (0xC000, &[0x20, 0x34, 0x12], "JSR $1234", 3),
            (0xC000, &[0x4C, 0x34, 0x12], "JMP $1234", 3),
            (0xC000, &[0x8D, 0x00, 0x20], "STA $2000", 3),
        ];

        for (addr, bytes, expected, length) in cases {
            let (text, size) = disassemble_bytes(*addr, bytes);
            assert_eq!(text, *expected, "opcode bytes {bytes:02X?}");
            assert_eq!(size, *length, "opcode bytes {bytes:02X?}");
        }
    }

    #[test]
    fn branches_resolve_targets() {
        let cases: &[(u16, &[u8], &str)] = &[
            (0xC000, &[0xD0, 0x05], "BNE $C007"),
            (0xC000, &[0xF0, 0xFE], "BEQ $C000"),
            (0xC000, &[0x10, 0x80], "BPL $BF82"),
            (0xFFFF, &[0x10, 0x00], "BPL $0001"),
            (0x8000, &[0x30, 0x7F], "BMI $8081"),
        ];

        for (addr, bytes, expected) in cases {
            let (text, size) = disassemble_bytes(*addr, bytes);
            assert_eq!(text, *expected);
            assert_eq!(size, 2);
        }
    }

    #[test]
    fn unofficial_opcodes_have_star_and_formats() {
        let cases: &[(u16, &[u8], &str, u8)] = &[
            (0xC000, &[0x02], "STP*", 1),
            (0xC000, &[0x1A], "NOP*", 1),
            (0xC000, &[0x03, 0x10], "SLO* ($10,X)", 2),
            (0xC000, &[0x13, 0x10], "SLO* ($10),Y", 2),
            (0xC000, &[0x1B, 0x34, 0x12], "SLO* $1234,Y", 3),
            (0xC000, &[0x0B, 0x42], "ANC* #$42", 2),
            (0xC000, &[0x8B, 0x42], "XAA* #$42", 2),
            (0xC000, &[0x89, 0x42], "NOP* #$42", 2),
            (0xC000, &[0x93, 0x10], "AHX* ($10),Y", 2),
            (0xC000, &[0x9C, 0x34, 0x12], "SHY* $1234,X", 3),
            (0xC000, &[0x9E, 0x34, 0x12], "SHX* $1234,Y", 3),
            (0xC000, &[0x9F, 0x34, 0x12], "AXA* $1234,Y", 3),
            (0xC000, &[0xBB, 0x34, 0x12], "LAS* $1234,Y", 3),
            (0xC000, &[0xCB, 0x42], "AXS* #$42", 2),
            (0xC000, &[0xEB, 0x42], "SBC* #$42", 2),
            (0xC000, &[0xFF, 0x34, 0x12], "ISC* $1234,X", 3),
        ];

        for (addr, bytes, expected, length) in cases {
            let (text, size) = disassemble_bytes(*addr, bytes);
            assert_eq!(text, *expected, "opcode bytes {bytes:02X?}");
            assert_eq!(size, *length);
        }
    }

    #[test]
    fn all_256_opcodes_decode_with_table_length() {
        let mut unofficial_count = 0;
        for opcode in 0..=0xFFu8 {
            let info = &OPCODES[opcode as usize];
            assert!(!info.mnemonic.is_empty(), "opcode {opcode:02X}");
            assert_eq!(info.size, info.mode.size(), "opcode {opcode:02X}");
            assert!((1..=3).contains(&info.size));

            if info.unofficial {
                unofficial_count += 1;
            }

            let mut calls = 0u8;
            let mut read = |_: u16, access: MemoryAccessType| {
                assert_eq!(access, MemoryAccessType::Read);
                let value = if calls == 0 { opcode } else { 0x34 };
                calls += 1;
                value
            };
            let (text, size) = Disassembler::disassemble(0xC000, &mut read);

            assert_eq!(size, info.size, "opcode {opcode:02X}");
            assert_eq!(calls, size, "opcode {opcode:02X} read count");
            assert!(!text.is_empty(), "opcode {opcode:02X}");
            assert!(
                text.starts_with(info.mnemonic),
                "opcode {opcode:02X}: {text}"
            );
            if info.unofficial {
                assert!(
                    text.starts_with(&format!("{}*", info.mnemonic)),
                    "opcode {opcode:02X}: {text}"
                );
            } else {
                assert!(!text.contains('*'), "opcode {opcode:02X}: {text}");
            }
        }

        assert_eq!(unofficial_count, 105);
        assert_eq!(256 - unofficial_count, 151);
        assert!(Disassembler::is_unofficial(0x03));
        assert!(!Disassembler::is_unofficial(0xA9));
    }

    #[test]
    fn lowercase_and_unofficial_marker_options() {
        let info = info_from(&[0xA9, 0x42]);
        let lower = DisassemblyOptions {
            lowercase: true,
            ..Default::default()
        };
        assert_eq!(info.text_with_options(0xC000, &lower), "lda #$42");

        let illegal = info_from(&[0x03, 0x10]);
        assert_eq!(
            illegal.text_with_options(
                0xC000,
                &DisassemblyOptions {
                    lowercase: true,
                    ..Default::default()
                }
            ),
            "slo* ($10,X)"
        );

        let no_marker = DisassemblyOptions {
            mark_unofficial: false,
            ..Default::default()
        };
        assert_eq!(illegal.text_with_options(0xC000, &no_marker), "SLO ($10,X)");
    }

    #[test]
    fn extend_zero_page_only_applies_to_zero_page_modes() {
        let options = DisassemblyOptions {
            extend_zero_page: true,
            ..Default::default()
        };

        let cases: &[(u8, &[u8], &str)] = &[
            (0xA5, &[0xA5, 0x10], "LDA $0010"),
            (0xB5, &[0xB5, 0x10], "LDA $0010,X"),
            (0xB6, &[0xB6, 0x10], "LDX $0010,Y"),
            (0xA1, &[0xA1, 0x10], "LDA ($0010,X)"),
            (0xB1, &[0xB1, 0x10], "LDA ($0010),Y"),
            (0xA9, &[0xA9, 0x10], "LDA #$10"),
            (0xAD, &[0xAD, 0x34, 0x12], "LDA $1234"),
        ];

        for (opcode, bytes, expected) in cases {
            let info = info_from(bytes);
            assert_eq!(info.opcode, *opcode);
            assert_eq!(info.text_with_options(0xC000, &options), *expected);
        }
    }

    #[test]
    fn immediate_and_indirect_prefixes() {
        assert_eq!(disassemble_bytes(0xC000, &[0xA9, 0x00]).0, "LDA #$00");
        assert_eq!(disassemble_bytes(0xC000, &[0xA1, 0x10]).0, "LDA ($10,X)");
        assert_eq!(disassemble_bytes(0xC000, &[0xB1, 0x10]).0, "LDA ($10),Y");
        assert_eq!(
            disassemble_bytes(0xC000, &[0x6C, 0x34, 0x12]).0,
            "JMP ($1234)"
        );
        assert_eq!(disassemble_bytes(0xC000, &[0x0A]).0, "ASL A");
    }

    #[test]
    fn labels_replace_operands_but_not_immediates() {
        let labels = |addr: u16| match addr {
            0x1234 => Some("PlayerX".to_string()),
            0x10 => Some("zp_ptr".to_string()),
            0xC007 => Some("loop".to_string()),
            _ => None,
        };

        assert_eq!(
            Disassembler::disassemble_with_labels(
                0xC000,
                &mut read_code(0xC000, &[0xAD, 0x34, 0x12]),
                &DisassemblyOptions::default(),
                labels
            )
            .0,
            "LDA PlayerX"
        );
        assert_eq!(
            Disassembler::disassemble_with_labels(
                0xC000,
                &mut read_code(0xC000, &[0xB1, 0x10]),
                &DisassemblyOptions::default(),
                labels
            )
            .0,
            "LDA (zp_ptr),Y"
        );
        assert_eq!(
            Disassembler::disassemble_with_labels(
                0xC000,
                &mut read_code(0xC000, &[0xA9, 0x10]),
                &DisassemblyOptions::default(),
                labels
            )
            .0,
            "LDA #$10",
            "immediate operands are never labels"
        );
        assert_eq!(
            Disassembler::disassemble_with_labels(
                0xC000,
                &mut read_code(0xC000, &[0xD0, 0x05]),
                &DisassemblyOptions::default(),
                labels
            )
            .0,
            "BNE loop"
        );
    }

    #[test]
    fn effective_addresses_follow_nes_wrapping() {
        let mut ram = vec![0u8; 0x10000];
        ram[0x14] = 0x34;
        ram[0x15] = 0x12;
        ram[0x10] = 0xFF;
        ram[0x11] = 0x12;
        ram[0xFF] = 0x78;
        ram[0x00] = 0x56;

        let mut read = read_from_ram(&ram);

        // LDA ($14,X) -> pointer at $14/$15 -> $1234
        let indx = info_from(&[0xA1, 0x10]);
        assert_eq!(indx.effective_address(0x04, 0, &mut read), Some(0x1234));
        assert_eq!(
            indx.effective_address_string(0x04, 0, &mut read, None),
            " @ $1234"
        );
        let labels = |addr: u16| (addr == 0x1234).then(|| "PlayerX".to_string());
        assert_eq!(
            indx.effective_address_string(0x04, 0, &mut read, Some(&labels)),
            " @ PlayerX"
        );

        // LDA $10,X with X=$FF wraps within zero page: $10 + $FF = $0F
        let zpx = info_from(&[0xB5, 0x10]);
        assert_eq!(zpx.effective_address(0xFF, 0, &mut read), Some(0x000F));
        assert_eq!(
            zpx.effective_address_string(0xFF, 0, &mut read, None),
            " @ $0F"
        );

        // LDA ($10),Y -> $12FF + 1 = $1300
        let indy = info_from(&[0xB1, 0x10]);
        assert_eq!(indy.effective_address(0, 1, &mut read), Some(0x1300));

        // LDA ($FF,X) with X=0 -> pointer bytes at $FF and $00 (zero-page wrap)
        let indx_wrap = info_from(&[0xA1, 0xFF]);
        assert_eq!(indx_wrap.effective_address(0, 0, &mut read), Some(0x5678));

        // Absolute indexed wrap.
        let absx = info_from(&[0xBD, 0xFF, 0xFF]);
        assert_eq!(absx.effective_address(1, 0, &mut read), Some(0x0000));

        // No effective address for absolute or immediate modes.
        let abs = info_from(&[0xAD, 0x34, 0x12]);
        assert_eq!(abs.effective_address(0xFF, 0xFF, &mut read), None);
        assert_eq!(
            abs.effective_address_string(0xFF, 0xFF, &mut read, None),
            ""
        );
        let imm = info_from(&[0xA9, 0x01]);
        assert_eq!(imm.effective_address(0xFF, 0xFF, &mut read), None);
    }

    #[test]
    fn indirect_jump_has_6502_page_wrap_bug() {
        let mut ram = vec![0u8; 0x10000];
        ram[0x12FF] = 0x34;
        ram[0x1200] = 0x12;
        ram[0x1300] = 0x99;

        let mut read = read_from_ram(&ram);
        let jump = info_from(&[0x6C, 0xFF, 0x12]);
        assert_eq!(jump.jump_destination(0xC000, &mut read), Some(0x1234));

        let normal = info_from(&[0x6C, 0x00, 0x13]);
        // $1300/$1301 (the latter defaults to 0)
        assert_eq!(normal.jump_destination(0xC000, &mut read), Some(0x0099));
    }

    #[test]
    fn jump_destinations_and_memory_values() {
        let mut ram = vec![0u8; 0x10000];
        ram[0x1234] = 0x5A;
        ram[0x0010] = 0x77;
        ram[0x0011] = 0x88;

        let mut read = read_from_ram(&ram);

        let abs = info_from(&[0xAD, 0x34, 0x12]);
        assert_eq!(abs.jump_destination(0xC000, &mut read), None);
        assert_eq!(abs.memory_value(0xC000, 0, 0, &mut read), Some(0x5A));

        let imm = info_from(&[0xA9, 0x42]);
        assert_eq!(imm.jump_destination(0xC000, &mut read), None);
        assert_eq!(imm.memory_value(0xC000, 0, 0, &mut read), None);

        let branch = info_from(&[0xD0, 0x05]);
        assert_eq!(branch.jump_destination(0xC000, &mut read), Some(0xC007));
        assert_eq!(branch.memory_value(0xC000, 0, 0, &mut read), Some(0x00));

        let jmp = info_from(&[0x4C, 0x34, 0x12]);
        assert_eq!(jmp.jump_destination(0xC000, &mut read), Some(0x1234));

        let jsr = info_from(&[0x20, 0x34, 0x12]);
        assert_eq!(jsr.jump_destination(0xC000, &mut read), Some(0x1234));

        // LDA ($10),Y with Y=1 reads $8888 + 1 = $8889 -> 0
        let indy = info_from(&[0xB1, 0x10]);
        assert_eq!(indy.memory_value(0xC000, 0, 1, &mut read), Some(0x00));
    }

    #[test]
    fn jump_classification_matches_mesen() {
        for opcode in 0..=0xFFu8 {
            let expected_jump = matches!(
                opcode,
                0x10 | 0x30
                    | 0x50
                    | 0x70
                    | 0x90
                    | 0xB0
                    | 0xD0
                    | 0xF0
                    | 0x4C
                    | 0x6C
                    | 0x20
            );
            let expected_unconditional =
                matches!(opcode, 0x40 | 0x60 | 0x6C | 0x4C | 0x20);

            assert_eq!(Disassembler::is_jump(opcode), expected_jump);
            assert_eq!(
                Disassembler::is_unconditional_jump(opcode),
                expected_unconditional
            );
        }
    }

    #[test]
    fn byte_code_and_size_helpers() {
        let info = info_from(&[0xAD, 0x34, 0x12]);
        assert_eq!(info.byte_code_string(), "$AD $34 $12");
        assert_eq!(info.byte_code_compact(), "AD3412");
        assert_eq!(info.size, 3);
        assert_eq!(Disassembler::size(0xAD), 3);
        assert!(!info.is_sub_entry_point());
        assert!(!info.is_sub_exit_point());

        let rts = info_from(&[0x60]);
        assert!(rts.is_sub_exit_point());

        let mut sub = info_from(&[0x20, 0x00, 0xC0]);
        sub.set_sub_entry_point();
        assert!(sub.is_sub_entry_point());
        assert!(!sub.is_sub_exit_point());

        assert!(DisassemblyInfo::from_bytes(&[0xAD, 0x34]).is_none());
        assert!(DisassemblyInfo::from_bytes(&[]).is_none());
    }

    #[test]
    fn register_and_flag_formatting() {
        assert_eq!(
            format_registers(0xC000, 0x01, 0x02, 0x03, 0xFD, 0x24),
            "PC:C000 A:01 X:02 Y:03 SP:FD PS:24"
        );
        assert_eq!(format_address(0x1234), "$1234");
        assert_eq!(format_status_flags(0x24, 8), "nv--dIzc");
        // The reserved bit prints as '-' just like Mesen.
        assert_eq!(format_status_flags(0x24, 4), "-I");
        assert_eq!(format_status_flags(0x04, 4), "I");
        assert_eq!(format_status_flags(0x81, 0), "NC");
    }

    #[test]
    fn trace_formatter_default_line() {
        let formatter = TraceFormatter::default();
        let ctx = TraceContext::new(0xC000, 0x00, 0x00, 0x00, 0xFD, 0x24);
        let info = info_from(&[0xEA]);
        let mut read = |_: u16, _: MemoryAccessType| 0u8;

        assert_eq!(
            formatter.format_line(&ctx, &info, &mut read),
            "C000 EA NOP A:00 X:00 Y:00 SP:FD P:24"
        );
        assert_eq!(
            formatter.format_line_eol(&ctx, &info, &mut read),
            "C000 EA NOP A:00 X:00 Y:00 SP:FD P:24\n"
        );
    }

    #[test]
    fn trace_formatter_tags_and_columns() {
        let formatter = TraceFormatter::with_format(
            "[PC,4h] [ByteCode,h] [Disassembly] A:[A,2h] X:[X,2h] Y:[Y,2h] \
             SP:[SP,2h] P:[P,8] [EffectiveAddress] [MemoryValue,h]",
        );
        let ctx = TraceContext::new(0xC000, 0x00, 0x11, 0x00, 0xFD, 0x24);
        let info = info_from(&[0xBD, 0x34, 0x12]);

        let mut ram = vec![0u8; 0x10000];
        ram[0x1234 + 0x11] = 0x5A;
        let mut read = read_from_ram(&ram);

        let line = formatter.format_line(&ctx, &info, &mut read);
        assert_eq!(
            line,
            "C000 $BD $34 $12 LDA $1234,X A:00 X:11 Y:00 SP:FD P:nv--dIzc  @ $1245 = $5A"
        );

        // Without the hex flag the byte code loses its `$` prefixes.
        let plain_byte_code = TraceFormatter::with_format("[ByteCode]");
        assert_eq!(
            plain_byte_code.format_line(&ctx, &info, &mut read),
            "BD 34 12"
        );
    }

    #[test]
    fn trace_formatter_align_invalid_and_eol() {
        let ctx = TraceContext::new(0xC000, 1, 2, 3, 0xFD, 0x00);
        let info = info_from(&[0xEA]);
        let mut read = |_: u16, _: MemoryAccessType| 0u8;

        let aligned = TraceFormatter::with_format("[A,2h][Align,10]X");
        assert_eq!(aligned.format_line(&ctx, &info, &mut read), "01        X");

        let invalid = TraceFormatter::with_format("[Bogus]");
        assert_eq!(invalid.format_line(&ctx, &info, &mut read), "[Invalid tag]");

        let case_insensitive = TraceFormatter::with_format("[disassembly]");
        assert_eq!(case_insensitive.format_line(&ctx, &info, &mut read), "NOP");

        let mut windows = TraceFormatter::with_format("[Disassembly]");
        windows.use_windows_eol = true;
        assert_eq!(
            windows.format_line_eol(&ctx, &info, &mut read),
            "NOP\r\n"
        );
    }

    #[test]
    fn trace_formatter_indent_and_labels() {
        let formatter = TraceFormatter::with_format("[Disassembly]");
        let ctx = TraceContext::new(0xC000, 0, 0, 0, 0xFB, 0x00);
        let info = info_from(&[0xAD, 0x34, 0x12]);
        let mut read = |_: u16, _: MemoryAccessType| 0u8;

        let labels = |addr: u16| (addr == 0x1234).then(|| "PlayerX".to_string());
        assert_eq!(
            formatter.format_line_with_labels(&ctx, &info, &mut read, Some(&labels)),
            "LDA PlayerX"
        );

        let mut indented = TraceFormatter::with_format("[Disassembly]");
        indented.indent_code = true;
        assert_eq!(
            indented.format_line(&ctx, &info, &mut read),
            "    LDA $1234"
        );
    }

    #[test]
    fn trace_disassembly_min_width_includes_indent() {
        let mut formatter = TraceFormatter::with_format("[Disassembly,12]");
        formatter.indent_code = true;
        let ctx = TraceContext::new(0xC000, 0, 0, 0, 0xFC, 0);
        let info = info_from(&[0xEA]);
        let mut read = |_: u16, _: MemoryAccessType| 0u8;

        let line = formatter.format_line(&ctx, &info, &mut read);
        assert_eq!(line, "   NOP      ");
        assert_eq!(line.len(), 12);
    }

    #[test]
    fn labels_are_not_queried_for_operandless_instructions() {
        let queried = std::cell::RefCell::new(Vec::new());
        let labels = |addr: u16| {
            queried.borrow_mut().push(addr);
            None
        };
        for bytes in [&[0xEA][..], &[0x0A][..], &[0x48][..]] {
            let info = info_from(bytes);
            assert_eq!(
                info.text_with_labels(0xC000, &DisassemblyOptions::default(), &labels),
                info.text(0xC000)
            );
        }
        assert!(queried.borrow().is_empty());
    }

    #[test]
    fn trace_formatter_next_and_csv() {
        let ctx = TraceContext::new(0xC000, 0x7F, 0x00, 0x00, 0xFD, 0x24)
            .with_cycle_count(123);
        let mut read = |_: u16, _: MemoryAccessType| 0xEAu8;

        let formatter = TraceFormatter::default();
        assert_eq!(
            formatter.format_next(&ctx, &mut read),
            "C000 EA NOP A:7F X:00 Y:00 SP:FD P:24"
        );

        let info = info_from(&[0xBD, 0x34, 0x12]);
        assert_eq!(
            format_trace_csv(&ctx, &info),
            "C000,BD3412,\"LDA $1234,X\",7F,00,00,FD,24,123"
        );
        assert_eq!(
            format_trace_csv(&ctx, &info_from(&[0xEA])),
            "C000,EA,NOP,7F,00,00,FD,24,123"
        );
        assert_eq!(
            TRACE_CSV_HEADER,
            "PC,ByteCode,Disassembly,A,X,Y,SP,PS,CycleCount"
        );
    }

    #[test]
    fn trace_logger_ring_buffer_and_formats() {
        let mut logger = TraceLogger::with_capacity(2);
        let mut read = |_: u16, _: MemoryAccessType| 0xEAu8;

        for pc in [0xC000u16, 0xC001, 0xC002] {
            let ctx = TraceContext::new(pc, 0, 0, 0, 0xFD, 0x24);
            logger.log_next(&ctx, &mut read);
        }

        assert_eq!(logger.len(), 2);
        let lines: Vec<&str> = logger.lines().collect();
        assert!(lines[0].starts_with("C001"));
        assert!(lines[1].starts_with("C002"));
        assert_eq!(logger.text(), lines.concat());

        logger.clear();
        assert!(logger.is_empty());

        let mut csv = TraceLogger::with_format(TraceLogFormat::Csv);
        let ctx = TraceContext::new(0xC000, 0, 0, 0, 0xFD, 0x24).with_cycle_count(5);
        csv.log_next(&ctx, &mut read);
        assert_eq!(csv.lines().next().unwrap(), "C000,EA,NOP,00,00,00,FD,24,5\n");

        csv.set_format(TraceLogFormat::Text);
        assert_eq!(csv.format(), TraceLogFormat::Text);

        csv.set_capacity(0);
        assert_eq!(csv.capacity(), 0);
        assert_eq!(TraceLogger::csv_header(), TRACE_CSV_HEADER);
    }
}
