//! 6502 CPU core.
//!
//! Direct Rust translation of Mesen 0.9.9 `Core/CPU.cpp` / `Core/CPU.h`.
//! Original C++: Copyright (C) 2014-2020 Sour, GPL-3.0.
//! Rust port: GPL-3.0-or-later.
//!
//! OWNER: cpu agent. The public API below is the contract used by `Bus` and
//! `Emulator`; internals can be rewritten freely as long as this file compiles
//! and the behavior is cycle-accurate enough for NES software.
//!
//! Timing model (see `PORTING.md` §2): each memory access advances the bus by
//! one CPU cycle (`bus.tick(1)`) before the access and latches the interrupt
//! lines after it, mirroring Mesen's `StartCpuCycle`/`EndCpuCycle` pair. One
//! `step` runs one Mesen `CPU::Exec()` (instruction + optional interrupt
//! sequence); the returned cycle count is exactly the number of cycles clocked.
//! DMA stall cycles are clocked one at a time as part of the same step.
//!
//! Dummy reads/writes, page-cross penalties, zero-page wrapping, the JMP
//! indirect page bug, interrupt sequences and the undocumented opcodes follow
//! Mesen's implementation. The decimal flag is stored/updated but has no effect
//! on ADC/SBC, as required by the NES 2A03.
//!
//! Deliberate deviations from a cycle-exact 6502 (and from Mesen internals):
//! * `_runIrq`/`_prevRunIrq` and `_needNmi`/`_prevNeedNmi` are latched per
//!   memory-access cycle, but the bus can only be advanced in whole CPU cycles
//!   (`bus.tick(1)` = 3 PPU dots). Tests that require single-PPU-dot NMI
//!   suppression/disable timing can therefore still differ by one dot.
//! * `PLP`/`RTI` force status bit 5 to 1, which is what the hardware exposes
//!   when the register is read (Mesen clears it internally); canonical nestest
//!   traces compare that bit.
//! * `$02`/`$12`/... (`JAM`/`HLT`) are 1-cycle no-ops exactly like Mesen's
//!   `HLT` handler instead of hanging the CPU.
//! * DMC DMA stalls are not generated here (the bus API does not expose them);
//!   OAM DMA stall cycles are clocked and sampled cycle by cycle.

use crate::bus::Bus;
use crate::state::{StateReader, StateResult, StateWriter};
use crate::types::flags::{
    BREAK, CARRY, DECIMAL, INTERRUPT, NEGATIVE, OVERFLOW, RESERVED, ZERO,
};
use crate::types::MemoryAccessType;

const NMI_VECTOR: u16 = 0xFFFA;
const RESET_VECTOR: u16 = 0xFFFC;
const IRQ_VECTOR: u16 = 0xFFFE;

/// The CPU takes 7 cycles (as reported by its cycle counter) before the first
/// instruction at the reset vector is fetched.
const RESET_CYCLES: u32 = 7;

const READ: MemoryAccessType = MemoryAccessType::Read;
const WRITE: MemoryAccessType = MemoryAccessType::Write;
const DUMMY_READ: MemoryAccessType = MemoryAccessType::DummyRead;
const DUMMY_WRITE: MemoryAccessType = MemoryAccessType::DummyWrite;
const EXEC_OPCODE: MemoryAccessType = MemoryAccessType::ExecOpCode;
const EXEC_OPERAND: MemoryAccessType = MemoryAccessType::ExecOperand;

#[derive(Clone, Copy, Debug, Default)]
pub struct CpuState {
    pub pc: u16,
    pub sp: u8,
    pub a: u8,
    pub x: u8,
    pub y: u8,
    pub ps: u8,
    pub irq_flag: u32,
    pub cycle_count: u64,
}

/// Addressing modes, in the same order as Mesen's `AddrMode` enum. The order
/// matters: modes below `Zero` are operand values, modes from `Zero` on are
/// effective addresses that still need a data read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum AddrMode {
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
    #[inline]
    fn is_memory(self) -> bool {
        self >= AddrMode::Zero
    }
}

/// One decoded 6502 operation. Mirrors the function pointers in Mesen's
/// `_opTable`; several opcodes share an operation but use different addressing
/// modes from `ADDR_MODES`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Brk,
    Ora,
    Hlt,
    Slo,
    Nop,
    AslMem,
    Php,
    AslAcc,
    Aac,
    Bpl,
    Clc,
    Jsr,
    And,
    Rla,
    Bit,
    RolMem,
    Plp,
    RolAcc,
    Bmi,
    Sec,
    Rti,
    Eor,
    Sre,
    LsrMem,
    Pha,
    LsrAcc,
    Asr,
    JmpAbs,
    Bvc,
    Cli,
    Rts,
    Adc,
    Rra,
    RorMem,
    Pla,
    RorAcc,
    Arr,
    JmpInd,
    Bvs,
    Sei,
    Sta,
    Sax,
    Sty,
    Stx,
    Dey,
    Txa,
    Unk,
    Bcc,
    Axa,
    Tya,
    Txs,
    Tas,
    Sya,
    Sxa,
    Ldy,
    Lda,
    Ldx,
    Lax,
    Tay,
    Tax,
    Atx,
    Bcs,
    Clv,
    Tsx,
    Las,
    Cpy,
    Cpa,
    Dcp,
    Dec,
    Inc,
    Iny,
    Dex,
    Axs,
    Bne,
    Cld,
    Cpx,
    Sbc,
    Isb,
    Inx,
    Beq,
    Sed,
}

pub struct Cpu {
    pub state: CpuState,
    /// Total CPU cycles executed (including DMA stalls).
    pub total_cycles: u64,
    /// Cycles accumulated by the instruction currently being stepped. This is
    /// transient (reset at the start of each step) but serialized so that every
    /// mutable field round-trips through save states.
    cycles: u32,
    /// Mesen CPU::_runIrq: IRQ line state sampled at the end of the most recent
    /// CPU cycle (`IRQFlag && !I`).
    run_irq: bool,
    /// Mesen CPU::_prevRunIrq: `run_irq` from the preceding cycle. Executor
    /// checks this after an instruction, producing the documented CLI/PLP/RTI
    /// interrupt delay.
    prev_run_irq: bool,
    /// Mesen CPU::_needNmi: an NMI edge has been detected and is awaiting
    /// service.
    need_nmi: bool,
    /// Mesen CPU::_prevNeedNmi: `need_nmi` delayed by one CPU cycle. An NMI
    /// edge detected during the last cycle of an instruction is therefore
    /// serviced only after the following instruction.
    prev_need_nmi: bool,
}

impl Cpu {
    pub fn new() -> Self {
        Self {
            // Post-construction, pre-reset state, mirroring Mesen's CPU
            // constructor (`state = {}`): the first `reset` then performs the
            // power-on adjustments, yielding SP=$FD, A=X=Y=0 and PS=$24 (the
            // reserved bit is kept set for hardware-accurate status reads).
            state: CpuState {
                sp: 0x00,
                ps: RESERVED | INTERRUPT,
                ..Default::default()
            },
            total_cycles: 0,
            cycles: 0,
            run_irq: false,
            prev_run_irq: false,
            need_nmi: false,
            prev_need_nmi: false,
        }
    }

    /// Interrupt/reset sequence: read the reset vector, apply the (soft) reset
    /// register/stack changes, and advance the machine over the cycles the CPU
    /// spends before fetching the first instruction.
    ///
    /// Mesen runs eight CPU cycles after reset while its reported cycle counter
    /// ends up at 7 (`_cycleCount` starts at -1). The extra cycles matter to
    /// the PPU/APU (apu_reset tests), while the first instruction still shows
    /// CYC=7 in traces/nestest.
    pub fn reset(&mut self, bus: &mut Bus) {
        self.reset_internal(bus);
        // Mesen runs eight CPU cycles after reset (while the reported cycle
        // counter ends at 7). Clock them one at a time so the PPU/APU see a
        // normal reset warm-up and the interrupt lines are sampled.
        for _ in 0..8 {
            bus.tick(1);
            self.end_cycle(bus);
        }
        bus.align_ppu_after_reset();
        // Mesen's reported CPU cycle counter is 7 after reset (_cycleCount
        // starts at -1). Keep the bus's CPU-cycle counter in step with it so
        // OAM DMA's odd/even extra cycle uses the same parity.
        bus.master_cycles = RESET_CYCLES as u64;
    }

    /// Reset logic without clocking the bus, split out for unit tests (the PPU
    /// and APU can be exercised separately) and reused by `reset`.
    fn reset_internal(&mut self, bus: &mut Bus) -> u32 {
        self.state.irq_flag = 0;

        // Use plain reads (no extra clocks) while setting PC, like Mesen's
        // reset path through MemoryManager::Read.
        let lo = bus.read(RESET_VECTOR, READ) as u16;
        let hi = bus.read(RESET_VECTOR + 1, READ) as u16;
        self.state.pc = lo | (hi << 8);

        // Mesen always performs a "soft" reset of the registers: A/X/Y are
        // preserved after the initial power-on, SP is decremented by 3 and the
        // interrupt-disable flag is set.
        self.state.sp = self.state.sp.wrapping_sub(3);
        self.state.ps |= INTERRUPT;
        self.state.ps &= !BREAK;
        self.state.ps |= RESERVED;

        // Mesen clears its NMI latch on reset. The PPU owns the flag in this
        // port, so consume/discard a pending edge as well.
        let _ = bus.take_nmi();

        self.cycles = 0;
        self.run_irq = false;
        self.prev_run_irq = false;
        self.need_nmi = false;
        self.prev_need_nmi = false;
        self.total_cycles = RESET_CYCLES as u64;
        self.state.cycle_count = self.total_cycles;
        RESET_CYCLES
    }

    /// Execute one instruction (including any interrupt/NMI it triggers) and
    /// return the number of CPU cycles it consumed. The bus is advanced one
    /// cycle at a time from inside the memory-access helpers.
    pub fn step(&mut self, bus: &mut Bus) -> u32 {
        self.execute_step(bus)
    }

    /// One Mesen `CPU::Exec()`: execute an instruction, then service an IRQ/NMI
    /// if the previous cycle's sampled lines say one is due.
    fn execute_step(&mut self, bus: &mut Bus) -> u32 {
        self.cycles = 0;

        self.execute_instruction(bus);

        // OAM DMA (and future DMC DMA) stalls are accumulated by the bus and
        // must be accounted for by the CPU. Clock them one at a time so
        // interrupt lines are sampled during the DMA.
        let irq_before_dma = self.prev_run_irq;
        let stall = bus.take_stall_cycles();
        for _ in 0..stall {
            bus.tick(1);
            self.end_cycle(bus);
        }

        // An IRQ that only became pending during sprite DMA is recognized one
        // instruction later on hardware (the DMA's last read is the polled
        // cycle). An IRQ already pending before DMA is serviced immediately.
        let irq_during_dma = stall > 0 && !irq_before_dma && self.prev_run_irq;

        // Mirrors `CPU::Exec()`: the decision uses the values sampled at the
        // end of the previous CPU cycle, so a newly asserted interrupt waits for
        // at least one more instruction (CLI/PLP/RTI and NMI latency).
        if (self.prev_run_irq || self.prev_need_nmi) && !irq_during_dma {
            self.interrupt(bus);
        }

        let cycles = self.cycles.wrapping_add(stall);

        self.total_cycles = self.total_cycles.wrapping_add(cycles as u64);
        self.state.cycle_count = self.total_cycles;
        cycles
    }

    fn execute_instruction(&mut self, bus: &mut Bus) {
        let opcode = self.fetch_opcode(bus);
        let mode = ADDR_MODES[opcode as usize];
        let operand = self.fetch_operand(bus, mode);
        self.execute_op(bus, OP_TABLE[opcode as usize], mode, operand);
    }

    // ---------------------------------------------------------------------
    // Flag / register helpers
    // ---------------------------------------------------------------------

    #[inline]
    fn flag(&self, flag: u8) -> bool {
        (self.state.ps & flag) == flag
    }

    #[inline]
    fn set_flag(&mut self, flag: u8) {
        self.state.ps |= flag;
    }

    #[inline]
    fn clear_flag(&mut self, flag: u8) {
        self.state.ps &= !flag;
    }

    /// Clear Z/N and set them from `value` (Mesen's `SetZeroNegativeFlags`).
    #[inline]
    fn set_zn(&mut self, value: u8) {
        self.state.ps &= !(ZERO | NEGATIVE);
        if value == 0 {
            self.state.ps |= ZERO;
        } else if value & 0x80 != 0 {
            self.state.ps |= NEGATIVE;
        }
    }

    #[inline]
    fn set_a(&mut self, value: u8) {
        self.state.a = value;
        self.set_zn(value);
    }

    #[inline]
    fn set_x(&mut self, value: u8) {
        self.state.x = value;
        self.set_zn(value);
    }

    #[inline]
    fn set_y(&mut self, value: u8) {
        self.state.y = value;
        self.set_zn(value);
    }

    /// PLP/RTI: B and bit 5 are not real flags. The NES forces bit 5 to 1 when
    /// the status register is read back, which is also what the canonical
    /// nestest traces show.
    #[inline]
    fn set_ps_from_stack(&mut self, value: u8) {
        self.state.ps = (value & 0xCF) | RESERVED;
    }

    #[inline]
    fn page_crossed(base: u16, index: u8) -> bool {
        (base & 0xFF00) != (base.wrapping_add(index as u16) & 0xFF00)
    }

    // ---------------------------------------------------------------------
    // Bus access helpers. Every call is exactly one CPU cycle, matching
    // Mesen's StartCpuCycle/EndCpuCycle around MemoryRead/MemoryWrite.
    // ---------------------------------------------------------------------

    #[inline]
    fn read(&mut self, bus: &mut Bus, addr: u16, access: MemoryAccessType) -> u8 {
        self.cycles = self.cycles.wrapping_add(1);
        // Mesen splits a CPU cycle into a pre-access and post-access half. The
        // access lands 5/12 of the way through an NTSC cycle (7/12 for writes),
        // which is required for single-PPU-dot NMI suppression tests.
        bus.begin_access(true);
        let value = bus.read(addr, access);
        bus.end_access(true);
        self.end_cycle(bus);
        value
    }

    #[inline]
    fn write(&mut self, bus: &mut Bus, addr: u16, value: u8, access: MemoryAccessType) {
        self.cycles = self.cycles.wrapping_add(1);
        bus.begin_access(false);
        bus.write(addr, value, access);
        bus.end_access(false);
        self.end_cycle(bus);
    }

    /// Mesen's `CPU::EndCpuCycle`: latch the NMI edge detector and the IRQ
    /// line state at the end of one CPU cycle. `_prevRunIrq`/`_prevNeedNmi`
    /// intentionally hold the previous cycle's values; `Exec` checks those
    /// after the instruction, which is what gives the 6502's one-instruction
    /// interrupt latency.
    #[inline]
    fn end_cycle(&mut self, bus: &mut Bus) {
        self.prev_need_nmi = self.need_nmi;

        // The PPU owns the NMI flag in this port. Its `take_nmi` returns (and
        // clears) a pending NMI edge; latch it exactly like Mesen's 0->1 edge
        // detector.
        if bus.take_nmi() {
            self.need_nmi = true;
        }

        self.prev_run_irq = self.run_irq;
        self.run_irq = bus.irq_pending() && !self.flag(INTERRUPT);
    }

    #[inline]
    fn dummy_read(&mut self, bus: &mut Bus, addr: u16) {
        let _ = self.read(bus, addr, DUMMY_READ);
    }

    #[inline]
    fn dummy_read_pc(&mut self, bus: &mut Bus) {
        self.dummy_read(bus, self.state.pc);
    }

    fn fetch_opcode(&mut self, bus: &mut Bus) -> u8 {
        let opcode = self.read(bus, self.state.pc, EXEC_OPCODE);
        self.state.pc = self.state.pc.wrapping_add(1);
        opcode
    }

    fn read_byte(&mut self, bus: &mut Bus) -> u8 {
        let value = self.read(bus, self.state.pc, EXEC_OPERAND);
        self.state.pc = self.state.pc.wrapping_add(1);
        value
    }

    /// Read a little-endian word at the current PC and advance PC by 2.
    fn read_word_at_pc(&mut self, bus: &mut Bus) -> u16 {
        let lo = self.read(bus, self.state.pc, EXEC_OPERAND) as u16;
        let hi = self.read(bus, self.state.pc.wrapping_add(1), EXEC_OPERAND) as u16;
        self.state.pc = self.state.pc.wrapping_add(2);
        lo | (hi << 8)
    }

    /// Read a little-endian word from an arbitrary address (vectors, JMP
    /// indirect). The high byte wraps within the 64KB address space.
    fn read_word(&mut self, bus: &mut Bus, addr: u16) -> u16 {
        let lo = self.read(bus, addr, READ) as u16;
        let hi = self.read(bus, addr.wrapping_add(1), READ) as u16;
        lo | (hi << 8)
    }

    /// Read a zero-page pointer. The high byte wraps inside the zero page.
    fn read_zero_word(&mut self, bus: &mut Bus, zero: u8) -> u16 {
        let lo = self.read(bus, zero as u16, READ) as u16;
        let hi = self.read(bus, zero.wrapping_add(1) as u16, READ) as u16;
        lo | (hi << 8)
    }

    #[inline]
    fn push(&mut self, bus: &mut Bus, value: u8) {
        self.write(bus, 0x0100 | self.state.sp as u16, value, WRITE);
        self.state.sp = self.state.sp.wrapping_sub(1);
    }

    #[inline]
    fn push_word(&mut self, bus: &mut Bus, value: u16) {
        self.push(bus, (value >> 8) as u8);
        self.push(bus, value as u8);
    }

    #[inline]
    fn pop(&mut self, bus: &mut Bus) -> u8 {
        self.state.sp = self.state.sp.wrapping_add(1);
        self.read(bus, 0x0100 | self.state.sp as u16, READ)
    }

    #[inline]
    fn pop_word(&mut self, bus: &mut Bus) -> u16 {
        let lo = self.pop(bus) as u16;
        let hi = self.pop(bus) as u16;
        lo | (hi << 8)
    }

    // ---------------------------------------------------------------------
    // Addressing modes (Mesen CPU.h Get* helpers)
    // ---------------------------------------------------------------------

    fn fetch_operand(&mut self, bus: &mut Bus, mode: AddrMode) -> u16 {
        match mode {
            AddrMode::Acc | AddrMode::Imp => {
                // Implied/accumulator instructions perform a dummy read at PC.
                self.dummy_read_pc(bus);
                0
            }
            AddrMode::None => 0,
            AddrMode::Imm | AddrMode::Rel => self.read_byte(bus) as u16,
            AddrMode::Zero => self.read_byte(bus) as u16,
            AddrMode::ZeroX => {
                let value = self.read_byte(bus);
                self.dummy_read(bus, value as u16);
                value.wrapping_add(self.state.x) as u16
            }
            AddrMode::ZeroY => {
                let value = self.read_byte(bus);
                self.dummy_read(bus, value as u16);
                value.wrapping_add(self.state.y) as u16
            }
            AddrMode::Ind => self.read_word_at_pc(bus),
            AddrMode::IndX => {
                let zero = self.read_byte(bus);
                self.dummy_read(bus, zero as u16);
                let zero = zero.wrapping_add(self.state.x);
                // The pointer itself wraps inside zero page.
                self.read_zero_word(bus, zero)
            }
            AddrMode::IndY | AddrMode::IndYW => {
                let zero = self.read_byte(bus);
                let base = self.read_zero_word(bus, zero);
                let crossed = Self::page_crossed(base, self.state.y);
                if crossed || mode == AddrMode::IndYW {
                    // Wrong-page dummy read. For read instructions this only
                    // happens on a page cross; for write/RMW forms it always
                    // happens.
                    let addr = base
                        .wrapping_add(self.state.y as u16)
                        .wrapping_sub(if crossed { 0x100 } else { 0 });
                    self.dummy_read(bus, addr);
                }
                base.wrapping_add(self.state.y as u16)
            }
            AddrMode::Abs => self.read_word_at_pc(bus),
            AddrMode::AbsX | AddrMode::AbsXW => {
                let base = self.read_word_at_pc(bus);
                let crossed = Self::page_crossed(base, self.state.x);
                if crossed || mode == AddrMode::AbsXW {
                    let addr = base
                        .wrapping_add(self.state.x as u16)
                        .wrapping_sub(if crossed { 0x100 } else { 0 });
                    self.dummy_read(bus, addr);
                }
                base.wrapping_add(self.state.x as u16)
            }
            AddrMode::AbsY | AddrMode::AbsYW => {
                let base = self.read_word_at_pc(bus);
                let crossed = Self::page_crossed(base, self.state.y);
                if crossed || mode == AddrMode::AbsYW {
                    let addr = base
                        .wrapping_add(self.state.y as u16)
                        .wrapping_sub(if crossed { 0x100 } else { 0 });
                    self.dummy_read(bus, addr);
                }
                base.wrapping_add(self.state.y as u16)
            }
        }
    }

    /// Mesen's `GetOperandValue`: immediate/implied modes return the operand
    /// byte directly, memory modes perform the data read.
    #[inline]
    fn operand_value(&mut self, bus: &mut Bus, mode: AddrMode, operand: u16) -> u8 {
        if mode.is_memory() {
            self.read(bus, operand, READ)
        } else {
            operand as u8
        }
    }

    /// JMP ($xxFF) page bug: the high byte comes from the same page.
    fn read_indirect(&mut self, bus: &mut Bus, addr: u16) -> u16 {
        if addr & 0xFF == 0xFF {
            let lo = self.read(bus, addr, READ) as u16;
            let hi = self.read(bus, addr & 0xFF00, READ) as u16;
            lo | (hi << 8)
        } else {
            self.read_word(bus, addr)
        }
    }

    // ---------------------------------------------------------------------
    // ALU / flag operations
    // ---------------------------------------------------------------------

    fn add(&mut self, value: u8) {
        let a = self.state.a;
        let carry = u16::from(self.flag(CARRY));
        let result = a as u16 + value as u16 + carry;

        self.state.ps &= !(CARRY | NEGATIVE | OVERFLOW | ZERO);
        self.set_zn(result as u8);
        if !(a ^ value) & (a ^ (result as u8)) & 0x80 != 0 {
            self.set_flag(OVERFLOW);
        }
        if result > 0xFF {
            self.set_flag(CARRY);
        }
        // Decimal mode is deliberately ignored on the 2A03.
        self.state.a = result as u8;
    }

    fn compare(&mut self, reg: u8, value: u8) {
        self.state.ps &= !(CARRY | NEGATIVE | ZERO);
        let result = reg.wrapping_sub(value);
        if reg >= value {
            self.set_flag(CARRY);
        }
        if reg == value {
            self.set_flag(ZERO);
        }
        if result & 0x80 != 0 {
            self.set_flag(NEGATIVE);
        }
    }

    fn asl(&mut self, value: u8) -> u8 {
        self.state.ps &= !(CARRY | NEGATIVE | ZERO);
        if value & 0x80 != 0 {
            self.set_flag(CARRY);
        }
        let result = value << 1;
        self.set_zn(result);
        result
    }

    fn lsr(&mut self, value: u8) -> u8 {
        self.state.ps &= !(CARRY | NEGATIVE | ZERO);
        if value & 0x01 != 0 {
            self.set_flag(CARRY);
        }
        let result = value >> 1;
        self.set_zn(result);
        result
    }

    fn rol(&mut self, value: u8) -> u8 {
        let carry_in = self.flag(CARRY);
        self.state.ps &= !(CARRY | NEGATIVE | ZERO);
        if value & 0x80 != 0 {
            self.set_flag(CARRY);
        }
        let result = (value << 1) | u8::from(carry_in);
        self.set_zn(result);
        result
    }

    fn ror(&mut self, value: u8) -> u8 {
        let carry_in = self.flag(CARRY);
        self.state.ps &= !(CARRY | NEGATIVE | ZERO);
        if value & 0x01 != 0 {
            self.set_flag(CARRY);
        }
        let result = (value >> 1) | if carry_in { 0x80 } else { 0 };
        self.set_zn(result);
        result
    }

    /// Read-modify-write memory operation: read, dummy write, compute, final
    /// write (3 cycles after operand resolution).
    fn rmw(&mut self, bus: &mut Bus, addr: u16, kind: Op) {
        let value = self.read(bus, addr, READ);
        self.write(bus, addr, value, DUMMY_WRITE);
        let result = match kind {
            Op::AslMem => self.asl(value),
            Op::LsrMem => self.lsr(value),
            Op::RolMem => self.rol(value),
            Op::RorMem => self.ror(value),
            _ => unreachable!(),
        };
        self.write(bus, addr, result, WRITE);
    }

    fn branch(&mut self, bus: &mut Bus, operand: u16, condition: bool) {
        if !condition {
            return;
        }

        // "A taken non-page-crossing branch ignores IRQ/NMI during its last
        // clock, so that next instruction executes before the IRQ" (Mesen
        // CPU.h BranchRelative; fixes branch_delays_irq).
        if self.run_irq && !self.prev_run_irq {
            self.run_irq = false;
        }

        let offset = (operand as u8) as i8;
        self.dummy_read_pc(bus);

        let pc = self.state.pc;
        let crossed = ((pc as i32 + offset as i32) & 0xFF00) != (pc as i32 & 0xFF00);
        if crossed {
            self.dummy_read_pc(bus);
        }
        self.state.pc = pc.wrapping_add(offset as u16);
    }

    // ---------------------------------------------------------------------
    // Interrupt sequences
    // ---------------------------------------------------------------------

    fn interrupt(&mut self, bus: &mut Bus) {
        // Two discarded opcode fetches (PC is not incremented).
        self.dummy_read_pc(bus);
        self.dummy_read_pc(bus);

        self.push_word(bus, self.state.pc);

        if self.need_nmi {
            self.need_nmi = false;
            // The interrupt sequence pushes the status byte with B clear and
            // bit 5 set.
            self.push(bus, self.state.ps | RESERVED);
            self.set_flag(INTERRUPT);
            self.state.pc = self.read_word(bus, NMI_VECTOR);
        } else {
            self.push(bus, self.state.ps | RESERVED);
            self.set_flag(INTERRUPT);
            self.state.pc = self.read_word(bus, IRQ_VECTOR);
        }
    }

    fn brk(&mut self, bus: &mut Bus) {
        // BRK pushes PC+1 (the byte after the padding byte).
        self.push_word(bus, self.state.pc.wrapping_add(1));

        let flags = self.state.ps | BREAK | RESERVED;
        if self.need_nmi {
            // NMI hijacks BRK: the NMI vector is used, but the B flag and the
            // pushed PC+1 remain those of BRK.
            self.need_nmi = false;
            self.push(bus, flags);
            self.set_flag(INTERRUPT);
            self.state.pc = self.read_word(bus, NMI_VECTOR);
        } else {
            self.push(bus, flags);
            self.set_flag(INTERRUPT);
            self.state.pc = self.read_word(bus, IRQ_VECTOR);
        }

        // Ensure an NMI is not started immediately after BRK (nmi_and_brk
        // test requires the first handler instruction to run first).
        self.prev_need_nmi = false;
    }

    // ---------------------------------------------------------------------
    // Instruction implementations
    // ---------------------------------------------------------------------

    fn execute_op(&mut self, bus: &mut Bus, op: Op, mode: AddrMode, operand: u16) {
        match op {
            Op::Brk => self.brk(bus),
            Op::Ora => {
                let value = self.operand_value(bus, mode, operand);
                self.set_a(self.state.a | value);
            }
            Op::And => {
                let value = self.operand_value(bus, mode, operand);
                self.set_a(self.state.a & value);
            }
            Op::Eor => {
                let value = self.operand_value(bus, mode, operand);
                self.set_a(self.state.a ^ value);
            }
            Op::Adc => {
                let value = self.operand_value(bus, mode, operand);
                self.add(value);
            }
            Op::Sbc => {
                let value = self.operand_value(bus, mode, operand);
                self.add(value ^ 0xFF);
            }
            Op::Cpa => {
                let value = self.operand_value(bus, mode, operand);
                let a = self.state.a;
                self.compare(a, value);
            }
            Op::Cpx => {
                let value = self.operand_value(bus, mode, operand);
                let x = self.state.x;
                self.compare(x, value);
            }
            Op::Cpy => {
                let value = self.operand_value(bus, mode, operand);
                let y = self.state.y;
                self.compare(y, value);
            }
            Op::Bit => {
                let value = self.operand_value(bus, mode, operand);
                let a = self.state.a;
                self.state.ps &= !(ZERO | OVERFLOW | NEGATIVE);
                if a & value == 0 {
                    self.set_flag(ZERO);
                }
                if value & 0x40 != 0 {
                    self.set_flag(OVERFLOW);
                }
                if value & 0x80 != 0 {
                    self.set_flag(NEGATIVE);
                }
            }
            Op::Lda => {
                let value = self.operand_value(bus, mode, operand);
                self.set_a(value);
            }
            Op::Ldx => {
                let value = self.operand_value(bus, mode, operand);
                self.set_x(value);
            }
            Op::Ldy => {
                let value = self.operand_value(bus, mode, operand);
                self.set_y(value);
            }
            Op::Sta => self.write(bus, operand, self.state.a, WRITE),
            Op::Stx => self.write(bus, operand, self.state.x, WRITE),
            Op::Sty => self.write(bus, operand, self.state.y, WRITE),
            Op::Tax => {
                let value = self.state.a;
                self.set_x(value);
            }
            Op::Tay => {
                let value = self.state.a;
                self.set_y(value);
            }
            Op::Txa => {
                let value = self.state.x;
                self.set_a(value);
            }
            Op::Tya => {
                let value = self.state.y;
                self.set_a(value);
            }
            Op::Tsx => {
                let value = self.state.sp;
                self.set_x(value);
            }
            Op::Txs => self.state.sp = self.state.x,
            Op::Inx => {
                let value = self.state.x.wrapping_add(1);
                self.set_x(value);
            }
            Op::Iny => {
                let value = self.state.y.wrapping_add(1);
                self.set_y(value);
            }
            Op::Dex => {
                let value = self.state.x.wrapping_sub(1);
                self.set_x(value);
            }
            Op::Dey => {
                let value = self.state.y.wrapping_sub(1);
                self.set_y(value);
            }
            Op::Pha => self.push(bus, self.state.a),
            Op::Php => self.push(bus, self.state.ps | BREAK | RESERVED),
            Op::Pla => {
                self.dummy_read_pc(bus);
                let value = self.pop(bus);
                self.set_a(value);
            }
            Op::Plp => {
                self.dummy_read_pc(bus);
                let value = self.pop(bus);
                self.set_ps_from_stack(value);
            }
            Op::AslAcc => {
                let value = self.state.a;
                let result = self.asl(value);
                self.set_a(result);
            }
            Op::LsrAcc => {
                let value = self.state.a;
                let result = self.lsr(value);
                self.set_a(result);
            }
            Op::RolAcc => {
                let value = self.state.a;
                let result = self.rol(value);
                self.set_a(result);
            }
            Op::RorAcc => {
                let value = self.state.a;
                let result = self.ror(value);
                self.set_a(result);
            }
            Op::AslMem | Op::LsrMem | Op::RolMem | Op::RorMem => {
                self.rmw(bus, operand, op);
            }
            Op::Inc => {
                self.state.ps &= !(ZERO | NEGATIVE);
                let value = self.read(bus, operand, READ);
                self.write(bus, operand, value, DUMMY_WRITE);
                let value = value.wrapping_add(1);
                self.set_zn(value);
                self.write(bus, operand, value, WRITE);
            }
            Op::Dec => {
                self.state.ps &= !(ZERO | NEGATIVE);
                let value = self.read(bus, operand, READ);
                self.write(bus, operand, value, DUMMY_WRITE);
                let value = value.wrapping_sub(1);
                self.set_zn(value);
                self.write(bus, operand, value, WRITE);
            }
            Op::JmpAbs => self.state.pc = operand,
            Op::JmpInd => {
                let target = self.read_indirect(bus, operand);
                self.state.pc = target;
            }
            Op::Jsr => {
                self.dummy_read_pc(bus);
                let return_addr = self.state.pc.wrapping_sub(1);
                self.push_word(bus, return_addr);
                self.state.pc = operand;
            }
            Op::Rts => {
                let addr = self.pop_word(bus);
                self.dummy_read_pc(bus);
                self.dummy_read_pc(bus);
                self.state.pc = addr.wrapping_add(1);
            }
            Op::Rti => {
                self.dummy_read_pc(bus);
                let ps = self.pop(bus);
                self.set_ps_from_stack(ps);
                self.state.pc = self.pop_word(bus);
            }
            Op::Bpl => {
                let cond = !self.flag(NEGATIVE);
                self.branch(bus, operand, cond);
            }
            Op::Bmi => {
                let cond = self.flag(NEGATIVE);
                self.branch(bus, operand, cond);
            }
            Op::Bvc => {
                let cond = !self.flag(OVERFLOW);
                self.branch(bus, operand, cond);
            }
            Op::Bvs => {
                let cond = self.flag(OVERFLOW);
                self.branch(bus, operand, cond);
            }
            Op::Bcc => {
                let cond = !self.flag(CARRY);
                self.branch(bus, operand, cond);
            }
            Op::Bcs => {
                let cond = self.flag(CARRY);
                self.branch(bus, operand, cond);
            }
            Op::Bne => {
                let cond = !self.flag(ZERO);
                self.branch(bus, operand, cond);
            }
            Op::Beq => {
                let cond = self.flag(ZERO);
                self.branch(bus, operand, cond);
            }
            Op::Clc => self.clear_flag(CARRY),
            Op::Cld => self.clear_flag(DECIMAL),
            Op::Cli => self.clear_flag(INTERRUPT),
            Op::Clv => self.clear_flag(OVERFLOW),
            Op::Sec => self.set_flag(CARRY),
            Op::Sed => self.set_flag(DECIMAL),
            Op::Sei => self.set_flag(INTERRUPT),
            Op::Nop | Op::Unk | Op::Hlt => {
                // Mesen's NOP/UNK/HLT burn the operand read (if any) so the
                // instruction takes its documented number of cycles. Opcode
                // $02/$12/... ("JAM") are treated as 1-cycle no-ops, matching
                // Mesen's HLT.
                let _ = self.operand_value(bus, mode, operand);
            }

            // ----- Undocumented / illegal opcodes -----
            Op::Slo => {
                // ASL + ORA
                let value = self.operand_value(bus, mode, operand);
                self.write(bus, operand, value, DUMMY_WRITE);
                let shifted = self.asl(value);
                self.set_a(self.state.a | shifted);
                self.write(bus, operand, shifted, WRITE);
            }
            Op::Sre => {
                // LSR + EOR
                let value = self.operand_value(bus, mode, operand);
                self.write(bus, operand, value, DUMMY_WRITE);
                let shifted = self.lsr(value);
                self.set_a(self.state.a ^ shifted);
                self.write(bus, operand, shifted, WRITE);
            }
            Op::Rla => {
                // ROL + AND
                let value = self.operand_value(bus, mode, operand);
                self.write(bus, operand, value, DUMMY_WRITE);
                let shifted = self.rol(value);
                self.set_a(self.state.a & shifted);
                self.write(bus, operand, shifted, WRITE);
            }
            Op::Rra => {
                // ROR + ADC
                let value = self.operand_value(bus, mode, operand);
                self.write(bus, operand, value, DUMMY_WRITE);
                let shifted = self.ror(value);
                self.add(shifted);
                self.write(bus, operand, shifted, WRITE);
            }
            Op::Sax => {
                let value = self.state.a & self.state.x;
                self.write(bus, operand, value, WRITE);
            }
            Op::Lax => {
                let value = self.operand_value(bus, mode, operand);
                self.set_x(value);
                self.set_a(value);
            }
            Op::Dcp => {
                let value = self.operand_value(bus, mode, operand);
                self.write(bus, operand, value, DUMMY_WRITE);
                let value = value.wrapping_sub(1);
                let a = self.state.a;
                self.compare(a, value);
                self.write(bus, operand, value, WRITE);
            }
            Op::Isb => {
                let value = self.operand_value(bus, mode, operand);
                self.write(bus, operand, value, DUMMY_WRITE);
                let value = value.wrapping_add(1);
                self.add(value ^ 0xFF);
                self.write(bus, operand, value, WRITE);
            }
            Op::Aac => {
                let value = self.operand_value(bus, mode, operand);
                self.set_a(self.state.a & value);
                self.clear_flag(CARRY);
                if self.flag(NEGATIVE) {
                    self.set_flag(CARRY);
                }
            }
            Op::Asr => {
                let value = self.operand_value(bus, mode, operand);
                self.clear_flag(CARRY);
                self.set_a(self.state.a & value);
                if self.state.a & 0x01 != 0 {
                    self.set_flag(CARRY);
                }
                let value = self.state.a >> 1;
                self.set_a(value);
            }
            Op::Arr => {
                let value = self.operand_value(bus, mode, operand);
                let carry_in = self.flag(CARRY);
                let a = ((self.state.a & value) >> 1) | if carry_in { 0x80 } else { 0 };
                self.state.a = a;
                self.set_zn(a);

                self.clear_flag(CARRY | OVERFLOW);
                let new_carry = a & 0x40 != 0;
                if new_carry {
                    self.set_flag(CARRY);
                }
                if u8::from(new_carry) ^ ((a >> 5) & 0x01) != 0 {
                    self.set_flag(OVERFLOW);
                }
            }
            Op::Atx => {
                // LDA + TAX
                let value = self.operand_value(bus, mode, operand);
                self.set_a(value);
                self.set_x(value);
            }
            Op::Axs => {
                let op_value = self.operand_value(bus, mode, operand);
                let ax = self.state.a & self.state.x;
                let value = ax.wrapping_sub(op_value);
                self.clear_flag(CARRY);
                if ax >= op_value {
                    self.set_flag(CARRY);
                }
                self.set_x(value);
            }
            Op::Sya => {
                let addr_high = (operand >> 8) as u8;
                let addr_low = (operand & 0xFF) as u8;
                let mask = addr_high.wrapping_add(1);
                let value = self.state.y & mask;
                let addr = ((self.state.y & mask) as u16) << 8 | addr_low as u16;
                self.write(bus, addr, value, WRITE);
            }
            Op::Sxa => {
                let addr_high = (operand >> 8) as u8;
                let addr_low = (operand & 0xFF) as u8;
                let mask = addr_high.wrapping_add(1);
                let value = self.state.x & mask;
                let addr = ((self.state.x & mask) as u16) << 8 | addr_low as u16;
                self.write(bus, addr, value, WRITE);
            }
            Op::Axa => {
                let value = ((operand >> 8) as u8).wrapping_add(1) & self.state.a & self.state.x;
                self.write(bus, operand, value, WRITE);
            }
            Op::Tas => {
                let mask = ((operand >> 8) as u8).wrapping_add(1);
                let value = self.state.x & self.state.a;
                self.state.sp = value;
                self.write(bus, operand, value & mask, WRITE);
            }
            Op::Las => {
                let value = self.operand_value(bus, mode, operand);
                let value = value & self.state.sp;
                self.set_a(value);
                self.set_x(value);
                self.state.sp = value;
            }
        }
    }

    // ---------------------------------------------------------------------
    // Public register accessors (used by the debugger/frontend)
    // ---------------------------------------------------------------------

    pub fn pc(&self) -> u16 {
        self.state.pc
    }
    pub fn a(&self) -> u8 {
        self.state.a
    }
    pub fn x(&self) -> u8 {
        self.state.x
    }
    pub fn y(&self) -> u8 {
        self.state.y
    }
    pub fn sp(&self) -> u8 {
        self.state.sp
    }
    pub fn ps(&self) -> u8 {
        self.state.ps
    }

    pub fn save_state(&self, w: &mut StateWriter) {
        w.u16(self.state.pc);
        w.u8(self.state.sp);
        w.u8(self.state.a);
        w.u8(self.state.x);
        w.u8(self.state.y);
        w.u8(self.state.ps);
        w.u32(self.state.irq_flag);
        w.u64(self.state.cycle_count);
        w.u64(self.total_cycles);
        w.u32(self.cycles);
        w.bool(self.run_irq);
        w.bool(self.prev_run_irq);
        w.bool(self.need_nmi);
        w.bool(self.prev_need_nmi);
    }

    pub fn load_state(&mut self, r: &mut StateReader<'_>) -> StateResult<()> {
        self.state.pc = r.u16()?;
        self.state.sp = r.u8()?;
        self.state.a = r.u8()?;
        self.state.x = r.u8()?;
        self.state.y = r.u8()?;
        self.state.ps = r.u8()?;
        self.state.irq_flag = r.u32()?;
        self.state.cycle_count = r.u64()?;
        self.total_cycles = r.u64()?;
        self.cycles = r.u32()?;
        self.run_irq = r.bool()?;
        self.prev_run_irq = r.bool()?;
        self.need_nmi = r.bool()?;
        self.prev_need_nmi = r.bool()?;
        Ok(())
    }
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

// -------------------------------------------------------------------------
// Opcode decoding tables (verbatim from Mesen's CPU constructor)
// -------------------------------------------------------------------------

use AddrMode::{
    Abs, AbsX, AbsXW, AbsY, AbsYW, Acc, Imm, Imp, Ind, IndX, IndY, IndYW, Rel, Zero, ZeroX, ZeroY,
};

const NONE: AddrMode = AddrMode::None;

const OP_TABLE: [Op; 256] = [
    // 0x00
    Op::Brk, Op::Ora, Op::Hlt, Op::Slo, Op::Nop, Op::Ora, Op::AslMem, Op::Slo,
    Op::Php, Op::Ora, Op::AslAcc, Op::Aac, Op::Nop, Op::Ora, Op::AslMem, Op::Slo,
    // 0x10
    Op::Bpl, Op::Ora, Op::Hlt, Op::Slo, Op::Nop, Op::Ora, Op::AslMem, Op::Slo,
    Op::Clc, Op::Ora, Op::Nop, Op::Slo, Op::Nop, Op::Ora, Op::AslMem, Op::Slo,
    // 0x20
    Op::Jsr, Op::And, Op::Hlt, Op::Rla, Op::Bit, Op::And, Op::RolMem, Op::Rla,
    Op::Plp, Op::And, Op::RolAcc, Op::Aac, Op::Bit, Op::And, Op::RolMem, Op::Rla,
    // 0x30
    Op::Bmi, Op::And, Op::Hlt, Op::Rla, Op::Nop, Op::And, Op::RolMem, Op::Rla,
    Op::Sec, Op::And, Op::Nop, Op::Rla, Op::Nop, Op::And, Op::RolMem, Op::Rla,
    // 0x40
    Op::Rti, Op::Eor, Op::Hlt, Op::Sre, Op::Nop, Op::Eor, Op::LsrMem, Op::Sre,
    Op::Pha, Op::Eor, Op::LsrAcc, Op::Asr, Op::JmpAbs, Op::Eor, Op::LsrMem, Op::Sre,
    // 0x50
    Op::Bvc, Op::Eor, Op::Hlt, Op::Sre, Op::Nop, Op::Eor, Op::LsrMem, Op::Sre,
    Op::Cli, Op::Eor, Op::Nop, Op::Sre, Op::Nop, Op::Eor, Op::LsrMem, Op::Sre,
    // 0x60
    Op::Rts, Op::Adc, Op::Hlt, Op::Rra, Op::Nop, Op::Adc, Op::RorMem, Op::Rra,
    Op::Pla, Op::Adc, Op::RorAcc, Op::Arr, Op::JmpInd, Op::Adc, Op::RorMem, Op::Rra,
    // 0x70
    Op::Bvs, Op::Adc, Op::Hlt, Op::Rra, Op::Nop, Op::Adc, Op::RorMem, Op::Rra,
    Op::Sei, Op::Adc, Op::Nop, Op::Rra, Op::Nop, Op::Adc, Op::RorMem, Op::Rra,
    // 0x80
    Op::Nop, Op::Sta, Op::Nop, Op::Sax, Op::Sty, Op::Sta, Op::Stx, Op::Sax,
    Op::Dey, Op::Nop, Op::Txa, Op::Unk, Op::Sty, Op::Sta, Op::Stx, Op::Sax,
    // 0x90
    Op::Bcc, Op::Sta, Op::Hlt, Op::Axa, Op::Sty, Op::Sta, Op::Stx, Op::Sax,
    Op::Tya, Op::Sta, Op::Txs, Op::Tas, Op::Sya, Op::Sta, Op::Sxa, Op::Axa,
    // 0xA0
    Op::Ldy, Op::Lda, Op::Ldx, Op::Lax, Op::Ldy, Op::Lda, Op::Ldx, Op::Lax,
    Op::Tay, Op::Lda, Op::Tax, Op::Atx, Op::Ldy, Op::Lda, Op::Ldx, Op::Lax,
    // 0xB0
    Op::Bcs, Op::Lda, Op::Hlt, Op::Lax, Op::Ldy, Op::Lda, Op::Ldx, Op::Lax,
    Op::Clv, Op::Lda, Op::Tsx, Op::Las, Op::Ldy, Op::Lda, Op::Ldx, Op::Lax,
    // 0xC0
    Op::Cpy, Op::Cpa, Op::Nop, Op::Dcp, Op::Cpy, Op::Cpa, Op::Dec, Op::Dcp,
    Op::Iny, Op::Cpa, Op::Dex, Op::Axs, Op::Cpy, Op::Cpa, Op::Dec, Op::Dcp,
    // 0xD0
    Op::Bne, Op::Cpa, Op::Hlt, Op::Dcp, Op::Nop, Op::Cpa, Op::Dec, Op::Dcp,
    Op::Cld, Op::Cpa, Op::Nop, Op::Dcp, Op::Nop, Op::Cpa, Op::Dec, Op::Dcp,
    // 0xE0
    Op::Cpx, Op::Sbc, Op::Nop, Op::Isb, Op::Cpx, Op::Sbc, Op::Inc, Op::Isb,
    Op::Inx, Op::Sbc, Op::Nop, Op::Sbc, Op::Cpx, Op::Sbc, Op::Inc, Op::Isb,
    // 0xF0
    Op::Beq, Op::Sbc, Op::Hlt, Op::Isb, Op::Nop, Op::Sbc, Op::Inc, Op::Isb,
    Op::Sed, Op::Sbc, Op::Nop, Op::Isb, Op::Nop, Op::Sbc, Op::Inc, Op::Isb,
];

const ADDR_MODES: [AddrMode; 256] = [
    // 0x00
    Imp, IndX, NONE, IndX, Zero, Zero, Zero, Zero, Imp, Imm, Acc, Imm, Abs, Abs, Abs, Abs,
    // 0x10
    Rel, IndY, NONE, IndYW, ZeroX, ZeroX, ZeroX, ZeroX, Imp, AbsY, Imp, AbsYW, AbsX, AbsX,
    AbsXW, AbsXW,
    // 0x20
    Abs, IndX, NONE, IndX, Zero, Zero, Zero, Zero, Imp, Imm, Acc, Imm, Abs, Abs, Abs, Abs,
    // 0x30
    Rel, IndY, NONE, IndYW, ZeroX, ZeroX, ZeroX, ZeroX, Imp, AbsY, Imp, AbsYW, AbsX, AbsX,
    AbsXW, AbsXW,
    // 0x40
    Imp, IndX, NONE, IndX, Zero, Zero, Zero, Zero, Imp, Imm, Acc, Imm, Abs, Abs, Abs, Abs,
    // 0x50
    Rel, IndY, NONE, IndYW, ZeroX, ZeroX, ZeroX, ZeroX, Imp, AbsY, Imp, AbsYW, AbsX, AbsX,
    AbsXW, AbsXW,
    // 0x60
    Imp, IndX, NONE, IndX, Zero, Zero, Zero, Zero, Imp, Imm, Acc, Imm, Ind, Abs, Abs, Abs,
    // 0x70
    Rel, IndY, NONE, IndYW, ZeroX, ZeroX, ZeroX, ZeroX, Imp, AbsY, Imp, AbsYW, AbsX, AbsX,
    AbsXW, AbsXW,
    // 0x80
    Imm, IndX, Imm, IndX, Zero, Zero, Zero, Zero, Imp, Imm, Imp, Imm, Abs, Abs, Abs, Abs,
    // 0x90
    Rel, IndYW, NONE, IndYW, ZeroX, ZeroX, ZeroY, ZeroY, Imp, AbsYW, Imp, AbsYW, AbsXW,
    AbsXW, AbsYW, AbsYW,
    // 0xA0
    Imm, IndX, Imm, IndX, Zero, Zero, Zero, Zero, Imp, Imm, Imp, Imm, Abs, Abs, Abs, Abs,
    // 0xB0
    Rel, IndY, NONE, IndY, ZeroX, ZeroX, ZeroY, ZeroY, Imp, AbsY, Imp, AbsY, AbsX, AbsX,
    AbsY, AbsY,
    // 0xC0
    Imm, IndX, Imm, IndX, Zero, Zero, Zero, Zero, Imp, Imm, Imp, Imm, Abs, Abs, Abs, Abs,
    // 0xD0
    Rel, IndY, NONE, IndYW, ZeroX, ZeroX, ZeroX, ZeroX, Imp, AbsY, Imp, AbsYW, AbsX, AbsX,
    AbsXW, AbsXW,
    // 0xE0
    Imm, IndX, Imm, IndX, Zero, Zero, Zero, Zero, Imp, Imm, Imp, Imm, Abs, Abs, Abs, Abs,
    // 0xF0
    Rel, IndY, NONE, IndYW, ZeroX, ZeroX, ZeroX, ZeroX, Imp, AbsY, Imp, AbsYW, AbsX, AbsX,
    AbsXW, AbsXW,
];

// -------------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::Cartridge;
    use crate::state::{StateReader, StateWriter};
    use crate::types::{Mirroring, Region, RomData, RomInfo};

    const PRG_SIZE: usize = 0x8000;

    fn nrom_bus(mut prg: Vec<u8>) -> Bus {
        assert_eq!(prg.len(), PRG_SIZE);
        // Default vectors: NMI=$9000, RESET=$8000, IRQ=$A000.
        prg[0x7FFA] = 0x00;
        prg[0x7FFB] = 0x90;
        prg[0x7FFC] = 0x00;
        prg[0x7FFD] = 0x80;
        prg[0x7FFE] = 0x00;
        prg[0x7FFF] = 0xA0;

        let mut info = RomInfo::default();
        info.mapper = 0;
        info.region = Region::Ntsc;
        info.mirroring = Mirroring::Horizontal;
        info.prg_rom_size = prg.len();
        info.chr_rom_size = 0x2000;
        let data = RomData::new(info, prg, vec![0u8; 0x2000]);
        let cart = Cartridge::from_rom_data(data).expect("failed to build synthetic NROM");
        Bus::new(cart, Region::Ntsc, 44_100)
    }

    fn blank_prg() -> Vec<u8> {
        vec![0xFF; PRG_SIZE]
    }

    fn put(prg: &mut [u8], addr: u16, bytes: &[u8]) {
        let start = (addr as usize) - 0x8000;
        prg[start..start + bytes.len()].copy_from_slice(bytes);
    }

    /// Fresh CPU after reset, without clocking the PPU/APU.
    fn test_cpu(bus: &mut Bus) -> Cpu {
        let mut cpu = Cpu::new();
        cpu.reset_internal(bus);
        cpu
    }

    fn run(cpu: &mut Cpu, bus: &mut Bus) -> u32 {
        cpu.execute_step(bus)
    }

    fn ram_write(bus: &mut Bus, addr: u16, value: u8) {
        bus.write(addr, value, MemoryAccessType::Write);
    }

    fn ram_read(bus: &mut Bus, addr: u16) -> u8 {
        bus.read(addr, MemoryAccessType::Read)
    }

    #[test]
    fn reset_state_and_vectors() {
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0xEA]);
        let mut bus = nrom_bus(prg);
        let mut cpu = Cpu::new();

        let cycles = cpu.reset_internal(&mut bus);
        assert_eq!(cycles, 7);
        assert_eq!(cpu.pc(), 0x8000);
        assert_eq!(cpu.sp(), 0xFD);
        assert_eq!(cpu.ps(), 0x24);
        assert_eq!(cpu.a(), 0);
        assert_eq!(cpu.total_cycles, 7);

        // A soft reset preserves A/X/Y, decrements SP by 3 and sets I.
        cpu.state.a = 0x12;
        cpu.state.x = 0x34;
        cpu.state.y = 0x56;
        cpu.state.sp = 0xF0;
        cpu.reset_internal(&mut bus);
        assert_eq!(cpu.a(), 0x12);
        assert_eq!(cpu.x(), 0x34);
        assert_eq!(cpu.y(), 0x56);
        assert_eq!(cpu.sp(), 0xED);
        assert_eq!(cpu.ps() & INTERRUPT, INTERRUPT);
    }

    #[test]
    fn arith_flags_and_decimal_ignored() {
        let mut prg = blank_prg();
        // LDA #$7F ; ADC #$01
        put(&mut prg, 0x8000, &[0xA9, 0x7F, 0x69, 0x01]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);

        assert_eq!(run(&mut cpu, &mut bus), 2);
        assert_eq!(cpu.a(), 0x7F);

        assert_eq!(run(&mut cpu, &mut bus), 2);
        assert_eq!(cpu.a(), 0x80);
        assert_eq!(cpu.ps() & CARRY, 0);
        assert_eq!(cpu.ps() & OVERFLOW, OVERFLOW);
        assert_eq!(cpu.ps() & NEGATIVE, NEGATIVE);
        assert_eq!(cpu.ps() & ZERO, 0);

        // Decimal flag must not affect ADC on the NES.
        let mut prg = blank_prg();
        // SED ; LDA #$09 ; CLC ; ADC #$01
        put(&mut prg, 0x8000, &[0xF8, 0xA9, 0x09, 0x18, 0x69, 0x01]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        run(&mut cpu, &mut bus); // SED
        run(&mut cpu, &mut bus); // LDA
        run(&mut cpu, &mut bus); // CLC
        run(&mut cpu, &mut bus); // ADC
        assert_eq!(cpu.a(), 0x0A);
        assert_eq!(cpu.ps() & DECIMAL, DECIMAL);

        // SBC: 0 - 1 with carry set => 0xFF, C clear, N set.
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0xA9, 0x00, 0x38, 0xE9, 0x01]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        for _ in 0..3 {
            run(&mut cpu, &mut bus);
        }
        assert_eq!(cpu.a(), 0xFF);
        assert_eq!(cpu.ps() & CARRY, 0);
        assert_eq!(cpu.ps() & NEGATIVE, NEGATIVE);
    }

    #[test]
    fn compare_and_bit_flags() {
        let mut prg = blank_prg();
        // LDA #$40 ; CMP #$40 ; BIT $10
        put(&mut prg, 0x8000, &[0xA9, 0x40, 0xC9, 0x40, 0x24, 0x10]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        run(&mut cpu, &mut bus); // LDA
        run(&mut cpu, &mut bus); // CMP
        assert_eq!(cpu.ps() & CARRY, CARRY);
        assert_eq!(cpu.ps() & ZERO, ZERO);

        ram_write(&mut bus, 0x0010, 0xC0);
        run(&mut cpu, &mut bus); // BIT
        assert_eq!(cpu.ps() & ZERO, 0);
        assert_eq!(cpu.ps() & OVERFLOW, OVERFLOW);
        assert_eq!(cpu.ps() & NEGATIVE, NEGATIVE);
    }

    #[test]
    fn branch_timing_and_page_cross() {
        // BEQ +$10 at $80FD: taken, page cross -> 4 cycles, target $810F.
        let mut prg = blank_prg();
        put(&mut prg, 0x80FD, &[0xF0, 0x10]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        cpu.state.pc = 0x80FD;
        cpu.set_flag(ZERO);
        assert_eq!(run(&mut cpu, &mut bus), 4);
        assert_eq!(cpu.pc(), 0x810F);

        // Not taken: 2 cycles, falls through to $80FF.
        cpu.state.pc = 0x80FD;
        cpu.clear_flag(ZERO);
        assert_eq!(run(&mut cpu, &mut bus), 2);
        assert_eq!(cpu.pc(), 0x80FF);

        // Backwards branch without crossing: 3 cycles.
        let mut prg = blank_prg();
        put(&mut prg, 0x8003, &[0xD0, 0xFD]); // BNE -3 from $8005 -> $8002
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        cpu.state.pc = 0x8003;
        cpu.clear_flag(ZERO);
        assert_eq!(run(&mut cpu, &mut bus), 3);
        assert_eq!(cpu.pc(), 0x8002);
    }

    #[test]
    fn addressing_modes_and_page_cross_penalties() {
        // LDA $00FF,X with X=1 -> $0100, page cross, 5 cycles.
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0xBD, 0xFF, 0x00]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        ram_write(&mut bus, 0x0100, 0x42);
        cpu.state.x = 1;
        assert_eq!(run(&mut cpu, &mut bus), 5);
        assert_eq!(cpu.a(), 0x42);

        // LDA $FFFF,X with X=1 wraps to $0000, still a page cross.
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0xBD, 0xFF, 0xFF]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        ram_write(&mut bus, 0x0000, 0x11);
        cpu.state.x = 1;
        assert_eq!(run(&mut cpu, &mut bus), 5);
        assert_eq!(cpu.a(), 0x11);

        // LDA ($FF),Y: zero-page pointer wraps to $0000; no page cross -> 5.
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0xB1, 0xFF]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        ram_write(&mut bus, 0x00FF, 0x00);
        ram_write(&mut bus, 0x0000, 0x02);
        ram_write(&mut bus, 0x0201, 0x77);
        cpu.state.y = 1;
        assert_eq!(run(&mut cpu, &mut bus), 5);
        assert_eq!(cpu.a(), 0x77);

        // Same with a page crossing base -> 6 cycles.
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0xB1, 0xFF]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        ram_write(&mut bus, 0x00FF, 0xFF);
        ram_write(&mut bus, 0x0000, 0x02);
        ram_write(&mut bus, 0x0300, 0x99);
        cpu.state.y = 1;
        assert_eq!(run(&mut cpu, &mut bus), 6);
        assert_eq!(cpu.a(), 0x99);

        // LDA ($FF,X) with X=1 wraps the pointer inside zero page.
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0xA1, 0xFF]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        ram_write(&mut bus, 0x0000, 0x80);
        ram_write(&mut bus, 0x0001, 0x00);
        ram_write(&mut bus, 0x0080, 0x5A);
        cpu.state.x = 1;
        assert_eq!(run(&mut cpu, &mut bus), 6);
        assert_eq!(cpu.a(), 0x5A);

        // STX $FF,Y writes to $0000 through zero-page wrapping.
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0x96, 0xFF]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        cpu.state.x = 0xAB;
        cpu.state.y = 1;
        assert_eq!(run(&mut cpu, &mut bus), 4);
        assert_eq!(ram_read(&mut bus, 0x0000), 0xAB);
    }

    #[test]
    fn jmp_indirect_page_bug() {
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0x6C, 0xFF, 0x02]); // JMP ($02FF)
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        ram_write(&mut bus, 0x02FF, 0x34);
        ram_write(&mut bus, 0x0200, 0x12); // high byte from same page
        ram_write(&mut bus, 0x0300, 0x99);
        assert_eq!(run(&mut cpu, &mut bus), 5);
        assert_eq!(cpu.pc(), 0x1234);
    }

    #[test]
    fn stack_ops_and_subroutines() {
        // JSR $9000 ; RTS at $9000
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0x20, 0x00, 0x90]);
        put(&mut prg, 0x9000, &[0x60]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);

        assert_eq!(run(&mut cpu, &mut bus), 6);
        assert_eq!(cpu.pc(), 0x9000);
        assert_eq!(cpu.sp(), 0xFB);
        assert_eq!(ram_read(&mut bus, 0x01FD), 0x80);
        assert_eq!(ram_read(&mut bus, 0x01FC), 0x02); // $8002 = return-1
        assert_eq!(run(&mut cpu, &mut bus), 6);
        assert_eq!(cpu.pc(), 0x8003);
        assert_eq!(cpu.sp(), 0xFD);

        // PHA / PLA and SP wrapping.
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0x48, 0x68]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        cpu.state.a = 0x55;
        cpu.state.sp = 0x00;
        assert_eq!(run(&mut cpu, &mut bus), 3);
        assert_eq!(cpu.sp(), 0xFF);
        assert_eq!(ram_read(&mut bus, 0x0100), 0x55);
        cpu.set_a(0);
        assert_eq!(run(&mut cpu, &mut bus), 4);
        assert_eq!(cpu.a(), 0x55);
        assert_eq!(cpu.sp(), 0x00);

        // PHP pushes B and bit 5; PLP masks B and forces bit 5.
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0x08, 0x28]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        cpu.state.ps = CARRY | RESERVED; // $21
        assert_eq!(run(&mut cpu, &mut bus), 3);
        assert_eq!(ram_read(&mut bus, 0x01FD), 0x31);
        ram_write(&mut bus, 0x01FD, 0x00);
        assert_eq!(run(&mut cpu, &mut bus), 4);
        assert_eq!(cpu.ps(), 0x20);
        assert_eq!(cpu.sp(), 0xFD);

        // TXS doesn't touch flags; TSX sets Z/N.
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0x9A, 0xBA]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        cpu.state.x = 0x80;
        cpu.state.ps = 0x24;
        run(&mut cpu, &mut bus); // TXS
        assert_eq!(cpu.sp(), 0x80);
        assert_eq!(cpu.ps(), 0x24);
        cpu.state.sp = 0x00;
        run(&mut cpu, &mut bus); // TSX
        assert_eq!(cpu.x(), 0x00);
        assert_eq!(cpu.ps() & ZERO, ZERO);
    }

    #[test]
    fn brk_rti_and_interrupt_sequences() {
        // BRK at $8000.
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0x00]);
        put(&mut prg, 0xA000, &[0x40]); // RTI at IRQ vector target
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        cpu.state.ps = RESERVED;

        assert_eq!(run(&mut cpu, &mut bus), 7);
        assert_eq!(cpu.pc(), 0xA000);
        assert_eq!(cpu.sp(), 0xFA);
        assert_eq!(ram_read(&mut bus, 0x01FD), 0x80);
        assert_eq!(ram_read(&mut bus, 0x01FC), 0x02); // PC+1
        assert_eq!(ram_read(&mut bus, 0x01FB), 0x30); // B | R | I
        assert_eq!(cpu.ps() & INTERRUPT, INTERRUPT);

        // RTI restores PC and PS (B cleared, bit 5 forced).
        ram_write(&mut bus, 0x01FB, 0x20);
        ram_write(&mut bus, 0x01FC, 0x34);
        ram_write(&mut bus, 0x01FD, 0x12);
        cpu.state.sp = 0xFA;
        assert_eq!(run(&mut cpu, &mut bus), 6);
        assert_eq!(cpu.pc(), 0x1234);
        assert_eq!(cpu.ps(), 0x20);

        // Direct NMI/IRQ entry sequences use the right vectors and push
        // PC with B clear.
        let prg = blank_prg();
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        cpu.state.pc = 0x8000;
        cpu.state.sp = 0xFD;
        cpu.state.ps = RESERVED | INTERRUPT;
        cpu.cycles = 0;
        cpu.need_nmi = true;
        cpu.interrupt(&mut bus);
        assert_eq!(cpu.cycles, 7);
        assert_eq!(cpu.pc(), 0x9000);
        assert_eq!(cpu.sp(), 0xFA);
        assert_eq!(ram_read(&mut bus, 0x01FD), 0x80);
        assert_eq!(ram_read(&mut bus, 0x01FC), 0x00);
        assert_eq!(ram_read(&mut bus, 0x01FB), 0x24);
        assert_eq!(cpu.ps() & INTERRUPT, INTERRUPT);

        // IRQ uses $FFFE -> $A000.
        cpu.state.pc = 0x8000;
        cpu.state.sp = 0xFD;
        cpu.state.ps = RESERVED;
        cpu.cycles = 0;
        cpu.need_nmi = false;
        cpu.interrupt(&mut bus);
        assert_eq!(cpu.cycles, 7);
        assert_eq!(cpu.pc(), 0xA000);
        assert_eq!(cpu.sp(), 0xFA);
    }

    #[test]
    fn illegal_opcodes() {
        // SLO $10: ASL memory then ORA with A.
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0x07, 0x10]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        ram_write(&mut bus, 0x0010, 0x81);
        cpu.state.a = 0x40;
        cpu.state.ps = RESERVED;
        assert_eq!(run(&mut cpu, &mut bus), 5);
        assert_eq!(ram_read(&mut bus, 0x0010), 0x02);
        assert_eq!(cpu.a(), 0x42);
        assert_eq!(cpu.ps() & CARRY, CARRY);

        // LAX $20: A = X = memory.
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0xA7, 0x20]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        ram_write(&mut bus, 0x0020, 0x80);
        assert_eq!(run(&mut cpu, &mut bus), 3);
        assert_eq!(cpu.a(), 0x80);
        assert_eq!(cpu.x(), 0x80);
        assert_eq!(cpu.ps() & NEGATIVE, NEGATIVE);

        // SAX $30: memory = A & X, flags untouched.
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0x87, 0x30]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        cpu.state.a = 0xF0;
        cpu.state.x = 0x0F;
        cpu.state.ps = RESERVED | CARRY;
        assert_eq!(run(&mut cpu, &mut bus), 3);
        assert_eq!(ram_read(&mut bus, 0x0030), 0x00);
        assert_eq!(cpu.ps(), RESERVED | CARRY);

        // DCP $40: decrement, then compare with A.
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0xC7, 0x40]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        ram_write(&mut bus, 0x0040, 0x42);
        cpu.state.a = 0x41;
        assert_eq!(run(&mut cpu, &mut bus), 5);
        assert_eq!(ram_read(&mut bus, 0x0040), 0x41);
        assert_eq!(cpu.ps() & CARRY, CARRY);
        assert_eq!(cpu.ps() & ZERO, ZERO);

        // ISB $50: increment, then SBC with A.
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0xE7, 0x50]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        ram_write(&mut bus, 0x0050, 0x10);
        cpu.state.a = 0x20;
        cpu.state.ps = RESERVED | CARRY;
        assert_eq!(run(&mut cpu, &mut bus), 5);
        assert_eq!(ram_read(&mut bus, 0x0050), 0x11);
        assert_eq!(cpu.a(), 0x0F);
        assert_eq!(cpu.ps() & CARRY, CARRY);

        // RLA $60: ROL memory then AND with A.
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0x27, 0x60]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        ram_write(&mut bus, 0x0060, 0x80);
        cpu.state.a = 0xFF;
        assert_eq!(run(&mut cpu, &mut bus), 5);
        assert_eq!(ram_read(&mut bus, 0x0060), 0x00);
        assert_eq!(cpu.a(), 0x00);
        assert_eq!(cpu.ps() & CARRY, CARRY);
        assert_eq!(cpu.ps() & ZERO, ZERO);

        // ANC #$0F: A &= imm, C = N.
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0x0B, 0x0F]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        cpu.state.a = 0xF0;
        assert_eq!(run(&mut cpu, &mut bus), 2);
        assert_eq!(cpu.a(), 0x00);
        assert_eq!(cpu.ps() & CARRY, 0);

        // ALR #$01: A = (A & imm) >> 1, C = old bit 0.
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0x4B, 0x01]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        cpu.state.a = 0x03;
        assert_eq!(run(&mut cpu, &mut bus), 2);
        assert_eq!(cpu.a(), 0x00);
        assert_eq!(cpu.ps() & CARRY, CARRY);

        // ARR #$FF with carry set: C = bit 6 of result, V = C ^ bit 6.
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0x6B, 0xFF]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        cpu.state.a = 0xFF;
        cpu.state.ps = RESERVED | CARRY;
        assert_eq!(run(&mut cpu, &mut bus), 2);
        assert_eq!(cpu.a(), 0xFF);
        assert_eq!(cpu.ps() & CARRY, CARRY);
        assert_eq!(cpu.ps() & OVERFLOW, 0);

        // AXS #$10: X = (A & X) - imm, C = no borrow.
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0xCB, 0x10]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        cpu.state.a = 0x50;
        cpu.state.x = 0x30;
        assert_eq!(run(&mut cpu, &mut bus), 2);
        assert_eq!(cpu.x(), 0x00);
        assert_eq!(cpu.ps() & CARRY, CARRY);
        assert_eq!(cpu.ps() & ZERO, ZERO);

        // LAS $0010: A = X = SP = mem & SP.
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0xBB, 0x10, 0x00]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        ram_write(&mut bus, 0x0010, 0xF0);
        cpu.state.sp = 0xF8;
        assert_eq!(run(&mut cpu, &mut bus), 4);
        assert_eq!(cpu.a(), 0xF0);
        assert_eq!(cpu.x(), 0xF0);
        assert_eq!(cpu.sp(), 0xF0);

        // SHY $0034,X (9C): unstable store, high byte = Y & (addr_high+1).
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0x9C, 0x34, 0x00]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        cpu.state.y = 1;
        cpu.state.x = 0;
        assert_eq!(run(&mut cpu, &mut bus), 5);
        assert_eq!(ram_read(&mut bus, 0x0134), 0x01);

        // SHX $0034,Y (9E).
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0x9E, 0x34, 0x00]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        cpu.state.x = 1;
        cpu.state.y = 0;
        assert_eq!(run(&mut cpu, &mut bus), 5);
        assert_eq!(ram_read(&mut bus, 0x0134), 0x01);

        // SHA $0034,Y (9F): value = (addr_high+1) & A & X.
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0x9F, 0x34, 0x00]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        cpu.state.a = 0xFF;
        cpu.state.x = 0x0F;
        cpu.state.y = 0;
        assert_eq!(run(&mut cpu, &mut bus), 5);
        assert_eq!(ram_read(&mut bus, 0x0034), 0x01);

        // SHS $0034,Y (9B): SP = A & X, value = SP & (addr_high+1).
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0x9B, 0x34, 0x00]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        cpu.state.a = 0xF0;
        cpu.state.x = 0x0F;
        cpu.state.y = 0;
        assert_eq!(run(&mut cpu, &mut bus), 5);
        assert_eq!(cpu.sp(), 0x00);
        assert_eq!(ram_read(&mut bus, 0x0034), 0x00);

        // SHA ($40),Y (93): 6 cycles through the IndYW path.
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0x93, 0x40]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        ram_write(&mut bus, 0x0040, 0x30);
        ram_write(&mut bus, 0x0041, 0x00);
        cpu.state.a = 0xFF;
        cpu.state.x = 0x0F;
        cpu.state.y = 0;
        assert_eq!(run(&mut cpu, &mut bus), 6);
        assert_eq!(ram_read(&mut bus, 0x0030), 0x01);
    }

    #[test]
    fn nop_and_jam_cycle_counts() {
        let cases: &[(&[u8], u32)] = &[
            (&[0xEA], 2),                   // NOP implied
            (&[0x04, 0x10], 3),             // NOP zp
            (&[0x0C, 0x00, 0x80], 4),       // NOP abs
            (&[0x1C, 0x10, 0x80], 4),       // NOP abs,X (no page cross)
            (&[0x80, 0x12], 2),             // NOP immediate
            (&[0x1A], 2),                   // NOP implied (unofficial)
            (&[0x02], 1),                   // JAM/HLT: Mesen treats as 1-cycle op
        ];
        for (code, expected) in cases {
            let mut prg = blank_prg();
            put(&mut prg, 0x8000, code);
            let mut bus = nrom_bus(prg);
            let mut cpu = test_cpu(&mut bus);
            assert_eq!(
                run(&mut cpu, &mut bus),
                *expected,
                "opcode bytes {:02X?}",
                code
            );
        }

        // NOP abs,X with a page cross adds the dummy-read cycle.
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0x1C, 0xFF, 0x00]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        cpu.state.x = 1;
        assert_eq!(run(&mut cpu, &mut bus), 5);
    }

    /// Smoke test: every one of the 256 decoded opcodes executes one step
    /// without panicking and reports a plausible cycle count. (nestest covers
    /// the semantics of 225 of them; focused tests above cover the rest.)
    #[test]
    fn every_opcode_executes() {
        for opcode in 0..=255u16 {
            let mut prg = blank_prg();
            put(&mut prg, 0x8000, &[opcode as u8, 0x00, 0x00]);
            let mut bus = nrom_bus(prg);
            let mut cpu = test_cpu(&mut bus);
            // Safe operands/pointers so indexed and indirect forms stay in RAM.
            ram_write(&mut bus, 0x0000, 0x00);
            ram_write(&mut bus, 0x0001, 0x00);
            let cycles = run(&mut cpu, &mut bus);
            assert!(
                (1..=8).contains(&cycles),
                "opcode {opcode:02X} returned implausible cycle count {cycles}"
            );
        }
    }

    #[test]
    fn rmw_cycles() {        // ASL $10 = 5; ASL $0010,X = 6 (no cross) / 7 (cross).
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0x06, 0x10, 0x16, 0x20, 0x1E, 0xFF, 0x00]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        ram_write(&mut bus, 0x0010, 0x01);
        ram_write(&mut bus, 0x0020, 0x01);
        assert_eq!(run(&mut cpu, &mut bus), 5);
        assert_eq!(ram_read(&mut bus, 0x0010), 0x02);

        cpu.state.x = 0;
        assert_eq!(run(&mut cpu, &mut bus), 6);
        assert_eq!(ram_read(&mut bus, 0x0020), 0x02);

        cpu.state.x = 1;
        ram_write(&mut bus, 0x0100, 0x04);
        assert_eq!(run(&mut cpu, &mut bus), 7);
        assert_eq!(ram_read(&mut bus, 0x0100), 0x08);
    }

    #[test]
    fn dma_stall_cycles_are_consumed_and_counted() {
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0xEA]); // NOP
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        bus.stall_cycles = 513;
        let before = cpu.total_cycles;
        assert_eq!(run(&mut cpu, &mut bus), 2 + 513);
        assert_eq!(cpu.total_cycles, before + 2 + 513);
        assert_eq!(bus.stall_cycles, 0);
    }

    #[test]
    fn step_ticks_the_bus_by_instruction_cycles() {
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0xEA]); // NOP
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        let bus_before = bus.master_cycles;
        let cpu_before = cpu.total_cycles;
        assert_eq!(cpu.step(&mut bus), 2);
        assert_eq!(bus.master_cycles, bus_before + 2);
        assert_eq!(cpu.total_cycles, cpu_before + 2);
    }

    #[test]
    fn reset_ticks_eight_bus_cycles_but_reports_seven() {
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0xEA]);
        let mut bus = nrom_bus(prg);
        let mut cpu = Cpu::new();
        cpu.reset(&mut bus);
        assert_eq!(cpu.pc(), 0x8000);
        assert_eq!(cpu.sp(), 0xFD);
        assert_eq!(cpu.ps(), 0x24);
        assert_eq!(cpu.total_cycles, 7);
        // Eight CPU cycles are clocked for PPU/APU warm-up, but the bus's
        // CPU-cycle counter (used for OAM DMA parity) matches the reported 7.
        assert_eq!(bus.master_cycles, 7);
    }

    #[test]
    fn nmi_is_taken_after_the_current_instruction() {
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0xEA]); // NOP
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        cpu.state.pc = 0x8000;
        cpu.need_nmi = true;
        cpu.prev_need_nmi = true;

        // NOP (2) executes first, then the 7-cycle NMI sequence.
        assert_eq!(run(&mut cpu, &mut bus), 9);
        assert_eq!(cpu.pc(), 0x9000);
        assert_eq!(cpu.sp(), 0xFA);
        assert_eq!(ram_read(&mut bus, 0x01FD), 0x80);
        assert_eq!(ram_read(&mut bus, 0x01FC), 0x01); // after the NOP
        assert_eq!(ram_read(&mut bus, 0x01FB), 0x24); // B clear, I set
        assert_eq!(cpu.ps() & INTERRUPT, INTERRUPT);
    }

    #[test]
    fn brk_is_hijacked_by_pending_nmi() {
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0x00]); // BRK
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        cpu.state.pc = 0x8000;
        cpu.need_nmi = true;

        assert_eq!(run(&mut cpu, &mut bus), 7);
        // NMI vector ($9000), BRK's pushed PC+1 and B flag are retained.
        assert_eq!(cpu.pc(), 0x9000);
        assert_eq!(cpu.sp(), 0xFA);
        assert_eq!(ram_read(&mut bus, 0x01FD), 0x80);
        assert_eq!(ram_read(&mut bus, 0x01FC), 0x02);
        assert_eq!(ram_read(&mut bus, 0x01FB), 0x34);
    }

    #[test]
    fn save_state_roundtrip() {
        let mut prg = blank_prg();
        put(&mut prg, 0x8000, &[0xA9, 0x12, 0xAA, 0xA8, 0xE8]);
        let mut bus = nrom_bus(prg);
        let mut cpu = test_cpu(&mut bus);
        for _ in 0..3 {
            run(&mut cpu, &mut bus);
        }

        let mut w = StateWriter::new();
        cpu.save_state(&mut w);
        let data = w.into_inner();

        let expected_state = (
            cpu.state.pc,
            cpu.state.sp,
            cpu.state.a,
            cpu.state.x,
            cpu.state.y,
            cpu.state.ps,
            cpu.state.irq_flag,
            cpu.state.cycle_count,
            cpu.total_cycles,
            cpu.cycles,
        );
        let expected_lines = (cpu.run_irq, cpu.prev_run_irq, cpu.need_nmi, cpu.prev_need_nmi);

        let mut restored = Cpu::new();
        restored.state.pc = 0x1234;
        restored.state.sp = 0;
        restored.state.a = 0;
        restored.state.x = 0;
        restored.state.y = 0;
        restored.state.ps = 0;
        restored.state.cycle_count = 0;
        restored.total_cycles = 0;
        restored.cycles = 0;
        restored.run_irq = false;
        restored.prev_run_irq = false;
        restored.need_nmi = false;
        restored.prev_need_nmi = false;
        let mut r = StateReader::new(&data);
        restored.load_state(&mut r).expect("load_state failed");

        assert_eq!(
            (
                restored.state.pc,
                restored.state.sp,
                restored.state.a,
                restored.state.x,
                restored.state.y,
                restored.state.ps,
                restored.state.irq_flag,
                restored.state.cycle_count,
                restored.total_cycles,
                restored.cycles,
            ),
            expected_state
        );
        assert_eq!(
            (
                restored.run_irq,
                restored.prev_run_irq,
                restored.need_nmi,
                restored.prev_need_nmi,
            ),
            expected_lines
        );
    }

    /// Full comparison against the canonical nestest trace (automation mode
    /// starts at $C000). Stops just before the ROM reaches the APU-register
    /// test, so this test exercises ~8,000 instructions (including all illegal
    /// opcodes) without needing PPU/APU implementations.
    #[test]
    fn nestest_matches_canonical_log() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let rom_path = root.join("test-roms").join("nestest.nes");
        let log_path = root.join("test-roms").join("nestest.log");

        let rom = std::fs::read(&rom_path)
            .unwrap_or_else(|e| panic!("cannot read {:?}: {e}", rom_path));
        let log = std::fs::read_to_string(&log_path)
            .unwrap_or_else(|e| panic!("cannot read {:?}: {e}", log_path));

        let cart = Cartridge::from_bytes(&rom).expect("failed to load nestest.nes");
        let mut bus = Bus::new(cart, Region::Ntsc, 44_100);
        let mut cpu = Cpu::new();
        cpu.reset_internal(&mut bus);

        // Automation-mode entry point; the canonical log starts here with
        // A=X=Y=0, P=$24, SP=$FD, CYC=7.
        cpu.state.pc = 0xC000;
        cpu.state.sp = 0xFD;
        cpu.state.ps = 0x24;
        cpu.state.a = 0;
        cpu.state.x = 0;
        cpu.state.y = 0;
        cpu.total_cycles = 7;
        cpu.state.cycle_count = 7;

        // The last handful of nestest instructions touch APU registers
        // ($4015/$4004-$4007). Stop just before them by default so the CPU
        // test does not depend on the APU implementation; set
        // MESEN_NESTEST_FULL=1 to compare the complete canonical log.
        let limit = if std::env::var_os("MESEN_NESTEST_FULL").is_some() {
            usize::MAX
        } else {
            8_978
        };
        let mut compared = 0usize;
        for (idx, line) in log.lines().enumerate() {
            if idx >= limit {
                break;
            }
            let line = line.trim_start_matches('\u{feff}');
            let Some(entry) = parse_nestest_line(line) else {
                continue;
            };
            let line_no = idx + 1;
            assert_eq!(cpu.pc(), entry.pc, "PC mismatch at log line {line_no}");
            assert_eq!(cpu.a(), entry.a, "A mismatch at log line {line_no}");
            assert_eq!(cpu.x(), entry.x, "X mismatch at log line {line_no}");
            assert_eq!(cpu.y(), entry.y, "Y mismatch at log line {line_no}");
            assert_eq!(cpu.ps(), entry.ps, "P mismatch at log line {line_no}");
            assert_eq!(cpu.sp(), entry.sp, "SP mismatch at log line {line_no}");
            assert_eq!(
                cpu.total_cycles, entry.cycles,
                "CYC mismatch at log line {line_no}"
            );
            compared += 1;

            if idx + 1 < limit {
                cpu.execute_step(&mut bus);
            }
        }
        assert!(compared > 8_000, "only compared {compared} instructions");
    }

    struct TraceEntry {
        pc: u16,
        a: u8,
        x: u8,
        y: u8,
        ps: u8,
        sp: u8,
        cycles: u64,
    }

    fn parse_nestest_line(line: &str) -> Option<TraceEntry> {
        let pc = u16::from_str_radix(line.get(0..4)?, 16).ok()?;
        let mut a = None;
        let mut x = None;
        let mut y = None;
        let mut ps = None;
        let mut sp = None;
        let mut cycles = None;
        for token in line.split_whitespace() {
            if let Some(v) = token.strip_prefix("A:") {
                a = u8::from_str_radix(v, 16).ok();
            } else if let Some(v) = token.strip_prefix("X:") {
                x = u8::from_str_radix(v, 16).ok();
            } else if let Some(v) = token.strip_prefix("Y:") {
                y = u8::from_str_radix(v, 16).ok();
            } else if let Some(v) = token.strip_prefix("P:") {
                ps = u8::from_str_radix(v, 16).ok();
            } else if let Some(v) = token.strip_prefix("SP:") {
                sp = u8::from_str_radix(v, 16).ok();
            } else if let Some(v) = token.strip_prefix("CYC:") {
                cycles = v.parse::<u64>().ok();
            }
        }
        Some(TraceEntry {
            pc,
            a: a?,
            x: x?,
            y: y?,
            ps: ps?,
            sp: sp?,
            cycles: cycles?,
        })
    }
}
