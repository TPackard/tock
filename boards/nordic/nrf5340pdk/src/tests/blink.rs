use kernel::debug;
use kernel::hil::gpio::{Configure, Output};
use nrf53::gpio;

const WAIT_ITER: u32 = 1000000;

pub unsafe fn run() {
    let led3 = &gpio::PORT[gpio::Pin::P0_30];
    let led4 = &gpio::PORT[gpio::Pin::P0_31];
    led3.make_output();
    led4.make_output();

    led3.clear();
    
    loop {
        for _ in 0..WAIT_ITER {
            led4.clear();
        }

        for _ in 0..WAIT_ITER {
            led4.set();
        }

        debug!("blink");
    }
}
