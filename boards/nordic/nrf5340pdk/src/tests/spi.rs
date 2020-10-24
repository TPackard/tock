use capsules::virtual_spi::{MuxSpiMaster, VirtualSpiMasterDevice};
use kernel::common::cells::TakeCell;
use kernel::debug;
use kernel::hil::gpio::{Client, Configure, Interrupt, InterruptEdge, Output};
use kernel::hil::spi::{SpiMasterClient, SpiMasterDevice};
use kernel::static_init;
use nrf53::gpio;
use nrf53::spi::SPIM;
use core::cell::Cell;

const WAIT_MS: u32 = 500;
static mut BUFFER: [u8; 64] = [0; 64];

pub unsafe fn run(
    mux_spi: &'static MuxSpiMaster<'static, SPIM>,
    spi_chip_select: &'static gpio::GPIOPin,
    led_pin: gpio::Pin,
    button_pin: gpio::Pin,
) {
    let spi_test = static_init!(
        SpiTest,
        SpiTest::new(
            VirtualSpiMasterDevice::new(mux_spi, spi_chip_select),
            &mut BUFFER,
            led_pin,
            button_pin
        )
    );

    BUFFER[0] = 'y' as u8;
    BUFFER[1] = 'e' as u8;
    BUFFER[2] = 'e' as u8;
    BUFFER[3] = 't' as u8;

    spi_test.spim.set_client(spi_test);
    spi_test.button.set_client(spi_test);
}

struct SpiTest {
    spim: VirtualSpiMasterDevice<'static, SPIM>,
    write_buffer: TakeCell<'static, [u8]>,
    led: &'static gpio::GPIOPin<'static>,
    button: &'static gpio::GPIOPin<'static>,
    ready: Cell<bool>,
}

impl SpiTest {
    fn new(
        spim: VirtualSpiMasterDevice<'static, SPIM>,
        write_buffer: &'static mut [u8],
        led_pin: gpio::Pin,
        button_pin: gpio::Pin
    ) -> SpiTest {
        let led = unsafe { &gpio::PORT[led_pin] };
        let button = unsafe { &gpio::PORT[button_pin] };

        led.make_output();
        button.make_input();
        button.enable_interrupts(InterruptEdge::FallingEdge);

        SpiTest {
            spim,
            write_buffer: TakeCell::new(write_buffer),
            led,
            button,
            ready: Cell::new(true),
        }
    }

    fn run(&self) {
        debug!("BLINK");

        self.ready.set(false);
        self.led.toggle();
        self.write_buffer.take()
            .map(move |write_buffer| {
                self.spim.read_write_bytes(write_buffer, None, 4);
            })
            .unwrap();
    }
}

impl SpiMasterClient for SpiTest {
    fn read_write_done(
        &self,
        write_buffer: &'static mut [u8],
        _read_buffer: Option<&'static mut [u8]>,
        _len: usize,
    ) {
        self.write_buffer.replace(write_buffer);
        self.ready.set(true);
    }
}

impl Client for SpiTest {
    fn fired(&self) {
        if self.ready.get() {
            self.run();
        }
    }
}

