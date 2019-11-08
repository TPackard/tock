use kernel::ReturnCode;

pub type StorageCookie = usize;
pub type StorageLen = usize;

pub const SEEK_BEGINNING: StorageCookie = 0;

pub trait HasClient<'a, C> {
    /// Set the client for a storage interface. The client will be called when
    /// operations complete.
    fn set_client(&'a self, client: &'a C);
}

/// An interface for reading from log storage.
pub trait LogRead {
    /// Read log data starting from the current read position.
    fn read(&self, buffer: &'static mut [u8], length: StorageLen) -> ReturnCode;

    /// Get cookie representing current read position.
    fn current_offset(&self) -> StorageCookie;

    /// Seek to a new read position.
    fn seek(&self, offset: StorageCookie) -> ReturnCode;

    /// Get approximate log capacity in bytes.
    fn get_size(&self) -> StorageLen;
}

/// Receive callbacks from `LogRead`.
pub trait LogReadClient {
    fn read_done(&self, buffer: &'static mut [u8], length: StorageLen, error: ReturnCode);

    fn seek_done(&self, error: ReturnCode);
}

/// An interface for writing to log storage.
pub trait LogWrite {
    /// Append bytes to the end of the log.
    fn append(&self, buffer: &'static mut [u8], length: StorageLen) -> ReturnCode;

    /// Get cookie representing current append position.
    fn current_offset(&self) -> StorageCookie;

    /// Erase the entire log.
    fn erase(&self) -> ReturnCode;

    /// Sync log to storage.
    fn sync(&self) -> ReturnCode;
}

/// Receive callbacks from `LogWrite`.
pub trait LogWriteClient {
    fn append_done(
        &self,
        buffer: &'static mut [u8],
        length: StorageLen,
        records_lost: bool,
        error: ReturnCode,
    );

    fn erase_done(&self, error: ReturnCode);

    fn sync_done(&self, error: ReturnCode);
}
