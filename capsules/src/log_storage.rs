use crate::nonvolatile_to_pages::NonvolatileToPages;
use crate::storage_interface::{
    HasClient, LogRead, LogReadClient, LogWrite, LogWriteClient, StorageCookie, StorageLen,
    SEEK_BEGINNING,
};
use core::cell::Cell;
use core::unreachable;
use kernel::common::cells::{NumericCellExt, OptionalCell, TakeCell};
use kernel::debug;
use kernel::hil::flash::{self, Flash};
use kernel::hil::nonvolatile_storage::{NonvolatileStorage, NonvolatileStorageClient};
use kernel::ReturnCode;

#[derive(Clone, Copy, PartialEq)]
enum State {
    Idle,
    Read,
    Write,
}

/// TODO: Maybe have a page buffer in memory for read and write pages? Not sure if this would
/// occupy too much memory, but it would be more efficient than using NonvolatileToPages to
/// read/write arbitrary data for every operation.
pub struct LogStorage<'a, F: Flash + 'static, C: LogReadClient + LogWriteClient> {
    /// Flash interface.
    driver: &'a F,
    /// Buffer for a flash page.
    pagebuffer: TakeCell<'static, F::Page>,
    /// Whether or not the log is circular.
    circular: Cell<bool>, // TODO: Since this is constant, should it really be a Cell?
    volume: &'static [u8],
    storage: &'a NonvolatileToPages<'a, F>, // TODO: remove this.
    client: OptionalCell<&'a C>,
    volume_address: Cell<usize>, // TODO: rename appropriately.
    volume_length: Cell<usize>,
    pagebuffer_offset: Cell<usize>,

    // TODO: make sure this auxiliary data is appropriate.
    state: Cell<State>,
    read_offset: Cell<StorageCookie>,
    append_offset: Cell<StorageCookie>,
    append_wrapped: Cell<bool>,

    buffer: TakeCell<'static, [u8]>,
    buffer_offset: Cell<usize>,
    length: Cell<StorageLen>,
    bytes_remaining: Cell<StorageLen>,
}

impl<'a, F: Flash + 'static, C: LogReadClient + LogWriteClient> LogStorage<'a, F, C> {
    pub fn new(
        driver: &'a F,
        pagebuffer: &'static mut F::Page,
        circular: bool,
        volume: &'static [u8],
        storage: &'a NonvolatileToPages<'a, F>,
    ) -> LogStorage<'a, F, C> {
        let volume_address = volume.as_ptr() as usize;
        let volume_length = volume.len();
        let log_storage: LogStorage<'a, F, C> = LogStorage {
            driver,
            pagebuffer: TakeCell::new(pagebuffer),
            circular: Cell::new(circular),
            volume,
            storage,
            volume_address: Cell::new(volume_address),
            volume_length: Cell::new(volume_length),
            pagebuffer_offset: Cell::new(SEEK_BEGINNING),
            client: OptionalCell::empty(),
            state: Cell::new(State::Idle),
            read_offset: Cell::new(SEEK_BEGINNING),
            append_offset: Cell::new(SEEK_BEGINNING), // TODO: need to recover write offset.
            append_wrapped: Cell::new(false), // TODO: also need to recover this.
            buffer: TakeCell::empty(),
            buffer_offset: Cell::new(0),
            length: Cell::new(0),
            bytes_remaining: Cell::new(0),
        };

        log_storage
    }

    fn append_chunk(&self, pagebuffer: &'static mut F::Page) -> ReturnCode{
        // TODO: move buffer take outside of append_chunk?
        self.buffer.take().map_or(ReturnCode::FAIL, move |buffer| {
            let length = self.length.get();
            let page_size = pagebuffer.as_mut().len();
            let mut append_offset = self.append_offset.get();
            let mut buffer_offset = self.buffer_offset.get();

            let page_offset = append_offset % page_size;
            let bytes_to_append = core::cmp::min(length, page_size - page_offset);

            // Copy data to pagebuffer.
            // TODO: oPTiMizE
            for offset in 0..bytes_to_append {
                pagebuffer.as_mut()[page_offset + offset] = buffer[buffer_offset + offset];
            }

            append_offset += bytes_to_append;
            if append_offset >= self.volume_length.get() && self.circular.get() {
                // Reset offset to start of volume if circular and the end is reached.
                append_offset = SEEK_BEGINNING;
                self.append_wrapped.set(true);
                debug!("APPEND WRAPPED");
            }
            debug_assert!(append_offset <= self.volume_length.get());
            buffer_offset += bytes_to_append;
            self.append_offset.set(append_offset);
            self.buffer_offset.set(buffer_offset);

            let bytes_remaining = self.bytes_remaining.get() - bytes_to_append;
            self.bytes_remaining.set(bytes_remaining);

            // Flush page if full.
            if append_offset % page_size == 0 {
                // TODO: track state to continue appropriate operation after flush?
                if bytes_remaining > 0 {
                    self.state.set(State::Write);
                }
                let page_number = self.pagebuffer_offset.get() / page_size; // TODO: off by 1?
                self.driver.write_page(page_number, pagebuffer)
            } else {
                debug_assert!(bytes_remaining == 0);
                self.state.set(State::Idle);
                self.pagebuffer.replace(pagebuffer);
                self.client.map(move |client| {
                    client.append_done(buffer, self.length.get(), self.append_wrapped.get(), ReturnCode::SUCCESS)
                });
                ReturnCode::SUCCESS
            }
        })
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
        // reset to `SEEK_BEGINNING` after the last read.
        if self.read_offset.get() < self.volume_length.get() {
            self.length.set(length);
            // TODO: shouldn't be able to read beyond append position.
            let bytes_to_read = core::cmp::min(length, self.volume_length.get() - self.read_offset.get());
            let mut bytes_remaining = 0;
            if bytes_to_read < length && self.circular.get() {
                // Read extends beyond end of circular volume, save number of bytes to read from
                // the start of the volume.
                bytes_remaining = length - bytes_to_read;
                debug!("READ WRAPPING: {} bytes", bytes_remaining);
            }

            let mut read_offset = self.read_offset.get();
            for offset in 0..bytes_to_read {
                buffer[offset] = self.volume[read_offset + offset];
            }

            read_offset += bytes_to_read;
            if read_offset >= self.volume_length.get() && self.circular.get() {
                read_offset = SEEK_BEGINNING;
                debug!("READ_WRAPPED");

                // TODO: what if first read read beyond the end of the log boundaries?
                if bytes_remaining > 0 {
                    for offset in 0..bytes_remaining {
                        buffer[bytes_to_read + offset] = self.volume[read_offset + offset];
                    }
                    read_offset += bytes_remaining;
                }
            }
            debug_assert!(read_offset <= self.volume_length.get());
            self.read_offset.set(read_offset);

            self.client.map(move |client| {
                client.read_done(buffer, length, ReturnCode::SUCCESS)
            });
            ReturnCode::SUCCESS
        } else {
            debug_assert!(!self.circular.get());
            ReturnCode::FAIL
        }
    }

    fn current_offset(&self) -> StorageCookie {
        self.read_offset.get()
    }

    fn seek(&self, offset: StorageCookie) -> ReturnCode {
        // TODO: no seeking beyond append offset.
        let status = if offset < self.volume_length.get() {
            self.read_offset.set(offset);
            ReturnCode::SUCCESS
        } else {
            ReturnCode::FAIL
        };

        self.client.map(move |client| client.seek_done(status));
        status
    }

    fn get_size(&self) -> StorageLen {
        self.volume_length.get()
    }
}

impl<'a, F: Flash + 'static, C: LogReadClient + LogWriteClient> LogWrite for LogStorage<'a, F, C> {
    fn append(&self, buffer: &'static mut [u8], length: usize) -> ReturnCode {
        // TODO: what should happen if the length of the buffer is greater than the size of the log?
        if length == 0 {
            return ReturnCode::SUCCESS;
        } else if self.state.get() != State::Idle {
            return ReturnCode::EBUSY;
        } else if buffer.len() < length {
            return ReturnCode::EINVAL;
        }

        // TODO: ensure non-circular logs do not overflow.
        // Ensure end of log hasn't been reached. If circular, append_offset should have already
        // been reset to `SEEK_BEGINNING` in `write_done`.
        if self.append_offset.get() < self.volume_length.get() {
            self.length.set(length);
            self.buffer.replace(buffer);
            self.buffer_offset.set(0);
            // TODO: shouldn't be able to write more bytes than the size of the volume.
            self.pagebuffer.take().map_or(ReturnCode::ERESERVE, move |pagebuffer| {
                self.append_chunk(pagebuffer)
            })
        } else {
            debug_assert!(!self.circular.get());
            ReturnCode::FAIL
        }
    }

    fn current_offset(&self) -> StorageCookie {
        self.append_offset.get()
    }

    fn erase(&self) -> ReturnCode {
        self.read_offset.set(0);
        self.append_offset.set(0);
        self.append_wrapped.set(false);
        self.client.map(move |client| client.erase_done(ReturnCode::SUCCESS));
        ReturnCode::SUCCESS
    }

    fn sync(&self) -> ReturnCode {
        // TODO: update this since there's actually a pagebuffer to sync now.
        self.client.map(move |client| client.sync_done(ReturnCode::SUCCESS));
        ReturnCode::SUCCESS
    }
}

impl<'a, F: Flash + 'static, C: LogReadClient + LogWriteClient> flash::Client<F> for LogStorage<'a, F, C> {
    /// Flash read complete.
    fn read_complete(&self, _read_buffer: &'static mut F::Page, _error: flash::Error) {
        // This callback should never be called since reads are done directly from memory.
        unreachable!();
    }

    /// Flash write complete.
    fn write_complete(&self, write_buffer: &'static mut F::Page, error: flash::Error) {
        if self.state.get() == State::Write {
            if self.bytes_remaining.get() == 0 {
                self.pagebuffer.replace(write_buffer);
                self.state.set(State::Idle);
            } else {
                self.append_chunk(write_buffer);
            }
        }
    }

    /// Flash erase complete.
    fn erase_complete(&self, error: flash::Error) {
    }
}

impl<'a, F: Flash + 'static, C: LogReadClient + LogWriteClient> NonvolatileStorageClient<'static>
    for LogStorage<'a, F, C>
{
    fn read_done(&self, _buffer: &'static mut [u8], _length: usize) {
        // This callback should never be called since reads are done directly from memory.
        unreachable!();
    }

    fn write_done(&self, buffer: &'static mut [u8], length: usize) {
        // Increment write offset by number of bytes written.
        // TODO: what if first write wrote beyond the end of the log boundaries?
        // TODO: increment read offset if invalidated by write.
        let mut append_offset = self.append_offset.get() + length;
        if append_offset >= self.volume_length.get() && self.circular.get() {
            // Reset offset to start of volume if circular and the end is reached.
            append_offset = SEEK_BEGINNING;
            self.append_wrapped.set(true);
            debug!("APPEND WRAPPED");
        }
        debug_assert!(append_offset <= self.volume_length.get());
        self.append_offset.set(append_offset);

        if self.bytes_remaining.get() > 0 && self.circular.get() {
            // If more bytes need to be written from the top of a circular log, perform another
            // write.
            let bytes_to_append = self.bytes_remaining.get();
            self.bytes_remaining.set(0);
            let status = self.storage.write(
                &mut buffer[length..],
                self.volume_address.get() + self.append_offset.get(),
                bytes_to_append,
            );
            // TODO: better error handling.
            if status != ReturnCode::SUCCESS {
                self.state.set(State::Idle);
            }
            debug!("Appending {} more bytes: {:?}", bytes_to_append, status);
        } else {
            // Read complete, issue callback.
            self.state.set(State::Idle);
            self.client.map(move |client| {
                client.append_done(buffer, self.length.get(), self.append_wrapped.get(), ReturnCode::SUCCESS)
            });
        }
    }
}
