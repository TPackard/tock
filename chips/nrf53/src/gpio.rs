//! GPIO and GPIOTE (task and events), nRF53. Based on nRF5x GPIO and GPIOTE driver by Philip
//! Levis.

use core::ops::{Index, IndexMut};
use enum_primitive::cast::FromPrimitive;
use enum_primitive::enum_from_primitive;
use kernel::common::cells::OptionalCell;
use kernel::common::registers::{register_bitfields, register_structs, ReadWrite, WriteOnly};
use kernel::common::StaticRef;
use kernel::debug;
use kernel::hil;

// Need to change reserved space placement in GpioteRegisters if modified.
/// Number of GPIOTE channels.
const NUM_GPIOTE: usize = 8;

// Need to change @END of GpioRegisters if modified.
/// Number of GPIO pins per port.
const GPIO_PER_PORT: usize = 32;

const GPIOTE0_BASE: StaticRef<GpioteRegisters> =
    unsafe { StaticRef::new(0x5000D000 as *const GpioteRegisters) };
#[allow(dead_code)]
const GPIOTE1_BASE: StaticRef<GpioteRegisters> =
    unsafe { StaticRef::new(0x4002F000 as *const GpioteRegisters) };
#[allow(dead_code)]
const GPIOTE_BASE_NETWORK: StaticRef<GpioteRegisters> =
    unsafe { StaticRef::new(0x4100A000 as *const GpioteRegisters) };

const GPIO_BASE_ADDRESS_SECURE: usize = 0x50842500;
#[allow(dead_code)]
const GPIO_BASE_ADDRESS_NONSECURE: usize = 0x40842500;
#[allow(dead_code)]
const GPIO_BASE_ADDRESS_NETWORK: usize = 0x418C0500;
const GPIO_SIZE: usize = 0x300;

register_structs! {
    GpioteRegisters {
        /// Task for writing to pin specified in config\[n\].PSEL. Action on pin is configured in
        /// config\[n\].POLARITY.
        (0x000 => tasks_out: [WriteOnly<u32, Task::Register>; NUM_GPIOTE]),
        (0x020 => _reserved1),
        /// Task for writing to pin specified in config\[n\].PSEL. Action on pin is to set it high.
        (0x030 => tasks_set: [WriteOnly<u32, Task::Register>; NUM_GPIOTE]),
        (0x050 => _reserved2),
        /// Task for writing to pin specified in config\[n\].PSEL. Action on pin is to set it low.
        (0x060 => tasks_clr: [WriteOnly<u32, Task::Register>; NUM_GPIOTE]),
        /// Subscribe configuration for task tasks_out\[n\].
        (0x080 => subscribe_out: [ReadWrite<u32, Configuration::Register>; NUM_GPIOTE]),
        (0x0A0 => _reserved3),
        /// Subscribe configuration for task tasks_set\[n\].
        (0x0B0 => subscribe_set: [ReadWrite<u32, Configuration::Register>; NUM_GPIOTE]),
        (0x0D0 => _reserved4),
        /// Subscribe configuration for task tasks_clr\[n\].
        (0x0E0 => subscribe_clr: [ReadWrite<u32, Configuration::Register>; NUM_GPIOTE]),
        /// Event generated from pin specified in config\[n\].PSEL.
        (0x100 => events_in: [ReadWrite<u32, Event::Register>; NUM_GPIOTE]),
        (0x120 => _reserved5),
        /// Event generated from multiple input GPIO pins with SENSE mechanism enabled.
        (0x17C => events_port: ReadWrite<u32, Event::Register>),
        /// Publish configuration for event events_in.
        (0x180 => publish_in: [ReadWrite<u32, Configuration::Register>; NUM_GPIOTE]),
        (0x1A0 => _reserved6),
        /// Publish configuration for event events_port.
        (0x1FC => publish_port: ReadWrite<u32, Configuration::Register>),
        /// Enable interrupts.
        (0x304 => intenset: ReadWrite<u32, Interrupt::Register>),
        /// Disable interrupts.
        (0x308 => intenclr: ReadWrite<u32, Interrupt::Register>),
        (0x30C => _reserved7),
        /// Configuration for tasks_out\[n\], tasks_set\[n\], tasks_clr\[n\], and events_in\[n\].
        (0x510 => config: [ReadWrite<u32, GpioteConfig::Register>; NUM_GPIOTE]),
        (0x530 => @END),
    },

    GpioRegisters {
        (0x000 => _reserved0),
        /// Write GPIO port.
        (0x004 => out: ReadWrite<u32, Gpio::Register>),
        /// Set GPIO port pins.
        (0x008 => outset: ReadWrite<u32, Gpio::Register>),
        /// Clear GPIO port pins.
        (0x00C => outclr: ReadWrite<u32, Gpio::Register>),
        /// Read GPIO port.
        (0x010 => in_: ReadWrite<u32, Gpio::Register>),
        /// Direction of GPIO pins.
        (0x014 => dir: ReadWrite<u32, Gpio::Register>),
        /// DIR set register.
        (0x018 => dirset: ReadWrite<u32, Gpio::Register>),
        /// DIR clear register.
        (0x01C => dirclr: ReadWrite<u32, Gpio::Register>),
        /// Latch register indicating what GPIO pins that have met the criteria set in the
        /// pin_cnf\[n\].SENSE registers. Write 1 to clear.
        (0x020 => latch: ReadWrite<u32, Gpio::Register>),
        /// Select between default DETECT signal behavior and LDETECT mode (for non-secure pin
        /// only).
        (0x024 => detectmode: ReadWrite<u32, DetectMode::Register>),
        /// Select between default DETECT signal behavior and LDETECT mode (for secure pin only).
        (0x028 => detectmode_sec: ReadWrite<u32, DetectMode::Register>),
        (0x02C => _reserved1),
        /// Configuration of GPIO pins.
        (0x200 => pin_cnf: [ReadWrite<u32, PinConfig::Register>; GPIO_PER_PORT]),
        (0x280 => @END),
    }
}

register_bitfields! [u32,
    Task [
        /// Write high to trigger task.
        ENABLE OFFSET(0) NUMBITS(1)
    ],

    Event [
        READY OFFSET(0) NUMBITS(1)
    ],

    Configuration [
        CHIDX OFFSET(0) NUMBITS(8),
        ENABLE OFFSET(31) NUMBITS(1)
    ],

    /// Enable or disable interrupts for IN\[n\] and PORT events.
    Interrupt [
        IN0 0,
        IN1 1,
        IN2 2,
        IN3 3,
        IN4 4,
        IN5 5,
        IN6 6,
        IN7 7,
        PORT 31
    ],

    GpioteConfig [
        MODE OFFSET(0) NUMBITS(2) [
            /// Specified pin will not be acquired by GPIOTE.
            Disabled = 0,
            /// Specified pin will be configured as an input and the events_in\[n\] event will by
            /// generated if operation specified in POLARITY occurs on the pin.
            Event = 1,
            /// Specified pin will be configured as an output and triggering the tasks_set\[n\],
            /// tasks_clr\[n\], or tasks_out\[n\] task will perform the operation specified by POLARITY
            /// on the pin. When enabled as a task the GPIOTE module will acquire the pin and the
            /// pin can no longer be written as a regular output pin from the GPIO module.
            Task = 3
        ],
        /// Selected GPIO pin.
        PSEL OFFSET(8) NUMBITS(5) [],
        /// Port number.
        PORT OFFSET(13) NUMBITS(1) [],
        /// Operation performed when tasks_out\[n\] triggered in task mode or that shall trigger
        /// events_in\[n\] event in event mode.
        POLARITY OFFSET(16) NUMBITS(2) [
            /// Task mode: no effect on pin from tasks_out\[n\].
            /// Event mode: no events_in\[n\] event generated on pin activity.
            NoEffect = 0,
            /// Task mode: set pin from tasks_out\[n\].
            /// Event mode: generate events_in\[n\] event when rising edge on pin.
            LoToHi = 1,
            /// Task mode: clear pin from tasks_out\[n\].
            /// Event mode: generate events_in\[n\] event when falling edge on pin.
            HiToLo = 2,
            /// Task mode: toggle pin from tasks_out\[n\].
            /// Event mode: generate events_in\[n\] when any change on pin.
            Toggle = 3
        ],
        /// When in task mode: initial value of the output when the GPIOTE channel is configured.
        /// When in event mode: no effect.
        OUTINIT OFFSET(20) NUMBITS(1) [
            Low = 0,
            High = 1
        ]
    ],

    Gpio [
        /// Pin\[n\], each bit corresponds to a pin 0 to 31. Used for the following operations:
        /// Write     (0 - low;         1 - high).
        /// Read      (0 - low;         1 - high).
        /// Direction (0 - input;       1 - output).
        /// Latch     (0 - not latched; 1 - latched).
        PIN OFFSET(0) NUMBITS(32)
    ],

    DetectMode [
        /// Select between default DETECT signal behavior and LDETECT mode.
        DETECTMODE OFFSET(0) NUMBITS(1) [
            Default = 0,
            LDetect = 1
        ]
    ],

    PinConfig [
        /// Pin direction. Same physical register as DIR register.
        DIR OFFSET(0) NUMBITS(1) [
            Input = 0,
            Output = 1
        ],
        /// Connect or disconnect input buffer.
        INPUT OFFSET(1) NUMBITS(1) [
            Connect = 0,
            Disconnect = 1
        ],
        /// Pull configuration.
        PULL OFFSET(2) NUMBITS(2) [
            Disabled = 0,
            Pulldown = 1,
            Pullup = 3
        ],
        /// Drive configuration.
        DRIVE OFFSET(8) NUMBITS(4) [
            /// Standard '0', standard '1'.
            S0S1 = 0,
            /// High drive '0', standard '1'.
            H0S1 = 1,
            /// Standard '0', high drive '1'.
            S0H1 = 2,
            /// High drive '0', high drive '1'.
            H0H1 = 3,
            /// Disconnect '0', standard '1' (normally used for wired-or connections).
            D0S1 = 4,
            /// Disconnect '0', high drive '1' (normally used for wired-or connections).
            D0H1 = 5,
            /// Standard '0', disconnect '1' (normally used for wired-and connections).
            S0D1 = 6,
            /// Standard '0', high drive '1' (normally used for wired-and connections).
            H0D1 = 7,
            /// Extra high drive '0', standard '1'.
            E0S1 = 9,
            /// Standard '0', extra high drive '1'.
            S0E1 = 10,
            /// Extra high drive '0', extra high drive '1'.
            E0E1 = 11,
            /// Disconnect '0', extra high drive '1' (normally used for wired-or connections).
            D0E1 = 13,
            /// Extra high drive '0', disconnect '1' (normally used for wired-and connections).
            E0D1 = 15
        ],
        /// Pin sensing mechanism.
        SENSE OFFSET(16) NUMBITS(2) [
            Disabled = 0,
            High = 2,
            Low = 3
        ],
        /// Select which MCU/subsystem controls this pin. Only accessible from secure code.
        MCUSEL OFFSET(28) NUMBITS(3) [
            AppMCU = 0x0,
            NetworkMCU = 0x1,
            Peripheral = 0x3,
            TND = 0x7
        ]
    ]
];

enum_from_primitive! {
    #[derive(Copy, Clone, Debug, PartialEq)]
    #[rustfmt::skip]
    pub enum Pin {
        P0_00, P0_01, P0_02, P0_03, P0_04, P0_05, P0_06, P0_07,
        P0_08, P0_09, P0_10, P0_11, P0_12, P0_13, P0_14, P0_15,
        P0_16, P0_17, P0_18, P0_19, P0_20, P0_21, P0_22, P0_23,
        P0_24, P0_25, P0_26, P0_27, P0_28, P0_29, P0_30, P0_31,
        P1_00, P1_01, P1_02, P1_03, P1_04, P1_05, P1_06, P1_07,
        P1_08, P1_09, P1_10, P1_11, P1_12, P1_13, P1_14, P1_15,
    }
}

static mut PINS: [GPIOPin; 48] = [
    GPIOPin::new(Pin::P0_00),
    GPIOPin::new(Pin::P0_01),
    GPIOPin::new(Pin::P0_02),
    GPIOPin::new(Pin::P0_03),
    GPIOPin::new(Pin::P0_04),
    GPIOPin::new(Pin::P0_05),
    GPIOPin::new(Pin::P0_06),
    GPIOPin::new(Pin::P0_07),
    GPIOPin::new(Pin::P0_08),
    GPIOPin::new(Pin::P0_09),
    GPIOPin::new(Pin::P0_10),
    GPIOPin::new(Pin::P0_11),
    GPIOPin::new(Pin::P0_12),
    GPIOPin::new(Pin::P0_13),
    GPIOPin::new(Pin::P0_14),
    GPIOPin::new(Pin::P0_15),
    GPIOPin::new(Pin::P0_16),
    GPIOPin::new(Pin::P0_17),
    GPIOPin::new(Pin::P0_18),
    GPIOPin::new(Pin::P0_19),
    GPIOPin::new(Pin::P0_20),
    GPIOPin::new(Pin::P0_21),
    GPIOPin::new(Pin::P0_22),
    GPIOPin::new(Pin::P0_23),
    GPIOPin::new(Pin::P0_24),
    GPIOPin::new(Pin::P0_25),
    GPIOPin::new(Pin::P0_26),
    GPIOPin::new(Pin::P0_27),
    GPIOPin::new(Pin::P0_28),
    GPIOPin::new(Pin::P0_29),
    GPIOPin::new(Pin::P0_30),
    GPIOPin::new(Pin::P0_31),
    GPIOPin::new(Pin::P1_00),
    GPIOPin::new(Pin::P1_01),
    GPIOPin::new(Pin::P1_02),
    GPIOPin::new(Pin::P1_03),
    GPIOPin::new(Pin::P1_04),
    GPIOPin::new(Pin::P1_05),
    GPIOPin::new(Pin::P1_06),
    GPIOPin::new(Pin::P1_07),
    GPIOPin::new(Pin::P1_08),
    GPIOPin::new(Pin::P1_09),
    GPIOPin::new(Pin::P1_10),
    GPIOPin::new(Pin::P1_11),
    GPIOPin::new(Pin::P1_12),
    GPIOPin::new(Pin::P1_13),
    GPIOPin::new(Pin::P1_14),
    GPIOPin::new(Pin::P1_15),
];

pub static mut PORT: Port = Port {
    pins: unsafe { &mut PINS },
};

pub struct GPIOPin {
    pin: u8,
    port: u8,
    client: OptionalCell<&'static dyn hil::gpio::Client>,
    gpiote_registers: StaticRef<GpioteRegisters>,
    gpio_registers: StaticRef<GpioRegisters>,
}

impl GPIOPin {
    pub const fn new(pin: Pin) -> GPIOPin {
        GPIOPin {
            pin: ((pin as usize) % GPIO_PER_PORT) as u8,
            port: ((pin as usize) / GPIO_PER_PORT) as u8,
            client: OptionalCell::empty(),
            gpiote_registers: GPIOTE0_BASE,
            gpio_registers: unsafe {
                StaticRef::new(
                    (GPIO_BASE_ADDRESS_SECURE + ((pin as usize) / GPIO_PER_PORT) * GPIO_SIZE)
                        as *const GpioRegisters,
                )
            },
        }
    }
}

impl hil::gpio::Configure for GPIOPin {
    fn set_floating_state(&self, mode: hil::gpio::FloatingState) {
        let gpio_regs = &*self.gpio_registers;
        let pin_config = match mode {
            hil::gpio::FloatingState::PullUp => PinConfig::PULL::Pullup,
            hil::gpio::FloatingState::PullDown => PinConfig::PULL::Pulldown,
            hil::gpio::FloatingState::PullNone => PinConfig::PULL::Disabled,
        };
        // PIN_CNF also holds the direction and the pin driving mode, settings we don't
        // want to overwrite!
        gpio_regs.pin_cnf[self.pin as usize].modify(pin_config);
    }

    fn floating_state(&self) -> hil::gpio::FloatingState {
        let gpio_regs = &*self.gpio_registers;
        match gpio_regs.pin_cnf[self.pin as usize].read_as_enum(PinConfig::PULL) {
            Some(PinConfig::PULL::Value::Pullup) => hil::gpio::FloatingState::PullUp,
            Some(PinConfig::PULL::Value::Pulldown) => hil::gpio::FloatingState::PullDown,
            Some(PinConfig::PULL::Value::Disabled) => hil::gpio::FloatingState::PullNone,
            None => hil::gpio::FloatingState::PullNone,
        }
    }

    fn make_output(&self) -> hil::gpio::Configuration {
        let gpio_regs = &*self.gpio_registers;
        gpio_regs.pin_cnf[self.pin as usize].modify(PinConfig::DIR::Output + PinConfig::INPUT::Disconnect);
        hil::gpio::Configuration::Output
    }

    fn disable_output(&self) -> hil::gpio::Configuration {
        let gpio_regs = &*self.gpio_registers;
        gpio_regs.pin_cnf[self.pin as usize]
            .modify(PinConfig::DIR::Input + PinConfig::INPUT::Disconnect);
        hil::gpio::Configuration::LowPower
    }

    fn make_input(&self) -> hil::gpio::Configuration {
        let gpio_regs = &*self.gpio_registers;
        gpio_regs.pin_cnf[self.pin as usize]
            .modify(PinConfig::DIR::Input + PinConfig::INPUT::Connect);
        hil::gpio::Configuration::Input
    }

    fn disable_input(&self) -> hil::gpio::Configuration {
        let gpio_regs = &*self.gpio_registers;
        gpio_regs.pin_cnf[self.pin as usize]
            .modify(PinConfig::INPUT::Disconnect);
        hil::gpio::Configuration::LowPower
    }

    fn configuration(&self) -> hil::gpio::Configuration {
        let gpio_regs = &*self.gpio_registers;
        let dir = gpio_regs.pin_cnf[self.pin as usize].read_as_enum(PinConfig::DIR);
        let connected = gpio_regs.pin_cnf[self.pin as usize].read_as_enum(PinConfig::INPUT);
        match (dir, connected) {
            (Some(PinConfig::DIR::Value::Input), Some(PinConfig::INPUT::Value::Connect)) => {
                hil::gpio::Configuration::Input
            }
            (Some(PinConfig::DIR::Value::Input), Some(PinConfig::INPUT::Value::Disconnect)) => {
                hil::gpio::Configuration::LowPower
            }
            (Some(PinConfig::DIR::Value::Output), _) => hil::gpio::Configuration::Output,
            _ => hil::gpio::Configuration::Other,
        }
    }

    fn deactivate_to_low_power(&self) {
        let gpio_regs = &*self.gpio_registers;
        gpio_regs.pin_cnf[self.pin as usize].write(
            PinConfig::DIR::Input + PinConfig::INPUT::Disconnect + PinConfig::PULL::Disabled,
        );
    }
}

impl hil::gpio::Input for GPIOPin {
    fn read(&self) -> bool {
        let gpio_regs = &*self.gpio_registers;
        gpio_regs.in_.get() & (1 << self.pin) != 0
    }
}

impl hil::gpio::Output for GPIOPin {
    fn set(&self) {
        let gpio_regs = &*self.gpio_registers;
        gpio_regs.outset.set(1 << self.pin);
    }

    fn clear(&self) {
        let gpio_regs = &*self.gpio_registers;
        gpio_regs.outclr.set(1 << self.pin);
    }

    fn toggle(&self) -> bool {
        let gpio_regs = &*self.gpio_registers;
        let result = (1 << self.pin) ^ gpio_regs.out.get();
        gpio_regs.out.set(result);
        result & (1 << self.pin) != 0
    }
}

impl hil::gpio::Pin for GPIOPin {}

impl hil::gpio::Interrupt for GPIOPin {
    fn set_client(&self, client: &'static dyn hil::gpio::Client) {
        self.client.set(client);
    }

    fn is_pending(&self) -> bool {
        if let Ok(channel) = self.find_channel(self.pin) {
            let regs = &*self.gpiote_registers;
            let ev = &regs.events_in[channel];
            ev.matches_any(Event::READY::SET)
        } else {
            false
        }
    }

    fn enable_interrupts(&self, mode: hil::gpio::InterruptEdge) {
        if let Ok(channel) = self.allocate_channel() {
            let polarity = match mode {
                hil::gpio::InterruptEdge::EitherEdge => GpioteConfig::POLARITY::Toggle,
                hil::gpio::InterruptEdge::RisingEdge => GpioteConfig::POLARITY::LoToHi,
                hil::gpio::InterruptEdge::FallingEdge => GpioteConfig::POLARITY::HiToLo,
            };
            let regs = &*self.gpiote_registers;
            let pin: u32 = (GPIO_PER_PORT as u32 * self.port as u32) + self.pin as u32;
            regs.config[channel].write(GpioteConfig::MODE::Event + GpioteConfig::PSEL.val(pin) + polarity);
            regs.intenset.set(1 << channel);
        } else {
            debug!("No available GPIOTE interrupt channels");
        }
    }

    fn disable_interrupts(&self) {
        if let Ok(channel) = self.find_channel(self.pin) {
            let regs = &*self.gpiote_registers;
            regs.config[channel]
                .write(GpioteConfig::MODE::CLEAR + GpioteConfig::PSEL::CLEAR + GpioteConfig::POLARITY::CLEAR);
            regs.intenclr.set(1 << channel);
        }
    }
}

impl hil::gpio::InterruptPin for GPIOPin {}

impl GPIOPin {
    /// Allocate a GPIOTE channel
    /// If the channel couldn't be allocated return error instead
    fn allocate_channel(&self) -> Result<usize, ()> {
        let regs = &*self.gpiote_registers;
        for (i, ch) in regs.config.iter().enumerate() {
            if ch.matches_all(GpioteConfig::MODE::Disabled) {
                return Ok(i);
            }
        }
        Err(())
    }

    /// Return which channel is allocated to a pin,
    /// If the channel is not found return an error instead
    fn find_channel(&self, pin: u8) -> Result<usize, ()> {
        let regs = &*self.gpiote_registers;
        for (i, ch) in regs.config.iter().enumerate() {
            let encoded_pin = (GPIO_PER_PORT as u32 * self.port as u32) + pin as u32;
            if ch.matches_all(GpioteConfig::PSEL.val(encoded_pin)) {
                return Ok(i);
            }
        }
        Err(())
    }

    fn handle_interrupt(&self) {
        self.client.map(|client| {
            client.fired();
        });
    }
}

pub struct Port {
    pub pins: &'static mut [GPIOPin],
}

impl Index<Pin> for Port {
    type Output = GPIOPin;

    fn index(&self, index: Pin) -> &GPIOPin {
        &self.pins[index as usize]
    }
}

impl IndexMut<Pin> for Port {
    fn index_mut(&mut self, index: Pin) -> &mut GPIOPin {
        &mut self.pins[index as usize]
    }
}

impl Port {
    /// GPIOTE interrupt: check each GPIOTE channel, if any has
    /// fired then trigger its corresponding pin's interrupt handler.
    pub fn handle_interrupt(&self) {
        // do this just to get a pointer the memory map
        // doesn't matter which pin is used because it is the same
        let regs = &*self.pins[0].gpiote_registers;

        for (i, ev) in regs.events_in.iter().enumerate() {
            if ev.matches_any(Event::READY::SET) {
                ev.write(Event::READY::SET);
                // Get pin number for the event and `trigger` an interrupt manually on that pin
                let pin = regs.config[i].read(GpioteConfig::PSEL) as usize;
                self.pins[pin].handle_interrupt();
            }
        }
    }
}
