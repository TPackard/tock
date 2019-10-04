//! Implementation of the SAM4L GLOC.

use core::cell::Cell;
use kernel::common::registers::{register_bitfields, ReadWrite};
use kernel::common::StaticRef;

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

pub struct Gloc {
    luts: [GlocLut; 2],
}

pub static mut GLOC: Gloc = Gloc {
    luts: [
        GlocLut::new(0),
        GlocLut::new(1),
    ],
};

impl Gloc {
    pub fn configure_lut(&mut self, lut: usize, config: u16) {
        self.luts[lut].configure(config);
    }

    pub fn enable_lut_input(&mut self, lut: usize, input_num: u8) {
        self.luts[lut].enable_input(input_num);
    }

    pub fn disable_lut_input(&mut self, lut: usize, input_num: u8) {
        self.luts[lut].disable_input(input_num);
    }

    pub fn disable_lut(&mut self, lut: usize) {
        self.luts[lut].disable();
    }

    pub fn is_lut_enabled(&self, lut: usize) -> bool {
        self.luts[lut].is_enabled()
    }

    pub fn enable_lut_filter(&mut self, lut: usize) {
        self.luts[lut].enable_filter();
    }

    pub fn disable_lut_filter(&mut self, lut: usize) {
        self.luts[lut].disable_filter();
    }
}

pub struct GlocLut {
    registers: StaticRef<GlocRegisters>,
    enabled: Cell<bool>,
}

impl GlocLut {
    const fn new(lut_num: usize) -> GlocLut {
        GlocLut {
            registers: unsafe {
                StaticRef::new(
                    (GLOC_BASE_ADDR + lut_num * GLOC_LUT_SIZE) as *const GlocRegisters
                )
            },
            enabled: Cell::new(false),
        }
    }

    pub fn configure(&mut self, config: u16) {
        let registers: &GlocRegisters = &*self.registers;
        registers.truth.write(Truth::TRUTH.val(config as u32));
    }

    pub fn enable_input(&mut self, input_num: u8) {
        let registers: &GlocRegisters = &*self.registers;
        let aen: u32 = registers.cr.read(Control::AEN) | 1 << input_num;
        registers.cr.modify(Control::AEN.val(aen));
        self.enabled.set(true);
    }

    pub fn disable_input(&mut self, input_num: u8) {
        if self.enabled.get() {
            let registers: &GlocRegisters = &*self.registers;
            let aen: u32 = registers.cr.read(Control::AEN) & !(1u32 << input_num);
            registers.cr.modify(Control::AEN.val(aen));

            if aen == 0 {
                self.enabled.set(false);
            }
        }
    }

    pub fn disable(&mut self) {
        if self.enabled.get() {
            let registers: &GlocRegisters = &*self.registers;
            registers.cr.modify(Control::AEN.val(0));
            self.enabled.set(false);
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.get()
    }

    pub fn enable_filter(&mut self) {
        // TODO: make sure that GCLK is enabled.
        let registers: &GlocRegisters = &*self.registers;
        registers.cr.modify(Control::FILTEN::GlitchFilter);
    }

    pub fn disable_filter(&mut self) {
        let registers: &GlocRegisters = &*self.registers;
        registers.cr.modify(Control::FILTEN::NoGlitchFilter);
    }
}
