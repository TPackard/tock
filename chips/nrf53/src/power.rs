//! Power management

use kernel::common::cells::OptionalCell;
use kernel::common::registers::{
    register_bitfields, register_structs, ReadOnly, ReadWrite, WriteOnly,
};
use kernel::common::StaticRef;

// Power management control
#[allow(dead_code)]
const POWER_BASE_NONSECURE: StaticRef<AppPowerRegisters> =
    unsafe { StaticRef::new(0x40005000 as *const AppPowerRegisters) };
const POWER_BASE_SECURE: StaticRef<AppPowerRegisters> =
    unsafe { StaticRef::new(0x50005000 as *const AppPowerRegisters) };
#[allow(dead_code)]
const POWER_BASE_NETWORK: StaticRef<NetworkPowerRegisters> =
    unsafe { StaticRef::new(0x41005000 as *const NetworkPowerRegisters) };

// Network core voltage request control
#[allow(dead_code)]
const VREQCTRL_BASE: StaticRef<VReqCtrlRegisters> =
    unsafe { StaticRef::new(0x41004000 as *const VReqCtrlRegisters) };

register_structs! {
    AppPowerRegisters {
        (0x000 => _reserved0),
        /// Enable Constant Latency mode
        (0x078 => task_constlat: WriteOnly<u32, Task::Register>),
        /// Enable Low-power mode (variable latency)
        (0x07C => task_lowpwr: WriteOnly<u32, Task::Register>),
        (0x080 => _reserved1),
        /// Subscribe configuration for Constant Latency mode
        (0x0F8 => subscribe_constlat: ReadWrite<u32, DPPIConfig::Register>),
        /// Subscribe configuration for Low-power mode
        (0x0FC => subscribe_lowpwr: ReadWrite<u32, DPPIConfig::Register>),
        (0x100 => _reserved2),
        /// Power failure warning
        (0x108 => event_pofwarn: ReadWrite<u32, Event::Register>),
        (0x10C => _reserved3),
        /// CPU entered WFI/WFE sleep
        (0x114 => event_sleepenter: ReadWrite<u32, Event::Register>),
        /// CPU exited WFI/WFE sleep
        (0x118 => event_sleepexit: ReadWrite<u32, Event::Register>),
        (0x11C => _reserved4),
        /// Publish configuration for power failure warning event
        (0x188 => publish_pofwarn: ReadWrite<u32, DPPIConfig::Register>),
        (0x18C => _reserved5),
        /// Publish configuration for sleep enter event
        (0x194 => publish_sleepenter: ReadWrite<u32, DPPIConfig::Register>),
        /// Publish configuration for sleep exit event
        (0x198 => publish_sleepexit: ReadWrite<u32, DPPIConfig::Register>),
        (0x19C => _reserved6),
        /// Enable or disable interrupt
        (0x300 => inten: ReadWrite<u32, Interrupt::Register>),
        /// Enable interrupt
        (0x304 => intenset: ReadWrite<u32, Interrupt::Register>),
        /// Disable interrupt
        (0x308 => intenclr: ReadWrite<u32, Interrupt::Register>),
        (0x30C => _reserved7),
        /// Reset reason
        (0x400 => resetreas: ReadWrite<u32, AppResetReason::Register>),
        (0x404 => _reserved8),
        /// Main supply status
        (0x428 => mainregstatus: ReadOnly<u32, MainSupply::Register>),
        (0x42C => _reserved9),
        /// System OFF register
        (0x500 => systemoff: WriteOnly<u32, Task::Register>),
        (0x504 => _reserved10),
        /// Power failure comparator configuration
        (0x510 => pofcon: ReadWrite<u32, PowerFailure::Register>),
        (0x514 => _reserved11),
        /// General purpose retention register
        (0x51C => gpregret: ReadWrite<u32, Byte::Register>),
        /// General purpose retention register
        (0x520 => gpregret2: ReadWrite<u32, Byte::Register>),
        (0x524 => _reserved12),
        /// Force off power and clock in network core
        (0x614 => network_forceoff: ReadWrite<u32, Task::Register>),
        (0x618 => _reserved13),
        /// DC/DC enable for VREGMAIN
        (0x704 => vregmain_dcdcen: ReadWrite<u32, Task::Register>),
        (0x708 => _reserved14),
        /// DC/DC enable register for VREGRADIO
        (0x904 => vregradio_dcdcen: ReadWrite<u32, Task::Register>),
        (0x908 => _reserved15),
        /// DC/DC enable register for VREGH
        (0xB00 => vregh_dcdcen: ReadWrite<u32, Task::Register>),
        (0xB04 => @END),
    },

    NetworkPowerRegisters {
        (0x000 => _reserved0),
        /// Enable Constant Latency mode
        (0x078 => task_constlat: WriteOnly<u32, Task::Register>),
        /// Enable Low-power mode (variable latency)
        (0x07C => task_lowpwr: WriteOnly<u32, Task::Register>),
        (0x080 => _reserved1),
        /// Subscribe configuration for Constant Latency mode
        (0x0F8 => subscribe_constlat: ReadWrite<u32, DPPIConfig::Register>),
        /// Subscribe configuration for Low-power mode
        (0x0FC => subscribe_lowpwr: ReadWrite<u32, DPPIConfig::Register>),
        (0x100 => _reserved2),
        /// Power failure warning
        (0x108 => event_pofwarn: ReadWrite<u32, Event::Register>),
        (0x10C => _reserved3),
        /// CPU entered WFI/WFE sleep
        (0x114 => event_sleepenter: ReadWrite<u32, Event::Register>),
        /// CPU exited WFI/WFE sleep
        (0x118 => event_sleepexit: ReadWrite<u32, Event::Register>),
        (0x11C => _reserved4),
        /// Publish configuration for power failure warning event
        (0x188 => publish_pofwarn: ReadWrite<u32, DPPIConfig::Register>),
        (0x18C => _reserved5),
        /// Publish configuration for sleep enter event
        (0x194 => publish_sleepenter: ReadWrite<u32, DPPIConfig::Register>),
        /// Publish configuration for sleep exit event
        (0x198 => publish_sleepexit: ReadWrite<u32, DPPIConfig::Register>),
        (0x19C => _reserved6),
        /// Enable or disable interrupt
        (0x300 => inten: ReadWrite<u32, Interrupt::Register>),
        /// Enable interrupt
        (0x304 => intenset: ReadWrite<u32, Interrupt::Register>),
        /// Disable interrupt
        (0x308 => intenclr: ReadWrite<u32, Interrupt::Register>),
        (0x30C => _reserved7),
        /// Reset reason
        (0x400 => resetreas: ReadWrite<u32, NetworkResetReason::Register>),
        (0x404 => _reserved8),
        /// General purpose retention register
        (0x51C => gpregret: ReadWrite<u32, Byte::Register>),
        /// General purpose retention register
        (0x520 => gpregret2: ReadWrite<u32, Byte::Register>),
        (0x524 => @END),
    },

    VReqCtrlRegisters {
        /// Request high voltage on radio
        (0x500 => vregradio_vreqh: ReadWrite<u32, Task::Register>),
        (0x504 => _reserved1),
        /// Signals when high voltage on radio is ready
        (0x508 => vregradio_vreqready: ReadOnly<u32, Event::Register>),
        (0x50C => @END),
    }
}

register_bitfields! [u32,
    /// Start task
    Task [
        ENABLE OFFSET(0) NUMBITS(1)
    ],

    /// Read event
    Event [
        READY OFFSET(0) NUMBITS(1)
    ],

    /// DPPI configuration for tasks and events
    DPPIConfig [
        CHIDX OFFSET(0) NUMBITS(8),
        ENABLE OFFSET(31) NUMBITS(1)
    ],

    /// Power management Interrupts
    Interrupt [
        POFWARN OFFSET(2) NUMBITS(1),
        SLEEPENTER OFFSET(5) NUMBITS(1),
        SLEEPEXIT OFFSET(6) NUMBITS(1)
    ],

    AppResetReason [
        RESETPIN OFFSET(0) NUMBITS(1) [
            Detected = 1
        ],
        DOG0 OFFSET(1) NUMBITS(1) [
            Detected = 1
        ],
        CTRLAP OFFSET(2) NUMBITS(1) [
            Detected = 1
        ],
        SREQ OFFSET(3) NUMBITS(1) [
            Detected = 1
        ],
        LOCKUP OFFSET(4) NUMBITS(1) [
            Detected = 1
        ],
        OFF OFFSET(5) NUMBITS(1) [
            Detected = 1
        ],
        LPCOMP OFFSET(6) NUMBITS(1) [
            Detected = 1
        ],
        DIF OFFSET(7) NUMBITS(1) [
            Detected = 1
        ],
        NFC OFFSET(24) NUMBITS(1) [
            Detected = 1
        ],
        DOG1 OFFSET(25) NUMBITS(1) [
            Detected = 1
        ],
        VBUS OFFSET(26) NUMBITS(1) [
            Detected = 1
        ]
    ],

    NetworkResetReason [
        RESETPIN OFFSET(0) NUMBITS(1) [
            Detected = 1
        ],
        DOG0 OFFSET(1) NUMBITS(1) [
            Detected = 1
        ],
        CTRLAP OFFSET(2) NUMBITS(1) [
            Detected = 1
        ],
        SREQ OFFSET(3) NUMBITS(1) [
            Detected = 1
        ],
        LOCKUP OFFSET(4) NUMBITS(1) [
            Detected = 1
        ],
        OFF OFFSET(5) NUMBITS(1) [
            Detected = 1
        ],
        LPCOMP OFFSET(6) NUMBITS(1) [
            Detected = 1
        ],
        DIF OFFSET(7) NUMBITS(1) [
            Detected = 1
        ],
        LSREQ OFFSET(16) NUMBITS(1) [
            Detected = 1
        ],
        LLOCKUP OFFSET(17) NUMBITS(1) [
            Detected = 1
        ],
        LDOG OFFSET(18) NUMBITS(1) [
            Detected = 1
        ],
        MFORCEOFF OFFSET(23) NUMBITS(1) [
            Detected = 1
        ],
        NFC OFFSET(24) NUMBITS(1) [
            Detected = 1
        ],
        DOG1 OFFSET(25) NUMBITS(1) [
            Detected = 1
        ],
        VBUS OFFSET(26) NUMBITS(1) [
            Detected = 1
        ],
        LCTRLAP OFFSET(27) NUMBITS(1) [
            Detected = 1
        ]
    ],

    PowerFailure [
        POF OFFSET(0) NUMBITS(1) [
            Disabled = 0,
            Enabled = 1
        ],
        THRESHOLD OFFSET(1) NUMBITS(4) [
            V19 = 6,
            V20 = 7,
            V21 = 8,
            V22 = 9,
            V23 = 10,
            V24 = 11,
            V25 = 12,
            V26 = 13,
            V27 = 14,
            V28 = 15
        ],
        THRESHOLDVDDH OFFSET(8) NUMBITS(4) [
            V27 = 0,
            V28 = 1,
            V29 = 2,
            V30 = 3,
            V31 = 4,
            V32 = 5,
            V33 = 6,
            V34 = 7,
            V35 = 8,
            V36 = 9,
            V37 = 10,
            V38 = 11,
            V39 = 12,
            V40 = 13,
            V41 = 14,
            V42 = 15
        ]
    ],

    Byte [
        VALUE OFFSET(0) NUMBITS(8)
    ],

    MainSupply [
        MAINREGSTATUS OFFSET(0) NUMBITS(1) [
            Normal = 0,
            High = 1
        ]
    ]
];

pub struct Power<'a> {
    registers: StaticRef<AppPowerRegisters>,
    client: OptionalCell<&'a dyn PowerClient>,
}

pub enum MainVoltage {
    /// Normal voltage mode, when supply voltage is connected to both the VDD and
    /// VDDH pins (so that VDD equals VDDH).
    Normal = 0,
    /// High voltage mode, when supply voltage is only connected to the VDDH pin,
    /// and the VDD pin is not connected to any voltage supply.
    High = 1,
}

pub enum PowerEvent {
    PowerFailure,
    EnterSleep,
    ExitSleep,
}

pub trait PowerClient {
    fn handle_power_event(&self, event: PowerEvent);
}

impl<'a> Power<'a> {
    const fn new() -> Self {
        Power {
            registers: POWER_BASE_SECURE,
            client: OptionalCell::empty(),
        }
    }

    pub fn set_client(&self, client: &'a dyn PowerClient) {
        self.client.set(client);
    }

    pub fn handle_interrupt(&self) {
        let regs = &*self.registers;
        self.disable_all_interrupts();

        if regs.event_pofwarn.is_set(Event::READY) {
            regs.event_pofwarn.write(Event::READY::CLEAR);
            self.client
                .map(|client| client.handle_power_event(PowerEvent::PowerFailure));
        }

        if regs.event_sleepenter.is_set(Event::READY) {
            regs.event_sleepenter.write(Event::READY::CLEAR);
            self.client
                .map(|client| client.handle_power_event(PowerEvent::EnterSleep));
        }

        if regs.event_sleepexit.is_set(Event::READY) {
            regs.event_sleepexit.write(Event::READY::CLEAR);
            self.client
                .map(|client| client.handle_power_event(PowerEvent::ExitSleep));
        }

        self.enable_interrupts();
    }

    pub fn enable_interrupts(&self) {
        let regs = &*self.registers;
        regs.intenset.write(
            Interrupt::POFWARN::SET + Interrupt::SLEEPENTER::SET + Interrupt::SLEEPEXIT::SET,
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

    pub fn get_main_supply_status(&self) -> MainVoltage {
        match self
            .registers
            .mainregstatus
            .read_as_enum(MainSupply::MAINREGSTATUS)
        {
            Some(MainSupply::MAINREGSTATUS::Value::Normal) => MainVoltage::Normal,
            Some(MainSupply::MAINREGSTATUS::Value::High) => MainVoltage::High,
            // This case shouldn't happen as the register only holds 1 bit.
            None => unreachable!(),
        }
    }
}

pub static mut POWER: Power<'static> = Power::new();
