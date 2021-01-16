use capsules::virtual_alarm::{MuxAlarm, VirtualMuxAlarm};
use kernel::debug;
use kernel::hil::gpio::{Client, Configure, Interrupt, InterruptEdge, Output};
use kernel::hil::time::{Alarm, AlarmClient, Frequency};
use kernel::static_init;
use nrf53::gpio;
use nrf53::rtc::Rtc;

const WAIT_MS: u32 = 500;

pub unsafe fn run(
    mux_alarm: &'static MuxAlarm<'static, Rtc>,
    led_pin: gpio::Pin,
    button_pin: gpio::Pin,
) {
    let blink = static_init!(
        Blink<VirtualMuxAlarm<'static, Rtc>>,
        Blink::new(VirtualMuxAlarm::new(mux_alarm), led_pin, button_pin)
    );

    blink.alarm.set_client(blink);
    blink.button.set_client(blink);
    blink.run();
}

struct Blink<A: Alarm<'static>> {
    alarm: A,
    led: &'static gpio::GPIOPin<'static>,
    button: &'static gpio::GPIOPin<'static>,
}

impl<A: Alarm<'static>> Blink<A> {
    fn new(alarm: A, led_pin: gpio::Pin, button_pin: gpio::Pin) -> Blink<A> {
        let led = unsafe { &gpio::PORT_APP[led_pin] };
        let button = unsafe { &gpio::PORT_APP[button_pin] };

        led.make_output();
        button.make_input();
        button.enable_interrupts(InterruptEdge::FallingEdge);

        Blink {
            alarm,
            led,
            button,
        }
    }

    fn run(&self) {
        self.led.toggle();

        let interval = WAIT_MS * <A::Frequency>::frequency() / 1000;
        let tics = self.alarm.now().wrapping_add(interval);
        self.alarm.set_alarm(tics);
    }
}

impl<A: Alarm<'static>> AlarmClient for Blink<A> {
    fn fired(&self) {
        debug!("blink");
        self.run();
    }
}

impl<A: Alarm<'static>> Client for Blink<A> {
    fn fired(&self) {
        debug!("CLICK");
        self.run();
    }
}
