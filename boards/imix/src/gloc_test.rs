//! Tests the glue logic controller (GLOC) and affirms that each lookup table
//! works as expected.

use core::cell::Cell;
use kernel::debug;
use kernel::static_init;
use kernel::hil::gpio::Configure;
use kernel::hil::time::{Alarm, AlarmClient, Frequency};
use sam4l::ast::{Ast, AST};
use sam4l::gloc::{self, GLOC, Lut};
use sam4l::gpio;

pub unsafe fn run() {
    let alarm = &AST;
    let test = static_init!(GlocTest<'static, Ast<'static>>, GlocTest::new(alarm));
    alarm.set_client(test);
    test.run();
}

#[derive(Copy, Clone)]
enum GlocTestState {
    Initialization,
    Test,
}

struct GlocTest<'a, A: 'a + Alarm<'a>> {
    alarm: &'a A,
    state: Cell<GlocTestState>
}

impl<'a, A: Alarm<'a>> GlocTest<'a, A> {
    pub fn new(alarm: &'a A) -> GlocTest<'a, A> {
        GlocTest {
            alarm,
            state: Cell::new(GlocTestState::Initialization)
        }
    }

    pub unsafe fn run(&self) {
        // GLOC pins.
        let gloc_in4 = &gpio::PC[28];  // D5
        let gloc_in5 = &gpio::PC[29];  // D4
        let gloc_in6 = &gpio::PC[30];  // D3
        let gloc_in7 = &gpio::PB[09];  // RX3
        let gloc_out1 = &gpio::PC[31]; // D2

        // Pin to read GLOC output from (wired to gloc_out1).
        let out = &gpio::PC[26];  //D7

        // Pin connected to LED.
        let led = &gpio::PC[10];

        match self.state.get() {
            GlocTestState::Initialization => {
                // Set up GLOC.
                gloc_in4.select_peripheral(gpio::PeripheralFunction::C);
                gloc_in5.select_peripheral(gpio::PeripheralFunction::C);
                gloc_in6.select_peripheral(gpio::PeripheralFunction::C);
                gloc_in7.select_peripheral(gpio::PeripheralFunction::C);
                gloc_out1.select_peripheral(gpio::PeripheralFunction::C);
                out.make_input();
                led.make_output();
                led.clear();

                GLOC.enable();
                GLOC.configure_lut(Lut::Lut2, 0b0110_1001_1001_0110);
                GLOC.enable_lut_inputs(Lut::Lut2, gloc::IN0 | gloc::IN1 | gloc::IN2 | gloc::IN3);
                GLOC.disable_lut_inputs(Lut::Lut2, gloc::IN3);
                //GLOC.enable_lut_filter(0);

                self.state.set(GlocTestState::Test);
                self.wait(1);
            },
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
        unsafe { self.run(); }
    }
}
