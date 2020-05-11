#![feature(asm, const_fn)]
#![no_std]
#![crate_name = "nrf53"]
#![crate_type = "rlib"]

pub mod chip;
pub mod clock;
mod deferred_call_tasks;
pub mod gpio;
pub mod interrupt_service;
pub mod nvmc;
pub mod peripheral_interrupts;
pub mod power;
pub mod usbreg;

pub use nrf5x::pinmux;
