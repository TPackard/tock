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
    Sync(bool), // Boolean flag for whether or not to make client callback.
}

/* TODO GENERAL:
 * Should functions that take a buffer also take a length or operate on entire buffer slice?
 * Make sure log state is used and checked consistently.
 */

pub struct LogStorage<'a, F: Flash + 'static, C: LogReadClient + LogWriteClient> {
    /// Underlying storage volume.
    volume: &'static [u8],
    /// Capacity of log in bytes.
    capacity: StorageLen,
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
            append_cookie: Cell::new(PAGE_HEADER_SIZE),
            oldest_cookie: Cell::new(PAGE_HEADER_SIZE),
            buffer: TakeCell::empty(),
            length: Cell::new(0),
        };

        log_storage.reconstruct();
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

    /// Gets the buffer containing the given cookie.
    fn get_buffer<'b>(&self, cookie: usize, pagebuffer: &'b mut F::Page) -> &'b [u8] {
        if cookie / self.page_size == self.append_cookie.get() / self.page_size {
            pagebuffer.as_mut()
        } else {
            self.volume
        }
    }

    /// Gets the byte pointed to by a cookie.
    fn get_byte(&self, cookie: usize, pagebuffer: &mut F::Page) -> u8 {
        let buffer = self.get_buffer(cookie, pagebuffer);
        buffer[cookie % buffer.len()]
    }

    /// Gets a `num_bytes` long slice of bytes starting from a cookie.
    fn get_bytes<'b>(
        &self,
        cookie: usize,
        num_bytes: usize,
        pagebuffer: &'b mut F::Page,
    ) -> &'b [u8] {
        let buffer = self.get_buffer(cookie, pagebuffer);
        let offset = cookie % buffer.len();
        &buffer[offset..offset + num_bytes]
    }

    /// Reconstructs a log from flash.
    fn reconstruct(&self) {
        // Read page headers, get oldest and newest cookies.
        let mut oldest_cookie = core::usize::MAX;
        let mut newest_cookie: usize = 0;
        for header_cookie in (0..self.volume.len()).step_by(self.page_size) {
            let cookie = {
                const COOKIE_SIZE: usize = size_of::<usize>();
                let cookie_bytes = &self.volume[header_cookie..header_cookie + COOKIE_SIZE];
                let cookie_bytes = <[u8; COOKIE_SIZE]>::try_from(cookie_bytes).unwrap();
                usize::from_ne_bytes(cookie_bytes)
            };

            // Validate cookie read from header.
            if cookie % self.volume.len() == header_cookie {
                if cookie < oldest_cookie {
                    oldest_cookie = cookie;
                }
                if cookie > newest_cookie {
                    newest_cookie = cookie;
                }
            }
        }

        // Walk entries in last (newest) page to find length of valid data in last page.
        let mut last_page_len = PAGE_HEADER_SIZE;
        loop {
            // Check if next byte is start of valid entry.
            let volume_offset = newest_cookie % self.volume.len() + last_page_len;
            if self.volume[volume_offset] == 0 || self.volume[volume_offset] == PAD_BYTE {
                break;
            }

            // Get next entry length.
            let entry_length = {
                const LENGTH_SIZE: usize = size_of::<usize>();
                let length_bytes = &self.volume[volume_offset..volume_offset + LENGTH_SIZE];
                let length_bytes = <[u8; LENGTH_SIZE]>::try_from(length_bytes).unwrap();
                usize::from_ne_bytes(length_bytes)
            } + ENTRY_HEADER_SIZE;

            // Add to page length if length is valid (fits within remainder of page.
            if last_page_len + entry_length <= self.page_size {
                last_page_len += entry_length;
            } else {
                break;
            }
        }

        // Set cookies.
        self.oldest_cookie.set(oldest_cookie + PAGE_HEADER_SIZE);
        self.read_cookie.set(oldest_cookie + PAGE_HEADER_SIZE);
        self.append_cookie.set(newest_cookie + last_page_len);

        // Populate page buffer.
        self.pagebuffer.take().map(move |pagebuffer| {
            if last_page_len % self.page_size == 0 {
                // Last page full, reset pagebuffer for next page.
                self.reset_pagebuffer(pagebuffer);
            } else {
                // Copy last page into pagebuffer.
                for i in 0..self.page_size {
                    pagebuffer.as_mut()[i] = self.volume[newest_cookie % self.volume.len() + i];
                }
            }
            self.pagebuffer.replace(pagebuffer);
        });
    }

    /// Returns the cookie of the next entry to read or an error if no entry could be retrieved.
    /// ReturnCodes used:
    ///     * FAIL: reached end of log, nothing to read.
    ///     * ERESERVE: client or internal pagebuffer missing.
    fn get_next_entry(&self) -> Result<usize, ReturnCode> {
        self.pagebuffer.take().map_or(Err(ReturnCode::ERESERVE), move |pagebuffer| {
            let mut entry_cookie = self.read_cookie.get();

            // Skip page header if at start of page or skip padded bytes if at end of page.
            if entry_cookie % self.page_size == 0 {
                entry_cookie += PAGE_HEADER_SIZE;
            } else if self.get_byte(entry_cookie, pagebuffer) == PAD_BYTE {
                entry_cookie += self.page_size - entry_cookie % self.page_size + PAGE_HEADER_SIZE;
            }

            // Check if end of log was reached and return.
            self.pagebuffer.replace(pagebuffer);
            if entry_cookie >= self.append_cookie.get() {
                Err(ReturnCode::FAIL)
            } else {
                Ok(entry_cookie)
            }
        })
    }

    /// Reads and returns the contents of an entry header at the given cookie. Fails if the header
    /// data is invalid.
    /// ReturnCodes used:
    ///     * FAIL: entry header invalid.
    ///     * ERESERVE: client or internal pagebuffer missing.
    fn read_entry_header(&self, entry_cookie: usize) -> Result<usize, ReturnCode> {
        self.pagebuffer.take().map_or(Err(ReturnCode::ERESERVE), move |pagebuffer| {
            // Get length.
            const LENGTH_SIZE: usize = size_of::<usize>();
            let length_bytes = self.get_bytes(entry_cookie, LENGTH_SIZE, pagebuffer);
            let length_bytes = <[u8; LENGTH_SIZE]>::try_from(length_bytes).unwrap();
            let length = usize::from_ne_bytes(length_bytes);

            // Return length of next entry.
            self.pagebuffer.replace(pagebuffer);
            if length == 0 || length > self.page_size - PAGE_HEADER_SIZE - ENTRY_HEADER_SIZE {
                Err(ReturnCode::FAIL)
            } else {
                Ok(length)
            }
        })
    }

    /// Reads the next entry into a buffer. Returns the number of bytes read on success, or an
    /// error otherwise.
    /// ReturnCodes used:
    ///     * FAIL: reached end of log, nothing to read.
    ///     * ERESERVE: internal pagebuffer missing, log is presumably broken.
    ///     * ESIZE: buffer not large enough to contain entry being read.
    fn read_entry(&self, buffer: &mut [u8], length: usize) -> Result<usize, ReturnCode> {
        // Get next entry to read. Immediately returns FAIL in event of failure.
        let entry_cookie = self.get_next_entry()?;
        let entry_length = self.read_entry_header(entry_cookie)?;

        // Read entry into buffer.
        self.pagebuffer
            .take()
            .map_or(Err(ReturnCode::ERESERVE), move |pagebuffer| {
                // Ensure buffer is large enough to hold log entry.
                if entry_length > length {
                    self.pagebuffer.replace(pagebuffer);
                    return Err(ReturnCode::ESIZE);
                }
                let entry_cookie = entry_cookie + ENTRY_HEADER_SIZE;

                // Copy data into client buffer.
                let data = self.get_bytes(entry_cookie, entry_length, pagebuffer);
                for i in 0..entry_length {
                    buffer[i] = data[i];
                }

                // Update read cookie and return number of bytes read.
                self.read_cookie.set(entry_cookie + entry_length);
                self.pagebuffer.replace(pagebuffer);
                Ok(entry_length)
        })
    }

    /// Writes an entry header at the given cookie within a page. Returns number of bytes
    /// written.
    fn write_entry_header(&self, length: usize, cookie: usize, pagebuffer: &mut F::Page) -> usize {
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

    /// Flushes the pagebuffer to flash.
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
            == (append_cookie - 1) / self.page_size
        {
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

        // Write page header to pagebuffer.
        let cookie_bytes = append_cookie.to_ne_bytes();
        for index in 0..cookie_bytes.len() {
            pagebuffer.as_mut()[index] = cookie_bytes[index];
        }

        // TODO: this does not seem like the right place to do this.
        // TODO: cannot be allowed to wrap if log is non-circular.
        // This is the only place where the append cookie can cross page boundaries.
        self.append_cookie.set(append_cookie + PAGE_HEADER_SIZE);
    }

    /// Erases a single page from storage.
    fn erase_page(&self) -> ReturnCode {
        // Uses oldest cookie to keep track of which page to erase. Thus, the oldest pages will be
        // erased first and the log will remain in a valid state even if it fails to be erased
        // completely.
        self.driver
            .erase_page(self.page_number(self.oldest_cookie.get()))
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
    /// Read a log entry into a buffer, if there are any remaining. Updates the read cookie to
    /// point at the next entry when done.
    /// Returns:
    ///     * Ok(()) on success.
    ///     * Err((ReturnCode, buffer)) on failure.
    /// ReturnCodes used:
    ///     * FAIL: reached end of log, nothing to read.
    ///     * EBUSY: log busy with another operation, try again later.
    ///     * EINVAL: provided client buffer is too small.
    ///     * ECANCEL: invalid internal state, read cookie was reset to SeekBeginning.
    ///     * ERESERVE: client or internal pagebuffer missing.
    ///     * ESIZE: buffer not large enough to contain entry being read.
    fn read(
        &self,
        buffer: &'static mut [u8],
        length: usize,
    ) -> Result<(), (ReturnCode, &'static mut [u8])> {
        // Check for failure cases.
        if self.state.get() != State::Idle {
            // Log busy, try reading again later.
            return Err((ReturnCode::EBUSY, buffer));
        } else if buffer.len() < length {
            // Client buffer too small for provided length.
            return Err((ReturnCode::EINVAL, buffer));
        } else if self.read_cookie.get() > self.append_cookie.get() {
            // Read cookie beyond append cookie, must be invalid.
            // TODO: is this an appropriate response for an invalid read cookie?
            self.read_cookie.set(self.oldest_cookie.get());
            return Err((ReturnCode::ECANCEL, buffer));
        } else if self.client.is_none() {
            // No client for callback.
            return Err((ReturnCode::ERESERVE, buffer));
        }

        // Try reading next entry.
        match self.read_entry(buffer, length) {
            Ok(bytes_read) => {
                // TODO: no client callbacks within original function?
                self.client.map(move |client| {
                    client.read_done(buffer, bytes_read, ReturnCode::SUCCESS);
                });
                Ok(())
            }
            Err(return_code) => Err((return_code, buffer)),
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
        } else if length <= 0 || buffer.len() < length {
            // Invalid length provided.
            Err((ReturnCode::EINVAL, buffer))
        } else if length + ENTRY_HEADER_SIZE + PAGE_HEADER_SIZE > self.page_size {
            // Entry too big, won't fit within a single page.
            Err((ReturnCode::ESIZE, buffer))
        } else if !self.circular
            && self.append_cookie.get() + length + ENTRY_HEADER_SIZE >= self.volume.len()
        {
            // End of non-circular log has been reached.
            // TODO: need to ensure that append cookie NEVER goes beyond end of non-circular log.
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

    /// Sync log to storage.
    fn sync(&self) -> ReturnCode {
        let append_cookie = self.append_cookie.get();
        if append_cookie % self.page_size == PAGE_HEADER_SIZE {
            // Pagebuffer empty, don't need to sync.
            self.client
                .map(move |client| client.sync_done(ReturnCode::SUCCESS));
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

    /// Erase the entire log.
    fn erase(&self) -> ReturnCode {
        self.erase_page()
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
                            || unreachable!(),
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
                            self.client
                                .map(move |client| client.sync_done(ReturnCode::SUCCESS));
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

    fn erase_complete(&self, error: flash::Error) {
        match error {
            flash::Error::CommandComplete => {
                let oldest_cookie = self.oldest_cookie.get();
                if self.page_number(oldest_cookie) == self.page_number(self.append_cookie.get()) {
                    // Erased all pages. Reset cookies and callback client.
                    self.read_cookie.set(PAGE_HEADER_SIZE);
                    self.append_cookie.set(PAGE_HEADER_SIZE);
                    self.oldest_cookie.set(PAGE_HEADER_SIZE);

                    self.client
                        .map(move |client| client.erase_done(ReturnCode::SUCCESS));
                } else {
                    // Not done, erase next page.
                    self.oldest_cookie.set(oldest_cookie + self.page_size);
                    let status = self.erase_page();

                    // Abort and alert client if flash driver is busy.
                    if status == ReturnCode::EBUSY {
                        self.read_cookie
                            .set(core::cmp::max(self.read_cookie.get(), oldest_cookie));
                        self.client
                            .map(move |client| client.erase_done(ReturnCode::EBUSY));
                    }
                }
            }
            flash::Error::FlashError => {
                // TODO: handle errors.
                panic!("FLASH ERROR");
            }
        }
    }
}
