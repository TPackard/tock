//! Universal asynchronous receiver/transmitter with EasyDMA (UARTE).
//! Based upon the nRF52 UART driver by Niklas Adolfsson.

use core;
use core::cell::Cell;
use core::cmp::min;
use kernel::common::cells::OptionalCell;
use kernel::common::registers::{
    register_bitfields, register_structs, ReadOnly, ReadWrite, WriteOnly,
};
use kernel::common::StaticRef;
use kernel::hil::uart;
use kernel::ReturnCode;
use crate::pinmux;

const UARTE_MAX_BUFFER_SIZE: u32 = 0xff;

static mut BYTE: u8 = 0;

// NOTE: UARTE2 and UARTE3 are not functional as of Sept. 2020 (see nRF5340 errata #84).
#[allow(dead_code)]
const UARTE0_BASE_NONSECURE: StaticRef<UarteRegisters> =
    unsafe { StaticRef::new(0x40008000 as *const UarteRegisters) };
const UARTE0_BASE_SECURE: StaticRef<UarteRegisters> =
    unsafe { StaticRef::new(0x50008000 as *const UarteRegisters) };

#[allow(dead_code)]
const UARTE1_BASE_NONSECURE: StaticRef<UarteRegisters> =
    unsafe { StaticRef::new(0x40009000 as *const UarteRegisters) };
#[allow(dead_code)]
const UARTE1_BASE_SECURE: StaticRef<UarteRegisters> =
    unsafe { StaticRef::new(0x50009000 as *const UarteRegisters) };

const UARTE0_BASE_NETWORK: StaticRef<UarteRegisters> =
    unsafe { StaticRef::new(0x41013000 as *const UarteRegisters) };

// These should only be accessed by the reset_handler on startup
pub static mut UARTE0_APP: Uarte = Uarte::new(UARTE0_BASE_SECURE);
pub static mut UARTE0_NET: Uarte = Uarte::new(UARTE0_BASE_NETWORK);

register_structs! {
    UarteRegisters {
        /// Start UART receiver
        (0x000 => task_startrx: WriteOnly<u32, Task::Register>),
        /// Stop UART receiver
        (0x004 => task_stoprx: WriteOnly<u32, Task::Register>),
        /// Start UART transmitter
        (0x008 => task_starttx: WriteOnly<u32, Task::Register>),
        /// Stop UART transmitter
        (0x00C => task_stoptx: WriteOnly<u32, Task::Register>),
        (0x010 => _reserved1),
        /// Flush  RX FIFO into RX buffer
        (0x02C => task_flush_rx: WriteOnly<u32, Task::Register>),
        (0x030 => _reserved2),
        /// Subscribe configuration for task STARTRX
        (0x080 => subscribe_startrx: WriteOnly<u32, DPPIConfig::Register>),
        /// Subscribe configuration for task STOPRX
        (0x084 => subscribe_stoprx: WriteOnly<u32, DPPIConfig::Register>),
        /// Subscribe configuration for task STARTTX
        (0x088 => subscribe_starttx: WriteOnly<u32, DPPIConfig::Register>),
        /// Subscribe configuration for task STOPTX
        (0x08C => subscribe_stoptx: WriteOnly<u32, DPPIConfig::Register>),
        (0x090 => _reserved3),
        /// Subscribe configuration for task FLUSHRX
        (0x0AC => subscribe_flushrx: WriteOnly<u32, DPPIConfig::Register>),
        (0x0B0 => _reserved4),
        /// Clear To Send
        (0x100 => event_cts: ReadWrite<u32, Event::Register>),
        /// Not Clear To Send
        (0x104 => event_ncts: ReadWrite<u32, Event::Register>),
        /// Data received in RXD
        (0x108 => event_rxdrdy: ReadWrite<u32, Event::Register>),
        (0x10C => _reserved5),
        /// Receive buffer is filled up
        (0x110 => event_endrx: ReadWrite<u32, Event::Register>),
        (0x114 => _reserved6),
        /// Data sent from TXD
        (0x11C => event_txdrdy: ReadWrite<u32, Event::Register>),
        /// Last TX byte transmitted
        (0x120 => event_endtx: ReadWrite<u32, Event::Register>),
        /// Error detected
        (0x124 => event_error: ReadWrite<u32, Event::Register>),
        (0x128 => _reserved7),
        /// Receiver timeout
        (0x144 => event_rxto: ReadWrite<u32, Event::Register>),
        (0x148 => _reserved8),
        /// UART receiver has started
        (0x14C => event_rxstarted: ReadWrite<u32, Event::Register>),
        /// UART transmitter has started
        (0x150 => event_txstarted: ReadWrite<u32, Event::Register>),
        (0x154 => _reserved9),
        /// Transmitter stopped
        (0x158 => event_txstopped: ReadWrite<u32, Event::Register>),
        (0x15C => _reserved10),
        /// Publish configuration for event CTS
        (0x180 => publish_cts: ReadWrite<u32, DPPIConfig::Register>),
        /// Publish configuration for event NCTS
        (0x184 => publish_ncts: ReadWrite<u32, DPPIConfig::Register>),
        /// Publish configuration for event RXDRDY
        (0x188 => publish_rxdrdy: ReadWrite<u32, DPPIConfig::Register>),
        (0x18C => _reserved11),
        /// Publish configuration for event ENDRX
        (0x190 => publish_endrx: ReadWrite<u32, DPPIConfig::Register>),
        (0x194 => _reserved12),
        /// Publish configuration for event TXDRDY
        (0x19C => publish_txdrdy: ReadWrite<u32, DPPIConfig::Register>),
        /// Publish configuration for event ENDTX
        (0x1A0 => publish_endtx: ReadWrite<u32, DPPIConfig::Register>),
        /// Publish configuration for event ERROR
        (0x1A4 => publish_error: ReadWrite<u32, DPPIConfig::Register>),
        (0x1A8 => _reserved13),
        /// Publish configuration for event RXTO
        (0x1C4 => publish_rxto: ReadWrite<u32, DPPIConfig::Register>),
        (0x1C8 => _reserved14),
        /// Publish configuration for event RXSTARTED
        (0x1CC => publish_rxstarted: ReadWrite<u32, DPPIConfig::Register>),
        /// Publish configuration for event TXSTARTED
        (0x1D0 => publish_txstarted: ReadWrite<u32, DPPIConfig::Register>),
        (0x1D4 => _reserved15),
        /// Publish configuration for event TXSTOPPED
        (0x1D8 => publish_txstopped: ReadWrite<u32, DPPIConfig::Register>),
        (0x1DC => _reserved16),
        /// Shortcuts between local events and tasks
        (0x200 => shorts: ReadWrite<u32, Shorts::Register>),
        (0x204 => _reserved17),
        /// Enable or disabled  interrupt
        (0x300 => inten: ReadWrite<u32, Interrupt::Register>),
        /// Enable interrupt
        (0x304 => intenset: ReadWrite<u32, Interrupt::Register>),
        /// Disable interrupt
        (0x308 => intenclr: ReadWrite<u32, Interrupt::Register>),
        (0x30C => _reserved18),
        /// Error source
        (0x480 => errorsrc: ReadWrite<u32, ErrorSrc::Register>),
        (0x484 => _reserved19),
        /// Enable UART
        (0x500 => enable: ReadWrite<u32, Uart::Register>),
        (0x504 => _reserved20),
        /// Pin select for RTS signal
        (0x508 => pselrts: ReadWrite<u32, Psel::Register>),
        /// Pin select for TXD signal
        (0x50C => pseltxd: ReadWrite<u32, Psel::Register>),
        /// Pin select for CTS signal
        (0x510 => pselcts: ReadWrite<u32, Psel::Register>),
        /// Pin Select for RXD signal
        (0x514 => pselrxd: ReadWrite<u32, Psel::Register>),
        (0x518 => _reserved21),
        /// Baud rate
        (0x524 => baudrate: ReadWrite<u32, Baudrate::Register>),
        (0x528 => _reserved22),
        /// Data pointer
        (0x534 => rxd_ptr: ReadWrite<u32, Pointer::Register>),
        /// Maximum number of bytes in receive buffer
        (0x538 => rxd_maxcnt: ReadWrite<u32, Counter::Register>),
        /// Number of bytes transferred in the last transaction
        (0x53C => rxd_amount: ReadOnly<u32, Counter::Register>),
        (0x540 => _reserved23),
        /// Data pointer
        (0x544 => txd_ptr: ReadWrite<u32, Pointer::Register>),
        /// Maximum number of bytes in transmit buffer
        (0x548 => txd_maxcnt: ReadWrite<u32, Counter::Register>),
        /// Number of bytes transferred in the last transaction
        (0x54C => txd_amount: ReadOnly<u32, Counter::Register>),
        (0x550 => _reserved24),
        /// Configuration of parity and hardware flow control
        (0x56C => config: ReadWrite<u32, Config::Register>),
        (0x570 => @END),
    }
}

register_bitfields! [u32,
    /// Start task
    Task [
        ENABLE OFFSET(0) NUMBITS(1)
    ],

    /// DPPI configuration register
    DPPIConfig [
        CHIDX OFFSET(0) NUMBITS(8),
        ENABLE OFFSET(31) NUMBITS(1)
    ],

    /// Read event
    Event [
        READY OFFSET(0) NUMBITS(1)
    ],

    /// Shortcuts
    Shorts [
        // Shortcut between ENDRX and STARTRX
        ENDRX_STARTRX OFFSET(5) NUMBITS(1),
        // Shortcut between ENDRX and STOPRX
        ENDRX_STOPRX OFFSET(6) NUMBITS(1)
    ],

    /// UART Interrupts
    Interrupt [
        CTS OFFSET(0) NUMBITS(1),
        NCTS OFFSET(1) NUMBITS(1),
        RXDRDY OFFSET(2) NUMBITS(1),
        ENDRX OFFSET(4) NUMBITS(1),
        TXDRDY OFFSET(7) NUMBITS(1),
        ENDTX OFFSET(8) NUMBITS(1),
        ERROR OFFSET(9) NUMBITS(1),
        RXTO OFFSET(17) NUMBITS(1),
        RXSTARTED OFFSET(19) NUMBITS(1),
        TXSTARTED OFFSET(20) NUMBITS(1),
        TXSTOPPED OFFSET(22) NUMBITS(1)
    ],

    /// UART Errors
    ErrorSrc [
        OVERRUN OFFSET(0) NUMBITS(1),
        PARITY OFFSET(1) NUMBITS(1),
        FRAMING OFFSET(2) NUMBITS(1),
        BREAK OFFSET(3) NUMBITS(1)
    ],

    /// Enable UART
    Uart [
        ENABLE OFFSET(0) NUMBITS(4) [
            ON = 8,
            OFF = 0
        ]
    ],

    /// Pin select
    Psel [
        // Pin number. MSB is actually the port indicator, but since we number
        // pins sequentially the binary representation of the pin number has
        // the port bit set correctly. So, for simplicity we just treat the
        // pin number as a 6 bit field.
        PIN OFFSET(0) NUMBITS(6) [],
        // Connect/Disconnect
        CONNECT OFFSET(31) NUMBITS(1) [
            Connected = 0,
            Disconnected = 1
        ]
    ],

    /// Baudrate
    Baudrate [
        BAUDRAUTE OFFSET(0) NUMBITS(32)
    ],

    /// DMA pointer
    Pointer [
        POINTER OFFSET(0) NUMBITS(32)
    ],

    /// Counter value
    Counter [
        COUNTER OFFSET(0) NUMBITS(16)
    ],

    /// Configuration of parity and flow control
    Config [
        HWFC OFFSET(0) NUMBITS(1) [
            Disabled = 0,
            Enabled = 1
        ],
        PARITY OFFSET(1) NUMBITS(3) [
            Excluded = 0,
            Included = 7
        ],
        STOP OFFSET(4) NUMBITS(1)  [
            One = 0,
            Two = 1
        ],
        PARITYTYPE OFFSET(8) NUMBITS(1) [
            Even = 0,
            Odd = 1
        ]
    ]
];

/// UARTE
// It should never be instanced outside this module but because a static mutable reference to it
// is exported outside this module it must be `pub`
pub struct Uarte<'a> {
    registers: StaticRef<UarteRegisters>,
    tx_client: OptionalCell<&'a dyn uart::TransmitClient>,
    tx_buffer: kernel::common::cells::TakeCell<'static, [u8]>,
    tx_len: Cell<usize>,
    tx_remaining_bytes: Cell<usize>,
    rx_client: OptionalCell<&'a dyn uart::ReceiveClient>,
    rx_buffer: kernel::common::cells::TakeCell<'static, [u8]>,
    rx_remaining_bytes: Cell<usize>,
    rx_abort_in_progress: Cell<bool>,
    offset: Cell<usize>,
}

#[derive(Copy, Clone)]
pub struct UARTParams {
    pub baud_rate: u32,
}

impl<'a> Uarte<'a> {
    /// Constructor
    const fn new(registers: StaticRef<UarteRegisters>) -> Uarte<'a> {
        Uarte {
            registers,
            tx_client: OptionalCell::empty(),
            tx_buffer: kernel::common::cells::TakeCell::empty(),
            tx_len: Cell::new(0),
            tx_remaining_bytes: Cell::new(0),
            rx_client: OptionalCell::empty(),
            rx_buffer: kernel::common::cells::TakeCell::empty(),
            rx_remaining_bytes: Cell::new(0),
            rx_abort_in_progress: Cell::new(false),
            offset: Cell::new(0),
        }
    }

    /// Configure which pins the UART should use for txd, rxd, cts and rts
    pub fn initialize(
        &self,
        txd: pinmux::Pinmux,
        rxd: pinmux::Pinmux,
        cts: Option<pinmux::Pinmux>,
        rts: Option<pinmux::Pinmux>,
    ) {
        self.registers.pseltxd.write(Psel::PIN.val(txd.into()));
        self.registers.pselrxd.write(Psel::PIN.val(rxd.into()));

        cts.map_or_else(
            || {
                self.registers.pselcts.write(Psel::CONNECT::Disconnected);
            },
            |c| {
                self.registers.pselcts.write(Psel::PIN.val(c.into()));
            },
        );

        rts.map_or_else(
            || {
                self.registers.pselrts.write(Psel::CONNECT::Disconnected);
            },
            |r| {
                self.registers.pselrts.write(Psel::PIN.val(r.into()));
            },
        );

        // Make sure we clear the endtx interrupt since that is what we rely on
        // to know when the DMA TX finishes. Normally, we clear this interrupt
        // as we handle it, so this is not necessary. However, a bootloader (or
        // some other startup code) may have setup TX interrupts, and there may
        // be one pending. We clear it to be safe.
        self.registers.event_endtx.write(Event::READY::CLEAR);

        self.enable_uart();
    }

    fn set_baud_rate(&self, baud_rate: u32) {
        match baud_rate {
            1200 => self.registers.baudrate.set(0x0004F000),
            2400 => self.registers.baudrate.set(0x0009D000),
            4800 => self.registers.baudrate.set(0x0013B000),
            9600 => self.registers.baudrate.set(0x00275000),
            14400 => self.registers.baudrate.set(0x003AF000),
            19200 => self.registers.baudrate.set(0x004EA000),
            28800 => self.registers.baudrate.set(0x0075C000),
            31250 => self.registers.baudrate.set(0x00800000),
            38400 => self.registers.baudrate.set(0x009D0000),
            56000 => self.registers.baudrate.set(0x00E50000),
            57600 => self.registers.baudrate.set(0x00EB0000),
            76800 => self.registers.baudrate.set(0x013A9000),
            230400 => self.registers.baudrate.set(0x03B00000),
            250000 => self.registers.baudrate.set(0x04000000),
            460800 => self.registers.baudrate.set(0x07400000),
            921600 => self.registers.baudrate.set(0x0F000000),
            1000000 => self.registers.baudrate.set(0x10000000),
            115200 | _ => self.registers.baudrate.set(0x01D60000),  // Default to 115200
        }
    }

    // Enable UART peripheral, this need to disabled for low power applications
    fn enable_uart(&self) {
        self.registers.enable.write(Uart::ENABLE::ON);
    }

    #[allow(dead_code)]
    fn disable_uart(&self) {
        self.registers.enable.write(Uart::ENABLE::OFF);
    }

    fn enable_rx_interrupts(&self) {
        self.registers.inten.write(Interrupt::ENDRX::SET);
    }

    fn enable_tx_interrupts(&self) {
        self.registers.inten.write(Interrupt::ENDTX::SET);
    }

    fn disable_rx_interrupts(&self) {
        self.registers.inten.write(Interrupt::ENDRX::CLEAR);
    }

    fn disable_tx_interrupts(&self) {
        self.registers.inten.write(Interrupt::ENDTX::CLEAR);
    }

    /// UART interrupt handler that listens for both tx_end and rx_end events
    #[inline(never)]
    pub fn handle_interrupt(&mut self) {
        if self.tx_ready() {
            self.disable_tx_interrupts();
            self.registers.event_endtx.write(Event::READY::CLEAR);
            let tx_bytes = self.registers.txd_amount.get() as usize;

            let rem = match self.tx_remaining_bytes.get().checked_sub(tx_bytes) {
                None => return,
                Some(r) => r,
            };

            // All bytes have been transmitted
            if rem == 0 {
                // Signal client write done
                self.tx_client.map(|client| {
                    self.tx_buffer.take().map(|tx_buffer| {
                        client.transmitted_buffer(
                            tx_buffer,
                            self.tx_len.get(),
                            ReturnCode::SUCCESS,
                        );
                    });
                });
            } else {
                // Not all bytes have been transmitted then update offset and continue transmitting
                self.offset.set(self.offset.get() + tx_bytes);
                self.tx_remaining_bytes.set(rem);
                self.set_tx_dma_pointer_to_buffer();
                self.registers.txd_maxcnt
                    .write(Counter::COUNTER.val(min(rem as u32, UARTE_MAX_BUFFER_SIZE)));
                self.registers.task_starttx.write(Task::ENABLE::SET);
                self.enable_tx_interrupts();
            }
        }

        if self.rx_ready() {
            self.disable_rx_interrupts();

            // Clear the ENDRX event
            self.registers.event_endrx.write(Event::READY::CLEAR);

            // Get the number of bytes in the buffer that was received this time
            let rx_bytes = self.registers.rxd_amount.get() as usize;

            // Check if this ENDRX is due to an abort. If so, we want to
            // do the receive callback immediately.
            if self.rx_abort_in_progress.get() {
                self.rx_abort_in_progress.set(false);
                self.rx_client.map(|client| {
                    self.rx_buffer.take().map(|rx_buffer| {
                        client.received_buffer(
                            rx_buffer,
                            self.offset.get() + rx_bytes,
                            ReturnCode::ECANCEL,
                            uart::Error::None,
                        );
                    });
                });
            } else {
                // In the normal case, we need to either pass call the callback
                // or do another read to get more bytes.

                // Update how many bytes we still need to receive and
                // where we are storing in the buffer.
                self.rx_remaining_bytes
                    .set(self.rx_remaining_bytes.get().saturating_sub(rx_bytes));
                self.offset.set(self.offset.get() + rx_bytes);

                let rem = self.rx_remaining_bytes.get();
                if rem == 0 {
                    // Signal client that the read is done
                    self.rx_client.map(|client| {
                        self.rx_buffer.take().map(|rx_buffer| {
                            client.received_buffer(
                                rx_buffer,
                                self.offset.get(),
                                ReturnCode::SUCCESS,
                                uart::Error::None,
                            );
                        });
                    });
                } else {
                    // Setup how much we can read. We already made sure that
                    // this will fit in the buffer.
                    let to_read = core::cmp::min(rem, 255);
                    self.registers.rxd_maxcnt.write(Counter::COUNTER.val(to_read as u32));

                    // Actually do the receive.
                    self.set_rx_dma_pointer_to_buffer();
                    self.registers.task_startrx.write(Task::ENABLE::SET);
                    self.enable_rx_interrupts();
                }
            }
        }
    }

    /// Transmit one byte at the time and the client is responsible for polling
    /// This is used by the panic handler
    pub unsafe fn send_byte(&self, byte: u8) {
        self.tx_remaining_bytes.set(1);
        self.registers.event_endtx.write(Event::READY::CLEAR);
        // precaution: copy value into variable with static lifetime
        BYTE = byte;
        self.registers.txd_ptr.set((&BYTE as *const u8) as u32);
        self.registers.txd_maxcnt.write(Counter::COUNTER.val(1));
        self.registers.task_starttx.write(Task::ENABLE::SET);
    }

    /// Check if the UART transmission is done
    pub fn tx_ready(&self) -> bool {
        self.registers.event_endtx.is_set(Event::READY)
    }

    /// Check if either the rx_buffer is full or the UART has timed out
    pub fn rx_ready(&self) -> bool {
        self.registers.event_endrx.is_set(Event::READY)
    }

    fn set_tx_dma_pointer_to_buffer(&self) {
        self.tx_buffer.map(|tx_buffer| {
            self.registers
                .txd_ptr
                .set(tx_buffer[self.offset.get()..].as_ptr() as u32);
        });
    }

    fn set_rx_dma_pointer_to_buffer(&self) {
        self.rx_buffer.map(|rx_buffer| {
            self.registers
                .rxd_ptr
                .set(rx_buffer[self.offset.get()..].as_ptr() as u32);
        });
    }

    // Helper function used by both transmit_word and transmit_buffer
    fn setup_buffer_transmit(&self, buf: &'static mut [u8], tx_len: usize) {
        self.tx_remaining_bytes.set(tx_len);
        self.tx_len.set(tx_len);
        self.offset.set(0);
        self.tx_buffer.replace(buf);
        self.set_tx_dma_pointer_to_buffer();

        self.registers
            .txd_maxcnt
            .write(Counter::COUNTER.val(min(tx_len as u32, UARTE_MAX_BUFFER_SIZE)));
        self.registers.task_starttx.write(Task::ENABLE::SET);

        self.enable_tx_interrupts();
    }
}

impl<'a> uart::UartData<'a> for Uarte<'a> {}
impl<'a> uart::Uart<'a> for Uarte<'a> {}

impl<'a> uart::Transmit<'a> for Uarte<'a> {
    fn set_transmit_client(&self, client: &'a dyn uart::TransmitClient) {
        self.tx_client.set(client);
    }

    fn transmit_buffer(
        &self,
        tx_data: &'static mut [u8],
        tx_len: usize,
    ) -> (ReturnCode, Option<&'static mut [u8]>) {
        if tx_len == 0 || tx_len > tx_data.len() {
            (ReturnCode::ESIZE, Some(tx_data))
        } else if self.tx_buffer.is_some() {
            (ReturnCode::EBUSY, Some(tx_data))
        } else {
            self.setup_buffer_transmit(tx_data, tx_len);
            (ReturnCode::SUCCESS, None)
        }
    }

    fn transmit_word(&self, _data: u32) -> ReturnCode {
        ReturnCode::FAIL
    }

    fn transmit_abort(&self) -> ReturnCode {
        ReturnCode::FAIL
    }
}

impl<'a> uart::Configure for Uarte<'a> {
    fn configure(&self, params: uart::Parameters) -> ReturnCode {
        // These could probably be implemented, but are currently ignored, so
        // throw an error.
        if params.stop_bits != uart::StopBits::One {
            return ReturnCode::ENOSUPPORT;
        }
        if params.parity != uart::Parity::None {
            return ReturnCode::ENOSUPPORT;
        }
        if params.hw_flow_control != false {
            return ReturnCode::ENOSUPPORT;
        }

        self.set_baud_rate(params.baud_rate);

        ReturnCode::SUCCESS
    }
}

impl<'a> uart::Receive<'a> for Uarte<'a> {
    fn set_receive_client(&self, client: &'a dyn uart::ReceiveClient) {
        self.rx_client.set(client);
    }

    fn receive_buffer(
        &self,
        rx_buf: &'static mut [u8],
        rx_len: usize,
    ) -> (ReturnCode, Option<&'static mut [u8]>) {
        if self.rx_buffer.is_some() {
            return (ReturnCode::EBUSY, Some(rx_buf));
        }
        // truncate rx_len if necessary
        let truncated_length = core::cmp::min(rx_len, rx_buf.len());

        self.rx_remaining_bytes.set(truncated_length);
        self.offset.set(0);
        self.rx_buffer.replace(rx_buf);
        self.set_rx_dma_pointer_to_buffer();

        let truncated_uart_max_length = core::cmp::min(truncated_length, 255);

        self.registers
            .rxd_maxcnt
            .write(Counter::COUNTER.val(truncated_uart_max_length as u32));
        self.registers.task_stoprx.write(Task::ENABLE::SET);
        self.registers.task_startrx.write(Task::ENABLE::SET);

        self.enable_rx_interrupts();
        (ReturnCode::SUCCESS, None)
    }

    fn receive_word(&self) -> ReturnCode {
        ReturnCode::FAIL
    }

    fn receive_abort(&self) -> ReturnCode {
        // Trigger the STOPRX event to cancel the current receive call.
        if self.rx_buffer.is_none() {
            ReturnCode::SUCCESS
        } else {
            self.rx_abort_in_progress.set(true);
            self.registers.task_stoprx.write(Task::ENABLE::SET);
            ReturnCode::EBUSY
        }
    }
}
