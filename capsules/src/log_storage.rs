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

const LOG_PAGE_MAGIC: u16 = 0xAE86;
const LOG_ENTRY_MAGIC: u16 = 0xFD35;

const PAGE_HEADER_OFFSET: usize = core::mem::size_of::<PageHeader>();
const ENTRY_HEADER_OFFSET: usize = core::mem::size_of::<EntryHeader>();

#[derive(Clone, Copy, PartialEq)]
enum State {
    Idle,
    //Read, // TODO: what do?
    Write,
}

#[derive(Clone, Copy)]
struct PageHeader {
    //crc: u32,
    position: StorageCookie,
    entry_offset: usize,
    _pad: u8, // TODO: remove, this is only so that the test entries cross pages.
}

#[derive(Clone, Copy)]
struct EntryHeader {
    magic: u16,
    length: u16,
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
    /// Header for page currently in pagebuffer.
    page_header: Cell<PageHeader>,
    /// Whether or not the log is circular.
    circular: bool,
    /// Client using LogStorage.
    client: OptionalCell<&'a C>,

    /// TODO: do I actually really need state?
    state: Cell<State>,
    /// Position within log to read from.
    read_position: Cell<StorageCookie>,
    /// Position within log to append to.
    append_position: Cell<StorageCookie>,

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
        debug!("page header size:  {}", core::mem::size_of::<PageHeader>());
        debug!("entry header size: {}", core::mem::size_of::<EntryHeader>());

        debug!("LOG STORAGE VOLUME STARTS AT {:?}", volume.as_ptr());
        let page_size = pagebuffer.as_mut().len();
        let log_storage: LogStorage<'a, F, C> = LogStorage {
            volume,
            driver,
            pagebuffer: TakeCell::new(pagebuffer),
            page_size,
            page_header: Cell::new(PageHeader {
                position: SEEK_BEGINNING,
                entry_offset: 0xFFFF,
                _pad: 0,
            }),
            circular,
            client: OptionalCell::empty(),
            state: Cell::new(State::Idle),
            read_position: Cell::new(PAGE_HEADER_OFFSET),
            append_position: Cell::new(PAGE_HEADER_OFFSET), // TODO: need to recover write offset.
            buffer: TakeCell::empty(),
            buffer_offset: Cell::new(0),
            length: Cell::new(0),
        };

        log_storage
    }

    fn page_number(&self, cookie: StorageCookie) -> usize {
        (self.volume.as_ptr() as usize + cookie % self.volume.len()) / self.page_size
    }

    fn records_lost(&self) -> bool {
        self.append_position.get() - PAGE_HEADER_OFFSET > self.volume.len()
    }

    fn read_chunk(&self, buffer: &mut [u8], length: usize, buffer_offset: usize) {
        self.pagebuffer.take().map(move |pagebuffer| {
            let mut read_position = self.read_position.get();
            let buffer_page_number = self.page_number(self.append_position.get());

            // Copy data into client buffer.
            for offset in 0..length {
                let mut volume_offset = (read_position + offset) % self.volume.len();
                // If a offset has crossed over into a new page, skip page header.
                if volume_offset % self.page_size == 0 {
                    volume_offset += PAGE_HEADER_OFFSET;
                    read_position += PAGE_HEADER_OFFSET;
                }

                // TODO: differentiate read from head of log on flash vs. tail in pagebuffer.
                if self.page_number(volume_offset) == buffer_page_number {
                    buffer[buffer_offset + offset] = pagebuffer.as_mut()[volume_offset % self.page_size];
                } else {
                    buffer[buffer_offset + offset] = self.volume[volume_offset];
                }
            }

            // Update read offset.
            read_position += length;
            self.read_position.set(read_position);
            self.pagebuffer.replace(pagebuffer);
        });
    }

    fn append_chunk(&self, buffer: &'static mut [u8], pagebuffer: &'static mut F::Page) {
        let length = self.length.get();
        let append_position = self.append_position.get();
        let buffer_offset = self.buffer_offset.get();

        // Copy data to pagebuffer.
        // TODO: oPTiMizE
        let page_offset = append_position % self.page_size;
        let bytes_to_append = core::cmp::min(length - buffer_offset, self.page_size - page_offset);
        for offset in 0..bytes_to_append {
            pagebuffer.as_mut()[page_offset + offset] = buffer[buffer_offset + offset];
        }

        // Increment append and buffer offset by number of bytes appended.
        self.increment_append_position(bytes_to_append);
        let buffer_offset = buffer_offset + bytes_to_append;
        self.buffer_offset.set(buffer_offset);

        // Update state depending on if a page needs to be flushed or not.
        let flush_page = (append_position + bytes_to_append) % self.page_size == 0;
        if flush_page {
            self.state.set(State::Write);
        } else {
            self.state.set(State::Idle);
        }

        // Check if append is finished.
        if buffer_offset == length {
            // Append finished, callback client.
            self.client.map(move |client| {
                client.append_done(buffer, length, self.records_lost(), ReturnCode::SUCCESS)
            });
        } else {
            // Append not finished, replace buffer for continuation.
            self.buffer.replace(buffer);
        }

        // Flush page if full.
        if flush_page {
            // TODO: what do if flushing fails?
            let status = self.flush_pagebuffer(pagebuffer, self.page_number(append_position));
        } else {
            self.pagebuffer.replace(pagebuffer);
        }
    }

    fn increment_append_position(&self, bytes_appended: usize) {
        let mut append_position = self.append_position.get() + bytes_appended;

        // If it's crossed into a new page, skip the header.
        if append_position % self.page_size == 0 {
            append_position += PAGE_HEADER_OFFSET;
        }
        self.append_position.set(append_position);
    }

    fn flush_pagebuffer(&self, pagebuffer: &'static mut F::Page, page_number: usize) -> ReturnCode {
        // Write page metadata to pagebuffer.
        let header = self.page_header.get();
        let mut index = 0;
        for byte in &header.position.to_ne_bytes() {
            pagebuffer.as_mut()[index] = *byte;
            index += 1;
        }
        for byte in &header.entry_offset.to_ne_bytes() {
            pagebuffer.as_mut()[index] = *byte;
            index += 1;
        }

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

    fn reset_pagebuffer(&self) {
        // Update page header for next page.
        self.page_header.set(PageHeader {
            position: self.append_position.get() - PAGE_HEADER_OFFSET, // TODO: is this right?
            entry_offset: 0xFFFF,
            _pad: 0,
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
    fn read(&self, buffer: &'static mut [u8], length: usize) -> ReturnCode {
        // TODO: what should happen if the length of the buffer is greater than the size of the log?
        if length == 0 {
            return ReturnCode::SUCCESS;
        } else if self.state.get() != State::Idle {
            return ReturnCode::EBUSY;
        } else if buffer.len() < length {
            return ReturnCode::EINVAL;
        }

        // Ensure end of log hasn't been reached.
        let read_position = self.read_position.get();
        // TODO: volume.len() is incorrect calculation of storage capacity.
        if !self.circular && read_position + length >= self.volume.len() {
            ReturnCode::FAIL
        } else {
            // TODO: shouldn't be able to read beyond append position.
            // TODO: I'm pretty sure I only have to do one read now.
            self.read_chunk(buffer, length, 0);

            self.client.map(move |client| {
                client.read_done(buffer, length, ReturnCode::SUCCESS)
            });

            ReturnCode::SUCCESS
        }
    }

    fn current_offset(&self) -> StorageCookie {
        self.read_position.get()
    }

    fn seek(&self, offset: StorageCookie) -> ReturnCode {
        // TODO: actually validate, also no seeking beyond append offset.
        self.read_position.set(offset);
        let status = ReturnCode::SUCCESS;

        self.client.map(move |client| client.seek_done(status));
        status
    }

    fn get_size(&self) -> StorageLen {
        // TODO: incorrect capacity calculation.
        self.volume.len()
    }
}

impl<'a, F: Flash + 'static, C: LogReadClient + LogWriteClient> LogWrite for LogStorage<'a, F, C> {
    fn append(&self, buffer: &'static mut [u8], length: usize) -> ReturnCode {
        // TODO: make sure length + header sizes is within bounds.
        // TODO: what should happen if the length of the buffer is greater than the size of the log?
        if length == 0 {
            return ReturnCode::SUCCESS;
        } else if self.state.get() != State::Idle {
            return ReturnCode::EBUSY;
        } else if buffer.len() < length || length > self.volume.len() {
            return ReturnCode::EINVAL;
        }

        // Update page header entry offset if first entry in page.
        let append_position = self.append_position.get();
        let mut page_header = self.page_header.get();
        if page_header.entry_offset == 0xFFFF {
            page_header.entry_offset = append_position % self.page_size;
            self.page_header.set(page_header);
        }

        // Ensure end of log hasn't been reached.
        if !self.circular && append_position + length >= self.volume.len() {
            ReturnCode::FAIL
        } else {
            // TODO: shouldn't be able to write more bytes than the size of the volume.
            self.length.set(length);
            self.buffer_offset.set(0);
            self.pagebuffer.take().map_or(ReturnCode::ERESERVE, move |pagebuffer| {
                self.append_chunk(buffer, pagebuffer);
                ReturnCode::SUCCESS
            })
        }
    }

    fn current_offset(&self) -> StorageCookie {
        self.append_position.get()
    }

    fn erase(&self) -> ReturnCode {
        // TODO: clear metadata that should be at the head of the volume.
        self.read_position.set(PAGE_HEADER_OFFSET);
        self.append_position.set(PAGE_HEADER_OFFSET);
        self.client.map(move |client| client.erase_done(ReturnCode::SUCCESS));
        ReturnCode::SUCCESS
    }

    fn sync(&self) -> ReturnCode {
        let append_position = self.append_position.get();
        if append_position % self.page_size == PAGE_HEADER_OFFSET {
            self.client.map(move |client| client.sync_done(ReturnCode::SUCCESS));
            ReturnCode::SUCCESS
        } else {
            let status = self.pagebuffer.take().map_or(ReturnCode::FAIL, move |pagebuffer| {
                self.flush_pagebuffer(pagebuffer, self.page_number(append_position))
            });

            // Advance read offset if invalidated.
            if status == ReturnCode::SUCCESS {
                // TODO
                /*
                let read_offset = self.read_offset.get();
                if read_offset > append_offset && read_offset / self.page_size == append_offset / self.page_size {
                    self.read_offset.set(read_offset + self.page_size - read_offset % self.page_size);
                }
                */
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
                let append_offset = self.append_position.get() % self.volume.len();
                let offset = if append_offset == PAGE_HEADER_OFFSET {
                    self.volume.len() - self.page_size
                } else {
                    append_offset - self.page_size - PAGE_HEADER_OFFSET
                };
                debug!(
                    "Write synced: {:?}...{:?} ({})",
                    &self.volume[offset..offset+16],
                    &self.volume[offset+496..offset+512],
                    offset,
                );

                if self.state.get() == State::Write {
                    self.reset_pagebuffer();

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
