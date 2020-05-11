//! Non-Volatile Memory Controller
//!
//! Used in order read and write to internal flash.

use core::cell::Cell;
use core::ops::{Index, IndexMut};
use kernel::common::cells::OptionalCell;
use kernel::common::cells::TakeCell;
use kernel::common::cells::VolatileCell;
use kernel::common::deferred_call::DeferredCall;
use kernel::common::registers::{register_bitfields, register_structs, ReadOnly, ReadWrite, WriteOnly};
use kernel::common::StaticRef;
use kernel::hil;
use kernel::ReturnCode;

use crate::deferred_call_tasks::DeferredCallTask;

const NVMC_BASE_SECURE: StaticRef<NvmcRegisters> =
    unsafe { StaticRef::new(0x50039000 as *const NvmcRegisters) };
#[allow(dead_code)]
const NVMC_BASE_NONSECURE: StaticRef<NvmcRegisters> =
    unsafe { StaticRef::new(0x40039000 as *const NvmcRegisters) };
#[allow(dead_code)]
const NVMC_BASE_NETWORK: StaticRef<NvmcRegisters> =
    unsafe { StaticRef::new(0x41080000 as *const NvmcRegisters) };

register_structs! {
    NvmcRegisters {
        /// Ready flag
        (0x400 => ready: ReadOnly<u32, Ready::Register>),
        (0x404 => _reserved1),
        ///Ready flag for new operation
        (0x408 => readynext: ReadOnly<u32, Ready::Register>),
        (0x40C => _reserved2),
        /// Configuration register
        (0x504 => config: ReadWrite<u32, Configuration::Register>),
        (0x508 => _reserved3),
        /// Erase all non-volatile user memory
        (0x50C => eraseall: WriteOnly<u32, EraseAll::Register>),
        (0x510 => _reserved4),
        /// Configure partial page erase
        (0x51C => erasepagepartialcfg: ReadWrite<u32, ErasePagePartialCfg::Register>),
        (0x520 => _reserved5),
        /// I-code cache configuration
        (0x540 => iacachecnf: ReadWrite<u32, CacheConfiguration::Register>),
        (0x544 => _reserved6),
        /// I-code cache hit counter
        (0x548 => ihit: ReadWrite<u32, CacheHit::Register>),
        /// I-code cache miss counter
        (0x54C => imiss: ReadWrite<u32, CacheMiss::Register>),
        (0x550 => _reserved7),
        /// Non-secure program memory access mode.
        (0x584 => configns: ReadWrite<u32, ConfigNs::Register>),
        /// Enable non-secure APPROTECT
        (0x588 => writeuicrns: WriteOnly<u32, WriteUicrNs::Register>),
        (0x58C => @END),
    }
}

register_bitfields! [u32,
    /// Ready flag
    Ready [
        /// NVMC is ready or busy
        READY OFFSET(0) NUMBITS(1) [
            /// NVMC is busy (on-going write or erase operation)
            BUSY = 0,
            /// NVMC is ready
            READY = 1
        ]
    ],
    /// Configuration register
    Configuration [
        /// Program memory access mode. It is strongly recommended
        /// to only activate erase and write modes when they are actively
        /// used. Enabling write or erase will invalidate the cache and keep
        /// it invalidated.
        WEN OFFSET(0) NUMBITS(2) [
            /// Read only access
            Ren = 0,
            /// Write Enabled
            Wen = 1,
            /// Erase enabled
            Een = 2,
            /// Partial erase enabled
            PEen = 4
        ]
    ],
    /// Register for erasing all non-volatile user memory
    EraseAll [
        /// Erase all non-volatile memory including UICR registers. Note
        /// that code erase has to be enabled by CONFIG.Een before the
        /// non-volatile memory can be erased
        ERASEALL OFFSET(0) NUMBITS(1) [
            /// No operation
            NOOPERATION = 0,
            /// Start chip erase
            ERASE = 1
        ]
    ],
    /// Register for partial erase configuration
    ErasePagePartialCfg [
        /// Duration of the partial erase in milliseconds. The user must ensure that the total
        /// erase time is long enough for a complete erase of the flash page.
        DURATION OFFSET(0) NUMBITS(7)
    ],
    /// I-Code cache configuration register
    CacheConfiguration [
        /// Cache enabled
        CACHEEN OFFSET(0) NUMBITS(1) [
            /// Disable cache. Invalidates all cache entries
            DISABLED = 0,
            /// Enable cache
            ENABLED = 1
        ],
        /// Cache profiling enable
        CACHEPROFEN OFFSET(8) NUMBITS(1) [
            /// Disable cache profiling
            DISABLED = 0,
            /// Enable cache profiling
            ENABLED = 1
        ]
    ],
    /// I-Code cache hit counter
    CacheHit [
        /// Number of cache hits. Write 0 to clear.
        HITS OFFSET(0) NUMBITS(32) []
    ],
    /// I-Code cache miss counter
    CacheMiss [
        /// Number of cache misses. Write 0 to clear.
        MISSES OFFSET(0) NUMBITS(32) []
    ],
    /// Non-secure configuration register
    ConfigNs [
        /// Non-secure program memory access mode. It is strongly recommended
        /// to only activate erase and write modes when they are actively
        /// used. Enabling write or erase will invalidate the cache and keep
        /// it invalidated.
        WEN OFFSET(0) NUMBITS(2) [
            /// Read only access
            Ren = 0,
            /// Write Enabled
            Wen = 1,
            /// Erase enabled
            Een = 2
        ]
    ],
    /// Non-secure APPROTECT enable register
    WriteUicrNs [
        SET OFFSET(0) NUMBITS(1) [
            Set = 1
        ],
        KEY OFFSET(4) NUMBITS(28) [
            Keyvalid = 0xAFBE5A7
        ]
    ]
];

/// This mechanism allows us to schedule "interrupts" even if the hardware
/// does not support them.
static DEFERRED_CALL: DeferredCall<DeferredCallTask> =
    unsafe { DeferredCall::new(DeferredCallTask::Nvmc) };

// FIXME: 2048 in network core
const PAGE_SIZE: usize = 4096;

/// This is a wrapper around a u8 array that is sized to a single page for the
/// nrf. Users of this module must pass an object of this type to use the
/// `hil::flash::Flash` interface.
///
/// An example looks like:
///
/// ```rust
/// # extern crate nrf53;
/// # use nrf53::nvmc::NrfPage;
///
/// static mut PAGEBUFFER: NrfPage = NrfPage::new();
/// ```
pub struct NrfPage(pub [u8; PAGE_SIZE as usize]);

impl NrfPage {
    pub const fn new() -> NrfPage {
        NrfPage([0; PAGE_SIZE as usize])
    }

    fn len(&self) -> usize {
        self.0.len()
    }
}

impl Index<usize> for NrfPage {
    type Output = u8;

    fn index(&self, idx: usize) -> &u8 {
        &self.0[idx]
    }
}

impl IndexMut<usize> for NrfPage {
    fn index_mut(&mut self, idx: usize) -> &mut u8 {
        &mut self.0[idx]
    }
}

impl AsMut<[u8]> for NrfPage {
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
}

/// FlashState is used to track the current state and command of the flash.
#[derive(Clone, Copy, PartialEq)]
pub enum FlashState {
    Ready, // Flash is ready to complete a command.
    Read,  // Performing a read operation.
    Write, // Performing a write operation.
    Erase, // Performing an erase operation.
}

pub static mut NVMC: Nvmc = Nvmc::new();

pub struct Nvmc {
    registers: StaticRef<NvmcRegisters>,
    client: OptionalCell<&'static dyn hil::flash::Client<Nvmc>>,
    buffer: TakeCell<'static, NrfPage>,
    state: Cell<FlashState>,
}

impl Nvmc {
    pub const fn new() -> Nvmc {
        Nvmc {
            registers: NVMC_BASE_SECURE,
            client: OptionalCell::empty(),
            buffer: TakeCell::empty(),
            state: Cell::new(FlashState::Ready),
        }
    }

    pub fn configure_read_only(&self) {
        let regs = &*self.registers;
        regs.config.write(Configuration::WEN::Ren);
    }

    /// Configure the NVMC to allow writes to flash.
    pub fn configure_writeable(&self) {
        let regs = &*self.registers;
        regs.config.write(Configuration::WEN::Wen);
    }

    pub fn configure_eraseable(&self) {
        let regs = &*self.registers;
        regs.config.write(Configuration::WEN::Een);
    }

    pub fn erase_all(&self) {
        let regs = &*self.registers;
        regs.config.write(Configuration::WEN::Een);
        while !self.is_ready() {}
        regs.eraseall.write(EraseAll::ERASEALL::ERASE);
        while !self.is_ready() {}

        // Reset to read only access to resume use of cache.
        regs.config.write(Configuration::WEN::Ren);
    }

    /// Check if there is an ongoing operation with the NVMC peripheral.
    pub fn is_ready(&self) -> bool {
        let regs = &*self.registers;
        regs.ready.is_set(Ready::READY)
    }

    /// Check if ready for the next write operation.
    pub fn is_ready_next(&self) -> bool {
        let regs = &*self.registers;
        regs.readynext.is_set(Ready::READY)
    }

    pub fn handle_interrupt(&self) {
        let state = self.state.get();
        self.state.set(FlashState::Ready);

        match state {
            FlashState::Read => {
                self.client.map(|client| {
                    self.buffer.take().map(|buffer| {
                        client.read_complete(buffer, hil::flash::Error::CommandComplete);
                    });
                });
            }
            FlashState::Write => {
                self.client.map(|client| {
                    self.buffer.take().map(|buffer| {
                        client.write_complete(buffer, hil::flash::Error::CommandComplete);
                    });
                });
            }
            FlashState::Erase => {
                self.client.map(|client| {
                    client.erase_complete(hil::flash::Error::CommandComplete);
                });
            }
            _ => {}
        }
    }

    fn erase_page_helper(&self, page_number: usize) {
        let regs = &*self.registers;

        // Put the NVMC in erase mode.
        regs.config.write(Configuration::WEN::Een);

        // Tell the NVMC to erase the correct page by writing 0xFFFFFFFF into the first word of the
        // page.
        let address = (page_number * PAGE_SIZE) as u32;
        let location = unsafe { &*(address as *const VolatileCell<u32>) };
        location.set(0xFFFFFFFF);

        // Make sure that the NVMC is done. The CPU should be blocked while the
        // erase is happening, but it doesn't hurt to check too.
        while !regs.ready.is_set(Ready::READY) {}
    }

    fn read_range(
        &self,
        page_number: usize,
        buffer: &'static mut NrfPage,
    ) -> Result<(), (ReturnCode, &'static mut NrfPage)> {
        // Actually do a copy from flash into the buffer.
        let mut byte: *const u8 = (page_number * PAGE_SIZE) as *const u8;
        unsafe {
            for i in 0..buffer.len() {
                buffer[i] = *byte;
                byte = byte.offset(1);
            }
        }

        // Hold on to the buffer for the callback.
        self.buffer.replace(buffer);

        // Mark the need for an interrupt so we can call the read done
        // callback.
        self.state.set(FlashState::Read);
        DEFERRED_CALL.set();

        Ok(())
    }

    fn write_page(
        &self,
        page_number: usize,
        data: &'static mut NrfPage,
    ) -> Result<(), (ReturnCode, &'static mut NrfPage)> {
        let regs = &*self.registers;

        // Need to erase the page first.
        self.erase_page_helper(page_number);

        // Put the NVMC in write mode.
        regs.config.write(Configuration::WEN::Wen);

        for i in (0..data.len()).step_by(4) {
            let word: u32 = (data[i + 0] as u32) << 0
                | (data[i + 1] as u32) << 8
                | (data[i + 2] as u32) << 16
                | (data[i + 3] as u32) << 24;

            let address = ((page_number * PAGE_SIZE) + i) as u32;
            let location = unsafe { &*(address as *const VolatileCell<u32>) };
            location.set(word);
        }

        // Make sure that the NVMC is done. The CPU should be blocked while the
        // write is happening, but it doesn't hurt to check too.
        while !regs.ready.is_set(Ready::READY) {}

        // Reset to read only access to resume use of cache.
        regs.config.write(Configuration::WEN::Ren);

        // Save the buffer so we can return it with the callback.
        self.buffer.replace(data);

        // Mark the need for an interrupt so we can call the write done
        // callback.
        self.state.set(FlashState::Write);
        DEFERRED_CALL.set();

        Ok(())
    }

    fn erase_page(&self, page_number: usize) -> ReturnCode {
        // Do the basic erase.
        self.erase_page_helper(page_number);

        // Reset to read only access to resume use of cache.
        self.configure_read_only();

        // Mark that we want to trigger a pseudo interrupt so that we can issue
        // the callback even though the NVMC is completely blocking.
        self.state.set(FlashState::Erase);
        DEFERRED_CALL.set();

        ReturnCode::SUCCESS
    }
}

impl<C: hil::flash::Client<Self>> hil::flash::HasClient<'static, C> for Nvmc {
    fn set_client(&self, client: &'static C) {
        self.client.set(client);
    }
}

impl hil::flash::Flash for Nvmc {
    type Page = NrfPage;

    fn read_page(
        &self,
        page_number: usize,
        buf: &'static mut Self::Page,
    ) -> Result<(), (ReturnCode, &'static mut Self::Page)> {
        self.read_range(page_number, buf)
    }

    fn write_page(
        &self,
        page_number: usize,
        buf: &'static mut Self::Page,
    ) -> Result<(), (ReturnCode, &'static mut Self::Page)> {
        self.write_page(page_number, buf)
    }

    fn erase_page(&self, page_number: usize) -> ReturnCode {
        self.erase_page(page_number)
    }
}
