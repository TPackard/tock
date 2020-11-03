//! Implementation of SPI for NRF53 using EasyDMA.
//!
//! This file only implements support for the three SPI master (`SPIM`)
//! peripherals, and not SPI slave (`SPIS`).
//!
//! Although `kernel::hil::spi::SpiMaster` is implemented for `SPIM`,
//! only the functions marked with `x` are fully defined:
//!
//! * ✓ set_client
//! * ✓ init
//! * ✓ is_busy
//! * ✓ read_write_bytes
//! * write_byte
//! * read_byte
//! * read_write_byte
//! * ✓ specify_chip_select
//! * ✓ set_rate
//! * ✓ get_rate
//! * ✓ set_clock
//! * ✓ get_clock
//! * ✓ set_phase
//! * ✓ get_phase
//! * hold_low
//! * release_low
//!
//! Author
//! -------------------
//!
//! * Author: Jay Kickliter
//! * Date: Sep 10, 2017

use core::cell::Cell;
use core::cmp;
use kernel::common::cells::{OptionalCell, TakeCell};
use kernel::common::registers::{register_bitfields, register_structs, ReadWrite, WriteOnly};
use kernel::common::StaticRef;
use kernel::debug;
use kernel::hil;
use kernel::ReturnCode;
use crate::pinmux::Pinmux;

/// SPI instance 1.
pub static mut SPI1: SPI = SPI::new(1);

const SECURE_INSTANCES: [StaticRef<SpiRegisters>; 5] = unsafe {
    [
        StaticRef::new(0x50008000 as *const SpiRegisters),
        StaticRef::new(0x50009000 as *const SpiRegisters),
        StaticRef::new(0x5000B000 as *const SpiRegisters),
        StaticRef::new(0x5000C000 as *const SpiRegisters),
        StaticRef::new(0x5000A000 as *const SpiRegisters),
    ]
};

#[allow(dead_code)]
const NONSECURE_INSTANCES: [StaticRef<SpiRegisters>; 5] = unsafe {
    [
        StaticRef::new(0x40008000 as *const SpiRegisters),
        StaticRef::new(0x40009000 as *const SpiRegisters),
        StaticRef::new(0x4000B000 as *const SpiRegisters),
        StaticRef::new(0x4000C000 as *const SpiRegisters),
        StaticRef::new(0x4000A000 as *const SpiRegisters),
    ]
};

#[allow(dead_code)]
const SPI0_BASE_NETWORK: StaticRef<SpiRegisters> =
    unsafe { StaticRef::new(0x41013000 as *const SpiRegisters) };

register_structs! {
    SpiRegisters {
        (0x000 => _reserved0),
        /// Start SPI transaction
        (0x010 => spim_task_start: WriteOnly<u32, Task::Register>),
        /// Stop SPI transaction
        (0x014 => spim_task_stop: WriteOnly<u32, Task::Register>),
        (0x018 => _reserved1),
        /// Suspend SPI transaction
        (0x01C => spim_task_suspend: WriteOnly<u32, Task::Register>),
        /// Resume SPI transaction
        (0x020 => spim_task_resume: WriteOnly<u32, Task::Register>),
        /// Acquire SPI semaphore
        (0x024 => spis_task_acquire: WriteOnly<u32, Task::Register>),
        /// Release SPI semaphore, enabling the SPIS to acquire it
        (0x028 => spis_task_release: WriteOnly<u32, Task::Register>),
        (0x02C => _reserved2),
        /// Subscribe configuration for task START
        (0x090 => spim_subscribe_start: WriteOnly<u32, DPPIConfig::Register>),
        /// Subscribe configuration for task STOP
        (0x094 => spim_subscribe_stop: WriteOnly<u32, DPPIConfig::Register>),
        (0x098 => _reserved3),
        /// Subscribe configuration for task SUSPEND
        (0x09C => spim_subscribe_suspend: WriteOnly<u32, DPPIConfig::Register>),
        /// Subscribe configuration for task RESUME
        (0x0A0 => spim_subscribe_resume: WriteOnly<u32, DPPIConfig::Register>),
        /// Subscribe  configuration for task ACQUIRE
        (0x0A4 => spis_subscribe_acquire: WriteOnly<u32, DPPIConfig::Register>),
        /// Subscribe configuration for task RELASE
        (0x0A8 => spis_subscribe_release: WriteOnly<u32, DPPIConfig::Register>),
        (0x0AC => _reserved4),
        /// SPIM transaction has stopped/SPIS granted transaction completed
        (0x104 => event_endstopped: ReadWrite<u32, Event::Register>),
        (0x108 => _reserved5),
        /// End of RXD buffer reached
        (0x110 => event_endrx: ReadWrite<u32, Event::Register>),
        (0x114 => _reserved6),
        /// End of RXD buffer and TXD buffer reached
        (0x118 => spim_event_end: ReadWrite<u32, Event::Register>),
        (0x11C => _reserved7),
        /// End of TXD buffer reached
        (0x120 => spim_event_endtx: ReadWrite<u32, Event::Register>),
        (0x124 => _reserved8),
        /// Semaphore acquired
        (0x128 => spis_event_acquired: ReadWrite<u32, Event::Register>),
        (0x12C => _reserved9),
        /// Transaction started
        (0x14C => spim_event_started: ReadWrite<u32, Event::Register>),
        (0x150 => _reserved10),
        /// Publish configuration for SPIM event STOPPED/SPIS event END
        (0x184 => publish_endstopped: ReadWrite<u32, DPPIConfig::Register>),
        (0x188 => _reserved11),
        /// Publish configuration for event ENDRX
        (0x190 => publish_endrx: ReadWrite<u32, DPPIConfig::Register>),
        (0x194 => _reserved12),
        /// Publish configuration for event END
        (0x198 => spim_publish_end: ReadWrite<u32, DPPIConfig::Register>),
        (0x19C => _reserved13),
        /// Publish configuration for event ENDTX
        (0x1A0 => spim_publish_endtx: ReadWrite<u32, DPPIConfig::Register>),
        (0x1A4 => _reserved14),
        /// Publish configuration for event ACQUIRED
        (0x1A8 => spis_publish_acquired: ReadWrite<u32, DPPIConfig::Register>),
        (0x1AC => _reserved15),
        /// Publish configuration for event STARTED
        (0x1CC => spim_publish_started: ReadWrite<u32, DPPIConfig::Register>),
        (0x1D0 => _reserved16),
        /// Shortcuts between local events and tasks
        (0x200 => shorts: ReadWrite<u32, Shorts::Register>),
        (0x204 => _reserved17),
        /// Enable interrupt
        (0x304 => intenset: ReadWrite<u32, Interrupt::Register>),
        /// Disable interrupt
        (0x308 => intenclr: ReadWrite<u32, Interrupt::Register>),
        (0x30C => _reserved18),
        /// SPIM stall status for EasyDMA RAM accesses/SPIS semaphore status
        (0x400 => stat: ReadWrite<u32, Stat::Register>),
        (0x404 => _reserved19),
        /// Status from last transaction
        (0x440 => spis_status: ReadWrite<u32, Status::Register>),
        (0x444 => _reserved20),
        /// Enable SPI
        (0x500 => enable: ReadWrite<u32, Spi::Register>),
        (0x504 => _reserved21),
        /// Pin select for SCK
        (0x508 => psel_sck: ReadWrite<u32, Psel::Register>),
        /// Pin select for output signal (SPIM MOSI signal or SPIS MISO signal)
        (0x50C => psel_output: ReadWrite<u32, Psel::Register>),
        /// Pin select for input signal (SPIM MISO signal or SPIS MISO signal)
        (0x510 => psel_input: ReadWrite<u32, Psel::Register>),
        /// Pin select for CSN
        (0x514 => psel_csn: ReadWrite<u32, Psel::Register>),
        (0x518 => _reserved22),
        /// SPI frequency
        (0x524 => spim_frequency: ReadWrite<u32, Freq::Register>),
        (0x528 => _reserved23),
        /// Data pointer
        (0x534 => rxd_ptr: ReadWrite<u32, Pointer::Register>),
        /// Maximum number of bytes in receive buffer
        (0x538 => rxd_maxcnt: ReadWrite<u32, Counter::Register>),
        /// Number of bytes transferred in the last transaction
        (0x53C => rxd_amount: ReadWrite<u32, Counter::Register>),
        /// EasyDMA list type
        (0x540 => rxd_list: ReadWrite<u32, List::Register>),
        /// Data pointer
        (0x544 => txd_ptr: ReadWrite<u32, Pointer::Register>),
        /// Number of bytes in transmit buffer
        (0x548 => txd_maxcnt: ReadWrite<u32, Counter::Register>),
        /// Number of bytes transferred in the last transaction
        (0x54C => txd_amount: ReadWrite<u32, Counter::Register>),
        /// EasyDMA list type
        (0x550 => txd_list: ReadWrite<u32, List::Register>),
        /// Configuration register
        (0x554 => config: ReadWrite<u32, Config::Register>),
        (0x558 => _reserved24),
        /// Default character clocked out in case of an ignored transaction
        (0x55C => spis_def: ReadWrite<u32, Def::Register>),
        /// Sample delay for input serial data on MISO
        (0x560 => spim_iftiming_rxdelay: ReadWrite<u32, Rxdelay::Register>),
        /// Minimum duration between edge of CSN and edge of SCK and minimum
        /// duration CSN must stay high between transactions
        (0x564 => spim_iftiming_csndur: ReadWrite<u32, Csndur::Register>),
        /// Polarity of CSN output
        (0x568 => spim_csnpol: ReadWrite<u32, Polarity::Register>),
        /// Pin select for DCX signal
        (0x56C => spim_pseldcx: ReadWrite<u32, Psel::Register>),
        /// DCX configuration
        (0x570 => spim_dcxcnt: ReadWrite<u32, Dcxcnt::Register>),
        (0x574 => _reserved25),
        /// Byte transmitted after TXD.MAXCNT bytes have been transmitted in the
        /// case when RXD.MAXCNT is greater than TXD.MAXCNT
        (0x5C0 => orc: ReadWrite<u32, Orc::Register>),
        (0x5C4 => @END),
    }
}

register_bitfields![u32,
    /// Start task
    Task [
        ENABLE OFFSET(0) NUMBITS(1)
    ],

    /// DPPI configuration register
    DPPIConfig [
        CHIDX OFFSET(0) NUMBITS(8),
        ENABLE OFFSET(31) NUMBITS(1)
    ],

    /// Event
    Event [
        READY OFFSET(0) NUMBITS(1)
    ],

    /// Shortcuts between local events and tasks
    Shorts [
        END_ACQUIRE OFFSET(2) NUMBITS(1),
        END_START OFFSET(17) NUMBITS(1)
    ],

    /// Interrupt configuration
    Interrupt [
        ENDSTOPPED OFFSET(1) NUMBITS(1),
        ENDRX OFFSET(4) NUMBITS(1),
        END OFFSET(6) NUMBITS(1),
        ENDTX OFFSET(8) NUMBITS(1),
        ACQUIRED OFFSET(10) NUMBITS(1),
        STARTED OFFSET(19) NUMBITS(1)
    ],

    /// SPIM: Stall status for EasyDMA RAM accesses. The fields in this register are
    /// set to STALL by hardware whenever a stall occurs and can be cleared
    /// (set to NOSTALL) by the CPU.
    ///
    /// SPIS: Semaphore status register.
    Stat [
        STATUS OFFSET(0) NUMBITS(2)
    ],

    /// Status from last transaction
    Status [
        OVERREAD OFFSET(0) NUMBITS(1) [
            NotPresent = 0,
            Present = 1
        ],
        OVERFLOW OFFSET(1) NUMBITS(1) [
            NotPresent = 0,
            Present = 1
        ]
    ],

    /// Enable SPI
    Spi [
        ENABLE OFFSET(0) NUMBITS(4) [
            Disabled = 0,
            SpisEnabled = 2,
            SpimEnabled = 7
        ]
    ],

    /// Pin select
    Psel [
        // Pin number. MSB is actually the port indicator, but since we number
        // pins sequentially the binary representation of the pin number has
        // the port bit set correctly. So, for simplicity we just treat the
        // pin number as a 6 bit field.
        PIN OFFSET(0) NUMBITS(6) [],
        // Connect/Disconnect
        CONNECT OFFSET(31) NUMBITS(1) [
            Connected = 0,
            Disconnected = 1
        ]
    ],

    /// SPI frequency. Accuracy depends on HFCLK source selected.
    Freq [
        FREQUENCY OFFSET(0) NUMBITS(32)
    ],

    /// DMA pointer
    Pointer [
        POINTER OFFSET(0) NUMBITS(32)
    ],

    /// Counter value
    Counter [
        COUNTER OFFSET(0) NUMBITS(16)
    ],

    /// EasyDMA list type
    List [
        LIST OFFSET(0) NUMBITS(2) [
            Disabled = 0,
            ArrayList = 1
        ]
    ],

    /// Configuration register
    Config [
        /// Bit order
        ORDER OFFSET(0) NUMBITS(1) [
            /// Most significant bit shifted out first
            MsbFirst = 0,
            /// Least significant bit shifted out first
            LsbFirst = 1
        ],
        /// Serial clock (SCK) phase
        CPHA OFFSET(1) NUMBITS(1) [
            /// Sample on leading edge of clock, shift serial data on trailing edge
            Leading = 0,
            /// Sample on trailing edge of clock, shift serial data on leading edge
            Trailing = 1
        ],
        /// Serial clock (SCK) polarity
        CPOL OFFSET(2) NUMBITS(1) [
            ActiveHigh = 0,
            ActiveLow = 1
        ]
    ],

    /// Default character clocked out in case of an ignored transaction
    Def [
        DEF OFFSET(0) NUMBITS(8)
    ],

    /// Sample delay for input  serial data on MISO
    Rxdelay [
        /// Sample delay for input serial data on MISO. The value specifies the
        /// number of 64 MHz clock cycles (15.625 ns) delay from the sampling
        /// edge of SCK (leading edge for CONFIG.CPHA = 0, trailing edge for
        /// CONFIG.CPHA = 1) until the input serial data is sampled. As an
        /// example, if RXDELAY = 0 and CONFIG.CPHA = 0, the input serial data
        /// is sampled on the rising edge of SCK.
        RXDELAY OFFSET(0) NUMBITS(3)
    ],

    /// Minimum duration between edge of CSN and edge of SCK and minimum
    /// duration CSN must stay highbetween transactions
    Csndur [
        /// The value is specified in number of 64 MHz clock cycles (15.625 ns)
        CSNDUR OFFSET(0) NUMBITS(8)
    ],

    /// Polarity of CSN output
    Polarity [
        CSNPOL OFFSET(0) NUMBITS(1) [
            LOW = 0,
            HIGH = 1
        ]
    ],

    /// DCX configuration
    Dcxcnt [
        /// This register specifies the number of command bytes preceding the
        /// data bytes. The PSEL.DCX line will be low during transmission of
        /// command bytes and high during transmission of data bytes. A value of
        /// 0xF indicates that all bytes are command bytes.
        DCXCNT OFFSET(0) NUMBITS(4)
    ],

    /// Byte transmitted after TXD.MAXCNT bytes have been transmitted in the
    /// case when RXD.MAXCNT isgreater than TXD.MAXCNT
    Orc [
        ORC OFFSET(0) NUMBITS(8)
    ]
];

/// An enum representing all allowable `frequency` register values.
#[repr(u32)]
#[derive(Copy, Clone)]
pub enum Frequency {
    K125 = 0x02000000,
    K250 = 0x04000000,
    K500 = 0x08000000,
    M1 =   0x10000000,
    M2 =   0x20000000,
    M4 =   0x40000000,
    M8 =   0x80000000,
    M16 =  0x0A000000,
    M32 =  0x14000000,
}

impl Frequency {
    pub fn from_register(reg: u32) -> Option<Frequency> {
        match reg {
            0x02000000 => Some(Frequency::K125),
            0x04000000 => Some(Frequency::K250),
            0x08000000 => Some(Frequency::K500),
            0x10000000 => Some(Frequency::M1),
            0x20000000 => Some(Frequency::M2),
            0x40000000 => Some(Frequency::M4),
            0x80000000 => Some(Frequency::M8),
            0x0A000000 => Some(Frequency::M16),
            0x14000000 => Some(Frequency::M32),
            _ => None,
        }
    }

    pub fn into_spi_rate(&self) -> u32 {
        match *self {
            Frequency::K125 => 125_000,
            Frequency::K250 => 250_000,
            Frequency::K500 => 500_000,
            Frequency::M1 => 1_000_000,
            Frequency::M2 => 2_000_000,
            Frequency::M4 => 4_000_000,
            Frequency::M8 => 8_000_000,
            Frequency::M16 => 16_000_000,
            Frequency::M32 => 32_000_000,
        }
    }

    pub fn from_spi_rate(freq: u32) -> Frequency {
        if freq < 250_000 {
            Frequency::K125
        } else if freq < 500_000 {
            Frequency::K250
        } else if freq < 1_000_000 {
            Frequency::K500
        } else if freq < 2_000_000 {
            Frequency::M1
        } else if freq < 4_000_000 {
            Frequency::M2
        } else if freq < 8_000_000 {
            Frequency::M4
        } else if freq < 16_000_000 {
            Frequency::M8
        } else if freq < 32_000_000 {
            Frequency::M16
        } else {
            Frequency::M32
        }
    }
}

/// Whether the SPI peripheral is acting as SPIM or SPIS.
#[derive(Copy, Clone, PartialEq)]
pub enum SpiRole {
    SPIM,
    SPIS,
}

/// A SPI device.
///
/// A `SPI` instance wraps a `registers::spi::SPI` together with
/// addition data necessary to implement an asynchronous interface.
pub struct SPI {
    registers: StaticRef<SpiRegisters>,
    role: Cell<SpiRole>,
    spim_client: OptionalCell<&'static dyn hil::spi::SpiMasterClient>,
    spis_client: OptionalCell<&'static dyn hil::spi::SpiSlaveClient>,
    chip_select: OptionalCell<&'static dyn hil::gpio::Pin>,
    initialized: Cell<bool>,
    busy: Cell<bool>,
    tx_buf: TakeCell<'static, [u8]>,
    rx_buf: TakeCell<'static, [u8]>,
    transfer_len: Cell<usize>,
}

impl SPI {
    const fn new(instance: usize) -> SPI {
        SPI {
            registers: SECURE_INSTANCES[instance],
            role: Cell::new(SpiRole::SPIM),
            spim_client: OptionalCell::empty(),
            spis_client: OptionalCell::empty(),
            chip_select: OptionalCell::empty(),
            initialized: Cell::new(false),
            busy: Cell::new(false),
            tx_buf: TakeCell::empty(),
            rx_buf: TakeCell::empty(),
            transfer_len: Cell::new(0),
        }
    }

    fn handle_spim_interrupt(&self) {
        if self.registers.spim_event_end.is_set(Event::READY) {
            // End of RXD buffer and TXD buffer reached

            if self.chip_select.is_none() {
                debug_assert!(false, "Invariant violated. Chip-select must be Some.");
                return;
            }

            self.chip_select.map(|cs| cs.set());
            self.registers.spim_event_end.write(Event::READY::CLEAR);

            self.spim_client.map(|client| match self.tx_buf.take() {
                None => (),
                Some(tx_buf) => {
                    client.read_write_done(tx_buf, self.rx_buf.take(), self.transfer_len.take())
                }
            });

            self.busy.set(false);
        }

        // Although we only configured the chip interrupt on the
        // above 'end' event, the other event fields also get set by
        // the chip. Let's clear those flags.

        if self.registers.event_endstopped.is_set(Event::READY) {
            // SPI transaction has stopped
            self.registers.event_endstopped.write(Event::READY::CLEAR);
        }

        if self.registers.event_endrx.is_set(Event::READY) {
            // End of RXD buffer reached
            self.registers.event_endrx.write(Event::READY::CLEAR);
        }

        if self.registers.spim_event_endtx.is_set(Event::READY) {
            // End of TXD buffer reached
            self.registers.spim_event_endtx.write(Event::READY::CLEAR);
        }

        if self.registers.spim_event_started.is_set(Event::READY) {
            // Transaction started
            self.registers.spim_event_started.write(Event::READY::CLEAR);
        }
    }

    fn handle_spis_interrupt(&self) {
        if self.registers.spis_event_acquired.is_set(Event::READY) {
            // Semaphore acquired by CPU, occurs once transaction is complete
            // due to END_ACQUIRE shortcut.

            self.registers.spis_event_acquired.write(Event::READY::CLEAR);

            self.spis_client.map(|client| {
                client.read_write_done(self.tx_buf.take(), self.rx_buf.take(), self.transfer_len.take())
            });

            self.busy.set(false);
        }

        // Although we only configured the chip interrupt on the
        // above 'acquired' event, the other event fields also get set by
        // the chip. Let's clear those flags.

        if self.registers.event_endstopped.is_set(Event::READY) {
            // SPIS transaction has ended
            self.registers.event_endstopped.write(Event::READY::CLEAR);
        }

        if self.registers.event_endrx.is_set(Event::READY) {
            // End of RXD buffer reached
            self.registers.event_endrx.write(Event::READY::CLEAR);
        }
    }

    #[inline(never)]
    pub fn handle_interrupt(&self) {
        match self.role.get() {
            SpiRole::SPIM => self.handle_spim_interrupt(),
            SpiRole::SPIS => self.handle_spis_interrupt(),
        }
    }

    /// Configures an already constructed `SPI`.
    pub fn configure(&self, mosi: Pinmux, miso: Pinmux, sck: Pinmux, csn: Pinmux, role: SpiRole) {
        match role {
            SpiRole::SPIM => {
                self.registers.psel_output.write(Psel::PIN.val(mosi.into()));
                self.registers.psel_input.write(Psel::PIN.val(miso.into()));
            }
            SpiRole::SPIS => {
                self.registers.psel_input.write(Psel::PIN.val(mosi.into()));
                self.registers.psel_output.write(Psel::PIN.val(miso.into()));
            }
        }

        self.registers.psel_sck.write(Psel::PIN.val(sck.into()));
        self.registers.psel_csn.write(Psel::PIN.val(csn.into()));

        self.enable(role);
    }

    /// Enables `SPI` peripheral.
    pub fn enable(&self, role: SpiRole) {
        match role {
            SpiRole::SPIM => self.registers.enable.write(Spi::ENABLE::SpimEnabled),
            SpiRole::SPIS => self.registers.enable.write(Spi::ENABLE::SpisEnabled),
        }

        self.role.set(role);
    }

    /// Disables `SPI` peripheral.
    pub fn disable(&self) {
        self.registers.enable.write(Spi::ENABLE::Disabled);
    }

    pub fn is_enabled(&self) -> bool {
        !self.registers.enable.matches_all(Spi::ENABLE::Disabled)
    }

    fn set_clock(&self, polarity: hil::spi::ClockPolarity) {
        debug_assert!(self.initialized.get());
        debug_assert!(self.initialized.get());
        let new_polarity = match polarity {
            hil::spi::ClockPolarity::IdleLow => Config::CPOL::ActiveHigh,
            hil::spi::ClockPolarity::IdleHigh => Config::CPOL::ActiveLow,
        };
        self.registers.config.modify(new_polarity);
    }

    fn get_clock(&self) -> hil::spi::ClockPolarity {
        debug_assert!(self.initialized.get());
        match self.registers.config.read(Config::CPOL) {
            0 => hil::spi::ClockPolarity::IdleLow,
            1 => hil::spi::ClockPolarity::IdleHigh,
            _ => unreachable!(),
        }
    }

    fn set_phase(&self, phase: hil::spi::ClockPhase) {
        debug_assert!(self.initialized.get());
        let new_phase = match phase {
            hil::spi::ClockPhase::SampleLeading => Config::CPHA::Leading,
            hil::spi::ClockPhase::SampleTrailing => Config::CPHA::Trailing,
        };
        self.registers.config.modify(new_phase);
    }

    fn get_phase(&self) -> hil::spi::ClockPhase {
        debug_assert!(self.initialized.get());
        match self.registers.config.read(Config::CPHA) {
            0 => hil::spi::ClockPhase::SampleLeading,
            1 => hil::spi::ClockPhase::SampleTrailing,
            _ => unreachable!(),
        }
    }
}

impl hil::spi::SpiMaster for SPI {
    type ChipSelect = &'static dyn hil::gpio::Pin;

    fn set_client(&self, client: &'static dyn hil::spi::SpiMasterClient) {
        self.spim_client.set(client);
    }

    fn init(&self) {
        self.registers.intenset.write(Interrupt::END::SET);
        self.initialized.set(true);
    }

    fn is_busy(&self) -> bool {
        self.busy.get()
    }

    fn read_write_bytes(
        &self,
        tx_buf: &'static mut [u8],
        rx_buf: Option<&'static mut [u8]>,
        len: usize,
    ) -> ReturnCode {
        debug_assert!(self.initialized.get());
        debug_assert!(!self.busy.get());
        debug_assert!(self.tx_buf.is_none());
        debug_assert!(self.rx_buf.is_none());

        // Clear (set to low) chip-select
        if self.chip_select.is_none() {
            return ReturnCode::ENODEVICE;
        }
        self.chip_select.map(|cs| cs.clear());

        // Setup transmit data registers
        let tx_len: u32 = cmp::min(len, tx_buf.len()) as u32;
        self.registers.txd_ptr.set(tx_buf.as_ptr() as u32);
        self.registers.txd_maxcnt.write(Counter::COUNTER.val(tx_len));
        self.tx_buf.replace(tx_buf);

        // Setup receive data registers
        match rx_buf {
            None => {
                self.registers.rxd_ptr.set(0);
                self.registers.rxd_maxcnt.write(Counter::COUNTER.val(0));
                self.transfer_len.set(tx_len as usize);
                self.rx_buf.put(None);
            }
            Some(buf) => {
                self.registers.rxd_ptr.set(buf.as_mut_ptr() as u32);
                let rx_len: u32 = cmp::min(len, buf.len()) as u32;
                self.registers.rxd_maxcnt.write(Counter::COUNTER.val(rx_len));
                self.transfer_len.set(cmp::min(tx_len, rx_len) as usize);
                self.rx_buf.put(Some(buf));
            }
        }

        // Start the transfer
        self.busy.set(true);
        self.registers.spim_task_start.write(Task::ENABLE::SET);
        ReturnCode::SUCCESS
    }

    fn write_byte(&self, _val: u8) {
        debug_assert!(self.initialized.get());
        unimplemented!("SPIM: Use `read_write_bytes()` instead.");
    }

    fn read_byte(&self) -> u8 {
        debug_assert!(self.initialized.get());
        unimplemented!("SPIM: Use `read_write_bytes()` instead.");
    }

    fn read_write_byte(&self, _val: u8) -> u8 {
        debug_assert!(self.initialized.get());
        unimplemented!("SPIM: Use `read_write_bytes()` instead.");
    }

    // Tell the SPI peripheral what to use as a chip select pin.
    // The type of the argument is based on what makes sense for the
    // peripheral when this trait is implemented.
    fn specify_chip_select(&self, cs: Self::ChipSelect) {
        cs.make_output();
        cs.set();
        self.chip_select.set(cs);
    }

    // Returns the actual rate set
    fn set_rate(&self, rate: u32) -> u32 {
        debug_assert!(self.initialized.get());
        let f = Frequency::from_spi_rate(rate);
        self.registers.spim_frequency.set(f as u32);
        f.into_spi_rate()
    }

    fn get_rate(&self) -> u32 {
        debug_assert!(self.initialized.get());

        // Reset value is a valid frequency (250kbps), so .expect
        // should be safe here
        let f = Frequency::from_register(self.registers.spim_frequency.get())
            .expect("nrf53 unknown spi rate");
        f.into_spi_rate()
    }

    fn set_clock(&self, polarity: hil::spi::ClockPolarity) {
        self.set_clock(polarity);
    }

    fn get_clock(&self) -> hil::spi::ClockPolarity {
        self.get_clock()
    }

    fn set_phase(&self, phase: hil::spi::ClockPhase) {
        self.set_phase(phase);
    }

    fn get_phase(&self) -> hil::spi::ClockPhase {
        self.get_phase()
    }

    // The following two trait functions are not implemented for
    // SAM4L, and appear to not provide much functionality. Let's not
    // bother implementing them unless needed.
    fn hold_low(&self) {
        unimplemented!("SPIM: Use `read_write_bytes()` instead.");
    }

    fn release_low(&self) {
        unimplemented!("SPIM: Use `read_write_bytes()` instead.");
    }
}

impl hil::spi::SpiSlave for SPI {
    fn init(&self) {
        self.registers.intenset.write(Interrupt::ACQUIRED::SET);
        self.registers.shorts.write(Shorts::END_ACQUIRE::SET);
        self.initialized.set(true);
    }

    fn has_client(&self) -> bool {
        !self.spis_client.is_none()
    }

    fn set_client(&self, client: Option<&'static dyn hil::spi::SpiSlaveClient>) {
        self.spis_client.insert(client);
    }

    fn set_write_byte(&self, _val: u8) {
        debug_assert!(self.initialized.get());
        unimplemented!("SPIS: Use `read_write_bytes()` instead.");
    }

    fn read_write_bytes(
        &self,
        tx_buf: Option<&'static mut [u8]>,
        rx_buf: Option<&'static mut [u8]>,
        len: usize,
    ) -> ReturnCode {
        // Invariant: SPIS semaphore must be held by CPU.
        debug_assert!(self.registers.stat.get() == 1);

        // Setup transmit data registers
        match tx_buf {
            None => {
                self.registers.txd_ptr.set(0);
                self.registers.txd_maxcnt.write(Counter::COUNTER.val(0));
                self.tx_buf.put(None);
            }
            Some(buf) => {
                let tx_len: u32 = cmp::min(len, buf.len()) as u32;
                self.registers.txd_ptr.set(buf.as_ptr() as u32);
                self.registers.txd_maxcnt.write(Counter::COUNTER.val(tx_len));
                self.tx_buf.put(Some(buf));
            }
        }

        // Setup receive data registers
        match rx_buf {
            None => {
                self.registers.rxd_ptr.set(0);
                self.registers.rxd_maxcnt.write(Counter::COUNTER.val(0));
                self.rx_buf.put(None);
            }
            Some(buf) => {
                let rx_len: u32 = cmp::min(len, buf.len()) as u32;
                self.registers.rxd_ptr.set(buf.as_mut_ptr() as u32);
                self.registers.rxd_maxcnt.write(Counter::COUNTER.val(rx_len));
                self.rx_buf.put(Some(buf));
            }
        }

        // Start the transfer
        self.busy.set(true);
        self.registers.spis_task_release.write(Task::ENABLE::SET);
        ReturnCode::SUCCESS
    }

    fn set_clock(&self, polarity: hil::spi::ClockPolarity) {
        self.set_clock(polarity);
    }

    fn get_clock(&self) -> hil::spi::ClockPolarity {
        self.get_clock()
    }

    fn set_phase(&self, phase: hil::spi::ClockPhase) {
        self.set_phase(phase);
    }

    fn get_phase(&self) -> hil::spi::ClockPhase {
        self.get_phase()
    }
}

impl hil::gpio::Client for SPI {
    fn fired(&self) {
        self.spis_client.map(|client| {
            client.chip_selected();
        });
    }
}
