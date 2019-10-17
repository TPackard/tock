//! Enables the glue logic controller (GLOC) and allows it to be manually
//! tested.  The first look up table is configured as a 4-way xor of its inputs,
//! and the second table is configured as a 4-way xnor.  The four inputs for the
//! first lookup table are EXT, A0, A1, and A2.  The output pin is RF233 IRQ,
//! which should be wired to GPIO pin D6.  The inputs for the second table are
//! D5, D4, D3, and RX3.  Its output pin is D2, which should be wired to GPIO
//! pin D7.  The test will check the GLOC output once a second, print the value
//! to the console, and turn on the LED to reflect the xor of the two tables'
//! outputs.
//!
//! To run the test, first disconnect the RF233 power jumper and add the
//! following line to the imix boot sequence:
//! ```
//!     gloc_test::run(mux_alarm);
//! ```
//! You should then see the states of the GLOC output pins printed to the
//! console and the user LED turned on and off appropriately.


use capsules::virtual_alarm::{MuxAlarm, VirtualMuxAlarm};
use kernel::debug;
use kernel::hil::gpio::Configure;
use kernel::hil::time::{Alarm, AlarmClient, Frequency};
use kernel::static_init;
use sam4l::ast::Ast;
use sam4l::gloc::{self, Lut, GLOC};
use sam4l::gpio;

const TRUTH_TABLE_XOR: u16 = 0b0110_1001_1001_0110;
const TRUTH_TABLE_XNOR: u16 = 0b1001_0110_0110_1001;

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
    gloc_in0: &'a gpio::GPIOPin,
    gloc_in1: &'a gpio::GPIOPin,
    gloc_in2: &'a gpio::GPIOPin,
    gloc_in3: &'a gpio::GPIOPin,
    gloc_out0: &'a gpio::GPIOPin,
    gloc_in4: &'a gpio::GPIOPin,
    gloc_in5: &'a gpio::GPIOPin,
    gloc_in6: &'a gpio::GPIOPin,
    gloc_in7: &'a gpio::GPIOPin,
    gloc_out1: &'a gpio::GPIOPin,
    out0: &'a gpio::GPIOPin, // Pins to read GLOC output from (should be wired to
    out1: &'a gpio::GPIOPin, // gloc_out0 and gloc_out1, respectively).
    led: &'a gpio::GPIOPin,
}

impl<'a, A: Alarm<'a>> GlocTest<'a, A> {
    pub unsafe fn new(alarm: A) -> GlocTest<'a, A> {
        GlocTest {
            alarm,
            gloc_in0: &gpio::PA[06],  // EXT
            gloc_in1: &gpio::PA[04],  // A0
            gloc_in2: &gpio::PA[05],  // A1
            gloc_in3: &gpio::PA[07],  // A2
            gloc_out0: &gpio::PA[08], // RF233 IRQ
            gloc_in4: &gpio::PC[28],  // D5
            gloc_in5: &gpio::PC[29],  // D4
            gloc_in6: &gpio::PC[30],  // D3
            gloc_in7: &gpio::PB[09],  // RX3
            gloc_out1: &gpio::PC[31], // D2
            out0: &gpio::PC[27],      //D6
            out1: &gpio::PC[26],      //D7
            led: &gpio::PC[10],
        }
    }

    pub unsafe fn run(&self) {
        // Set up GLOC pins.
        self.gloc_in0.select_peripheral(gpio::PeripheralFunction::D);
        self.gloc_in1.select_peripheral(gpio::PeripheralFunction::D);
        self.gloc_in2.select_peripheral(gpio::PeripheralFunction::D);
        self.gloc_in3.select_peripheral(gpio::PeripheralFunction::D);
        self.gloc_out0
            .select_peripheral(gpio::PeripheralFunction::D);
        self.gloc_in4.select_peripheral(gpio::PeripheralFunction::C);
        self.gloc_in5.select_peripheral(gpio::PeripheralFunction::C);
        self.gloc_in6.select_peripheral(gpio::PeripheralFunction::C);
        self.gloc_in7.select_peripheral(gpio::PeripheralFunction::C);
        self.gloc_out1
            .select_peripheral(gpio::PeripheralFunction::C);
        self.out0.make_input();
        self.out1.make_input();
        self.led.make_output();
        self.led.clear();

        // Enable and configure GLOC.
        GLOC.enable();

        // Enable LUT0, set to 4-way xor, and enable output filtering.
        GLOC.configure_lut(Lut::Lut0, TRUTH_TABLE_XOR);
        GLOC.enable_lut_inputs(
            Lut::Lut0,
            gloc::IN_0_4 | gloc::IN_1_5 | gloc::IN_2_6 | gloc::IN_3_7,
        );
        GLOC.enable_lut_filter(Lut::Lut0);

        // Enable LUT1, set to 4-way xnor, and disable 4th input pin.
        GLOC.configure_lut(Lut::Lut1, TRUTH_TABLE_XNOR);
        GLOC.enable_lut_inputs(
            Lut::Lut1,
            gloc::IN_0_4 | gloc::IN_1_5 | gloc::IN_2_6 | gloc::IN_3_7,
        );
        GLOC.disable_lut_inputs(Lut::Lut1, gloc::IN_3_7);

        self.wait(1);
    }

    fn sample(&self) {
        // Print GLOC output once a second.
        debug!(
            "GLOC (OUT0: {}, OUT1: {})",
            pin_state_str(self.out0),
            pin_state_str(self.out1)
        );

        // Turn on output if the xor of the two outputs is true.
        if self.out0.read() ^ self.out1.read() {
            self.led.set();
        } else {
            self.led.clear();
        }

        self.wait(1);
    }

    fn wait(&self, secs: u32) {
        let interval = secs * <A::Frequency>::frequency();
        let tics = self.alarm.now().wrapping_add(interval);
        self.alarm.set_alarm(tics);
    }
}

impl<'a, A: Alarm<'a>> AlarmClient for GlocTest<'a, A> {
    fn fired(&self) {
        self.sample();
    }
}

fn pin_state_str(pin: &gpio::GPIOPin) -> &str {
    match pin.read() {
        true => "high",
        false => "low",
    }
}
