//! RTC driver, nRF5X-family

use kernel::common::cells::OptionalCell;
use kernel::common::registers::{
    register_bitfields, register_structs, ReadOnly, ReadWrite, WriteOnly,
};
use kernel::common::StaticRef;
use kernel::hil::time::{self, Alarm, Freq32KHz, Time};
use kernel::hil::Controller;

/// Number of capture/compare registers.
const NUM_CC: usize = 4;

#[allow(dead_code)]
const RTC0_BASE_NONSECURE: StaticRef<RtcRegisters> =
    unsafe { StaticRef::new(0x40014000 as *const RtcRegisters) };
const RTC0_BASE_SECURE: StaticRef<RtcRegisters> =
    unsafe { StaticRef::new(0x50014000 as *const RtcRegisters) };
#[allow(dead_code)]
const RTC0_BASE_NETWORK: StaticRef<RtcRegisters> =
    unsafe { StaticRef::new(0x41011000 as *const RtcRegisters) };

#[allow(dead_code)]
const RTC1_BASE_NONSECURE: StaticRef<RtcRegisters> =
    unsafe { StaticRef::new(0x40015000 as *const RtcRegisters) };
#[allow(dead_code)]
const RTC1_BASE_SECURE: StaticRef<RtcRegisters> =
    unsafe { StaticRef::new(0x50015000 as *const RtcRegisters) };
#[allow(dead_code)]
const RTC1_BASE_NETWORK: StaticRef<RtcRegisters> =
    unsafe { StaticRef::new(0x41016000 as *const RtcRegisters) };

register_structs! {
    RtcRegisters {
        /// Start RTC Counter.
        (0x000 => tasks_start: WriteOnly<u32, Task::Register>),
        /// Stop RTC Counter.
        (0x004 => tasks_stop: WriteOnly<u32, Task::Register>),
        /// Clear RTC Counter.
        (0x008 => tasks_clear: WriteOnly<u32, Task::Register>),
        /// Set COUNTER to 0xFFFFFFF0.
        (0x00C => tasks_trigovrflw: WriteOnly<u32, Task::Register>),
        (0x010 => _reserved1),
        /// Capture RTC counter to CC\[n\] register.
        (0x040 => tasks_capture: [WriteOnly<u32, Task::Register>; NUM_CC]),
        (0x050 => _reserved2),
        /// Subscribe configuration for task START.
        (0x080 => subscribe_start: ReadWrite<u32, Configuration::Register>),
        /// Subscribe configuration for task STOP.
        (0x084 => subscribe_stop: ReadWrite<u32, Configuration::Register>),
        /// Subscribe configuration for task CLEAR.
        (0x088 => subscribe_clear: ReadWrite<u32, Configuration::Register>),
        /// Subscribe configuration for task TRIGOVRFLW.
        (0x08C => subscribe_trigovrflw: ReadWrite<u32, Configuration::Register>),
        (0x090 => _reserved3),
        /// Subscribe configuration for task CAPTURE\[n\].
        (0x0C0 => subscribe_capture: [ReadWrite<u32, Configuration::Register>; NUM_CC]),
        (0x0D0 => _reserved4),
        /// Event on COUNTER increment.
        (0x100 => events_tick: ReadWrite<u32, Event::Register>),
        /// Event on COUNTER overflow.
        (0x104 => events_ovrflw: ReadWrite<u32, Event::Register>),
        (0x108 => _reserved5),
        /// Compare event on CC\[n\] match.
        (0x140 => events_compare: [ReadWrite<u32, Event::Register>; NUM_CC]),
        (0x150 => _reserved6),
        /// Publish configuration for event TICK.
        (0x180 => publish_tick: ReadWrite<u32, Configuration::Register>),
        /// Publish configuration for event OVRFLW.
        (0x184 => publish_ovrflw: ReadWrite<u32, Configuration::Register>),
        (0x188 => _reserved7),
        /// Publish configuration for event COMPARE.
        (0x1C0 => publish_compare: [ReadWrite<u32, Configuration::Register>; NUM_CC]),
        (0x1D0 => _reserved8),
        /// Shortcuts between local events and tasks.
        (0x200 => shorts: ReadWrite<u32, Shorts::Register>),
        (0x204 => _reserved9),
        /// Interrupt enable set register.
        (0x304 => intenset: ReadWrite<u32, Inte::Register>),
        /// Interrupt enable clear register.
        (0x308 => intenclr: ReadWrite<u32, Inte::Register>),
        (0x30C => _reserved10),
        /// Configures event enable routing to PPI for each RTC event.
        (0x340 => evten: ReadWrite<u32, Inte::Register>),
        /// Enable events routing to PPI.
        (0x344 => evtenset: ReadWrite<u32, Inte::Register>),
        /// Disable events routing to PPI.
        (0x348 => evtenclr: ReadWrite<u32, Inte::Register>),
        (0x34C => _reserved11),
        /// Current COUNTER value.
        (0x504 => counter: ReadOnly<u32, Counter::Register>),
        /// 12-bit prescaler for COUNTER frequency (32768/(PRESCALER+1)).
        /// Must be written when RTC is stopped.
        (0x508 => prescaler: ReadWrite<u32, Prescaler::Register>),
        (0x50C => _reserved12),
        /// Capture/compare registers.
        (0x540 => cc: [ReadWrite<u32, Counter::Register>; NUM_CC]),
        (0x550 => @END),
    }
}

register_bitfields![u32,
    Task [
        ENABLE 0
    ],

    Configuration [
        CHIDX OFFSET(0) NUMBITS(8),
        ENABLE OFFSET(31) NUMBITS(1)
    ],

    Event [
        READY 0
    ],

    Shorts [
        /// Shortcuts between event COMPARE\[0\] and task CLEAR.
        COMPARE0_CLEAR 0,
        /// Shortcuts between event COMPARE\[1\] and task CLEAR.
        COMPARE1_CLEAR 1,
        /// Shortcuts between event COMPARE\[2\] and task CLEAR.
        COMPARE2_CLEAR 2,
        /// Shortcuts between event COMPARE\[3\] and task CLEAR.
        COMPARE3_CLEAR 3
    ],

    Inte [
        /// Enable interrupt on TICK event.
        TICK 0,
        /// Enable interrupt on OVRFLW event.
        OVRFLW 1,
        /// Enable interrupt on COMPARE\[0\] event.
        COMPARE0 16,
        /// Enable interrupt on COMPARE\[1\] event.
        COMPARE1 17,
        /// Enable interrupt on COMPARE\[2\] event.
        COMPARE2 18,
        /// Enable interrupt on COMPARE\[3\] event.
        COMPARE3 19
    ],

    Prescaler [
        PRESCALER OFFSET(0) NUMBITS(12)
    ],

    Counter [
        VALUE OFFSET(0) NUMBITS(24)
    ]
];

pub struct Rtc<'a> {
    registers: StaticRef<RtcRegisters>,
    callback: OptionalCell<&'a dyn time::AlarmClient>,
}

pub static mut RTC: Rtc = Rtc {
    registers: RTC0_BASE_SECURE,
    callback: OptionalCell::empty(),
};

impl<'a> Controller for Rtc<'a> {
    type Config = &'a dyn time::AlarmClient;

    fn configure(&self, client: &'a dyn time::AlarmClient) {
        self.callback.set(client);

        // FIXME: what to do here?
        // self.start();
        // Set counter incrementing frequency to 16KHz
        // rtc1().prescaler.set(1);
    }
}

impl<'a> Rtc<'a> {
    pub fn start(&self) {
        // This function takes a nontrivial amount of time
        // So it should only be called during initialization, not each tick
        self.registers.prescaler.write(Prescaler::PRESCALER.val(0));
        self.registers.tasks_start.write(Task::ENABLE::SET);
    }

    pub fn stop(&self) {
        self.registers.cc[0].write(Counter::VALUE.val(0));
        self.registers.tasks_stop.write(Task::ENABLE::SET);
    }

    fn is_running(&self) -> bool {
        self.registers.evten.is_set(Inte::COMPARE0)
    }

    pub fn handle_interrupt(&self) {
        self.registers.events_compare[0].write(Event::READY::CLEAR);
        self.registers.intenclr.write(Inte::COMPARE0::SET);
        self.callback.map(|cb| {
            cb.fired();
        });
    }
}

impl<'a> Time for Rtc<'a> {
    type Frequency = Freq32KHz;

    fn now(&self) -> u32 {
        self.registers.counter.read(Counter::VALUE)
    }

    fn max_tics(&self) -> u32 {
        (1 << 24) - 1
    }
}

impl<'a> Alarm<'a> for Rtc<'a> {
    fn set_client(&self, client: &'a dyn time::AlarmClient) {
        self.callback.set(client);
    }

    fn set_alarm(&self, tics: u32) {
        // Similarly to the disable function, here we don't restart the timer
        // Instead, we just listen for it again
        self.registers.intenset.write(Inte::COMPARE0::SET);
        self.registers.cc[0].write(Counter::VALUE.val(tics));
        self.registers.events_compare[0].write(Event::READY::CLEAR);
    }

    fn get_alarm(&self) -> u32 {
        self.registers.cc[0].read(Counter::VALUE)
    }

    fn disable(&self) {
        self.registers.intenclr.write(Inte::COMPARE0::SET);
        self.registers.events_compare[0].write(Event::READY::CLEAR);
    }

    fn is_enabled(&self) -> bool {
        self.is_running()
    }
}
