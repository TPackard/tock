use crate::storage_interface::{
    HasClient, LogRead, LogReadClient, LogWrite, LogWriteClient, StorageCookie, StorageLen,
    SEEK_BEGINNING,
};
use core::cell::Cell;
use core::convert::TryFrom;
use core::mem::size_of;
use core::unreachable;
use kernel::common::cells::{OptionalCell, TakeCell};
use kernel::debug;
use kernel::hil::flash::{self, Flash};
use kernel::ReturnCode;

// TODO: changing this seems to break things...
/// Maximum page header size.
const PAGE_HEADER_SIZE: usize = size_of::<PageHeader>();
/// Maximum entry header size.
const ENTRY_HEADER_SIZE: usize = size_of::<EntryHeader>();

/// Byte used to pad the end of a page.
const PAD_BYTE: u8 = 0xFF;
/// Entry length representing an invalid entry.
const INVALID_ENTRY_LENGTH: usize = 0xFFFFFFFF;

#[derive(Clone, Copy, PartialEq)]
enum State {
    Idle,
    Write,
    Sync,
}

#[derive(Clone, Copy)]
#[repr(packed)]
struct PageHeader {
    //crc: u32,
    position: StorageCookie,
}

#[derive(Clone, Copy)]
#[repr(packed)]
struct EntryHeader {
    length: usize,
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
    /// Header for page currently in pagebuffer.
    page_header: Cell<PageHeader>,
    /// Whether or not the log is circular.
    circular: bool,
    /// Client using LogStorage.
    client: OptionalCell<&'a C>,

    /// Current operation being executed, if asynchronous.
    state: Cell<State>,
    /// Position within log to read from.
    read_position: Cell<StorageCookie>,
    /// Position within log to append to.
    append_position: Cell<StorageCookie>,

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
        debug!("page header size:  {}", size_of::<PageHeader>());
        debug!("entry header size: {}", size_of::<EntryHeader>());

        debug!("LOG STORAGE VOLUME STARTS AT {:?}", volume.as_ptr());
        let page_size = pagebuffer.as_mut().len();
        let capacity = volume.len() - PAGE_HEADER_SIZE * (volume.len() / page_size);

        let log_storage: LogStorage<'a, F, C> = LogStorage {
            volume,
            capacity,
            driver,
            pagebuffer: TakeCell::new(pagebuffer),
            page_size,
            page_header: Cell::new(PageHeader {
                position: SEEK_BEGINNING,
            }),
            circular,
            client: OptionalCell::empty(),
            state: Cell::new(State::Idle),
            read_position: Cell::new(PAGE_HEADER_SIZE),
            append_position: Cell::new(PAGE_HEADER_SIZE), // TODO: need to recover write offset.
            buffer: TakeCell::empty(),
            length: Cell::new(0),
        };

        log_storage
    }

    /// Returns the page number of the page containing a cookie.
    fn page_number(&self, cookie: StorageCookie) -> usize {
        (self.volume.as_ptr() as usize + cookie % self.volume.len()) / self.page_size
    }

    /// Whether or not records may have been previously lost.
    fn records_lost(&self) -> bool {
        self.append_position.get() - PAGE_HEADER_SIZE > self.volume.len()
    }

    /// Gets the data structure containing the given cookie.
    fn get_data<'b>(&self, cookie: StorageCookie, pagebuffer: &'b mut F::Page) -> &'b [u8] {
        // If a cookie is in the same page as the append position and within a page of the append
        // position, then it's one of the newest entries and is stored in the pagebuffer. If it's
        // in the same page, but not within a page of the append position, then it's one of the
        // oldest entries and should be read from flash. This accounts for when the pagebuffer
        // hasn't been flushed and the page it represents still contains older data on flash.
        // TODO: simplify to just a division my dude.
        if cookie / self.page_size == self.append_position.get() / self.page_size {
        //if self.page_number(cookie) == self.page_number(self.append_position.get())
            //&& cookie + self.page_size > self.append_position.get()
        //{
            pagebuffer.as_mut()
        } else {
            self.volume
        }
    }

    /// Gets the byte pointed to by a cookie.
    fn get_byte(&self, cookie: StorageCookie, pagebuffer: &mut F::Page) -> u8 {
        let buffer = self.get_data(cookie, pagebuffer);
        buffer[cookie % buffer.len()]
    }

    /// Gets a `num_bytes` long slice of bytes starting from a cookie.
    fn get_bytes<'b>(
        &self,
        cookie: StorageCookie,
        num_bytes: usize,
        pagebuffer: &'b mut F::Page,
    ) -> &'b [u8] {
        let buffer = self.get_data(cookie, pagebuffer);
        let offset = cookie % buffer.len();
        &buffer[offset..offset + num_bytes]
    }

    /// Advances read_position to the start of the next log entry. Returns the length of the entry
    /// on success, or an error otherwise.
    fn get_next_entry(&self, pagebuffer: &mut F::Page) -> Result<usize, ReturnCode> {
        // Check if end of log was reached.
        let mut read_position = self.read_position.get();
        if read_position == self.append_position.get() {
            return Err(ReturnCode::FAIL);
        }

        // Skip padded bytes if at end of page.
        if self.get_byte(read_position, pagebuffer) == PAD_BYTE {
            read_position += self.page_size - read_position % self.page_size + PAGE_HEADER_SIZE;
            self.read_position.set(read_position);

            if read_position == self.append_position.get() {
                return Err(ReturnCode::FAIL);
            }
        }

        // Get length.
        let length = {
            // TODO: use u32 instead of usize everywhere?
            const LENGTH_SIZE: usize = size_of::<usize>();
            let length_bytes = self.get_bytes(read_position, LENGTH_SIZE, pagebuffer);
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
                        let read_position = self.read_position.get() + ENTRY_HEADER_SIZE;

                        // Copy data into client buffer.
                        let data = self.get_bytes(read_position, entry_length, pagebuffer);
                        for i in 0..entry_length {
                            buffer[i] = data[i];
                        }

                        // Update read offset.
                        self.read_position.set(read_position + entry_length);
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

    /// Writes an entry header at the given position within a page. Returns number of bytes
    /// written.
    fn write_entry_header(
        &self,
        entry_header: EntryHeader,
        position: usize,
        pagebuffer: &mut F::Page,
    ) -> usize {
        let mut offset = 0;
        for byte in &entry_header.length.to_ne_bytes() {
            pagebuffer.as_mut()[position + offset] = *byte;
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
        let append_position = self.append_position.get();
        let mut page_offset = append_position % self.page_size;

        // Write entry header to pagebuffer.
        let entry_header = EntryHeader { length };
        let header_size = self.write_entry_header(entry_header, page_offset, pagebuffer);
        page_offset += header_size;

        // Copy data to pagebuffer.
        for offset in 0..length {
            pagebuffer.as_mut()[page_offset + offset] = buffer[offset];
        }

        // Increment append and buffer offset by number of bytes appended.
        self.increment_append_position(length + header_size);

        // Update state depending on if a page needs to be flushed or not.
        if (append_position + length + header_size) % self.page_size == 0 {
            // Flush page after callback client.
            self.state.set(State::Sync);
            self.client.map(move |client| {
                client.append_done(buffer, length, self.records_lost(), ReturnCode::SUCCESS)
            });
            self.flush_pagebuffer(pagebuffer, true);
        } else {
            // Replace pagebuffer and callback client.
            self.state.set(State::Idle);
            self.pagebuffer.replace(pagebuffer);
            self.client.map(move |client| {
                client.append_done(buffer, length, self.records_lost(), ReturnCode::SUCCESS)
            });
        }
    }

    /// Increments the append position. If a new page is reached, enough space for the page header
    /// is skipped.
    fn increment_append_position(&self, bytes_appended: usize) {
        let mut append_position = self.append_position.get() + bytes_appended;

        // If it's crossed into a new page, skip the header.
        if append_position % self.page_size == 0 {
            append_position += PAGE_HEADER_SIZE;
        }
        self.append_position.set(append_position);
    }

    /// Writes the page header and flushes the pagebuffer to flash.
    fn flush_pagebuffer(&self, pagebuffer: &'static mut F::Page, advance_position: bool) -> ReturnCode {
        let mut append_position = self.append_position.get();
        let page_number = self.page_number(append_position - 1);

        // Write page metadata to pagebuffer.
        // TODO: maybe put this in reset_pagebuffer?
        let page_header = self.page_header.get();
        let mut index = 0;
        for byte in &page_header.position.to_ne_bytes() {
            pagebuffer.as_mut()[index] = *byte;
            index += 1;
        }

        // Pad end of page.
        while append_position % self.page_size != 0 {
            pagebuffer.as_mut()[append_position % self.page_size] = PAD_BYTE;
            append_position += 1;
        }

        // Advance read and append positions.
        let read_position = self.read_position.get();
        if (read_position + self.volume.len()) / self.page_size == (append_position - 1) / self.page_size {
            // Read position clobbered by flush, move to start of next page.
            self.read_position.set(read_position + self.page_size + PAGE_HEADER_SIZE - read_position % self.page_size);
        }
        if advance_position {
            self.append_position.set(append_position + PAGE_HEADER_SIZE);
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
    fn reset_pagebuffer(&self) {
        // Update page header for next page.
        self.page_header.set(PageHeader {
            position: self.append_position.get() - PAGE_HEADER_SIZE,
        });
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
        let read_position = self.read_position.get();
        // TODO: incorrect calculation of storage capacity. Replace this with ability to detect end
        // of log, regardless of whether or not it's circular.
        if !self.circular && read_position + length >= self.volume.len() {
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

    /// Get cookie representing current read position.
    fn current_read_offset(&self) -> StorageCookie {
        self.read_position.get()
    }

    /// Seek to a new read position.
    fn seek(&self, cookie: StorageCookie) -> ReturnCode {
        // TODO: actually validate, also no seeking beyond append position.
        self.read_position.set(cookie);
        let status = ReturnCode::SUCCESS;

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
            && self.append_position.get() + length + ENTRY_HEADER_SIZE >= self.volume.len()
        {
            // End of non-circular log has been reached.
            Err((ReturnCode::FAIL, buffer))
        } else {
            // Check if new entry will fit into current page.
            let result_size =
                self.append_position.get() % self.page_size + length + ENTRY_HEADER_SIZE;
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
                        self.flush_pagebuffer(
                            pagebuffer,
                            true
                        );
                        Ok(())
                    }
                    None => Err((ReturnCode::ERESERVE, buffer)),
                }
            }
        }
    }

    /// Get cookie representing current append position.
    fn current_append_offset(&self) -> StorageCookie {
        self.append_position.get()
    }

    /// Erase the entire log.
    fn erase(&self) -> ReturnCode {
        // TODO: actually erase every page.
        self.read_position.set(PAGE_HEADER_SIZE);
        self.append_position.set(PAGE_HEADER_SIZE);
        self.client
            .map(move |client| client.erase_done(ReturnCode::SUCCESS));
        ReturnCode::SUCCESS
    }

    /// Sync log to storage.
    fn sync(&self) -> ReturnCode {
        let append_position = self.append_position.get();
        if append_position % self.page_size == PAGE_HEADER_SIZE {
            // Pagebuffer empty, don't need to sync.
            self.client
                .map(move |client| client.sync_done(ReturnCode::SUCCESS));
            ReturnCode::SUCCESS
        } else {
            self.pagebuffer
                .take()
                .map_or(ReturnCode::ERESERVE, move |pagebuffer| {
                    self.flush_pagebuffer(pagebuffer, false)
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

    fn write_complete(&self, write_buffer: &'static mut F::Page, error: flash::Error) {
        match error {
            flash::Error::CommandComplete => {
                let append_offset = self.append_position.get() % self.volume.len();
                let offset = if append_offset == PAGE_HEADER_SIZE {
                    self.volume.len() - self.page_size
                } else {
                    append_offset - self.page_size - PAGE_HEADER_SIZE
                };
                debug!(
                    "Synced:  {:?}...{:?} ({})",
                    &self.volume[offset..offset + 16],
                    &self.volume[offset + 496..offset + 512],
                    offset,
                );

                self.reset_pagebuffer();

                match self.state.get() {
                    State::Write => {
                        self.buffer.take().map_or_else(
                            || panic!("No buffer"),
                            move |buffer| {
                                self.append_entry(buffer, self.length.get(), write_buffer);
                            },
                        );
                    }
                    State::Sync => {
                        self.pagebuffer.replace(write_buffer);
                        self.state.set(State::Idle);
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
