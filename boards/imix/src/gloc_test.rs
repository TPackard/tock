//! Enables the glue logic controller (GLOC) and allows it to be manually
//! tested.  The look up table is set to act as a 4-way XOR between the inputs.
//! The four inputs are pins D3, D4, D5, and RX3; however the input on RX3 is
//! disabled and should not affect the output.  The GLOC output pin is D2, and
//! it must be wired to GPIO pin D7 so that the test can read the output value.
//! The test will check the GLOC output once a second, print the value to the
//! console, and turn on the LED if the output is high.

use capsules::virtual_alarm::{MuxAlarm, VirtualMuxAlarm};
use core::cell::Cell;
use core::marker::PhantomData;
use kernel::debug;
use kernel::hil::gpio::Configure;
use kernel::hil::time::{Alarm, AlarmClient, Frequency};
use kernel::static_init;
use sam4l::ast::Ast;
use sam4l::gloc::{self, Lut, GLOC};
use sam4l::gpio;

const TRUTH_TABLE: u16 = 0b0110_1001_1001_0110;

pub unsafe fn run(mux_alarm: &'static MuxAlarm<'static, Ast>) {
    let gloc_test = static_init!(
        GlocTest<'static, VirtualMuxAlarm<'static, Ast>>,
        GlocTest::new(VirtualMuxAlarm::new(mux_alarm))
    );
    gloc_test.alarm.set_client(gloc_test);

    gloc_test.run();
}

#[derive(Copy, Clone)]
enum GlocTestState {
    Initialization,
    Test,
}

struct GlocTest<'a, A: Alarm<'a>> {
    alarm: A,
    state: Cell<GlocTestState>,
    phantom: PhantomData<&'a A>,
}

impl<'a, A: Alarm<'a>> GlocTest<'a, A> {
    pub fn new(alarm: A) -> GlocTest<'a, A> {
        GlocTest {
            alarm,
            state: Cell::new(GlocTestState::Initialization),
            phantom: PhantomData,
        }
    }

    pub unsafe fn run(&self) {
        // GLOC pins.
        let gloc_in4 = &gpio::PC[28]; // D5
        let gloc_in5 = &gpio::PC[29]; // D4
        let gloc_in6 = &gpio::PC[30]; // D3
        let gloc_in7 = &gpio::PB[09]; // RX3
        let gloc_out1 = &gpio::PC[31]; // D2

        // Pin to read GLOC output from (should be wired to gloc_out1).
        let out = &gpio::PC[26]; //D7

        // Pin connected to LED.
        let led = &gpio::PC[10];

        match self.state.get() {
            GlocTestState::Initialization => {
                // Set up GLOC pins.
                gloc_in4.select_peripheral(gpio::PeripheralFunction::C);
                gloc_in5.select_peripheral(gpio::PeripheralFunction::C);
                gloc_in6.select_peripheral(gpio::PeripheralFunction::C);
                gloc_in7.select_peripheral(gpio::PeripheralFunction::C);
                gloc_out1.select_peripheral(gpio::PeripheralFunction::C);
                out.make_input();
                led.make_output();
                led.clear();

                // Enable and configure GLOC.
                GLOC.enable();
                GLOC.configure_lut(Lut::Lut1, TRUTH_TABLE);
                GLOC.enable_lut_inputs(Lut::Lut1, gloc::IN4 | gloc::IN5 | gloc::IN6 | gloc::IN7);
                GLOC.disable_lut_inputs(Lut::Lut1, gloc::IN7);
                GLOC.enable_lut_filter(Lut::Lut1);

                self.state.set(GlocTestState::Test);
                self.wait(1);
            }
            GlocTestState::Test => {
                // Print GLOC output once a second. Turn on LED if output is high.
                if out.read() {
                    debug!("GLOC OUT1: 1");
                    led.set();
                } else {
                    debug!("GLOC OUT1: 0");
                    led.clear();
                }

                self.wait(1);
            }
        }
    }

    fn wait(&self, secs: u32) {
        let interval = secs * <A::Frequency>::frequency();
        let tics = self.alarm.now().wrapping_add(interval);
        self.alarm.set_alarm(tics);
    }
}

impl<'a, A: Alarm<'a>> AlarmClient for GlocTest<'a, A> {
    fn fired(&self) {
        unsafe {
            self.run();
        }
    }
}
