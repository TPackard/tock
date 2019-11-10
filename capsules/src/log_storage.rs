use crate::storage_interface::{
    HasClient, LogRead, LogReadClient, LogWrite, LogWriteClient, StorageCookie, StorageLen,
    SEEK_BEGINNING,
};
use core::cell::Cell;
use core::unreachable;
use kernel::common::cells::{OptionalCell, TakeCell};
use kernel::debug;
use kernel::hil::flash::{self, Flash};
use kernel::ReturnCode;

#[derive(Clone, Copy, PartialEq)]
enum State {
    Idle,
    //Read, // TODO: what do?
    Write,
}

/// TODO: Maybe have a page buffer in memory for read and write pages? Not sure if this would
/// occupy too much memory, but it would be more efficient than using NonvolatileToPages to
/// read/write arbitrary data for every operation.
pub struct LogStorage<'a, F: Flash + 'static, C: LogReadClient + LogWriteClient> {
    /// Underlying storage volume.
    volume: &'static [u8],
    /// Flash interface.
    driver: &'a F,
    /// Buffer for a flash page.
    pagebuffer: TakeCell<'static, F::Page>,
    /// Whether or not the log is circular.
    circular: Cell<bool>, // TODO: Since this is constant, should it really be a Cell?
    /// Client using LogStorage.
    client: OptionalCell<&'a C>,

    // TODO: make sure this auxiliary data is appropriate.
    /// TODO: do I actually really need state?
    state: Cell<State>,
    /// Offset within volume to read from.
    read_offset: Cell<StorageCookie>,
    /// Offset within volume to append to.
    append_offset: Cell<StorageCookie>,
    /// Whether or not old records have been overwritten (circular log has wrapped).
    records_lost: Cell<bool>,

    /// Client-provided buffer to write from.
    buffer: TakeCell<'static, [u8]>,
    /// Offset of unwritten data within buffer.
    buffer_offset: Cell<usize>,
    /// Length of data to write from buffer.
    length: Cell<StorageLen>,
}

impl<'a, F: Flash + 'static, C: LogReadClient + LogWriteClient> LogStorage<'a, F, C> {
    pub fn new(
        volume: &'static [u8],
        driver: &'a F,
        pagebuffer: &'static mut F::Page,
        circular: bool,
    ) -> LogStorage<'a, F, C> {
        let log_storage: LogStorage<'a, F, C> = LogStorage {
            volume,
            driver,
            pagebuffer: TakeCell::new(pagebuffer),
            circular: Cell::new(circular),
            client: OptionalCell::empty(),
            state: Cell::new(State::Idle),
            read_offset: Cell::new(SEEK_BEGINNING),
            append_offset: Cell::new(SEEK_BEGINNING), // TODO: need to recover write offset.
            records_lost: Cell::new(false), // TODO: also need to recover this.
            buffer: TakeCell::empty(),
            buffer_offset: Cell::new(0),
            length: Cell::new(0),
        };

        log_storage
    }

    fn append_chunk(&self, buffer: &'static mut [u8], pagebuffer: &'static mut F::Page) -> ReturnCode{
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
        if append_offset >= self.volume.len() && self.circular.get() {
            // Reset offset to start of volume if circular and the end is reached.
            append_offset = SEEK_BEGINNING;
            self.records_lost.set(true);
            debug!("APPEND WRAPPED");
        }
        debug_assert!(append_offset <= self.volume.len());
        buffer_offset += bytes_to_append;
        self.append_offset.set(append_offset);
        self.buffer_offset.set(buffer_offset);

        let bytes_remaining = length - buffer_offset;

        // Flush page if full.
        if append_offset % page_size == 0 {
            // TODO: track state to continue appropriate operation after flush?
            if bytes_remaining > 0 {
                self.state.set(State::Write);
            }
            let page_number = (self.volume.as_ptr() as usize + append_offset) / page_size; // TODO: off by 1?
            self.driver.write_page(page_number, pagebuffer)
        } else {
            debug_assert!(bytes_remaining == 0);
            self.state.set(State::Idle);
            self.pagebuffer.replace(pagebuffer);
            self.client.map(move |client| {
                client.append_done(buffer, self.length.get(), self.records_lost.get(), ReturnCode::SUCCESS)
            });
            ReturnCode::SUCCESS
        }
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
        if self.read_offset.get() < self.volume.len() {
            self.length.set(length);
            // TODO: shouldn't be able to read beyond append position.
            let bytes_to_read = core::cmp::min(length, self.volume.len() - self.read_offset.get());
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
            if read_offset >= self.volume.len() && self.circular.get() {
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
            debug_assert!(read_offset <= self.volume.len());
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
        let status = if offset < self.volume.len() {
            self.read_offset.set(offset);
            ReturnCode::SUCCESS
        } else {
            ReturnCode::FAIL
        };

        self.client.map(move |client| client.seek_done(status));
        status
    }

    fn get_size(&self) -> StorageLen {
        self.volume.len()
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
        if self.append_offset.get() < self.volume.len() {
            self.length.set(length);
            self.buffer_offset.set(0);
            // TODO: shouldn't be able to write more bytes than the size of the volume.
            self.pagebuffer.take().map_or(ReturnCode::ERESERVE, move |pagebuffer| {
                self.append_chunk(buffer, pagebuffer)
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
        self.records_lost.set(false);
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
        // TODO: what if first write wrote beyond the end of the log boundaries?
        // TODO: increment read offset if invalidated by write.
        match error {
            flash::Error::CommandComplete => {
                if self.state.get() == State::Write {
                    if self.buffer_offset.get() == self.length.get() {
                        self.pagebuffer.replace(write_buffer);
                        self.state.set(State::Idle);
                    } else {
                        self.buffer.take().map(move |buffer| {
                            self.append_chunk(buffer, write_buffer);
                        });
                    }
                }
            },
            flash::Error::FlashError => {
                debug!("flash literally just shit itself");
            }
        }
    }

    /// Flash erase complete.
    fn erase_complete(&self, _error: flash::Error) {
        // TODO remember to error check.
    }
}
