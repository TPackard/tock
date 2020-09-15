//! Tock kernel for the Nordic Semiconductor nRF5340 preview development kit (PDK).
//!
//! It is based on nRF5340 SoC (dual Cortex M33 cores with a BLE transceiver) with
//! many exported I/O and peripherals.
//!
//! Pin Configuration
//! -------------------
//!
//! ### `GPIO`
//!
//! | #  | Pin   | Ix | Header | Arduino |
//! |----|-------|----|--------|---------|
//! | 0  | P1.01 | 33 | P3 1   | D0      |
//! | 1  | P1.02 | 34 | P3 2   | D1      |
//! | 2  | P1.03 | 35 | P3 3   | D2      |
//! | 3  | P1.04 | 36 | P3 4   | D3      |
//! | 4  | P1.05 | 37 | P3 5   | D4      |
//! | 5  | P1.06 | 38 | P3 6   | D5      |
//! | 6  | P1.07 | 39 | P3 7   | D6      |
//! | 7  | P1.08 | 40 | P3 8   | D7      |
//! | 8  | P1.10 | 42 | P4 1   | D8      |
//! | 9  | P1.11 | 43 | P4 2   | D9      |
//! | 10 | P1.12 | 44 | P4 3   | D10     |
//! | 11 | P1.13 | 45 | P4 4   | D11     |
//! | 12 | P1.14 | 46 | P4 5   | D12     |
//! | 13 | P1.15 | 47 | P4 6   | D13     |
//! | 14 | P0.26 | 26 | P4 9   | D14     |
//! | 15 | P0.27 | 27 | P4 10  | D15     |
//!
//! ### `GPIO` / Analog Inputs
//!
//! | #  | Pin        | Header | Arduino |
//! |----|------------|--------|---------|
//! |    | P0.04 AIN1 | P2 1   | A0      |
//! |    | P0.05 AIN2 | P2 2   | A1      |
//! |    | P0.06 AIN4 | P2 3   | A2      |
//! |    | P0.07 AIN5 | P2 4   | A3      |
//! |    | P0.25 AIN6 | P2 5   | A4      |
//! |    | P0.26 AIN7 | P2 6   | A5      |
//!
//! ### Onboard Functions
//!
//! | Pin   | Header | Function |
//! |-------|--------|----------|
//! | P0.08 | P6  6  | Button 3 |
//! | P0.09 | P6  7  | Button 4 |
//! | P0.13 | P24 3  | SPI MOSI |
//! | P0.14 | P24 4  | SPI MISO |
//! | P0.17 | P24 7  | SPI CLK  |
//! | P0.19 | P24 9  | UART RTS |
//! | P0.20 | P24 10 | UART TXD |
//! | P0.21 | P24 11 | UART CTS |
//! | P0.22 | P24 12 | UART RXD |
//! | P0.23 | P24 13 | Button 1 |
//! | P0.24 | P24 14 | Button 2 |
//! | P0.28 | P24 15 | LED 1    |
//! | P0.29 | P24 16 | LED 2    |
//! | P0.30 | P24 17 | LED 3    |
//! | P0.31 | P24 18 | LED 4    |

#![no_std]
#![no_main]
#![deny(missing_docs)]

#[allow(unused_imports)]
use kernel::{create_capability, debug, debug_gpio, debug_verbose, static_init};

use kernel::capabilities;
use kernel::common::dynamic_deferred_call::{DynamicDeferredCall, DynamicDeferredCallClientState};
use kernel::component::Component;
use nrf53::gpio::Pin;

// The nRF5340PDK LEDs (see back of board)
const LED1_PIN: Pin = Pin::P0_28;
const LED2_PIN: Pin = Pin::P0_29;
const LED3_PIN: Pin = Pin::P0_30;
const LED4_PIN: Pin = Pin::P0_31;

// The nRF5340PDK buttons (see back of board)
const BUTTON1_PIN: Pin = Pin::P0_23;
const BUTTON2_PIN: Pin = Pin::P0_24;
const BUTTON3_PIN: Pin = Pin::P0_08;
const BUTTON4_PIN: Pin = Pin::P0_09;

const UART_RTS: Option<Pin> = Some(Pin::P0_19);
const UART_TXD: Pin = Pin::P0_20;
const UART_CTS: Option<Pin> = Some(Pin::P0_21);
const UART_RXD: Pin = Pin::P0_22;

#[allow(dead_code)]
const SPI_MOSI: Pin = Pin::P0_13;
#[allow(dead_code)]
const SPI_MISO: Pin = Pin::P0_14;
#[allow(dead_code)]
const SPI_CLK: Pin = Pin::P0_17;

#[allow(dead_code)]
const SPI_MX25R6435F_CHIP_SELECT: Pin = Pin::P0_18;
#[allow(dead_code)]
const SPI_MX25R6435F_WRITE_PROTECT_PIN: Pin = Pin::P0_15;
#[allow(dead_code)]
const SPI_MX25R6435F_HOLD_PIN: Pin = Pin::P0_16;

/// Debug Writer
pub mod io;

/// Tests
#[allow(dead_code)]
mod tests;

// Whether to use UART debugging or Segger RTT (USB) debugging.
// - Set to false to use UART.
// - Set to true to use Segger RTT over USB.
const USB_DEBUGGING: bool = false;

// State for loading and holding applications.
// How should the kernel respond when a process faults.
const FAULT_RESPONSE: kernel::procs::FaultResponse = kernel::procs::FaultResponse::Panic;

// Number of concurrent processes this platform supports.
const NUM_PROCS: usize = 8;

#[link_section = ".app_memory"]
static mut APP_MEMORY: [u8; 0x3C000] = [0; 0x3C000];

static mut PROCESSES: [Option<&'static dyn kernel::procs::ProcessType>; NUM_PROCS] =
    [None, None, None, None, None, None, None, None];

static mut CHIP: Option<&'static nrf53::chip::NRF53> = None;

/// Dummy buffer that causes the linker to reserve enough space for the stack.
#[no_mangle]
#[link_section = ".stack_buffer"]
pub static mut STACK_MEMORY: [u8; 0x1000] = [0; 0x1000];

struct Platform {
    alarm: &'static capsules::alarm::AlarmDriver<
        'static,
        capsules::virtual_alarm::VirtualMuxAlarm<'static, nrf53::rtc::Rtc<'static>>,
    >,
    button: &'static capsules::button::Button<'static>,
    gpio: &'static capsules::gpio::GPIO<'static>,
    led: &'static capsules::led::LED<'static>,
    pconsole: &'static capsules::process_console::ProcessConsole<
        'static,
        components::process_console::Capability,
    >,
    console: &'static capsules::console::Console<'static>,
}

impl kernel::Platform for Platform {
    fn with_driver<F, R>(&self, driver_num: usize, f: F) -> R
    where
        F: FnOnce(Option<&dyn kernel::Driver>) -> R,
    {
        match driver_num {
            capsules::alarm::DRIVER_NUM => f(Some(self.alarm)),
            capsules::button::DRIVER_NUM => f(Some(self.button)),
            capsules::gpio::DRIVER_NUM => f(Some(self.gpio)),
            capsules::led::DRIVER_NUM => f(Some(self.led)),
            capsules::console::DRIVER_NUM => f(Some(self.console)),
            _ => f(None),
        }
    }
}

/// Entry point in the vector table called on hard reset.
#[no_mangle]
pub unsafe fn reset_handler() {
    // Loads relocations and clears BSS
    nrf53::init();

    let board_kernel = static_init!(kernel::Kernel, kernel::Kernel::new(&PROCESSES));
    let gpio = components::gpio::GpioComponent::new(board_kernel).finalize(
        components::gpio_component_helper!(
            &nrf53::gpio::PORT[Pin::P1_01],
            &nrf53::gpio::PORT[Pin::P1_04],
            &nrf53::gpio::PORT[Pin::P1_05],
            &nrf53::gpio::PORT[Pin::P1_06],
            &nrf53::gpio::PORT[Pin::P1_07],
            &nrf53::gpio::PORT[Pin::P1_08],
            &nrf53::gpio::PORT[Pin::P1_09],
            &nrf53::gpio::PORT[Pin::P1_10],
            &nrf53::gpio::PORT[Pin::P1_11],
            &nrf53::gpio::PORT[Pin::P1_12],
            &nrf53::gpio::PORT[Pin::P1_13],
            &nrf53::gpio::PORT[Pin::P1_14],
            &nrf53::gpio::PORT[Pin::P1_15]
        ),
    );

    let button = components::button::ButtonComponent::new(board_kernel).finalize(
        components::button_component_helper!(
            (
                &nrf53::gpio::PORT[BUTTON1_PIN],
                kernel::hil::gpio::ActivationMode::ActiveLow,
                kernel::hil::gpio::FloatingState::PullUp
            ), //13
            (
                &nrf53::gpio::PORT[BUTTON2_PIN],
                kernel::hil::gpio::ActivationMode::ActiveLow,
                kernel::hil::gpio::FloatingState::PullUp
            ), //14
            (
                &nrf53::gpio::PORT[BUTTON3_PIN],
                kernel::hil::gpio::ActivationMode::ActiveLow,
                kernel::hil::gpio::FloatingState::PullUp
            ), //15
            (
                &nrf53::gpio::PORT[BUTTON4_PIN],
                kernel::hil::gpio::ActivationMode::ActiveLow,
                kernel::hil::gpio::FloatingState::PullUp
            ) //16
        ),
    );

    let led = components::led::LedsComponent::new().finalize(components::led_component_helper!(
        (
            &nrf53::gpio::PORT[LED1_PIN],
            kernel::hil::gpio::ActivationMode::ActiveLow
        ),
        (
            &nrf53::gpio::PORT[LED2_PIN],
            kernel::hil::gpio::ActivationMode::ActiveLow
        ),
        (
            &nrf53::gpio::PORT[LED3_PIN],
            kernel::hil::gpio::ActivationMode::ActiveLow
        ),
        (
            &nrf53::gpio::PORT[LED4_PIN],
            kernel::hil::gpio::ActivationMode::ActiveLow
        )
    ));

    let chip = static_init!(nrf53::chip::NRF53, nrf53::chip::NRF53::new());
    CHIP = Some(chip);

    // Create capabilities that the board needs to call certain protected kernel
    // functions.
    let process_management_capability =
        create_capability!(capabilities::ProcessManagementCapability);
    let main_loop_capability = create_capability!(capabilities::MainLoopCapability);

    // Configure kernel debug gpios as early as possible
    kernel::debug::assign_gpios(
        Some(&nrf53::gpio::PORT[LED1_PIN]),
        Some(&nrf53::gpio::PORT[LED2_PIN]),
        Some(&nrf53::gpio::PORT[LED3_PIN]),
    );

    let dynamic_deferred_call_clients =
        static_init!([DynamicDeferredCallClientState; 2], Default::default());
    let dynamic_deferred_caller = static_init!(
        DynamicDeferredCall,
        DynamicDeferredCall::new(dynamic_deferred_call_clients)
    );
    DynamicDeferredCall::set_global_instance(dynamic_deferred_caller);

    let rtc = &nrf53::rtc::RTC;
    rtc.start();
    let mux_alarm = components::alarm::AlarmMuxComponent::new(rtc)
        .finalize(components::alarm_mux_component_helper!(nrf53::rtc::Rtc));
    let alarm = components::alarm::AlarmDriverComponent::new(board_kernel, mux_alarm)
        .finalize(components::alarm_component_helper!(nrf53::rtc::Rtc));

    let uart_channel: &dyn kernel::hil::uart::Uart = if USB_DEBUGGING {
        let mut rtt_memory_refs =
            components::segger_rtt::SeggerRttMemoryComponent::new().finalize(());

        // XXX: This is inherently unsafe as it aliases the mutable reference to rtt_memory. This
        // aliases reference is only used inside a panic handler, which should be OK, but maybe we
        // should use a const reference to rtt_memory and leverage interior mutability instead.
        self::io::set_rtt_memory(&mut *rtt_memory_refs.get_rtt_memory_ptr());

        let rtt = components::segger_rtt::SeggerRttComponent::new(mux_alarm, rtt_memory_refs)
            .finalize(components::segger_rtt_component_helper!(nrf53::rtc::Rtc));
        rtt
    } else {
        nrf53::uart::UARTE0.initialize(
            nrf53::pinmux::Pinmux::new(UART_TXD as u32),
            nrf53::pinmux::Pinmux::new(UART_RXD as u32),
            UART_CTS.map(|x| nrf53::pinmux::Pinmux::new(x as u32)),
            UART_RTS.map(|x| nrf53::pinmux::Pinmux::new(x as u32)),
        );
        &nrf53::uart::UARTE0
    };

    // Create a shared UART channel for the console and for kernel debug.
    let uart_mux =
        components::console::UartMuxComponent::new(uart_channel, 115200, dynamic_deferred_caller)
            .finalize(());

    let pconsole =
        components::process_console::ProcessConsoleComponent::new(board_kernel, uart_mux)
            .finalize(());

    // Setup the console.
    let console = components::console::ConsoleComponent::new(board_kernel, uart_mux).finalize(());
    // Create the debugger object that handles calls to `debug!()`.
    components::debug_writer::DebugWriterComponent::new(uart_mux).finalize(());

    // Start all of the clocks. Low power operation will require a better
    // approach than this.
    nrf53::clock::CLOCK.low_stop();
    nrf53::clock::CLOCK.high_stop();

    nrf53::clock::CLOCK.low_set_source(nrf53::clock::LowClockSource::LFXO);
    nrf53::clock::CLOCK.low_start();
    nrf53::clock::CLOCK.high_set_source(nrf53::clock::HighClockSource::HFXO);
    nrf53::clock::CLOCK.high_start();
    while !nrf53::clock::CLOCK.low_started() {}
    while !nrf53::clock::CLOCK.high_started() {}

    let platform = Platform {
        alarm,
        button,
        gpio,
        led,
        pconsole: pconsole,
        console: console,
    };

    platform.pconsole.start();
    debug!("Initialization complete. Entering main loop\r");

    // Run optional kernel tests.
    //
    tests::blink::run();

    extern "C" {
        /// Beginning of the ROM region containing app images.
        static _sapps: u8;

        /// End of the ROM region containing app images.
        ///
        /// This symbol is defined in the linker script.
        static _eapps: u8;
    }
    kernel::procs::load_processes(
        board_kernel,
        chip,
        core::slice::from_raw_parts(
            &_sapps as *const u8,
            &_eapps as *const u8 as usize - &_sapps as *const u8 as usize,
        ),
        &mut APP_MEMORY,
        &mut PROCESSES,
        FAULT_RESPONSE,
        &process_management_capability,
    )
    .unwrap_or_else(|err| {
        debug!("Error loading processes!");
        debug!("{:?}", err);
    });

    board_kernel.kernel_loop(&platform, chip, None, &main_loop_capability);
}
