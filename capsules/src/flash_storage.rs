use kernel::hil::flash::Flash;
use kernel::ReturnCode;

pub struct BlockStorage<'a, F: Flash + 'static> {
    flash: &'a F,
}

impl<F: Flash> BlockStorage<'a, F> {
    pub const fn new(flash: &'a F) -> BlockStorage<'a, F> {
        BlockStorage {
            flash,
        }
    }

    pub fn read(addr: usize, buf: &'static mut [u8], len: usize) -> ReturnCode {
        ReturnCode::ENOSUPPORT
    }

    pub fn write(addr: usize, buf: &'static mut [u8], len: usize) -> ReturnCode {
        ReturnCode::ENOSUPPORT
    }

    pub fn erase() -> ReturnCode {
        ReturnCode::ENOSUPPORT
    }

    pub fn sync() -> ReturnCode {
        ReturnCode::ENOSUPPORT
    }

    pub fn compute_crc(addr: usize, len: usize, crc: u16) -> ReturnCode {
        ReturnCode::ENOSUPPORT
    }

    pub fn get_size() -> usize {
        0
    }
}

pub trait BlockStorageClient {
    fn read_complete(&self, read_buffer: &'static mut [u8], error: ReturnCode);

    fn write_complete(&self, write_buffer: &'static mut [u8], error: ReturnCode);

    fn erase_complete(&self, error: ReturnCode);

    fn sync_complete(&self, error: ReturnCode);

    fn compute_crc_complete(&self, crc: u16, error: ReturnCode);
}
