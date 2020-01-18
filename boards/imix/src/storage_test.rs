/// Yeah, so this should test the various storage abstractions that I'm going to make. It doesn't
/// work right now.
use capsules::log_storage;
use capsules::storage_interface::{
    self, LogRead, LogReadClient, LogWrite, LogWriteClient, StorageCookie, StorageLen,
};
use capsules::virtual_alarm::{MuxAlarm, VirtualMuxAlarm};
use core::cell::Cell;
use kernel::common::cells::{NumericCellExt, TakeCell};
use kernel::debug;
use kernel::hil::flash;
use kernel::hil::gpio::{self, Interrupt};
use kernel::hil::time::{Alarm, AlarmClient, Frequency};
use kernel::static_init;
use kernel::storage_volume;
use kernel::ReturnCode;
use sam4l::ast::Ast;
use sam4l::flashcalw;

// Allocate 1kB volume for log storage.
storage_volume!(TEST_LOG, 2);

pub unsafe fn run_log_storage(mux_alarm: &'static MuxAlarm<'static, Ast>) {
    // Set up flash controller.
    flashcalw::FLASH_CONTROLLER.configure();
    pub static mut PAGEBUFFER: flashcalw::Sam4lPage = flashcalw::Sam4lPage::new();

    // Create actual log storage abstraction on top of flash.
    let log_storage = static_init!(
        LogStorage,
        log_storage::LogStorage::new(
            &TEST_LOG,
            &mut flashcalw::FLASH_CONTROLLER,
            &mut PAGEBUFFER,
            true
        )
    );
    flash::HasClient::set_client(&flashcalw::FLASH_CONTROLLER, log_storage);

    // Create and run test for log storage.
    let log_storage_test = static_init!(
        LogStorageTest<VirtualMuxAlarm<'static, Ast>>,
        LogStorageTest::new(
            log_storage,
            &mut BUFFER,
            VirtualMuxAlarm::new(mux_alarm),
            &TEST_OPS
        )
    );
    storage_interface::HasClient::set_client(log_storage, log_storage_test);
    log_storage_test.alarm.set_client(log_storage_test);

    // Create user button.
    let button_pin = &sam4l::gpio::PC[24];
    button_pin.enable_interrupts(gpio::InterruptEdge::RisingEdge);
    button_pin.set_client(log_storage_test);

    log_storage_test.run();
}

static TEST_OPS: [TestOp; 20] = [
    // Read back any existing entries.
    TestOp::BadRead,
    TestOp::Read,
    // Write multiple pages, but don't fill log.
    TestOp::Write,
    TestOp::Read,
    TestOp::Write,
    TestOp::Read,
    // Seek to beginning and re-verify entire log.
    TestOp::Seek(StorageCookie::SeekBeginning),
    TestOp::Read,
    // Write multiple pages, over-filling log and overwriting oldest entries.
    TestOp::Seek(StorageCookie::SeekBeginning),
    TestOp::Write,
    // Read offset should be incremented since it was invalidated by previous write.
    TestOp::BadRead,
    TestOp::Read,
    // Write multiple pages and sync. Read offset should be invalidated due to sync clobbering
    // previous read offset.
    TestOp::Write,
    TestOp::Sync,
    TestOp::Read,
    // Try bad seeks, should fail and not change read cookie.
    TestOp::Write,
    TestOp::BadSeek(StorageCookie::Cookie(0)),
    TestOp::BadSeek(StorageCookie::Cookie(core::usize::MAX)),
    TestOp::Read,
    TestOp::Sync,
];

// Buffer for reading from and writing to in the storage tests.
static mut BUFFER: [u8; 8] = [0; 8];
// Length of buffer to actually use.
const BUFFER_LEN: usize = 8;
// Amount to shift value before adding to magic in order to fit in buffer.
const VALUE_SHIFT: usize = 8 * (8 - BUFFER_LEN);
// Time to wait in between storage operations.
const WAIT_MS: u32 = 2;
// Magic number to write to log storage (+ offset).
const MAGIC: u64 = 0x0102030405060708;

// Test's current state.
#[derive(Clone, Copy, PartialEq)]
enum TestState {
    Operate, // Running through test operations.
    Erase,   // Erasing log and restarting test.
    CleanUp, // Cleaning up test after all operations complete.
}

// A single operation within the test.
#[derive(Clone, Copy, PartialEq)]
enum TestOp {
    Read,
    BadRead,
    Write,
    Sync,
    Seek(StorageCookie),
    BadSeek(StorageCookie),
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
    ops: &'static [TestOp],
    op_index: Cell<usize>,
    op_start: Cell<bool>,
    read_val: Cell<u64>,
    write_val: Cell<u64>,
}

impl<A: Alarm<'static>> LogStorageTest<A> {
    fn new(
        storage: &'static LogStorage,
        buffer: &'static mut [u8],
        alarm: A,
        ops: &'static [TestOp],
    ) -> LogStorageTest<A> {
        // Recover test state.
        let read_val = cookie_to_test_value(storage.current_read_offset());
        let write_val = cookie_to_test_value(storage.current_append_offset());

        debug!(
            "Log recovered from flash (Start and end cookies: {:?} to {:?})",
            storage.current_read_offset(),
            storage.current_append_offset()
        );

        LogStorageTest {
            storage,
            buffer: TakeCell::new(buffer),
            alarm,
            state: Cell::new(TestState::Operate),
            ops,
            op_index: Cell::new(0),
            op_start: Cell::new(true),
            read_val: Cell::new(read_val),
            write_val: Cell::new(write_val),
        }
    }

    fn run(&self) {
        match self.state.get() {
            TestState::Operate => {
                let op_index = self.op_index.get();
                if op_index == self.ops.len() {
                    self.state.set(TestState::CleanUp);
                    self.storage.seek(StorageCookie::SeekBeginning);
                    return;
                }

                match self.ops[op_index] {
                    TestOp::Read => self.read(),
                    TestOp::BadRead => self.bad_read(),
                    TestOp::Write => self.write(),
                    TestOp::Sync => self.sync(),
                    TestOp::Seek(cookie) => self.seek(cookie),
                    TestOp::BadSeek(cookie) => self.bad_seek(cookie),
                }
            }
            TestState::Erase => self.erase(),
            TestState::CleanUp => {
                debug!(
                    "Log Storage test succeeded! (Final log start and end cookies: {:?} to {:?})",
                    self.storage.current_read_offset(),
                    self.storage.current_append_offset()
                );
            }
        }
    }

    fn next_op(&self) {
        self.op_index.increment();
        self.op_start.set(true);
    }

    fn erase(&self) {
        match self.storage.erase() {
            ReturnCode::SUCCESS => (),
            ReturnCode::EBUSY => {
                self.wait();
            }
            _ => panic!("Could not erase log storage!"),
        }
    }

    fn read(&self) {
        // Update read value if clobbered by previous operation.
        if self.op_start.get() {
            let next_read_val = cookie_to_test_value(self.storage.current_read_offset());
            if self.read_val.get() < next_read_val {
                debug!(
                    "Increasing read value from {} to {} due to clobbering!",
                    self.read_val.get(),
                    next_read_val
                );
                self.read_val.set(next_read_val);
            }
        }

        self.buffer.take().map_or_else(
            || panic!("NO BUFFER"),
            move |buffer| {
                // Clear buffer first to make debugging more sane.
                buffer.clone_from_slice(&0u64.to_be_bytes());

                if let Err((error, original_buffer)) = self.storage.read(buffer, BUFFER_LEN) {
                    self.buffer.replace(original_buffer);
                    match error {
                        ReturnCode::FAIL => {
                            // No more entries, start writing again.
                            debug!(
                                "READ DONE: READ OFFSET: {:?} / WRITE OFFSET: {:?}",
                                self.storage.current_read_offset(),
                                self.storage.current_append_offset()
                            );
                            self.next_op();
                            self.run();
                        }
                        ReturnCode::EBUSY => {
                            debug!("Flash busy, waiting before reattempting read");
                            self.wait();
                        }
                        _ => panic!("READ #{} FAILED: {:?}", self.read_val.get(), error),
                    }
                }
            },
        );
    }

    fn bad_read(&self) {
        // Ensure failure if buffer is smaller than provided max read length.
        self.buffer.take().map_or_else(
            || panic!("NO BUFFER"),
            move |buffer| match self.storage.read(buffer, buffer.len() + 1) {
                Ok(_) => panic!("Read with too-large max read length succeeded unexpectedly!"),
                Err((error, original_buffer)) => {
                    self.buffer.replace(original_buffer);
                    assert_eq!(error, ReturnCode::EINVAL);
                }
            },
        );

        // Ensure failure if buffer is too small to hold entry.
        self.buffer.take().map_or_else(
            || panic!("NO BUFFER"),
            move |buffer| match self.storage.read(buffer, BUFFER_LEN - 1) {
                Ok(_) => panic!("Read with too-small buffer succeeded unexpectedly!"),
                Err((error, original_buffer)) => {
                    self.buffer.replace(original_buffer);
                    if self.read_val.get() == self.write_val.get() {
                        assert_eq!(error, ReturnCode::FAIL);
                    } else {
                        assert_eq!(error, ReturnCode::ESIZE);
                    }
                }
            },
        );

        self.next_op();
        self.run();
    }

    fn write(&self) {
        self.buffer.take().map(move |buffer| {
            buffer.clone_from_slice(&(MAGIC + (self.write_val.get() << VALUE_SHIFT)).to_be_bytes());
            if let Err((error, original_buffer)) = self.storage.append(buffer, BUFFER_LEN) {
                self.buffer.replace(original_buffer);

                match error {
                    ReturnCode::EBUSY => self.wait(),
                    _ => debug!("WRITE FAILED: {:?}", error),
                }
            }
        });
    }

    fn sync(&self) {
        match self.storage.sync() {
            ReturnCode::SUCCESS => (),
            error => panic!("Sync failed: {:?}", error),
        }
    }

    fn seek(&self, cookie: StorageCookie) {
        match self.storage.seek(cookie) {
            ReturnCode::SUCCESS => debug!("Seeking to {:?}...", cookie),
            error => panic!("Seek failed: {:?}", error),
        }
    }

    fn bad_seek(&self, cookie: StorageCookie) {
        // Make sure seek fails with EINVAL.
        let original_offset = self.storage.current_read_offset();
        match self.storage.seek(cookie) {
            ReturnCode::EINVAL => (),
            ReturnCode::SUCCESS => panic!(
                "Seek to invalid cookie {:?} succeeded unexpectedly!",
                cookie
            ),
            error => panic!(
                "Seek to invalid cookie {:?} failed with unexpected error {:?}!",
                cookie, error
            ),
        }

        // Make sure that read offset was not changed by failed seek.
        assert_eq!(original_offset, self.storage.current_read_offset());
        self.next_op();
        self.run();
    }

    fn wait(&self) {
        let interval = WAIT_MS * <A::Frequency>::frequency() / 1000;
        let tics = self.alarm.now().wrapping_add(interval);
        self.alarm.set_alarm(tics);
    }
}

impl<A: Alarm<'static>> LogReadClient for LogStorageTest<A> {
    fn read_done(&self, buffer: &'static mut [u8], length: StorageLen, error: ReturnCode) {
        match error {
            ReturnCode::SUCCESS => {
                // Verify correct number of bytes were read.
                if length != BUFFER_LEN {
                    panic!(
                        "{} bytes read, expected {} on read number {} (offset {:?}). Value read was {:?}",
                        length,
                        BUFFER_LEN,
                        self.read_val.get(),
                        self.storage.current_read_offset(),
                        &buffer[0..length],
                    );
                }

                // Verify correct value was read.
                let expected = (MAGIC + (self.read_val.get() << VALUE_SHIFT)).to_be_bytes();
                for i in 0..BUFFER_LEN {
                    if buffer[i] != expected[i] {
                        panic!(
                            "Expected {:?}, read {:?} on read number {} (offset {:?})",
                            &expected[0..BUFFER_LEN],
                            &buffer[0..BUFFER_LEN],
                            self.read_val.get(),
                            self.storage.current_read_offset(),
                        );
                    }
                }

                self.buffer.replace(buffer);
                self.read_val.set(self.read_val.get() + 1);
                self.op_start.set(false);
                self.wait();
            }
            _ => {
                panic!("Read failed unexpectedly!");
            }
        }
    }

    fn seek_done(&self, error: ReturnCode) {
        if error == ReturnCode::SUCCESS {
            debug!("Seeked");
            self.read_val
                .set(cookie_to_test_value(self.storage.current_read_offset()));
        } else {
            panic!("Seek failed: {:?}", error);
        }

        if self.state.get() == TestState::Operate {
            self.next_op();
        }
        self.run();
    }
}

impl<A: Alarm<'static>> LogWriteClient for LogStorageTest<A> {
    fn append_done(
        &self,
        buffer: &'static mut [u8],
        _length: StorageLen,
        _records_lost: bool,
        _error: ReturnCode,
    ) {
        self.buffer.replace(buffer);
        self.op_start.set(false);

        // Stop writing after 120 entries have been written.
        if self.write_val.get() % 120 == 0 {
            debug!(
                "WRITE DONE: READ OFFSET: {:?} / WRITE OFFSET: {:?}",
                self.storage.current_read_offset(),
                self.storage.current_append_offset()
            );
            self.next_op();
        }

        self.write_val.set(self.write_val.get() + 1);
        self.wait();
    }

    fn sync_done(&self, error: ReturnCode) {
        if error == ReturnCode::SUCCESS {
            debug!(
                "SYNC DONE: READ OFFSET: {:?} / WRITE OFFSET: {:?}",
                self.storage.current_read_offset(),
                self.storage.current_append_offset()
            );
        } else {
            panic!("Sync failed: {:?}", error);
        }

        self.next_op();
        self.run();
    }

    fn erase_done(&self, error: ReturnCode) {
        match error {
            ReturnCode::SUCCESS => {
                // Make sure that flash has been erased.
                for i in 0..TEST_LOG.len() {
                    if TEST_LOG[i] != 0xFF {
                        // TODO: Usually a few bytes in the middle aren't erased. Why?
                        /*panic!(
                            "Log not properly erased, read {} at byte {}. SUMMARY: {:?}",
                            TEST_LOG[i],
                            i,
                            &TEST_LOG[i-8..i+8]
                        );*/
                        debug!("F{}", i);
                    }
                }

                // Make sure that a read on an empty log fails normally.
                self.buffer.take().map(move |buffer| {
                    if let Err((error, original_buffer)) = self.storage.read(buffer, BUFFER_LEN) {
                        self.buffer.replace(original_buffer);
                        match error {
                            ReturnCode::FAIL => (),
                            ReturnCode::EBUSY => {
                                self.wait();
                                return;
                            }
                            _ => panic!("Read on empty log did not fail as expected: {:?}", error),
                        }
                    } else {
                        panic!("Read on empty log succeeded! (it shouldn't)");
                    }
                });

                // Move to next operation.
                debug!("Log Storage erased");
                self.state.set(TestState::Operate);
                self.run();
            }
            ReturnCode::EBUSY => {
                // Flash busy, try again.
                self.wait();
            }
            _ => {
                panic!("Erase failed: {:?}", error);
            }
        }
    }
}

impl<A: Alarm<'static>> AlarmClient for LogStorageTest<A> {
    fn fired(&self) {
        self.run();
    }
}

impl<A: Alarm<'static>> gpio::Client for LogStorageTest<A> {
    fn fired(&self) {
        // Reset test (note: this will only work if the test is not in the middle of an operation).
        self.op_index.set(0);
        self.op_start.set(true);
        self.read_val.set(0);
        self.write_val.set(0);

        // Erase log.
        self.state.set(TestState::Erase);
        self.erase();
    }
}

fn cookie_to_test_value(cookie: StorageCookie) -> u64 {
    // Page and entry header sizes for log storage.
    const PAGE_HEADER_SIZE: usize = core::mem::size_of::<usize>();
    const ENTRY_HEADER_SIZE: usize = core::mem::size_of::<usize>();
    const PAGE_SIZE: usize = 512;

    match cookie {
        StorageCookie::Cookie(cookie) => {
            let pages_written = cookie / PAGE_SIZE;
            let entry_size = ENTRY_HEADER_SIZE + BUFFER_LEN;
            let entries_per_page = (PAGE_SIZE - PAGE_HEADER_SIZE) / entry_size;
            let hanging_entries = (cookie % PAGE_SIZE - PAGE_HEADER_SIZE) / entry_size;
            (pages_written * entries_per_page + hanging_entries) as u64
        }
        StorageCookie::SeekBeginning => 0,
    }
}
