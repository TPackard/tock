/// Yeah, so this should test the various storage abstractions that I'm going to make. It doesn't
/// work right now.
use capsules::log_storage;
use capsules::storage_interface::{
    self, LogRead, LogReadClient, LogWrite, LogWriteClient, StorageLen,
};
use capsules::virtual_alarm::{MuxAlarm, VirtualMuxAlarm};
use core::cell::Cell;
use kernel::common::cells::TakeCell;
use kernel::debug;
use kernel::hil::flash;
use kernel::hil::time::{Alarm, AlarmClient, Frequency};
use kernel::static_init;
use kernel::storage_volume;
use kernel::ReturnCode;
use sam4l::ast::Ast;
use sam4l::flashcalw;

// Allocate 1kB volume for log storage.
storage_volume!(TEST_LOG, 2);

pub unsafe fn run_log_storage(mux_alarm: &'static MuxAlarm<'static, Ast>) {
    // TODO: figure out why this is not properly aligned.
    let storage_offset = 512 - TEST_LOG.as_ptr() as usize % 512;
    debug!(
        "STORAGE VOLUME PHYSICALLY STARTS AT {:?}, offsetting by {}",
        TEST_LOG.as_ptr(),
        storage_offset
    );

    // Set up flash controller.
    flashcalw::FLASH_CONTROLLER.configure();
    pub static mut PAGEBUFFER: flashcalw::Sam4lPage = flashcalw::Sam4lPage::new();

    // Create actual log storage abstraction on top of flash.
    let log_storage = static_init!(
        LogStorage,
        log_storage::LogStorage::new(
            &TEST_LOG[storage_offset..storage_offset + 1536],
            &mut flashcalw::FLASH_CONTROLLER,
            &mut PAGEBUFFER,
            true
        )
    );
    flash::HasClient::set_client(&flashcalw::FLASH_CONTROLLER, log_storage);

    // Create and run test for log storage.
    let log_storage_test = static_init!(
        LogStorageTest<VirtualMuxAlarm<'static, Ast>>,
        LogStorageTest::new(log_storage, &mut BUFFER, VirtualMuxAlarm::new(mux_alarm))
    );
    storage_interface::HasClient::set_client(log_storage, log_storage_test);
    log_storage_test.alarm.set_client(log_storage_test);

    log_storage_test.run();
}

// Buffer for reading from and writing to in the storage tests.
static mut BUFFER: [u8; 8] = [0; 8];
// Length of buffer to actually use.
const BUFFER_LEN: usize = 7;
// Time to wait in between storage operations.
const WAIT_MS: u32 = 7;

#[derive(Clone, Copy, PartialEq)]
enum TestState {
    Read,
    Write,
}

type LogStorage = log_storage::LogStorage<
    'static,
    flashcalw::FLASHCALW,
    LogStorageTest<VirtualMuxAlarm<'static, Ast<'static>>>,
>;
struct LogStorageTest<A: Alarm<'static>> {
    storage: &'static LogStorage,
    buffer: TakeCell<'static, [u8]>,
    alarm: A,
    state: Cell<TestState>,
    val: Cell<u64>,
}

impl<A: Alarm<'static>> LogStorageTest<A> {
    fn new(storage: &'static LogStorage, buffer: &'static mut [u8], alarm: A) -> LogStorageTest<A> {
        LogStorageTest {
            storage,
            buffer: TakeCell::new(buffer),
            alarm,
            state: Cell::new(TestState::Write),
            val: Cell::new(0x0102030405060708),
        }
    }

    fn run(&self) {
        match self.state.get() {
            TestState::Read => {
                self.buffer.take().map(move |buffer| {
                    let status = self.storage.read(buffer, BUFFER_LEN);
                    if status != ReturnCode::SUCCESS {
                        debug!("READ FAILED: {:?}", status);
                    }
                });
            }
            TestState::Write => {
                self.buffer.take().map(move |buffer| {
                    buffer.clone_from_slice(&self.val.get().to_be_bytes());
                    let status = self.storage.append(buffer, BUFFER_LEN);
                    if status != ReturnCode::SUCCESS {
                        debug!("WRITE FAILED: {:?}", status);
                    }
                });
            }
        }
    }

    fn wait(&self, ms: u32) {
        let interval = ms * <A::Frequency>::frequency() / 1000;
        let tics = self.alarm.now().wrapping_add(interval);
        self.alarm.set_alarm(tics);
    }
}

impl<A: Alarm<'static>> LogReadClient for LogStorageTest<A> {
    fn read_done(&self, buffer: &'static mut [u8], _length: StorageLen, _error: ReturnCode) {
        debug!("READ DONE: {:?}", buffer);

        // Verify correct value was read.
        let expected = self.val.get().to_be_bytes();
        for i in 0..8 {
            if buffer[i] != expected[i] {
                panic!("Expected {:?}, read {:?}", expected, buffer);
            }
        }

        self.buffer.replace(buffer);
        self.state.set(TestState::Write);
        self.val.set(self.val.get() + (1 << (8 * (8 - BUFFER_LEN))));
        self.wait(WAIT_MS);
    }

    fn seek_done(&self, _error: ReturnCode) {}
}

impl<A: Alarm<'static>> LogWriteClient for LogStorageTest<A> {
    fn append_done(
        &self,
        buffer: &'static mut [u8],
        _length: StorageLen,
        _records_lost: bool,
        _error: ReturnCode,
    ) {
        debug!("WRITE DONE");
        self.buffer.replace(buffer);
        self.state.set(TestState::Read);
        self.wait(WAIT_MS);
    }

    fn erase_done(&self, _error: ReturnCode) {}

    fn sync_done(&self, _error: ReturnCode) {}
}

impl<A: Alarm<'static>> AlarmClient for LogStorageTest<A> {
    fn fired(&self) {
        self.run();
    }
}
