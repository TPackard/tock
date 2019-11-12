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

struct Metadata {
    magic: u16,
    valid_bytes: u16,
    crc: u32,
    position: StorageCookie,
}

pub struct LogStorage<'a, F: Flash + 'static, C: LogReadClient + LogWriteClient> {
    /// Underlying storage volume.
    volume: &'static [u8],
    /// Flash interface.
    driver: &'a F,
    /// Buffer for a flash page.
    pagebuffer: TakeCell<'static, F::Page>,
    /// Size of a flash page.
    page_size: usize,
    /// Whether or not the log is circular.
    circular: bool,
    /// Client using LogStorage.
    client: OptionalCell<&'a C>,

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
    buffer_offset: Cell<StorageLen>,
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
        debug!("LOG STORAGE VOLUME STARTS AT {:?}", volume.as_ptr());
        let page_size = pagebuffer.as_mut().len();
        let log_storage: LogStorage<'a, F, C> = LogStorage {
            volume,
            driver,
            pagebuffer: TakeCell::new(pagebuffer),
            page_size,
            circular,
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

    fn read_chunk(&self, buffer: &mut [u8], length: usize, buffer_offset: usize) {
        self.pagebuffer.take().map(move |pagebuffer| {
            let mut read_offset = self.read_offset.get();
            let page_size = pagebuffer.as_mut().len();
            let buffer_page_number = self.append_offset.get() / page_size;

            // Copy data into client buffer.
            for offset in 0..length {
                let volume_offset = read_offset + offset;
                if volume_offset / page_size == buffer_page_number {
                    buffer[buffer_offset + offset] = pagebuffer.as_mut()[volume_offset % page_size];
                } else {
                    buffer[buffer_offset + offset] = self.volume[volume_offset];
                }
            }

            // Update read offset.
            read_offset += length;
            if read_offset >= self.volume.len() && self.circular {
                read_offset = SEEK_BEGINNING;
                debug!("READ WRAPPING");
            }

            self.read_offset.set(read_offset);
            self.pagebuffer.replace(pagebuffer);
        });
    }

    fn append_chunk(&self, buffer: &'static mut [u8], pagebuffer: &'static mut F::Page) {
        let length = self.length.get();
        let page_size = pagebuffer.as_mut().len();
        let mut append_offset = self.append_offset.get();
        let buffer_offset = self.buffer_offset.get();

        // Copy data to pagebuffer.
        // TODO: oPTiMizE
        let page_offset = append_offset % page_size;
        let bytes_to_append = core::cmp::min(length - buffer_offset, page_size - page_offset);
        for offset in 0..bytes_to_append {
            pagebuffer.as_mut()[page_offset + offset] = buffer[buffer_offset + offset];
        }

        // Increment append and buffer offset by number of bytes appended.
        append_offset += bytes_to_append;
        if append_offset >= self.volume.len() && self.circular {
            // Reset offset to start of volume if circular and the end is reached.
            append_offset = SEEK_BEGINNING;
            self.records_lost.set(true);
            debug!("APPEND WRAPPED");
        }
        let buffer_offset = buffer_offset + bytes_to_append;
        self.append_offset.set(append_offset);
        self.buffer_offset.set(buffer_offset);

        // Update state depending on if a page needs to be flushed or not.
        let flush_page = append_offset % page_size == 0;
        if flush_page {
            self.state.set(State::Write);
        } else {
            self.state.set(State::Idle);
        }

        // Check if append is finished.
        if buffer_offset == length {
            // Append finished, callback client.
            self.client.map(move |client| {
                client.append_done(buffer, length, self.records_lost.get(), ReturnCode::SUCCESS)
            });
        } else {
            // Append not finished, replace buffer for continuation.
            self.buffer.replace(buffer);
        }

        // Flush page if full.
        if flush_page {
            // TODO: what do if flushing fails?
            let status = self.flush_pagebuffer(pagebuffer);
        } else {
            self.pagebuffer.replace(pagebuffer);
        }
    }

    fn flush_pagebuffer(&self, pagebuffer: &'static mut F::Page) -> ReturnCode {
        let mut offset = self.append_offset.get();
        if offset == SEEK_BEGINNING {
            offset = self.volume.len();
        }
        let page_number = (self.volume.as_ptr() as usize + offset - 1) / self.page_size;

        // TODO: what do if flushing fails?
        let shit = pagebuffer.as_mut();
        debug!(
            "Syncing: {:?}...{:?} ({})",
            &shit[0..16],
            &shit[496..512],
            page_number
        );
        self.driver.write_page(page_number, pagebuffer)
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
            self.read_chunk(buffer, bytes_to_read, 0);
            if bytes_to_read < length && self.circular {
                // Read extends beyond end of circular volume.
                // TODO: what if first read read beyond the end of the log boundaries?
                debug!("READ WRAP {} bytes", length - bytes_to_read);
                self.read_chunk(buffer, length - bytes_to_read, bytes_to_read);
            } else if bytes_to_read < length && !self.circular {
                // TODO: THIS SHOULDN'T BE ALLOWED TO HAPPEN (but it is)!
                unreachable!();
            }

            self.client.map(move |client| {
                client.read_done(buffer, length, ReturnCode::SUCCESS)
            });

            ReturnCode::SUCCESS
        } else {
            debug_assert!(!self.circular);
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
        } else if buffer.len() < length || length > self.volume.len() {
            return ReturnCode::EINVAL;
        }

        // TODO: ensure non-circular logs do not overflow.
        // Ensure end of log hasn't been reached. If circular, append_offset should have already
        // been reset to `SEEK_BEGINNING` in `write_done`.
        if self.append_offset.get() < self.volume.len() {
            // TODO: shouldn't be able to write more bytes than the size of the volume.
            self.length.set(length);
            self.buffer_offset.set(0);
            self.pagebuffer.take().map_or(ReturnCode::ERESERVE, move |pagebuffer| {
                self.append_chunk(buffer, pagebuffer);
                ReturnCode::SUCCESS
            })
        } else {
            debug_assert!(!self.circular);
            ReturnCode::FAIL
        }
    }

    fn current_offset(&self) -> StorageCookie {
        self.append_offset.get()
    }

    fn erase(&self) -> ReturnCode {
        // TODO: clear metadata that should be at the head of the volume.
        self.read_offset.set(0);
        self.append_offset.set(0);
        self.records_lost.set(false);
        self.client.map(move |client| client.erase_done(ReturnCode::SUCCESS));
        ReturnCode::SUCCESS
    }

    fn sync(&self) -> ReturnCode {
        let append_offset = self.append_offset.get();
        if append_offset % self.page_size == 0 {
            self.client.map(move |client| client.sync_done(ReturnCode::SUCCESS));
            ReturnCode::SUCCESS
        } else {
            let status = self.pagebuffer.take().map_or(ReturnCode::FAIL, move |pagebuffer| {
                self.flush_pagebuffer(pagebuffer)
            });

            // Advance read offset if invalidated.
            if status == ReturnCode::SUCCESS {
                let read_offset = self.read_offset.get();
                if read_offset > append_offset && read_offset / self.page_size == append_offset / self.page_size {
                    self.read_offset.set(read_offset + self.page_size - read_offset % self.page_size);
                }
            }

            status
        }
    }
}

impl<'a, F: Flash + 'static, C: LogReadClient + LogWriteClient> flash::Client<F> for LogStorage<'a, F, C> {
    fn read_complete(&self, _read_buffer: &'static mut F::Page, _error: flash::Error) {
        // Reads are made directly from the storage volume, not through the flash interface.
        unreachable!();
    }

    fn write_complete(&self, write_buffer: &'static mut F::Page, error: flash::Error) {
        // TODO: what if first write wrote beyond the end of the log boundaries?
        // TODO: increment read offset if invalidated by write.
        match error {
            flash::Error::CommandComplete => {
                let offset = if self.append_offset.get() == 0 {
                    self.volume.len() - 512
                } else {
                    self.append_offset.get() - 512
                };
                debug!(
                    "Write synced: {:?}...{:?} ({})",
                    &self.volume[offset..offset+16],
                    &self.volume[offset+496..offset+512],
                    offset,
                );

                if self.state.get() == State::Write {
                    if self.buffer_offset.get() == self.length.get() {
                        // No pending append, clean up and do nothing.
                        self.pagebuffer.replace(write_buffer);
                        self.state.set(State::Idle);
                    } else {
                        // Continue appending to next page.
                        debug!("Resuming split append");
                        self.buffer.take().map(move |buffer| {
                            self.append_chunk(buffer, write_buffer);
                        });
                    }
                }
            },
            flash::Error::FlashError => {
                // TODO: handle errors.
                panic!("FLASH ERROR");
            }
        }
    }

    fn erase_complete(&self, _error: flash::Error) {
        // TODO: are erases necessary?
        unreachable!();
    }
}
