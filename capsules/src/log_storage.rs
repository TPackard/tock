use crate::storage_interface::{
    HasClient, LogRead, LogReadClient, LogWrite, LogWriteClient, StorageCookie, StorageLen,
    SEEK_BEGINNING,
};
use core::cell::Cell;
use core::convert::TryFrom;
use core::unreachable;
use kernel::common::cells::{OptionalCell, TakeCell};
use kernel::debug;
use kernel::hil::flash::{self, Flash};
use kernel::ReturnCode;

const PAGE_HEADER_SIZE: usize = core::mem::size_of::<PageHeader>();
const ENTRY_HEADER_SIZE: usize = core::mem::size_of::<EntryHeader>();

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

    /// TODO: do I actually really need state?
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
        debug!("page header size:  {}", core::mem::size_of::<PageHeader>());
        debug!("entry header size: {}", core::mem::size_of::<EntryHeader>());

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

    fn page_number(&self, cookie: StorageCookie) -> usize {
        (self.volume.as_ptr() as usize + cookie % self.volume.len()) / self.page_size
    }

    fn records_lost(&self) -> bool {
        self.append_position.get() - PAGE_HEADER_SIZE > self.volume.len()
    }

    // Advances read_position to the start of the next log entry and returns the length of the
    // entry.
    fn get_next_entry(&self) -> usize {
        // TODO: holy shit this functions is a mess.
        // TODO: what if no remaining entries?
        self.pagebuffer.take().map_or(0xFFFF, move |pagebuffer| {
            let mut read_position = self.read_position.get();
            let buffer_page_number = self.page_number(self.append_position.get());

            // Skip padded bytes if at end of page.
            // TODO: simplify? Should only need this if entry is not in pagebuffer.
            let volume_offset = read_position % self.volume.len();
            let next_byte = if self.page_number(volume_offset) == buffer_page_number {
                pagebuffer.as_mut()[volume_offset % self.page_size]
            } else {
                self.volume[volume_offset]
            };
            if next_byte == 0xFF {
                read_position +=  self.page_size - read_position % self.page_size +
                    PAGE_HEADER_SIZE;
            }

            self.read_position.set(read_position);

            // Get length.
            let volume_offset = read_position % self.volume.len();
            let length_bytes = if self.page_number(volume_offset) == buffer_page_number {
                let offset = volume_offset % self.page_size;
                &pagebuffer.as_mut()[offset..offset+4]
            } else {
                &self.volume[volume_offset..volume_offset+4]
            };
            // TODO: use u32 instead of usize everywhere?
            let length_bytes = <[u8; 4]>::try_from(length_bytes).unwrap();
            let length = usize::from_ne_bytes(length_bytes);
            self.pagebuffer.replace(pagebuffer);
            length
        })
    }

    fn read_entry(&self, buffer: &mut [u8], length: usize) {
        self.pagebuffer.take().map(move |pagebuffer| {
            let mut read_position = self.read_position.get();
            let buffer_page_number = self.page_number(self.append_position.get());

            // Skip entry header.
            read_position += ENTRY_HEADER_SIZE;

            // Copy data into client buffer.
            for offset in 0..length {
                let mut volume_offset = (read_position + offset) % self.volume.len();
                // If a offset has crossed over into a new page, skip page header.
                if volume_offset % self.page_size == 0 {
                    volume_offset += PAGE_HEADER_SIZE;
                    read_position += PAGE_HEADER_SIZE;
                }

                // TODO: differentiate read from head of log on flash vs. tail in pagebuffer.
                if self.page_number(volume_offset) == buffer_page_number {
                    buffer[offset] = pagebuffer.as_mut()[volume_offset % self.page_size];
                } else {
                    buffer[offset] = self.volume[volume_offset];
                }
            }

            // Update read offset.
            read_position += length;
            self.read_position.set(read_position);
            self.pagebuffer.replace(pagebuffer);
        });
    }

    fn append_entry(&self, buffer: &'static mut [u8], pagebuffer: &'static mut F::Page) {
        let length = self.length.get();
        let append_position = self.append_position.get();
        let mut page_offset = append_position % self.page_size;

        // Write entry header to pagebuffer.
        let entry_header = EntryHeader {
            length,
        };
        for byte in &entry_header.length.to_ne_bytes() {
            pagebuffer.as_mut()[page_offset] = *byte;
            page_offset += 1;
        }

        // Copy data to pagebuffer.
        // TODO: oPTiMizE
        let bytes_to_append = core::cmp::min(length, self.page_size - page_offset);
        for offset in 0..bytes_to_append {
            pagebuffer.as_mut()[page_offset + offset] = buffer[offset];
        }

        // Increment append and buffer offset by number of bytes appended.
        self.increment_append_position(bytes_to_append + ENTRY_HEADER_SIZE);

        // Update state depending on if a page needs to be flushed or not.
        let flush_page = (append_position + bytes_to_append + ENTRY_HEADER_SIZE) % self.page_size == 0;
        if flush_page {
            self.state.set(State::Sync);
        } else {
            self.state.set(State::Idle);
        }

        // Append finished, callback client.
        self.client.map(move |client| {
            client.append_done(buffer, length, self.records_lost(), ReturnCode::SUCCESS)
        });

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
            append_position += PAGE_HEADER_SIZE;
        }
        self.append_position.set(append_position);
    }

    fn flush_pagebuffer(&self, pagebuffer: &'static mut F::Page, page_number: usize) -> ReturnCode {
        // TODO: need to erase before writing...
        // Write page metadata to pagebuffer.
        let page_header = self.page_header.get();
        let mut index = 0;
        for byte in &page_header.position.to_ne_bytes() {
            pagebuffer.as_mut()[index] = *byte;
            index += 1;
        }

        // Pad end of page with 0xFF.
        let mut append_position = self.append_position.get();
        while append_position % self.page_size != 0 {
            pagebuffer.as_mut()[append_position % self.page_size] = 0xFF;
            append_position += 1;
        }
        self.append_position.set(append_position + PAGE_HEADER_SIZE);

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
            position: self.append_position.get() - PAGE_HEADER_SIZE, // TODO: is this right?
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
        if length == 0 {
            return ReturnCode::SUCCESS;
        } else if self.state.get() != State::Idle {
            return ReturnCode::EBUSY;
        } else if buffer.len() < length {
            return ReturnCode::EINVAL;
        }

        // Ensure end of log hasn't been reached.
        let read_position = self.read_position.get();
        // TODO: incorrect calculation of storage capacity.
        if !self.circular && read_position + length >= self.volume.len() {
            ReturnCode::FAIL
        } else {
            // TODO: shouldn't be able to read beyond append position.
            let entry_length = self.get_next_entry();
            if entry_length > length {
                return ReturnCode::EINVAL;
            }

            self.read_entry(buffer, entry_length);

            self.client.map(move |client| {
                client.read_done(buffer, entry_length, ReturnCode::SUCCESS)
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
        self.capacity
    }
}

impl<'a, F: Flash + 'static, C: LogReadClient + LogWriteClient> LogWrite for LogStorage<'a, F, C> {
    fn append(&self, buffer: &'static mut [u8], length: usize) -> ReturnCode {
        if length == 0 {
            return ReturnCode::SUCCESS;
        } else if self.state.get() != State::Idle {
            return ReturnCode::EBUSY;
        } else if buffer.len() < length {
            return ReturnCode::EINVAL;
        } else if length + ENTRY_HEADER_SIZE + PAGE_HEADER_SIZE > self.page_size {
            return ReturnCode::ESIZE;
        }

        // Ensure end of log hasn't been reached.
        if !self.circular && self.append_position.get() + length >= self.volume.len() {
            ReturnCode::FAIL
        } else {
            self.length.set(length);

            // Check if new entry will fit into current page.
            if self.append_position.get() % self.page_size + length + ENTRY_HEADER_SIZE <=
                self.page_size {
                self.pagebuffer.take().map_or(ReturnCode::ERESERVE, move |pagebuffer| {
                    self.append_entry(buffer, pagebuffer);
                    ReturnCode::SUCCESS
                })
            } else {
                self.state.set(State::Write);
                self.buffer.replace(buffer);
                self.pagebuffer.take().map_or(ReturnCode::ERESERVE, move |pagebuffer| {
                    self.flush_pagebuffer(pagebuffer, self.page_number(self.append_position.get()));
                    ReturnCode::SUCCESS
                })
            }
        }
    }

    fn current_offset(&self) -> StorageCookie {
        self.append_position.get()
    }

    fn erase(&self) -> ReturnCode {
        // TODO: clear metadata that should be at the head of the volume.
        self.read_position.set(PAGE_HEADER_SIZE);
        self.append_position.set(PAGE_HEADER_SIZE);
        self.client.map(move |client| client.erase_done(ReturnCode::SUCCESS));
        ReturnCode::SUCCESS
    }

    fn sync(&self) -> ReturnCode {
        let append_position = self.append_position.get();
        if append_position % self.page_size == PAGE_HEADER_SIZE {
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
                let offset = if append_offset == PAGE_HEADER_SIZE {
                    self.volume.len() - self.page_size
                } else {
                    append_offset - self.page_size - PAGE_HEADER_SIZE
                };
                debug!(
                    "Write synced: {:?}...{:?} ({})",
                    &self.volume[offset..offset+16],
                    &self.volume[offset+496..offset+512],
                    offset,
                );

                self.reset_pagebuffer();

                match self.state.get() {
                    State::Write => {
                        self.buffer.take().map(move |buffer| {
                            self.append_entry(buffer, write_buffer);
                        });
                    },
                    State::Sync => {
                        self.pagebuffer.replace(write_buffer);
                        self.state.set(State::Idle);
                    },
                    _ => {}
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
