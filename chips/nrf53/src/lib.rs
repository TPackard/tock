#![feature(llvm_asm, const_fn)]
#![no_std]
#![crate_name = "nrf53"]
#![crate_type = "rlib"]

pub mod app_peripheral_ids;
pub mod chip;
pub mod clock;
pub mod crt1;
mod deferred_call_tasks;
pub mod gpio;
pub mod interrupt_service;
pub mod net_peripheral_ids;
pub mod nvmc;
pub mod pinmux;
pub mod power;
pub mod rtc;
pub mod spi;
pub mod uart;
pub mod usbreg;

pub use crt1::init;

#[derive(Debug)]
pub enum Core {
    Application,
    Network,
}
