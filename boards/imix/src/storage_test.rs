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
    read_val: Cell<u64>,
    write_val: Cell<u64>,
}

impl<A: Alarm<'static>> LogStorageTest<A> {
    fn new(storage: &'static LogStorage, buffer: &'static mut [u8], alarm: A) -> LogStorageTest<A> {
        LogStorageTest {
            storage,
            buffer: TakeCell::new(buffer),
            alarm,
            state: Cell::new(TestState::Erase),
            alarm_task: Cell::new(AlarmTask::Write),
            read_val: Cell::new(0),
            write_val: Cell::new(0),
        }
    }

    fn run(&self) {
        match self.state.get() {
            TestState::Erase => {
                // Erase log.
                if self.storage.erase() != ReturnCode::SUCCESS {
                    panic!("Could not erase log storage!");
                }

                // Make sure that a read on an empty log fails normally.
                self.buffer.take().map(move |buffer| {
                    if let Err((error, original_buffer)) = self.storage.read(buffer, BUFFER_LEN) {
                        self.buffer.replace(original_buffer);
                        if error != ReturnCode::FAIL {
                            panic!("Read on empty log: {:?}", error);
                        }
                    } else {
                        panic!("Read on empty log succeeded! (it shouldn't)");
                    }
                });

                debug!("Log Storage erased");

                // Start writing data.
                self.state.set(TestState::WriteReadNoSync(10));
                self.run();
            }
            TestState::WriteReadNoSync(iterations) => {
                if iterations == 0 {
                    self.state.set(TestState::Done);
                    self.run();
                } else {
                    self.alarm_task.set(AlarmTask::Write);
                    self.state.set(TestState::WriteReadNoSync(iterations - 1));
                    self.write();
                }
            }
            TestState::Done => {
                debug!("Log Storage test succeeded!");
                // TODO: actually allow this when done:
                //debug!("Press the reset button to verify storage persistence");
                //debug!("Press the user button to erase log storage");
            }
        }
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
                        self.run();
                    }
                    ReturnCode::EBUSY => self.wait(WAIT_MS),
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
                    ReturnCode::EBUSY => self.wait(WAIT_MS),
                    _ => debug!("WRITE FAILED: {:?}", error),
                }
            }
        });
    }

    fn wait(&self, ms: u32) {
        let interval = ms * <A::Frequency>::frequency() / 1000;
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
                self.wait(WAIT_MS);
            }
            _ => self.wait(WAIT_MS),
        }
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
        self.buffer.replace(buffer);

        if self.write_val.get() % 120 == 0 {
            debug!(
                "WRITE DONE: READ OFFSET: {:?} / WRITE OFFSET: {:?}",
                self.storage.current_read_offset(),
                self.storage.current_append_offset()
            );
            self.alarm_task.set(AlarmTask::Read);
        }

        self.write_val.set(self.write_val.get() + 1);
        self.wait(WAIT_MS);
    }

    fn erase_done(&self, _error: ReturnCode) {}

    fn sync_done(&self, _error: ReturnCode) {}
}

impl<A: Alarm<'static>> AlarmClient for LogStorageTest<A> {
    fn fired(&self) {
        match self.alarm_task.get() {
            AlarmTask::Read => self.read(),
            AlarmTask::Write => self.write(),
        }
    }
}
