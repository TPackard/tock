//! Clock peripheral driver, nRF53
//!
//! Based on clock driver for nRF52
//!
//! HFCLK - High Frequency Clock:
//!
//! * 192 MHz/128 MHz/64 MHz internal oscillator (HFINT)
//! * 32 MHz crystal oscillator (HFXO)
//! * The HFXO must be running to use the RADIO, USBD, NFC module or the calibration mechanism
//!   associated with the 32.768 kHz RC oscillator.
//!
//! LFCLK - Low Frequency Clock Source:
//!
//! * 32.768 kHz RC oscillator (LFRC)
//! * 32.768 kHz ultra-low power RC oscillator (LFULP)
//! * 32.768 kHz crystal oscillator (LFXO)
//! * 32.768 kHz synthesized from HFCLK (LFSYNT)

use kernel::common::cells::OptionalCell;
use kernel::common::registers::{
    register_bitfields, register_structs, ReadOnly, ReadWrite, WriteOnly,
};
use kernel::common::StaticRef;

pub static mut CLOCK_APP: Clock = Clock::new(CLOCK_BASE_SECURE);
pub static mut CLOCK_NET: Clock = Clock::new(CLOCK_BASE_NETWORK);

register_structs! {
    ClockRegisters {
        (0x000 => tasks_hfclkstart: WriteOnly<u32, Control::Register>),
        (0x004 => tasks_hfclkstop: WriteOnly<u32, Control::Register>),
        (0x008 => tasks_lfclkstart: WriteOnly<u32, Control::Register>),
        (0x00C => tasks_lfclkstop: WriteOnly<u32, Control::Register>),
        (0x010 => tasks_cal: WriteOnly<u32, Control::Register>),
        (0x014 => _reserved1),
        (0x018 => tasks_hfclkaudiostart: WriteOnly<u32, Control::Register>),
        (0x01C => tasks_hfclkaudiostop: WriteOnly<u32, Control::Register>),
        (0x020 => tasks_hfclk192mstart: WriteOnly<u32, Control::Register>),
        (0x024 => tasks_hfclk192mstop: WriteOnly<u32, Control::Register>),
        (0x028 => _reserved2),
        (0x080 => subscribe_hfclkstart: ReadWrite<u32, DPPIConfig::Register>),
        (0x084 => subscribe_hfclkstop: ReadWrite<u32, DPPIConfig::Register>),
        (0x088 => subscribe_lfclkstart: ReadWrite<u32, DPPIConfig::Register>),
        (0x08C => subscribe_lfclkstop: ReadWrite<u32, DPPIConfig::Register>),
        (0x090 => subscribe_cal: ReadWrite<u32, DPPIConfig::Register>),
        (0x094 => _reserved3),
        (0x098 => subscribe_hfclkaudiostart: ReadWrite<u32, DPPIConfig::Register>),
        (0x09C => subscribe_hfclkaudiostop: ReadWrite<u32, DPPIConfig::Register>),
        (0x0A0 => subscribe_hfclk192mstart: ReadWrite<u32, DPPIConfig::Register>),
        (0x0A4 => subscribe_hfclk192mstop: ReadWrite<u32, DPPIConfig::Register>),
        (0x0A8 => _reserved4),
        (0x100 => events_hfclkstarted: ReadWrite<u32, Status::Register>),
        (0x104 => events_lfclkstarted: ReadWrite<u32, Status::Register>),
        (0x108 => _reserved5),
        (0x11C => events_done: ReadWrite<u32, Status::Register>),
        (0x120 => events_hfclkaudiostarted: ReadWrite<u32, Status::Register>),
        (0x124 => events_hfclk192mstarted: ReadWrite<u32, Status::Register>),
        (0x128 => _reserved6),
        (0x180 => publish_hfclkstarted: ReadWrite<u32, DPPIConfig::Register>),
        (0x184 => publish_lfclkstarted: ReadWrite<u32, DPPIConfig::Register>),
        (0x188 => _reserved7),
        (0x19C => publish_done: ReadWrite<u32, DPPIConfig::Register>),
        (0x1A0 => publish_hfclkaudiostarted: ReadWrite<u32, DPPIConfig::Register>),
        (0x1A4 => publish_hfclk192mstarted: ReadWrite<u32, DPPIConfig::Register>),
        (0x1A8 => _reseved8),
        (0x300 => inten: ReadWrite<u32, Interrupt::Register>),
        (0x304 => intenset: ReadWrite<u32, Interrupt::Register>),
        (0x308 => intenclr: ReadWrite<u32, Interrupt::Register>),
        (0x30C => intpend: ReadOnly<u32, Interrupt::Register>),
        (0x310 => _reserved9),
        (0x408 => hfclkrun: ReadOnly<u32, Status::Register>),
        (0x40C => hfclkstat: ReadOnly<u32, HfClkStat::Register>),
        (0x410 => _reserved10),
        (0x414 => lfclkrun: ReadOnly<u32, Control::Register>),
        (0x418 => lfclkstat: ReadOnly<u32, LfClkStat::Register>),
        (0x41C => lfclksrccopy: ReadOnly<u32, LfClkSrc::Register>),
        (0x420 => _reserved11),
        (0x450 => hfclkaudiorun: ReadOnly<u32, Control::Register>),
        (0x454 => hfclkaudiostat: ReadOnly<u32, HfClkAudioStat::Register>),
        (0x458 => hfclk192mrun: ReadOnly<u32, Control::Register>),
        (0x45C => hfclk192mstat: ReadOnly<u32, HfClkStat::Register>),
        (0x460 => _reserved12),
        (0x514 => hfclksrc: ReadWrite<u32, HfClkSrc::Register>),
        (0x518 => lfclksrc: ReadWrite<u32, LfClkSrc::Register>),
        (0x51C => _reserved13),
        /// Not present in network core
        (0x558 => hfclkctrl: ReadWrite<u32, HfClkCtrl::Register>),
        /// Not present in network core
        (0x55C => hfclkaudio_frequency: ReadWrite<u32, HfClkAudioFrequency::Register>),
        (0x560 => _reserved14),
        (0x570 => hfclkalwaysrun: ReadOnly<u32, Control::Register>),
        (0x574 => lfclkalwaysrun: ReadOnly<u32, Control::Register>),
        (0x578 => _reserved15),
        (0x57C => hfclkaudioalwaysrun: ReadOnly<u32, Control::Register>),
        (0x580 => hfclk192msrc: ReadWrite<u32, HfClkSrc::Register>),
        (0x584 => hfclk192malwaysrun: ReadOnly<u32, Control::Register>),
        (0x588 => _reserved16),
        (0x5B8 => hfclk192mctrl: ReadWrite<u32, HfClk192mCtrl::Register>),
        (0x5BC => @END),
    },

    OscillatorsRegisters {
        /// Programmable capacitance of XC1 and XC2
        (0x5C4 => xosc32mcaps: ReadWrite<u32, Capacitance::Register>),
        (0x5C8 => _reserved1),
        /// Enable bypass of LFCLK crystal oscillator with external clock source
        (0x6C0 => xosc32ki_bypass: ReadWrite<u32, Control::Register>),
        (0x6C4 => _reserved2),
        /// Control usage of internal load capacitors
        (0x6D0 => xosc32ki_intcap: ReadWrite<u32, IntCap::Register>),
        (0x6D4 => @END),
    }
}

register_bitfields! [u32,
    Control [
        ENABLE OFFSET(0) NUMBITS(1)
    ],

    Status [
        READY OFFSET(0) NUMBITS(1)
    ],

    DPPIConfig [
        CHIDX OFFSET(0) NUMBITS(8),
        ENABLE OFFSET(31) NUMBITS(1)
    ],

    Interrupt [
        HFCLKSTARTED OFFSET(0) NUMBITS(1),
        LFCLKSTARTED OFFSET(1) NUMBITS(1),
        DONE OFFSET(7) NUMBITS(1),
        HFCLKAUDIOSTARTED OFFSET(8) NUMBITS(1),
        HFCLK192MSTARTED OFFSET(9) NUMBITS(1)
    ],

    HfClkStat [
        SRC OFFSET(0) NUMBITS(1) [
            HFINT = 0,
            HFXO = 1
        ],
        ALWAYSRUNNING OFFSET(4) NUMBITS(1) [
            RUNNING = 1
        ],
        STATE OFFSET(16) NUMBITS(1) [
            RUNNING = 1
        ]
    ],

    LfClkStat [
        SRC OFFSET(0) NUMBITS(2) [
            LFULP = 0,
            LFRC = 1,
            LFXO = 2,
            LFSYNT = 3
        ],
        ALWAYSRUNNING OFFSET(4) NUMBITS(1) [
            RUNNING = 1
        ],
        STATE OFFSET(16) NUMBITS(1) [
            RUNNING = 1
        ]
    ],

    HfClkAudioStat [
        ALWAYSRUNNING OFFSET(4) NUMBITS(1) [
            RUNNING = 1
        ],
        STATE OFFSET(16) NUMBITS(1) [
            RUNNING = 1
        ]
    ],

    HfClkSrc [
        SRC OFFSET(0) NUMBITS(1) [
            HFINT = 0,
            HFXO = 1
        ]
    ],

    LfClkSrc [
        SRC OFFSET(0) NUMBITS(2) [
            LFULP = 0,
            LFRC = 1,
            LFXO = 2,
            LFSYNT = 3
        ]
    ],

    HfClkCtrl [
        HCLK OFFSET(0) NUMBITS(1) [
            Div1 = 0,
            Div2 = 1
        ]
    ],

    HfClkAudioFrequency [
        FREQUENCY OFFSET(0) NUMBITS(16)
    ],

    HfClk192mCtrl [
        HCLK192M OFFSET(0) NUMBITS(2) [
            Div1 = 0,
            Div2 = 1,
            Div4 = 2
        ]
    ],

    Capacitance [
        CAPVALUE OFFSET(0) NUMBITS(5),
        ENABLE OFFSET(8) NUMBITS(1)
    ],

    IntCap [
        INTCAP OFFSET(0) NUMBITS(2) [
            External = 0,
            C6PF = 1,
            C7PF = 2,
            C11PF = 3
        ]
    ]
];

// Clock registers
#[allow(dead_code)]
const CLOCK_BASE_NONSECURE: StaticRef<ClockRegisters> =
    unsafe { StaticRef::new(0x40005000 as *const ClockRegisters) };
const CLOCK_BASE_SECURE: StaticRef<ClockRegisters> =
    unsafe { StaticRef::new(0x50005000 as *const ClockRegisters) };
const CLOCK_BASE_NETWORK: StaticRef<ClockRegisters> =
    unsafe { StaticRef::new(0x41005000 as *const ClockRegisters) };

// Oscillator registers
#[allow(dead_code)]
const OSCILLATORS_BASE_NONSECURE: StaticRef<OscillatorsRegisters> =
    unsafe { StaticRef::new(0x40004000 as *const OscillatorsRegisters) };
#[allow(dead_code)]
const OSCILLATORS_BASE_SECURE: StaticRef<OscillatorsRegisters> =
    unsafe { StaticRef::new(0x50004000 as *const OscillatorsRegisters) };

/// Interrupt sources
pub enum InterruptField {
    HFCLKSTARTED = 1 << 0,
    LFCLKSTARTED = 1 << 1,
    DONE = 1 << 7,
    HFCLKAUDIOSTARTED = 1 << 8,
    HFCLK192MSTARTED = 1 << 9,
}

/// Low frequency clock source
pub enum LowClockSource {
    LFULP = 0,
    LFRC = 1,
    LFXO = 2,
    LFSYNT = 3,
}

/// High frequency clock source
pub enum HighClockSource {
    HFINT = 0,
    HFXO = 1,
}

/// Clock struct
pub struct Clock {
    registers: StaticRef<ClockRegisters>,
    client: OptionalCell<&'static dyn ClockClient>,
}

pub trait ClockClient {
    /// All clock interrupts are control signals, e.g., when
    /// a clock has started etc. We don't actually handle any
    /// of them for now, but keep this trait in place for if we
    /// do need to in the future.
    fn event(&self);
}

impl Clock {
    /// Constructor
    const fn new(registers: StaticRef<ClockRegisters>) -> Clock {
        Clock {
            registers,
            client: OptionalCell::empty(),
        }
    }

    /// Client for callbacks
    pub fn set_client(&self, client: &'static dyn ClockClient) {
        self.client.set(client);
    }

    /// Enable interrupt
    pub fn interrupt_enable(&self, interrupt: InterruptField) {
        let regs = &*self.registers;
        // this is a little too verbose
        match interrupt {
            InterruptField::DONE => regs.intenset.write(Interrupt::DONE::SET),
            InterruptField::HFCLKSTARTED => regs.intenset.write(Interrupt::HFCLKSTARTED::SET),
            InterruptField::LFCLKSTARTED => regs.intenset.write(Interrupt::LFCLKSTARTED::SET),
            InterruptField::HFCLKAUDIOSTARTED => regs.intenset.write(Interrupt::HFCLKAUDIOSTARTED::SET),
            InterruptField::HFCLK192MSTARTED => regs.intenset.write(Interrupt::HFCLK192MSTARTED::SET),
        }
    }

    /// Disable interrupt
    pub fn interrupt_disable(&self, interrupt: InterruptField) {
        let regs = &*self.registers;
        // this is a little too verbose
        match interrupt {
            InterruptField::DONE => regs.intenset.write(Interrupt::DONE::SET),
            InterruptField::HFCLKSTARTED => regs.intenset.write(Interrupt::HFCLKSTARTED::SET),
            InterruptField::LFCLKSTARTED => regs.intenset.write(Interrupt::LFCLKSTARTED::SET),
            InterruptField::HFCLKAUDIOSTARTED => regs.intenset.write(Interrupt::HFCLKAUDIOSTARTED::SET),
            InterruptField::HFCLK192MSTARTED => regs.intenset.write(Interrupt::HFCLK192MSTARTED::SET),
        }
    }

    /// Start the high frequency clock
    pub fn high_start(&self) {
        let regs = &*self.registers;
        regs.tasks_hfclkstart.write(Control::ENABLE::SET);
    }

    /// Stop the high frequency clock
    pub fn high_stop(&self) {
        let regs = &*self.registers;
        regs.tasks_hfclkstop.write(Control::ENABLE::SET);
    }

    /// Check if the high frequency clock has started
    pub fn high_started(&self) -> bool {
        let regs = &*self.registers;
        regs.events_hfclkstarted.matches_all(Status::READY.val(1))
    }

    /// Read clock source from the high frequency clock
    pub fn high_source(&self) -> HighClockSource {
        let regs = &*self.registers;
        match regs.hfclkstat.read(HfClkStat::SRC) {
            0 => HighClockSource::HFINT,
            1 => HighClockSource::HFXO,
            _ => unreachable!(),
        }
    }

    /// Check if the high frequency clock is running
    pub fn high_running(&self) -> bool {
        let regs = &*self.registers;
        regs.hfclkstat.matches_all(HfClkStat::STATE::RUNNING)
    }

    /// Start the low frequency clock
    pub fn low_start(&self) {
        let regs = &*self.registers;
        regs.tasks_lfclkstart.write(Control::ENABLE::SET);
    }

    /// Stop the low frequency clock
    pub fn low_stop(&self) {
        let regs = &*self.registers;
        regs.tasks_lfclkstop.write(Control::ENABLE::SET);
    }

    /// Check if the low frequency clock has started
    pub fn low_started(&self) -> bool {
        let regs = &*self.registers;
        regs.events_lfclkstarted.matches_all(Status::READY::SET)
    }

    /// Read clock source from the low frequency clock
    pub fn low_source(&self) -> LowClockSource {
        let regs = &*self.registers;
        match regs.lfclkstat.read(LfClkStat::SRC) {
            0 => LowClockSource::LFULP,
            1 => LowClockSource::LFRC,
            2 => LowClockSource::LFXO,
            3 => LowClockSource::LFSYNT,
            _ => unreachable!(),
        }
    }

    /// Check if the low frequency clock is running
    pub fn low_running(&self) -> bool {
        let regs = &*self.registers;
        regs.lfclkstat.matches_all(LfClkStat::STATE::RUNNING)
    }

    /// Set low frequency clock source
    pub fn low_set_source(&self, clock_source: LowClockSource) {
        let regs = &*self.registers;
        regs.lfclksrc.write(LfClkSrc::SRC.val(clock_source as u32));
    }

    /// Set high frequency clock source
    pub fn high_set_source(&self, clock_source: HighClockSource) {
        let regs = &*self.registers;
        regs.hfclksrc
            .write(HfClkSrc::SRC.val(clock_source as u32));
    }

    /// Configure high frequency clock frequency
    pub fn high_frequency(&self, freq_divisor: u32) {
        let regs = &*self.registers;
        let divisor = match freq_divisor {
            1 => HfClkCtrl::HCLK::Div1,
            2 => HfClkCtrl::HCLK::Div2,
            _ => return,
        };
        regs.hfclkctrl.write(divisor);
    }

    /// Start the high frequency audio clock
    pub fn audio_start(&self) {
        let regs = &*self.registers;
        regs.tasks_hfclkaudiostart.write(Control::ENABLE::SET);
    }

    /// Stop the high frequency audio clock
    pub fn audio_stop(&self) {
        let regs = &*self.registers;
        regs.tasks_hfclkaudiostop.write(Control::ENABLE::SET);
    }

    /// Check if the high frequency audio clock has started
    pub fn audio_started(&self) -> bool {
        let regs = &*self.registers;
        regs.events_hfclkaudiostarted.matches_all(Status::READY.val(1))
    }

    /// Check if the high frequency audio clock is running
    pub fn audio_running(&self) -> bool {
        let regs = &*self.registers;
        regs.hfclkaudiostat.matches_all(HfClkAudioStat::STATE::RUNNING)
    }

    /// Configure high frequency audio clock frequency
    pub fn audio_frequency(&self, frequency: u16) {
        let regs = &*self.registers;
        regs.hfclkaudio_frequency
            .write(HfClkAudioFrequency::FREQUENCY.val(frequency as u32));
    }

    /// Start the 192 MHz high frequency clock
    pub fn high_192m_start(&self) {
        let regs = &*self.registers;
        regs.tasks_hfclk192mstart.write(Control::ENABLE::SET);
    }

    /// Stop the 192 MHz high frequency clock
    pub fn high_192m_stop(&self) {
        let regs = &*self.registers;
        regs.tasks_hfclk192mstop.write(Control::ENABLE::SET);
    }

    /// Check if the 192 MHz high frequency clock has started
    pub fn high_192m_started(&self) -> bool {
        let regs = &*self.registers;
        regs.events_hfclk192mstarted.matches_all(Status::READY.val(1))
    }

    /// Read clock source from the 192 MHz high frequency clock
    pub fn high_192m_source(&self) -> HighClockSource {
        let regs = &*self.registers;
        match regs.hfclk192mstat.read(HfClkStat::SRC) {
            0 => HighClockSource::HFINT,
            1 => HighClockSource::HFXO,
            _ => unreachable!(),
        }
    }

    /// Check if the 192 MHz high frequency clock is running
    pub fn high_192m_running(&self) -> bool {
        let regs = &*self.registers;
        regs.hfclk192mstat.matches_all(HfClkStat::STATE::RUNNING)
    }

    /// Set 192 MHz high frequency clock source
    pub fn high_192m_set_source(&self, clock_source: HighClockSource) {
        let regs = &*self.registers;
        regs.hfclk192msrc
            .write(HfClkSrc::SRC.val(clock_source as u32));
    }

    /// Configure 192 MHz high frequency clock frequency
    pub fn high_192m_frequency(&self, freq_divisor: u32) {
        let regs = &*self.registers;
        let divisor = match freq_divisor {
            1 => HfClk192mCtrl::HCLK192M::Div1,
            2 => HfClk192mCtrl::HCLK192M::Div2,
            4 => HfClk192mCtrl::HCLK192M::Div4,
            _ => return,
        };
        regs.hfclk192mctrl.write(divisor);
    }
}
