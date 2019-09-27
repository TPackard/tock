//! Implementation of the SAM4L GLOC.

use kernel::common::registers::{register_bitfields, ReadWrite};

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
