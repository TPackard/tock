#![feature(llvm_asm, const_fn)]
#![no_std]
#![crate_name = "nrf53"]
#![crate_type = "rlib"]

pub mod chip;
pub mod clock;
pub mod crt1;
mod deferred_call_tasks;
pub mod gpio;
pub mod interrupt_service;
pub mod nvmc;
pub mod peripheral_interrupts;
pub mod pinmux;
pub mod power;
pub mod rtc;
pub mod uart;
pub mod usbreg;

pub use crt1::init;
