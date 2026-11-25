use super::{Error, Nor, StoragePort};

#[cfg(fm1)]
unsafe extern "C" {
    fn fm1_storage_probe() -> i32;
    fn fm1_storage_read(off: usize, dst: *mut u8, len: usize) -> i32;
    fn fm1_storage_program(off: usize, src: *const u8, len: usize) -> i32;
    fn fm1_storage_erase(off: usize) -> i32;
}

#[cfg(fm1)]
fn status(value: i32) -> Result<(), Error> {
    match value {
        0 => Ok(()),
        1 => Err(Error::Busy),
        2 => Err(Error::Unsupported),
        4 => Err(Error::OutOfRange),
        _ => Err(Error::Io),
    }
}

#[cfg(fm1)]
impl StoragePort {
    /// # Safety
    /// Requires the system-wide exclusion and linker contract of StoragePort::open.
    pub(super) unsafe fn probe(&mut self) -> Result<(), Error> {
        // SAFETY: the caller establishes the image layout and excludes external XIP
        // and bus users. Board::take_storage claims one core-affine token. The routine
        // checks CPU0, the peer and the XIP mapping with CPU0 IRQs masked. Its
        // SPI command and stack execute entirely from startup-initialized SRAM.
        status(unsafe { fm1_storage_probe() })
    }
}

#[cfg(fm1)]
impl Nor for StoragePort {
    fn read(&mut self, off: usize, dst: &mut [u8]) -> Result<(), Error> {
        // SAFETY: dst exclusively borrows len initialized bytes for this call.
        // The shim checks the reservation and page length, then copies exactly
        // len bytes back after XIP resumes. It retains no pointer or DMA job.
        status(unsafe { fm1_storage_read(off, dst.as_mut_ptr(), dst.len()) })
    }

    fn program(&mut self, off: usize, src: &[u8]) -> Result<(), Error> {
        // SAFETY: src is readable for len bytes until return. The shim copies
        // them to its own stack before suspending XIP. It independently checks
        // page and partition bounds and never mutates or retains src.
        status(unsafe { fm1_storage_program(off, src.as_ptr(), src.len()) })
    }

    fn erase(&mut self, off: usize) -> Result<(), Error> {
        // SAFETY: the exclusive port serializes operations. The linked reservation
        // contains four whole data sectors. The shim checks range and alignment
        // before issuing a single 4 KiB erase with the same CPU/XIP gate as read.
        status(unsafe { fm1_storage_erase(off) })
    }
}

#[cfg(not(fm1))]
impl StoragePort {
    /// # Safety
    /// Mirrors the target probe contract. Host builds never access a device.
    pub(super) unsafe fn probe(&mut self) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
}

#[cfg(not(fm1))]
impl Nor for StoragePort {
    fn read(&mut self, _: usize, _: &mut [u8]) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    fn program(&mut self, _: usize, _: &[u8]) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    fn erase(&mut self, _: usize) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
}
