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
pub const PAGE_HEADER_SIZE: usize = size_of::<usize>();
/// Maximum entry header size.
pub const ENTRY_HEADER_SIZE: usize = size_of::<usize>();

/// Byte used to pad the end of a page.
const PAD_BYTE: u8 = 0xFF;

#[derive(Clone, Copy, PartialEq)]
enum State {
    Idle,
    Write,
    Sync,
}

/* TODO GENERAL:
 * Should functions that take a buffer also take a length or operate on entire buffer slice?
 *
 * Make sure log state is used and checked consistently. Make sure log state is reset to Idle int
 * the event of an error.
 *
 * No calls to `map`, should only use `map_or` or `map_or_else`.
 *
 * Remove debug statements.
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
        // Subtract 1 from append cookie to get cookie of last bit written. This is needed because
        // the pagebuffer always contains the last written bit, but not necessarily the append
        // cookie (i.e. the pagebuffer isn't flushed yet when append_cookie % page_size == 0).
        if cookie / self.page_size == (self.append_cookie.get() - 1) / self.page_size {
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

        // Walk entries in last (newest) page to calculate last page length.
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
                if last_page_len == self.page_size {
                    break;
                }
            } else {
                break;
            }
        }

        // Set cookies.
        self.oldest_cookie.set(oldest_cookie + PAGE_HEADER_SIZE);
        self.read_cookie.set(oldest_cookie + PAGE_HEADER_SIZE);
        self.append_cookie.set(newest_cookie + last_page_len);

        // Populate page buffer.
        // TODO: no unchecked map.
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
        self.pagebuffer
            .take()
            .map_or(Err(ReturnCode::ERESERVE), move |pagebuffer| {
                let mut entry_cookie = self.read_cookie.get();

                // Skip page header if at start of page or skip padded bytes if at end of page.
                if entry_cookie % self.page_size == 0 {
                    entry_cookie += PAGE_HEADER_SIZE;
                } else if self.get_byte(entry_cookie, pagebuffer) == PAD_BYTE {
                    entry_cookie +=
                        self.page_size - entry_cookie % self.page_size + PAGE_HEADER_SIZE;
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
        self.pagebuffer
            .take()
            .map_or(Err(ReturnCode::ERESERVE), move |pagebuffer| {
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

    /// Writes an entry header at the given cookie within a page. Must write at most
    /// ENTRY_HEADER_SIZE bytes.
    fn write_entry_header(&self, length: usize, cookie: usize, pagebuffer: &mut F::Page) {
        let mut offset = 0;
        for byte in &length.to_ne_bytes() {
            pagebuffer.as_mut()[cookie + offset] = *byte;
            offset += 1;
        }

        assert!(offset <= ENTRY_HEADER_SIZE);
    }

    /// Appends data from a buffer onto the end of the log. Requires that there is enough space
    /// remaining in the pagebuffer for the entry (including metadata).
    fn append_entry(
        &self,
        buffer: &'static mut [u8],
        length: usize,
        pagebuffer: &'static mut F::Page,
    ) {
        // Offset within page to append to.
        let append_cookie = self.append_cookie.get();
        let mut page_offset = append_cookie % self.page_size;

        // Write entry header to pagebuffer.
        self.write_entry_header(length, page_offset, pagebuffer);
        page_offset += ENTRY_HEADER_SIZE;

        // Copy data to pagebuffer.
        for offset in 0..length {
            pagebuffer.as_mut()[page_offset + offset] = buffer[offset];
        }

        // Increment append offset by number of bytes appended.
        let append_cookie = append_cookie + length + ENTRY_HEADER_SIZE;
        self.append_cookie.set(append_cookie);

        // Replace pagebuffer and callback client.
        self.pagebuffer.replace(pagebuffer);
        self.state.set(State::Idle);
        self.client
            .map(move |client| {
                client.append_done(buffer, length, self.records_lost(), ReturnCode::SUCCESS)
            })
            .unwrap();
    }

    /// Flushes the pagebuffer to flash. Log state must be non-idle before calling, else data races
    /// may occur due to asynchronous page write.
    /// ReturnCodes used:
    ///     * SUCCESS: flush started successfully.
    ///     * FAIL: flash driver not configured.
    ///     * EBUSY: flash driver busy.
    fn flush_pagebuffer(&self, pagebuffer: &'static mut F::Page) -> ReturnCode {
        // Pad end of page.
        let mut append_cookie = self.append_cookie.get();
        while append_cookie % self.page_size != 0 {
            pagebuffer.as_mut()[append_cookie % self.page_size] = PAD_BYTE;
            append_cookie += 1;
        }

        // Get flash page to write to and log page being overwritten. Subtract page_size since
        // append cookie points to start of the page following the one we want to flush after the
        // padding operation.
        let page_number = self.page_number(append_cookie - self.page_size);
        let overwritten_page =
            (append_cookie - self.volume.len() - self.page_size) / self.page_size;

        // Advance read and oldest cookies, if within flash page being overwritten.
        let read_cookie = self.read_cookie.get();
        if read_cookie / self.page_size == overwritten_page {
            // Move read cookie to start of next page.
            self.read_cookie.set(
                read_cookie + self.page_size + PAGE_HEADER_SIZE - read_cookie % self.page_size,
            );
        }

        let oldest_cookie = self.oldest_cookie.get();
        if oldest_cookie / self.page_size == overwritten_page {
            self.oldest_cookie.set(oldest_cookie + self.page_size);
        }

        // Sync page to flash.
        let page = pagebuffer.as_mut();
        debug!(
            "Syncing: {:?}...{:?} ({})",
            &page[0..16],
            &page[496..512],
            page_number
        );
        self.driver.write_page(page_number, pagebuffer)
    }

    /// Resets the pagebuffer so that new data can be written. Note that this also increments the
    /// append cookie to point to the start of writable data in this new page. Does not reset
    /// pagebuffer or modify append cookie if the end of a non-circular log is reached.
    fn reset_pagebuffer(&self, pagebuffer: &mut F::Page) {
        // Make sure end of non-circular log has not been reached.
        let mut append_cookie = self.append_cookie.get();
        if !self.circular && self.volume.len() - append_cookie < self.page_size {
            return;
        }

        // Increment append cookie to point at start of next page.
        if append_cookie % self.page_size != 0 {
            append_cookie += self.page_size - append_cookie % self.page_size;
        }

        // Write page header to pagebuffer.
        let cookie_bytes = append_cookie.to_ne_bytes();
        for index in 0..cookie_bytes.len() {
            pagebuffer.as_mut()[index] = cookie_bytes[index];
        }

        // Note: this is the only place where the append cookie can cross page boundaries.
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
    /// ReturnCodes used in read_done callback:
    ///     * SUCCESS: read succeeded.
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
                self.client
                    .map(move |client| {
                        client.read_done(buffer, bytes_read, ReturnCode::SUCCESS);
                    })
                    .unwrap();
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
    /// ReturnCodes used:
    ///     * SUCCESS: seek succeeded.
    ///     * EINVAL: cookie not valid seek position within current log.
    ///     * ERESERVE: no log client set.
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

        // Make client callback on success.
        if status == ReturnCode::SUCCESS {
            self.client.map_or(ReturnCode::ERESERVE, move |client| {
                client.seek_done(status);
                ReturnCode::SUCCESS
            })
        } else {
            status
        }
    }

    /// Get approximate log capacity in bytes.
    fn get_size(&self) -> StorageLen {
        self.capacity
    }
}

impl<'a, F: Flash + 'static, C: LogReadClient + LogWriteClient> LogWrite for LogStorage<'a, F, C> {
    /// Appends an entry onto the end of the log. Entry must fit within a page (including log
    /// metadata).
    /// Returns:
    ///     * Ok(()) on success.
    ///     * Err((ReturnCode, buffer)) on failure.
    /// ReturnCodes used:
    ///     * FAIL: end of non-circular log reached, cannot append any more entries.
    ///     * EBUSY: log busy with another operation, try again later.
    ///     * EINVAL: provided client buffer is too small.
    ///     * ERESERVE: client or internal pagebuffer missing.
    ///     * ESIZE: entry too large to append to log.
    /// ReturnCodes used in append_done callback:
    ///     * SUCCESS: append succeeded.
    ///     * FAIL: write failed due to flash error.
    fn append(
        &self,
        buffer: &'static mut [u8],
        length: usize,
    ) -> Result<(), (ReturnCode, &'static mut [u8])> {
        let entry_size = length + ENTRY_HEADER_SIZE;

        // Check for failure cases.
        if self.state.get() != State::Idle {
            // Busy with another operation.
            return Err((ReturnCode::EBUSY, buffer));
        } else if length <= 0 || buffer.len() < length {
            // Invalid length provided.
            return Err((ReturnCode::EINVAL, buffer));
        } else if entry_size + PAGE_HEADER_SIZE > self.page_size {
            // Entry too big, won't fit within a single page.
            return Err((ReturnCode::ESIZE, buffer));
        } else if !self.circular && self.append_cookie.get() + entry_size >= self.volume.len() {
            // End of non-circular log has been reached.
            // TODO: need to ensure that append cookie NEVER goes beyond end of non-circular log.
            return Err((ReturnCode::FAIL, buffer));
        }

        // Perform append.
        match self.pagebuffer.take() {
            Some(pagebuffer) => {
                // Check if previous page needs to be flushed and new entry will fit within space
                // remaining in current page.
                let append_cookie = self.append_cookie.get();
                let flush_prev_page = append_cookie % self.page_size == 0;
                let space_remaining = self.page_size - append_cookie % self.page_size;
                if !flush_prev_page && entry_size <= space_remaining {
                    // Entry fits, append it.
                    self.append_entry(buffer, length, pagebuffer);
                    Ok(())
                } else {
                    // Need to sync pagebuffer first, then append to new page.
                    self.state.set(State::Write);
                    self.buffer.replace(buffer);
                    self.length.set(length);

                    match self.flush_pagebuffer(pagebuffer) {
                        ReturnCode::SUCCESS => Ok(()),
                        error => {
                            self.state.set(State::Idle);
                            Err((error, self.buffer.take().unwrap()))
                        }
                    }
                }
            }
            None => Err((ReturnCode::ERESERVE, buffer)),
        }
    }

    /// Get cookie representing current append offset.
    fn current_append_offset(&self) -> StorageCookie {
        StorageCookie::Cookie(self.append_cookie.get())
    }

    /// Sync log to storage.
    /// ReturnCodes used:
    ///     * SUCCESS: flush started successfully.
    ///     * FAIL: flash driver not configured.
    ///     * EBUSY: flash driver busy.
    ///     * ERESERVE: no log client set.
    /// ReturnCodes used in sync_done callback:
    ///     * SUCCESS: append succeeded.
    ///     * FAIL: write failed due to flash error.
    fn sync(&self) -> ReturnCode {
        self.pagebuffer
            .take()
            .map_or(ReturnCode::ERESERVE, move |pagebuffer| {
                self.state.set(State::Sync);
                let retval = self.flush_pagebuffer(pagebuffer);

                if retval != ReturnCode::SUCCESS {
                    self.state.set(State::Idle);
                }
                retval
            })
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
                // Debug print, remove this.
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
                        // TODO: don't write if end of non-circular buffer reached.
                        self.reset_pagebuffer(pagebuffer);
                        self.buffer
                            .take()
                            .map(move |buffer| {
                                self.append_entry(buffer, self.length.get(), pagebuffer);
                            })
                            .unwrap();
                    }
                    State::Sync => {
                        // Reset pagebuffer if synced page was full.
                        if self.append_cookie.get() % self.page_size == 0 {
                            self.reset_pagebuffer(pagebuffer);
                        }

                        self.pagebuffer.replace(pagebuffer);
                        self.state.set(State::Idle);
                        self.client
                            .map(move |client| client.sync_done(ReturnCode::SUCCESS))
                            .unwrap();
                    }
                    _ => unreachable!(),
                }
            }
            flash::Error::FlashError => {
                // Make client callback with FAIL return code.
                self.pagebuffer.replace(pagebuffer);
                self.state.set(State::Idle);
                match self.state.get() {
                    State::Write => self
                        .buffer
                        .take()
                        .map(move |buffer| {
                            self.client
                                .map(move |client| {
                                    client.append_done(buffer, 0, false, ReturnCode::FAIL)
                                })
                                .unwrap();
                        })
                        .unwrap(),
                    State::Sync => self
                        .client
                        .map(move |client| client.sync_done(ReturnCode::FAIL))
                        .unwrap(),
                    _ => unreachable!(),
                }
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

                    // TODO: no unchecked map!
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
                        // TODO: no unchecked map!
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
