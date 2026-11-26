use super::{RxError, TrsMidiRx};

impl embedded_io::Error for RxError {
    fn kind(&self) -> embedded_io::ErrorKind {
        embedded_io::ErrorKind::Other
    }
}

/// A blocking reader borrowed from [`TrsMidiRx::blocking`]. A nonempty read
/// waits for at least one byte or an overrun.
#[derive(Debug)]
pub struct Blocking<'a>(pub(super) &'a mut TrsMidiRx);

impl embedded_io::ErrorType for Blocking<'_> {
    type Error = RxError;
}
impl embedded_io::ReadReady for Blocking<'_> {
    fn read_ready(&mut self) -> Result<bool, RxError> {
        self.0.collect()?;
        Ok(self.0.available != 0)
    }
}
impl embedded_io::Read for Blocking<'_> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, RxError> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            let n = self.0.try_read(buf)?;
            if n != 0 {
                return Ok(n);
            }
            core::hint::spin_loop();
        }
    }
}
