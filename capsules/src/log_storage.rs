use crate::storage_interface::{
    HasClient, LogRead, LogReadClient, LogWrite, LogWriteClient, StorageCookie, StorageLen,
};
use core::cell::Cell;
use core::convert::TryFrom;
use core::mem::size_of;
use core::unreachable;
use kernel::common::cells::{OptionalCell, TakeCell};
use kernel::debug;
use kernel::hil::flash::{self, Flash};
use kernel::ReturnCode;

/// Maximum page header size.
const PAGE_HEADER_SIZE: usize = size_of::<usize>();
/// Maximum entry header size.
const ENTRY_HEADER_SIZE: usize = size_of::<usize>();

/// Byte used to pad the end of a page.
const PAD_BYTE: u8 = 0xFF;
/// Entry length representing an invalid entry.
const INVALID_ENTRY_LENGTH: usize = 0xFFFFFFFF;

#[derive(Clone, Copy, PartialEq)]
enum State {
    Idle,
    Write,
    Sync(bool),
}

pub struct LogStorage<'a, F: Flash + 'static, C: LogReadClient + LogWriteClient> {
    /// Underlying storage volume.
    volume: &'static [u8],
    /// Capacity of log in bytes.
    capacity: usize,
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

    /// Current operation being executed, if asynchronous.
    state: Cell<State>,
    /// Cookie within log to read from.
    read_cookie: Cell<usize>,
    /// Cookie within log to append to.
    append_cookie: Cell<usize>,
    /// Oldest cookie still in log.
    oldest_cookie: Cell<usize>,

    /// Client-provided buffer to write from.
    buffer: TakeCell<'static, [u8]>,
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
        // TODO: remove debug statements.
        debug!("page header size:  {}", PAGE_HEADER_SIZE);
        debug!("entry header size: {}", ENTRY_HEADER_SIZE);

        debug!("LOG STORAGE VOLUME STARTS AT {:?}", volume.as_ptr());
        let page_size = pagebuffer.as_mut().len();
        let capacity = volume.len() - PAGE_HEADER_SIZE * (volume.len() / page_size);

        let log_storage: LogStorage<'a, F, C> = LogStorage {
            volume,
            capacity,
            driver,
            pagebuffer: TakeCell::new(pagebuffer),
            page_size,
            circular,
            client: OptionalCell::empty(),
            state: Cell::new(State::Idle),
            read_cookie: Cell::new(PAGE_HEADER_SIZE),
            append_cookie: Cell::new(PAGE_HEADER_SIZE), // TODO: need to recover write offset.
            oldest_cookie: Cell::new(PAGE_HEADER_SIZE),
            buffer: TakeCell::empty(),
            length: Cell::new(0),
        };

        log_storage
    }

    /// Returns the page number of the page containing a cookie.
    fn page_number(&self, cookie: usize) -> usize {
        (self.volume.as_ptr() as usize + cookie % self.volume.len()) / self.page_size
    }

    /// Whether or not records may have been previously lost.
    fn records_lost(&self) -> bool {
        self.oldest_cookie.get() != PAGE_HEADER_SIZE
    }

    /// Gets the data structure containing the given cookie.
    fn get_data<'b>(&self, cookie: usize, pagebuffer: &'b mut F::Page) -> &'b [u8] {
        if cookie / self.page_size == self.append_cookie.get() / self.page_size {
            pagebuffer.as_mut()
        } else {
            self.volume
        }
    }

    /// Gets the byte pointed to by a cookie.
    fn get_byte(&self, cookie: usize, pagebuffer: &mut F::Page) -> u8 {
        let buffer = self.get_data(cookie, pagebuffer);
        buffer[cookie % buffer.len()]
    }

    /// Gets a `num_bytes` long slice of bytes starting from a cookie.
    fn get_bytes<'b>(
        &self,
        cookie: usize,
        num_bytes: usize,
        pagebuffer: &'b mut F::Page,
    ) -> &'b [u8] {
        let buffer = self.get_data(cookie, pagebuffer);
        let offset = cookie % buffer.len();
        &buffer[offset..offset + num_bytes]
    }

    /// Advances read_cookie to the start of the next log entry. Returns the length of the entry
    /// on success, or an error otherwise.
    fn get_next_entry(&self, pagebuffer: &mut F::Page) -> Result<usize, ReturnCode> {
        // Check if end of log was reached.
        let mut read_cookie = self.read_cookie.get();
        if read_cookie == self.append_cookie.get() {
            return Err(ReturnCode::FAIL);
        }

        // Skip padded bytes and page header if at end of page.
        if read_cookie % self.page_size == 0 {
            read_cookie += PAGE_HEADER_SIZE;
            self.read_cookie.set(read_cookie);
        } else if self.get_byte(read_cookie, pagebuffer) == PAD_BYTE {
            read_cookie += self.page_size - read_cookie % self.page_size + PAGE_HEADER_SIZE;
            self.read_cookie.set(read_cookie);

            if read_cookie == self.append_cookie.get() {
                return Err(ReturnCode::FAIL);
            }
        }

        // Get length.
        let length = {
            // TODO: use u32 instead of usize everywhere?
            const LENGTH_SIZE: usize = size_of::<usize>();
            let length_bytes = self.get_bytes(read_cookie, LENGTH_SIZE, pagebuffer);
            let length_bytes = <[u8; LENGTH_SIZE]>::try_from(length_bytes).unwrap();
            usize::from_ne_bytes(length_bytes)
        };

        // Return length of next entry.
        if length == INVALID_ENTRY_LENGTH {
            Err(ReturnCode::FAIL)
        } else {
            Ok(length)
        }
    }

    /// Reads the next entry into a buffer. Returns the number of bytes read on success, or an
    /// error otherwise.
    fn read_entry(&self, buffer: &mut [u8], length: usize) -> Result<usize, ReturnCode> {
        self.pagebuffer
            .take()
            .map_or(Err(ReturnCode::ERESERVE), move |pagebuffer| {
                // Read entry header.
                let entry_length = self.get_next_entry(pagebuffer);
                match entry_length {
                    Ok(entry_length) => {
                        // Ensure buffer is large enough to hold log entry.
                        if entry_length > length {
                            return Err(ReturnCode::ESIZE);
                        }
                        let read_cookie = self.read_cookie.get() + ENTRY_HEADER_SIZE;

                        // Copy data into client buffer.
                        let data = self.get_bytes(read_cookie, entry_length, pagebuffer);
                        for i in 0..entry_length {
                            buffer[i] = data[i];
                        }

                        // Update read offset.
                        self.read_cookie.set(read_cookie + entry_length);
                        self.pagebuffer.replace(pagebuffer);

                        Ok(entry_length)
                    }
                    error => {
                        self.pagebuffer.replace(pagebuffer);
                        error
                    }
                }
            })
    }

    /// Writes an entry header at the given cookie within a page. Returns number of bytes
    /// written.
    fn write_entry_header(
        &self,
        length: usize,
        cookie: usize,
        pagebuffer: &mut F::Page,
    ) -> usize {
        let mut offset = 0;
        for byte in &length.to_ne_bytes() {
            pagebuffer.as_mut()[cookie + offset] = *byte;
            offset += 1;
        }
        offset
    }

    /// Appends data from a buffer onto the end of the log. Requires that there is enough space
    /// remaining in the pagebuffer for the entry (including metadata).
    fn append_entry(
        &self,
        buffer: &'static mut [u8],
        length: usize,
        pagebuffer: &'static mut F::Page,
    ) {
        let append_cookie = self.append_cookie.get();
        let mut page_offset = append_cookie % self.page_size;

        // Write entry header to pagebuffer.
        let header_size = self.write_entry_header(length, page_offset, pagebuffer);
        page_offset += header_size;

        // Copy data to pagebuffer.
        for offset in 0..length {
            pagebuffer.as_mut()[page_offset + offset] = buffer[offset];
        }

        // Increment append and buffer offset by number of bytes appended.
        let append_cookie = append_cookie + length + header_size;
        self.append_cookie.set(append_cookie);

        // Update state depending on if a page needs to be flushed or not.
        if append_cookie % self.page_size == 0 {
            // Flush page after callback client.
            self.state.set(State::Sync(false));
            self.client.map(move |client| {
                client.append_done(buffer, length, self.records_lost(), ReturnCode::SUCCESS)
            });
            self.flush_pagebuffer(pagebuffer);
        } else {
            // Replace pagebuffer and callback client.
            self.state.set(State::Idle);
            self.pagebuffer.replace(pagebuffer);
            self.client.map(move |client| {
                client.append_done(buffer, length, self.records_lost(), ReturnCode::SUCCESS)
            });
        }
    }

    /// Writes the page header and flushes the pagebuffer to flash.
    fn flush_pagebuffer(&self, pagebuffer: &'static mut F::Page) -> ReturnCode {
        let mut append_cookie = self.append_cookie.get();
        let page_number = self.page_number(append_cookie - 1);

        // Pad end of page.
        while append_cookie % self.page_size != 0 {
            pagebuffer.as_mut()[append_cookie % self.page_size] = PAD_BYTE;
            append_cookie += 1;
        }

        // Advance read and oldest cookies, if clobbered.
        let read_cookie = self.read_cookie.get();
        if (read_cookie + self.volume.len()) / self.page_size
            == (append_cookie - 1) / self.page_size
        {
            // Move read cookie to start of next page.
            self.read_cookie.set(
                read_cookie + self.page_size + PAGE_HEADER_SIZE - read_cookie % self.page_size,
            );
        }
        
        let oldest_cookie = self.oldest_cookie.get();
        if (oldest_cookie + self.volume.len()) / self.page_size
            == (append_cookie - 1) / self.page_size {
            self.oldest_cookie.set(oldest_cookie + self.page_size);
        }

        // Write page to flash.
        let shit = pagebuffer.as_mut();
        debug!(
            "Syncing: {:?}...{:?} ({})",
            &shit[0..16],
            &shit[496..512],
            page_number
        );
        // TODO: what do if flushing fails? Need to handle.
        self.driver.write_page(page_number, pagebuffer)
    }

    /// Resets the pagebuffer so that new data can be written.
    fn reset_pagebuffer(&self, pagebuffer: &mut F::Page) {
        let mut append_cookie = self.append_cookie.get();
        if append_cookie % self.page_size != 0 {
            append_cookie += self.page_size - append_cookie % self.page_size;
        }

        // Write page metadata to pagebuffer.
        let cookie_bytes = append_cookie.to_ne_bytes();
        for index in 0..cookie_bytes.len() {
            pagebuffer.as_mut()[index] = cookie_bytes[index];
        }

        self.append_cookie.set(append_cookie + PAGE_HEADER_SIZE);
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
    /// Read a log entry into a buffer.
    fn read(
        &self,
        buffer: &'static mut [u8],
        length: usize,
    ) -> Result<(), (ReturnCode, &'static mut [u8])> {
        if self.state.get() != State::Idle {
            return Err((ReturnCode::EBUSY, buffer));
        } else if buffer.len() < length {
            return Err((ReturnCode::EINVAL, buffer));
        }

        // Ensure end of log hasn't been reached.
        let read_cookie = self.read_cookie.get();
        // TODO: incorrect calculation of storage capacity. Replace this with ability to detect end
        // of log, regardless of whether or not it's circular.
        if !self.circular && read_cookie + length >= self.volume.len() {
            Err((ReturnCode::FAIL, buffer))
        } else {
            match self.read_entry(buffer, length) {
                Ok(bytes_read) => {
                    self.client.map(move |client| {
                        client.read_done(buffer, bytes_read, ReturnCode::SUCCESS);
                    });
                    Ok(())
                }
                Err(return_code) => Err((return_code, buffer)),
            }
        }
    }

    /// Get cookie representing current read offset.
    fn current_read_offset(&self) -> StorageCookie {
        StorageCookie::Cookie(self.read_cookie.get())
    }

    /// Seek to a new read cookie.
    fn seek(&self, cookie: StorageCookie) -> ReturnCode {
        let status = match cookie {
            StorageCookie::SeekBeginning => {
                self.read_cookie.set(self.oldest_cookie.get());
                ReturnCode::SUCCESS
            }
            StorageCookie::Cookie(cookie) => {
                if cookie <= self.append_cookie.get() && cookie >= self.oldest_cookie.get() {
                    self.read_cookie.set(cookie);
                    ReturnCode::SUCCESS
                } else {
                    ReturnCode::EINVAL
                }
            }
        };

        self.client.map(move |client| client.seek_done(status));
        status
    }

    /// Get approximate log capacity in bytes.
    fn get_size(&self) -> StorageLen {
        self.capacity
    }
}

impl<'a, F: Flash + 'static, C: LogReadClient + LogWriteClient> LogWrite for LogStorage<'a, F, C> {
    /// Appends an entry onto the end of the log. Entry must fit within a page (including log
    /// metadata).
    fn append(
        &self,
        buffer: &'static mut [u8],
        length: usize,
    ) -> Result<(), (ReturnCode, &'static mut [u8])> {
        if self.state.get() != State::Idle {
            // Busy with another operation.
            Err((ReturnCode::EBUSY, buffer))
        } else if buffer.len() < length {
            // Invalid length provided.
            Err((ReturnCode::EINVAL, buffer))
        } else if length + ENTRY_HEADER_SIZE + PAGE_HEADER_SIZE > self.page_size {
            // Entry too big, won't fit within a single page.
            Err((ReturnCode::ESIZE, buffer))
        } else if !self.circular
            && self.append_cookie.get() + length + ENTRY_HEADER_SIZE >= self.volume.len()
        {
            // End of non-circular log has been reached.
            Err((ReturnCode::FAIL, buffer))
        } else {
            // Check if new entry will fit into current page.
            let result_size =
                self.append_cookie.get() % self.page_size + length + ENTRY_HEADER_SIZE;
            if result_size <= self.page_size {
                match self.pagebuffer.take() {
                    Some(pagebuffer) => {
                        self.append_entry(buffer, length, pagebuffer);
                        Ok(())
                    }
                    None => Err((ReturnCode::ERESERVE, buffer)),
                }
            } else {
                // Need to sync pagebuffer first, then write to new page.
                self.state.set(State::Write);
                match self.pagebuffer.take() {
                    Some(pagebuffer) => {
                        self.buffer.replace(buffer);
                        self.length.set(length);
                        self.flush_pagebuffer(pagebuffer);
                        Ok(())
                    }
                    None => Err((ReturnCode::ERESERVE, buffer)),
                }
            }
        }
    }

    /// Get cookie representing current append offset.
    fn current_append_offset(&self) -> StorageCookie {
        StorageCookie::Cookie(self.append_cookie.get())
    }

    /// Erase the entire log.
    fn erase(&self) -> ReturnCode {
        // TODO: actually erase every page.
        self.read_cookie.set(PAGE_HEADER_SIZE);
        self.append_cookie.set(PAGE_HEADER_SIZE);
        self.oldest_cookie.set(PAGE_HEADER_SIZE);
        self.client
            .map(move |client| client.erase_done(ReturnCode::SUCCESS));
        ReturnCode::SUCCESS
    }

    /// Sync log to storage.
    fn sync(&self) -> ReturnCode {
        let append_cookie = self.append_cookie.get();
        if append_cookie % self.page_size == PAGE_HEADER_SIZE {
            // Pagebuffer empty, don't need to sync.
            self.client.map(move |client| client.sync_done(ReturnCode::SUCCESS));
            ReturnCode::SUCCESS
        } else {
            self.pagebuffer
                .take()
                .map_or(ReturnCode::ERESERVE, move |pagebuffer| {
                    self.state.set(State::Sync(true));
                    self.flush_pagebuffer(pagebuffer)
                })
        }
    }
}

impl<'a, F: Flash + 'static, C: LogReadClient + LogWriteClient> flash::Client<F>
    for LogStorage<'a, F, C>
{
    fn read_complete(&self, _read_buffer: &'static mut F::Page, _error: flash::Error) {
        // Reads are made directly from the storage volume, not through the flash interface.
        unreachable!();
    }

    fn write_complete(&self, pagebuffer: &'static mut F::Page, error: flash::Error) {
        match error {
            flash::Error::CommandComplete => {
                let offset = self.append_cookie.get() - 1;
                let offset = offset - offset % self.page_size;
                let offset = offset % self.volume.len();
                debug!(
                    "Synced:  {:?}...{:?} ({})",
                    &self.volume[offset..offset + 16],
                    &self.volume[offset + 496..offset + 512],
                    offset,
                );

                match self.state.get() {
                    State::Write => {
                        // Reset pagebuffer and finish writing on the new page.
                        self.reset_pagebuffer(pagebuffer);
                        self.buffer.take().map_or_else(
                            || panic!("No buffer"),
                            move |buffer| {
                                self.append_entry(buffer, self.length.get(), pagebuffer);
                            },
                        );
                    }
                    State::Sync(callback_client) => {
                        // Reset pagebuffer if full.
                        if self.append_cookie.get() % self.page_size == 0 {
                            self.reset_pagebuffer(pagebuffer);
                        }

                        self.pagebuffer.replace(pagebuffer);
                        self.state.set(State::Idle);

                        if callback_client {
                            self.client.map(move |client| client.sync_done(ReturnCode::SUCCESS));
                        }
                    }
                    _ => {}
                }
            }
            flash::Error::FlashError => {
                // TODO: handle errors.
                panic!("FLASH ERROR");
            }
        }
    }

    fn erase_complete(&self, _error: flash::Error) {
        // TODO
        unreachable!();
    }
}
