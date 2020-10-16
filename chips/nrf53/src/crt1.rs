use cortexm33::{generic_isr, hard_fault_handler, nvic, scb, svc_handler, systick_handler};
use tock_rt0;

/*
 * Adapted from crt1.c which was relicensed by the original author from
 * GPLv3 to Apache 2.0.
 * The original version of the file, under GPL can be found at
 * https://github.com/SoftwareDefinedBuildings/stormport/blob/rebase0/tos/platforms/storm/stormcrt1.c
 *
 * Copyright 2016, Michael Andersen <m.andersen@eecs.berkeley.edu>
 */

extern "C" {
    // Symbols defined in the linker file
    static mut _erelocate: usize;
    static mut _etext: usize;
    static mut _ezero: usize;
    static mut _srelocate: usize;
    static mut _szero: usize;
    fn reset_handler();

    // _estack is not really a function, but it makes the types work
    // You should never actually invoke it!!
    fn _estack();
}

#[cfg(not(any(target_arch = "arm", target_os = "none")))]
unsafe extern "C" fn unhandled_interrupt() {
    unimplemented!()
}

#[cfg(all(target_arch = "arm", target_os = "none"))]
unsafe extern "C" fn unhandled_interrupt() {
    let mut interrupt_number: u32;

    // IPSR[8:0] holds the currently active interrupt
    llvm_asm!(
    "mrs    r0, ipsr                    "
    : "={r0}"(interrupt_number)
    :
    : "r0"
    :
    );

    interrupt_number = interrupt_number & 0x1ff;
    panic!("Unhandled Interrupt. ISR {} is active.", interrupt_number);
}

#[cfg_attr(
    all(target_arch = "arm", target_os = "none"),
    link_section = ".vectors"
)]
// used Ensures that the symbol is kept until the final binary
#[cfg_attr(all(target_arch = "arm", target_os = "none"), used)]
/// ARM Cortex M Vector Table
pub static BASE_VECTORS: [unsafe extern "C" fn(); 16] = [
    // Stack Pointer
    _estack,
    // Reset Handler
    reset_handler,
    // NMI
    unhandled_interrupt,
    // Hard Fault
    hard_fault_handler,
    // Memory Managment Fault
    unhandled_interrupt,
    // Bus Fault
    unhandled_interrupt,
    // Usage Fault
    unhandled_interrupt,
    // Reserved
    unhandled_interrupt,
    // Reserved
    unhandled_interrupt,
    // Reserved
    unhandled_interrupt,
    // Reserved
    unhandled_interrupt,
    // SVCall
    svc_handler,
    // Reserved for Debug
    unhandled_interrupt,
    // Reserved
    unhandled_interrupt,
    // PendSv
    unhandled_interrupt,
    // SysTick
    systick_handler,
];

#[cfg_attr(
    all(target_arch = "arm", target_os = "none"),
    link_section = ".vectors"
)]
// used Ensures that the symbol is kept until the final binary
#[cfg_attr(all(target_arch = "arm", target_os = "none"), used)]
pub static IRQS: [unsafe extern "C" fn(); 80] = [generic_isr; 80];

#[no_mangle]
pub unsafe extern "C" fn init() {
    // Workaround for Errata 42
    // CLOCK: Reset value of HFCLKCTRL is invalid
    *(0x50039530 as *mut u32) = 0xBEEF0044u32;

    // Workaround for Errata 46
    // CLOCK: LFRC has higher current consuption
    *(0x5003254C as *mut u32) = 0;

    // Workaround for Errata 53
    // REGULATORS: Current consumption in normal voltage mode is higher in System ON idle
    *(0x50004728 as *mut u32) = 0x1;

    // Workaround for Errata 64
    // REGULATORS: VREGMAIN has invalid configuration when CPU is running
    *(0x5000470C as *mut u32) = 0x29;
    *(0x5000473C as *mut u32) = 0x3;

    // Workaround for Errata 69
    // VREGMAIN configuration is not retained in System OFF
    *(0x5000470C as *mut u32) = 0x65;

    tock_rt0::init_data(&mut _etext, &mut _srelocate, &mut _erelocate);
    tock_rt0::zero_bss(&mut _szero, &mut _ezero);

    // Explicitly tell the core where Tock's vector table is located. If Tock is the
    // only thing on the chip then this is effectively a no-op. If, however, there is
    // a bootloader present then we want to ensure that the vector table is set
    // correctly for Tock. The bootloader _may_ set this for us, but it may not
    // so that any errors early in the Tock boot process trap back to the bootloader.
    // To be safe we unconditionally set the vector table.
    scb::set_vector_table_offset(BASE_VECTORS.as_ptr() as *const ());

    nvic::enable_all();
}
