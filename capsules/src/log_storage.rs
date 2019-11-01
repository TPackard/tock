use crate::nonvolatile_to_pages::NonvolatileToPages;
use crate::storage_interface::{
    HasClient, LogRead, LogReadClient, LogWrite, LogWriteClient, StorageCookie, StorageLen, Volume,
    SEEK_BEGINNING,
};
use core::cell::Cell;
use kernel::common::cells::OptionalCell;
use kernel::hil::flash::Flash;
use kernel::hil::nonvolatile_storage::{NonvolatileStorage, NonvolatileStorageClient};
use kernel::ReturnCode;

#[derive(Clone, Copy, PartialEq)]
enum State {
    Idle,
    Read,
    //Write,
}

pub struct LogStorage<'a, F: Flash + 'static, C: LogReadClient + LogWriteClient> {
    circular: Cell<bool>,
    volume: Volume,
    storage: &'a NonvolatileToPages<'a, F>,
    client: OptionalCell<&'a C>,

    state: Cell<State>,
    read_offset: Cell<StorageCookie>,
    write_offset: Cell<StorageCookie>,

    length: Cell<StorageLen>,
    bytes_remaining: Cell<StorageLen>,
}

impl<'a, F: Flash + 'static, C: LogReadClient + LogWriteClient> LogStorage<'a, F, C> {
    pub fn new(
        circular: bool,
        volume: Volume,
        storage: &'a NonvolatileToPages<'a, F>,
    ) -> LogStorage<'a, F, C> {
        let log_storage: LogStorage<'a, F, C> = LogStorage {
            circular: Cell::new(circular),
            volume,
            storage,
            client: OptionalCell::empty(),
            state: Cell::new(State::Idle),
            read_offset: Cell::new(SEEK_BEGINNING),
            write_offset: Cell::new(SEEK_BEGINNING), // TODO: need to recover write offset.
            length: Cell::new(0),
            bytes_remaining: Cell::new(0),
        };

        log_storage
    }
}

impl<'a, F: Flash + 'static, C: LogReadClient + LogWriteClient> HasClient<'a, C>
    for LogStorage<'a, F, C>
{
    fn set_client(&'a self, client: &'a C) {
        self.client.set(client);
    }
}

impl<'a, F: Flash + 'static, C: LogReadClient + LogWriteClient> LogRead for LogStorage<'a, F, C> {
    fn read(&self, buffer: &'static mut [u8], length: usize) -> ReturnCode {
        // TODO: what should happen if the length of the buffer is greater than the size of the log?
        if length == 0 {
            return ReturnCode::SUCCESS;
        } else if self.state.get() != State::Idle {
            return ReturnCode::EBUSY;
        } else if buffer.len() < length {
            return ReturnCode::EINVAL;
        }

        // Ensure end of log hasn't been reached. If circular, read_offset should have already been
        // reset to `SEEK_BEGINNING` in `read_done`.
        if self.read_offset.get() < self.volume.size {
            self.length.set(length);
            // TODO: shouldn't be able to read beyond append position.
            let bytes_to_read = core::cmp::min(length, self.volume.size - self.read_offset.get());
            if bytes_to_read < length && self.circular.get() {
                // Read extends beyond end of circular volume, save number of bytes to read from
                // the start of the volume.
                self.bytes_remaining.set(length - bytes_to_read);
            }

            let status = self.storage.read(
                buffer,
                self.volume.base + self.read_offset.get(),
                bytes_to_read,
            );
            if status == ReturnCode::SUCCESS {
                self.state.set(State::Read);
            }
            status
        } else {
            debug_assert!(!self.circular.get());
            ReturnCode::FAIL
        }
    }

    fn current_offset(&self) -> StorageCookie {
        self.read_offset.get()
    }

    fn seek(&self, offset: StorageCookie) -> ReturnCode {
        let status = if offset < self.volume.size {
            self.read_offset.set(offset);
            ReturnCode::SUCCESS
        } else {
            ReturnCode::FAIL
        };

        self.client.map(move |client| client.seek_done(status));
        status
    }

    fn get_size(&self) -> StorageLen {
        self.volume.size
    }
}

impl<'a, F: Flash + 'static, C: LogReadClient + LogWriteClient> LogWrite for LogStorage<'a, F, C> {
    fn append(&self, _buffer: &'static mut [u8], _length: usize) -> ReturnCode {
        // TODO
        ReturnCode::SUCCESS
    }

    fn current_offset(&self) -> StorageCookie {
        self.write_offset.get()
    }

    fn erase(&self) -> ReturnCode {
        // TODO
        ReturnCode::SUCCESS
    }

    fn sync(&self) -> ReturnCode {
        // TODO
        ReturnCode::SUCCESS
    }
}

impl<'a, F: Flash + 'static, C: LogReadClient + LogWriteClient> NonvolatileStorageClient<'static>
    for LogStorage<'a, F, C>
{
    fn read_done(&self, buffer: &'static mut [u8], length: usize) {
        // Increment read offset by number of bytes read.
        // TODO: what if first read read beyond the end of the log boundaries?
        let mut read_offset = self.read_offset.get() + length;
        if read_offset >= self.volume.size && self.circular.get() {
            // Reset offset to start of volume if circular and the end is reached.
            read_offset = SEEK_BEGINNING;
        }
        debug_assert!(read_offset <= self.volume.size);
        self.read_offset.set(read_offset);

        if self.bytes_remaining.get() > 0 && self.circular.get() {
            // If more bytes need to be read from the top of a circular log, perform another read.
            let bytes_to_read = self.bytes_remaining.get();
            self.bytes_remaining.set(0);
            let status = self.storage.read(
                buffer,
                self.volume.base + self.read_offset.get(),
                bytes_to_read,
            );
            // TODO: better error handling.
            if status != ReturnCode::SUCCESS {
                self.state.set(State::Idle);
            }
        } else {
            // Read complete, issue callback.
            self.state.set(State::Idle);
            self.client.map(move |client| {
                client.read_done(buffer, self.length.get(), ReturnCode::SUCCESS)
            });
        }
    }

    fn write_done(&self, _buffer: &'static mut [u8], _length: usize) {
        // TODO
    }
}
