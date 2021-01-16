use kernel::common::cells::TakeCell;
use kernel::debug;
use kernel::hil::gpio::{Configure, FloatingState, Interrupt, InterruptEdge, Output};
use kernel::hil::spi::{SpiSlave, SpiSlaveClient};
use kernel::static_init;
use nrf53::gpio;
use nrf53::spi::SPI;

static mut WRITE_BUFFER: [u8; 4] = [0; 4];
static mut READ_BUFFER: [u8; 4] = [0; 4];

pub unsafe fn run(
    mosi: gpio::Pin,
    miso: gpio::Pin,
    sck: gpio::Pin,
    csn: gpio::Pin,
    led1_pin: gpio::Pin,
    led2_pin: gpio::Pin,
    led3_pin: gpio::Pin,
    led4_pin: gpio::Pin,
) {
    nrf53::gpio::PORT_APP[mosi].make_input();
    nrf53::gpio::PORT_APP[sck].make_input();
    nrf53::gpio::PORT_APP[csn].make_input();
    nrf53::gpio::PORT_APP[csn].set_floating_state(FloatingState::PullUp);
    nrf53::gpio::PORT_APP[csn].enable_interrupts(InterruptEdge::FallingEdge);
    nrf53::gpio::PORT_APP[csn].set_client(&nrf53::spi::SPI1_APP);

    nrf53::spi::SPI1_APP.configure(
        nrf53::pinmux::Pinmux::new(mosi as u32),
        nrf53::pinmux::Pinmux::new(miso as u32),
        nrf53::pinmux::Pinmux::new(sck as u32),
        nrf53::pinmux::Pinmux::new(csn as u32),
        nrf53::spi::SpiRole::SPIS,
    );

    let spi_test = static_init!(
        SpiTest,
        SpiTest::new(
            &nrf53::spi::SPI1_APP,
            &mut WRITE_BUFFER,
            &mut READ_BUFFER,
            led1_pin,
            led2_pin,
            led3_pin,
            led4_pin,
        )
    );

    READ_BUFFER[0] = 1;
    READ_BUFFER[1] = 2;
    READ_BUFFER[2] = 3;
    READ_BUFFER[3] = 4;

    WRITE_BUFFER[0] = 'A' as u8;
    WRITE_BUFFER[1] = 'C' as u8;
    WRITE_BUFFER[2] = 'K' as u8;

    spi_test.spi.set_client(Some(spi_test));
    spi_test.spi.init();
    spi_test.run();
}

struct SpiTest {
    spi: &'static SPI,
    write_buffer: TakeCell<'static, [u8]>,
    read_buffer: TakeCell<'static, [u8]>,
    leds: [&'static gpio::GPIOPin<'static>; 4],
}

impl SpiTest {
    fn new(
        spi: &'static SPI,
        write_buffer: &'static mut [u8],
        read_buffer: &'static mut [u8],
        led1_pin: gpio::Pin,
        led2_pin: gpio::Pin,
        led3_pin: gpio::Pin,
        led4_pin: gpio::Pin,
    ) -> SpiTest {
        let led1 = unsafe { &gpio::PORT_APP[led1_pin] };
        let led2 = unsafe { &gpio::PORT_APP[led2_pin] };
        let led3 = unsafe { &gpio::PORT_APP[led3_pin] };
        let led4 = unsafe { &gpio::PORT_APP[led4_pin] };

        led1.make_output();
        led2.make_output();
        led3.make_output();
        led4.make_output();

        SpiTest {
            spi,
            write_buffer: TakeCell::new(write_buffer),
            read_buffer: TakeCell::new(read_buffer),
            leds: [led1, led2, led3, led4],
        }
    }

    fn run(&self) {
        self.spi.read_write_bytes(self.write_buffer.take(),
                                  self.read_buffer.take(),
                                  4);
    }
}

impl SpiSlaveClient for SpiTest {
    fn chip_selected(&self) {
        debug!("CLIENT SELECTED");
    }

    fn read_write_done(
        &self,
        write_buffer: Option<&'static mut [u8]>,
        read_buffer: Option<&'static mut [u8]>,
        _len: usize,
    ) {
        let read_buffer = read_buffer.unwrap();
        debug!("SPIS: received {:?} from SPIM", read_buffer);

        let led_id: usize = read_buffer[0].into();
        if led_id < 4 {
            self.leds[led_id].toggle();
        }

        self.write_buffer.replace(write_buffer.unwrap());
        self.read_buffer.replace(read_buffer);

        self.run();
    }
}
