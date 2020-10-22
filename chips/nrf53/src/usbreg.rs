//! USB Regulator control

use kernel::common::cells::OptionalCell;
use kernel::common::registers::{
    register_bitfields, register_structs, ReadOnly, ReadWrite,
};
use kernel::common::StaticRef;

#[allow(dead_code)]
const USBREG_BASE_NONSECURE: StaticRef<UsbRegRegisters> =
    unsafe { StaticRef::new(0x40037000 as *const UsbRegRegisters) };
const USBREG_BASE_SECURE: StaticRef<UsbRegRegisters> =
    unsafe { StaticRef::new(0x50037000 as *const UsbRegRegisters) };

register_structs! {
    UsbRegRegisters {
        /// Voltage supply detected on VBUS
        (0x100 => event_usbdetected: ReadWrite<u32, Event::Register>),
        /// Voltage supply removed from VBUS
        (0x104 => event_usbremoved: ReadWrite<u32, Event::Register>),
        /// USB 3.3V supply ready
        (0x108 => event_usbpwrrdy: ReadWrite<u32, Event::Register>),
        (0x10C => _reserved1),
        /// Publish configuration for USB detection event
        (0x180 => publish_usbdetected: ReadWrite<u32, DPPIConfig::Register>),
        /// Publish configuration for USB removal event
        (0x184 => publish_usbremoved: ReadWrite<u32, DPPIConfig::Register>),
        /// Publish configuration for USB power ready event
        (0x188 => publish_usbpwrrdy: ReadWrite<u32, DPPIConfig::Register>),
        (0x18C => _reserved2),
        /// Enable or disable interrupt
        (0x300 => inten: ReadWrite<u32, Interrupt::Register>),
        /// Enable interrupt
        (0x304 => intenset: ReadWrite<u32, Interrupt::Register>),
        /// Disable interrupt
        (0x308 => intenclr: ReadWrite<u32, Interrupt::Register>),
        (0x30C => _reserved3),
        /// USB supply status
        (0x400 => usbregstatus: ReadOnly<u32, UsbRegStatus::Register>),
        (0x404 => @END),
    }
}

register_bitfields! [u32,
    /// Read event
    Event [
        READY OFFSET(0) NUMBITS(1)
    ],

    /// DPPI configuration for tasks and events
    DPPIConfig [
        CHIDX OFFSET(0) NUMBITS(8),
        ENABLE OFFSET(31) NUMBITS(1)
    ],

    Interrupt [
        USBDETECTED OFFSET(0) NUMBITS(1),
        USBREMOVED OFFSET(1) NUMBITS(1),
        USBPWRRDY OFFSET(2) NUMBITS(1)
    ],

    UsbRegStatus [
        VBUSDETECT OFFSET(0) NUMBITS(1),
        OUTPUTRDY OFFSET(1) NUMBITS(1)
    ]
];

/// The USB state machine needs to be notified of power events (USB detected, USB
/// removed, USB power ready) in order to be initialized and shut down properly.
pub struct UsbReg<'a> {
    registers: StaticRef<UsbRegRegisters>,
    /// A client to which to notify USB plug-in/plug-out/power-ready events.
    usb_client: OptionalCell<&'a dyn UsbRegClient>,
}

pub enum UsbRegEvent {
    UsbPluggedIn,
    UsbPluggedOut,
    UsbPowerReady,
}

pub trait UsbRegClient {
    fn handle_usb_reg_event(&self, event: UsbRegEvent);
}

impl<'a> UsbReg<'a> {
    const fn new() -> Self {
        UsbReg {
            registers: USBREG_BASE_SECURE,
            usb_client: OptionalCell::empty(),
        }
    }

    pub fn set_usb_client(&self, client: &'a dyn UsbRegClient) {
        self.usb_client.set(client);
    }

    pub fn handle_interrupt(&self) {
        let regs = &*self.registers;
        self.disable_all_interrupts();

        if regs.event_usbdetected.is_set(Event::READY) {
            regs.event_usbdetected.write(Event::READY::CLEAR);
            self.usb_client
                .map(|client| client.handle_usb_reg_event(UsbRegEvent::UsbPluggedIn));
        }

        if regs.event_usbremoved.is_set(Event::READY) {
            regs.event_usbremoved.write(Event::READY::CLEAR);
            self.usb_client
                .map(|client| client.handle_usb_reg_event(UsbRegEvent::UsbPluggedOut));
        }

        if regs.event_usbpwrrdy.is_set(Event::READY) {
            regs.event_usbpwrrdy.write(Event::READY::CLEAR);
            self.usb_client
                .map(|client| client.handle_usb_reg_event(UsbRegEvent::UsbPowerReady));
        }

        self.enable_interrupts();
    }

    pub fn enable_interrupts(&self) {
        let regs = &*self.registers;
        regs.intenset.write(
            Interrupt::USBDETECTED::SET + Interrupt::USBREMOVED::SET + Interrupt::USBPWRRDY::SET,
        );
    }

    pub fn enable_interrupt(&self, intr: u32) {
        let regs = &*self.registers;
        regs.intenset.set(intr);
    }

    pub fn clear_interrupt(&self, intr: u32) {
        let regs = &*self.registers;
        regs.intenclr.set(intr);
    }

    pub fn disable_all_interrupts(&self) {
        let regs = &*self.registers;
        // disable all possible interrupts
        regs.intenclr.set(0xffffffff);
    }

    pub fn is_vbus_present(&self) -> bool {
        self.registers.usbregstatus.is_set(UsbRegStatus::VBUSDETECT)
    }

    pub fn is_usb_power_ready(&self) -> bool {
        self.registers.usbregstatus.is_set(UsbRegStatus::OUTPUTRDY)
    }
}

pub static mut USBREG: UsbReg<'static> = UsbReg::new();
