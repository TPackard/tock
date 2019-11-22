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
        LogStorageTest::new(log_storage, &mut BUFFER, VirtualMuxAlarm::new(mux_alarm), &TEST_OPS)
    );
    storage_interface::HasClient::set_client(log_storage, log_storage_test);
    log_storage_test.alarm.set_client(log_storage_test);

    log_storage_test.run();
}

static TEST_OPS: [TestOp; 15] = [
    TestOp::Erase,
    TestOp::Write,
    TestOp::Read,
    TestOp::Write,
    TestOp::Read,
    TestOp::Seek(StorageCookie::SeekBeginning, 0),
    TestOp::Read,
    TestOp::Seek(StorageCookie::SeekBeginning, 0),
    TestOp::Write,
    TestOp::SetReadVal(84),
    TestOp::Read,
    TestOp::Write,
    TestOp::Sync,
    TestOp::SetReadVal(252),
    TestOp::Read,
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

// Current state test is operating in.
#[derive(Clone, Copy, PartialEq)]
enum TestState {
    Erase,
    WriteReadNoSync(u8),
    Done,
}

// Task to perform after test alarm is fired.
#[derive(Clone, Copy, PartialEq)]
enum AlarmTask {
    Read,
    Write,
}

// A single operation within the test.
#[derive(Clone, Copy, PartialEq)]
enum TestOp {
    Erase,
    Read,
    Write,
    Sync,
    Seek(StorageCookie, u64),
    SetReadVal(u64),
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
    alarm_task: Cell<AlarmTask>,
    ops: &'static [TestOp],
    op_index: Cell<usize>,
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
        LogStorageTest {
            storage,
            buffer: TakeCell::new(buffer),
            alarm,
            state: Cell::new(TestState::Erase),
            alarm_task: Cell::new(AlarmTask::Write),
            ops,
            op_index: Cell::new(0),
            read_val: Cell::new(0),
            write_val: Cell::new(0),
        }
    }

    fn run(&self) {
        let op_index = self.op_index.get();
        if op_index == self.ops.len() {
            debug!("Log Storage test succeeded!");
            return;
        }

        match self.ops[op_index] {
            TestOp::Erase => self.erase(),
            TestOp::Read => self.read(),
            TestOp::Write => self.write(),
            TestOp::Sync => self.sync(),
            TestOp::Seek(cookie, read_val) => self.seek(cookie, read_val),
            TestOp::SetReadVal(read_val) => {
                self.read_val.set(read_val);
                self.op_index.increment();
                self.run();
            }
        }
    }

    fn erase(&self) {
        // Erase log.
        match self.storage.erase() {
            ReturnCode::SUCCESS => (),
            ReturnCode::EBUSY => {
                self.wait();
                return;
            }
            _ => panic!("Could not erase log storage!"),
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
        self.op_index.increment();
        self.run();
    }

    fn read(&self) {
        self.buffer.take().map(move |buffer| {
            // Clear buffer first to make debugging more sane.
            buffer.clone_from_slice(&0u64.to_be_bytes());

            if let Err((error, original_buffer)) = self.storage.read(buffer, BUFFER_LEN) {
                self.buffer.replace(original_buffer);
                match error {
                    ReturnCode::SUCCESS => (),
                    ReturnCode::FAIL => {
                        // No more entries, start writing again.
                        debug!(
                            "READ DONE: READ OFFSET: {:?} / WRITE OFFSET: {:?}",
                            self.storage.current_read_offset(),
                            self.storage.current_append_offset()
                        );
                        self.op_index.increment();
                        self.run();
                    }
                    ReturnCode::EBUSY => self.wait(),
                    _ => debug!("READ FAILED: {:?}", error),
                }
            }
        });
    }

    fn write(&self) {
        self.buffer.take().map(move |buffer| {
            buffer.clone_from_slice(&(MAGIC + (self.write_val.get() << VALUE_SHIFT)).to_be_bytes());
            if let Err((error, original_buffer)) = self.storage.append(buffer, BUFFER_LEN) {
                self.buffer.replace(original_buffer);

                match error {
                    ReturnCode::SUCCESS => (),
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

    fn seek(&self, cookie: StorageCookie, read_val: u64) {
        self.read_val.set(read_val);
        match self.storage.seek(cookie) {
            ReturnCode::SUCCESS => debug!("Seeking to {:?}...", cookie),
            error => panic!("Seek failed: {:?}", error),
        }
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
                self.wait();
            }
            _ => {
                debug!("Read failed, trying again");
                self.wait();
            }
        }
    }

    fn seek_done(&self, error: ReturnCode) {
        if error == ReturnCode::SUCCESS {
            debug!("Seeked");
        } else {
            panic!("Seek failed: {:?}", error);
        }

        self.op_index.increment();
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

        // Stop writing after 120 entries have been written.
        if self.write_val.get() % 120 == 0 {
            debug!(
                "WRITE DONE: READ OFFSET: {:?} / WRITE OFFSET: {:?}",
                self.storage.current_read_offset(),
                self.storage.current_append_offset()
            );
            self.op_index.increment();
        }

        self.write_val.set(self.write_val.get() + 1);
        self.wait();
    }

    fn erase_done(&self, _error: ReturnCode) {}

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

        self.op_index.increment();
        self.run();
    }
}

impl<A: Alarm<'static>> AlarmClient for LogStorageTest<A> {
    fn fired(&self) {
        self.run();
    }
}
