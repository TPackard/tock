use capsules::virtual_spi::VirtualSpiMasterDevice;
use kernel::common::cells::TakeCell;
use kernel::component::Component;
use kernel::debug;
use kernel::hil::gpio::{Client, Configure, Interrupt, InterruptEdge, Output};
use kernel::hil::spi::{SpiMasterClient, SpiMasterDevice};
use kernel::static_init;
use nrf53::gpio;
use nrf53::spi::SPI;
use core::cell::Cell;

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
    button1_pin: gpio::Pin,
    button2_pin: gpio::Pin,
    button3_pin: gpio::Pin,
    button4_pin: gpio::Pin,
) {
    let mux_spi = components::spi::SpiMuxComponent::new(&nrf53::spi::SPI1_APP)
        .finalize(components::spi_mux_component_helper!(nrf53::spi::SPI));

    nrf53::gpio::PORT_APP[csn].set();
    nrf53::gpio::PORT_APP[miso].make_input();

    nrf53::spi::SPI1_APP.configure(
        nrf53::pinmux::Pinmux::new(mosi as u32),
        nrf53::pinmux::Pinmux::new(miso as u32),
        nrf53::pinmux::Pinmux::new(sck as u32),
        nrf53::pinmux::Pinmux::new(csn as u32),
        nrf53::spi::SpiRole::SPIM,
    );

    WRITE_BUFFER[0] = 'J' as u8;
    WRITE_BUFFER[1] = 'U' as u8;
    WRITE_BUFFER[2] = 'N' as u8;
    WRITE_BUFFER[3] = 'K' as u8;

    let spi_test = static_init!(
        SpiTest,
        SpiTest::new(
            VirtualSpiMasterDevice::new(mux_spi, &gpio::PORT_APP[csn]),
            &mut WRITE_BUFFER,
            &mut READ_BUFFER,
        )
    );

    let button_handler0 = static_init!(
        ButtonHandler,
        ButtonHandler::new(0, spi_test, led1_pin, button1_pin)
    );
    let button_handler1 = static_init!(
        ButtonHandler,
        ButtonHandler::new(1, spi_test, led2_pin, button2_pin)
    );
    let button_handler2 = static_init!(
        ButtonHandler,
        ButtonHandler::new(2, spi_test, led3_pin, button3_pin)
    );
    let button_handler3 = static_init!(
        ButtonHandler,
        ButtonHandler::new(3, spi_test, led4_pin, button4_pin)
    );

    spi_test.spi.set_client(spi_test);
    button_handler0.button.set_client(button_handler0);
    button_handler1.button.set_client(button_handler1);
    button_handler2.button.set_client(button_handler2);
    button_handler3.button.set_client(button_handler3);
}

struct ButtonHandler {
    id: u8,
    parent: &'static SpiTest,
    led: &'static gpio::GPIOPin<'static>,
    button: &'static gpio::GPIOPin<'static>,
}

impl ButtonHandler {
    fn new(
        id: u8,
        parent: &'static SpiTest,
        led: gpio::Pin,
        button: gpio::Pin,
    ) -> ButtonHandler {
        let led = unsafe { &gpio::PORT_APP[led] };
        let button = unsafe { &gpio::PORT_APP[button] };

        led.make_output();
        button.make_input();
        button.enable_interrupts(InterruptEdge::FallingEdge);

        ButtonHandler {
            id,
            parent,
            led,
            button,
        }
    }
}

impl Client for ButtonHandler {
    fn fired(&self) {
        self.led.toggle();
        self.parent.send_msg(self.id);
    }
}

struct SpiTest {
    spi: VirtualSpiMasterDevice<'static, SPI>,
    write_buffer: TakeCell<'static, [u8]>,
    read_buffer: TakeCell<'static, [u8]>,
    ready: Cell<bool>,
}

impl SpiTest {
    fn new(
        spi: VirtualSpiMasterDevice<'static, SPI>,
        write_buffer: &'static mut [u8],
        read_buffer: &'static mut [u8],
    ) -> SpiTest {
        SpiTest {
            spi,
            write_buffer: TakeCell::new(write_buffer),
            read_buffer: TakeCell::new(read_buffer),
            ready: Cell::new(true),
        }
    }

    fn send_msg(&self, id: u8) {
        self.ready.set(false);

        debug!("SENDING {}", id);
        self.write_buffer.take().map(move |write_buffer| {
            write_buffer[0] = id;
            self.spi.read_write_bytes(write_buffer,
                                      self.read_buffer.take(),
                                      4);
        });
    }
}

impl SpiMasterClient for SpiTest {
    fn read_write_done(
        &self,
        write_buffer: &'static mut [u8],
        read_buffer: Option<&'static mut [u8]>,
        _len: usize,
    ) {
        let read_buffer = read_buffer.unwrap();
        debug!("SPIM: received {:?} from SPIS", read_buffer);

        self.write_buffer.replace(write_buffer);
        self.read_buffer.replace(read_buffer);
        self.ready.set(true);
    }
}
