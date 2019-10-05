//! Implementation of the SAM4L GLOC.

use kernel::common::registers::{register_bitfields, ReadWrite};
use kernel::common::StaticRef;
use crate::pm::{self, Clock, PBAClock};

#[repr(C)]
pub struct GlocRegisters {
    cr: ReadWrite<u32, Control::Register>,
    truth: ReadWrite<u32, Truth::Register>,
}

register_bitfields![u32,
    Control [
        /// Filter Enable
        FILTEN OFFSET(31) NUMBITS(1) [
            NoGlitchFilter = 0,
            GlitchFilter = 1
        ],
        /// Enable IN Inputs
        AEN OFFSET(0) NUMBITS(4) []
    ],

    Truth [
        TRUTH OFFSET(0) NUMBITS(16) []
    ]
];

/// The GLOC's base addresses in memory (Section 7.1 of manual).
const GLOC_BASE_ADDR: usize = 0x40060000;

/// The number of bytes between each memory mapped GLOC LUT (Section 36.7).
const GLOC_LUT_SIZE: usize = 0x8;

pub const IN0: u8 = 0b0001;
pub const IN1: u8 = 0b0010;
pub const IN2: u8 = 0b0100;
pub const IN3: u8 = 0b1000;

pub struct Gloc {
    luts: [GlocLut; 2],
}

pub static mut GLOC: Gloc = Gloc {
    luts: [
        GlocLut::new(Lut::Lut1),
        GlocLut::new(Lut::Lut2),
    ],
};

pub enum Lut {
    Lut1 = 0,
    Lut2 = 1
}

impl Gloc {
    pub fn enable(&self) {
        pm::enable_clock(Clock::PBA(PBAClock::GLOC));
    }

    pub fn disable(&mut self) {
        self.disable_lut(Lut::Lut1);
        self.disable_lut(Lut::Lut2);
        pm::disable_clock(Clock::PBA(PBAClock::GLOC));
    }

    fn lut_registers(&self, lut: Lut) -> &GlocRegisters {
        &*self.luts[lut as usize].registers
    }

    pub fn configure_lut(&mut self, lut: Lut, config: u16) {
        let registers = self.lut_registers(lut);
        registers.truth.write(Truth::TRUTH.val(config as u32));
    }

    pub fn enable_lut_inputs(&mut self, lut: Lut, inputs: u8) {
        let registers = self.lut_registers(lut);
        let aen: u32 = registers.cr.read(Control::AEN) | (inputs as u32);
        registers.cr.modify(Control::AEN.val(aen));
    }

    pub fn disable_lut_inputs(&mut self, lut: Lut, inputs: u8) {
        let registers = self.lut_registers(lut);
        let aen: u32 = registers.cr.read(Control::AEN) & !(inputs as u32);
        registers.cr.modify(Control::AEN.val(aen));
    }

    pub fn disable_lut(&mut self, lut: Lut) {
        let registers = self.lut_registers(lut);
        registers.truth.write(Truth::TRUTH.val(0));
        registers.cr.modify(Control::AEN.val(0));
    }

    pub fn enable_lut_filter(&mut self, lut: Lut) {
        // TODO: enable GCLK.
        let registers = self.lut_registers(lut);
        registers.cr.modify(Control::FILTEN::GlitchFilter);
    }

    pub fn disable_lut_filter(&mut self, lut: Lut) {
        let registers = self.lut_registers(lut);
        registers.cr.modify(Control::FILTEN::NoGlitchFilter);
    }
}

pub struct GlocLut {
    registers: StaticRef<GlocRegisters>
}

impl GlocLut {
    const fn new(lut: Lut) -> GlocLut {
        GlocLut {
            registers: unsafe {
                StaticRef::new(
                    (GLOC_BASE_ADDR + (lut as usize) * GLOC_LUT_SIZE) as *const GlocRegisters
                )
            }
        }
    }
}
