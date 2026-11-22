//! rusb exposes synchronous control/bulk I/O. Keep the single asynchronous
//! isochronous transfer and its callback ownership in this module.
use super::{Arc, DATA_TIMEOUT, IsoCompletion, invalid};
use core::time::Duration;
use rusb::{
    Context, DeviceHandle, UsbContext,
    ffi::{self, constants::LIBUSB_TRANSFER_FREE_TRANSFER},
};
use std::{io, sync::mpsc, time::Instant};

struct Pending {
    // libusb can retain all three until it dispatches completion, even after cancellation.
    _handle: Arc<DeviceHandle<Context>>,
    bytes: Vec<u8>,
    reply: mpsc::Sender<IsoCompletion>,
}

extern "system" fn complete(transfer: *mut ffi::libusb_transfer) {
    // SAFETY: send installs this callback exactly once with one packet and one
    // owned Pending. libusb frees the transfer after this callback returns.
    unsafe {
        let pending = Box::from_raw((*transfer).user_data.cast::<Pending>());
        let packet =
            &*(&raw const (*transfer).iso_packet_desc).cast::<ffi::libusb_iso_packet_descriptor>();
        let _ = pending.reply.send(IsoCompletion {
            transfer_status: (*transfer).status,
            packet_status: packet.status,
            actual: packet.actual_length,
        });
    }
}

pub(super) fn send(handle: &Arc<DeviceHandle<Context>>, data: &[u8]) -> io::Result<IsoCompletion> {
    let len = u16::try_from(data.len())
        .ok()
        .filter(|len| (1..=1023).contains(len))
        .ok_or_else(|| invalid("one full-speed isochronous packet must fit in 1023 bytes"))?;
    let (reply, done) = mpsc::channel();
    let pending = Box::into_raw(Box::new(Pending {
        _handle: Arc::clone(handle),
        bytes: data.to_vec(),
        reply,
    }));
    // SAFETY: this private context is serviced exclusively by the caller. The
    // transfer, handle, payload and completion state outlive every submission.
    let transfer = unsafe {
        let transfer = ffi::libusb_alloc_transfer(1);
        if transfer.is_null() {
            drop(Box::from_raw(pending));
            return Err(io::ErrorKind::OutOfMemory.into());
        }
        ffi::libusb_fill_iso_transfer(
            transfer,
            handle.as_raw(),
            1,
            (*pending).bytes.as_mut_ptr(),
            i32::from(len),
            1,
            complete,
            pending.cast(),
            200,
        );
        ffi::libusb_set_iso_packet_lengths(transfer, u32::from(len));
        (*transfer).flags = LIBUSB_TRANSFER_FREE_TRANSFER;
        let result = ffi::libusb_submit_transfer(transfer);
        if result != 0 {
            ffi::libusb_free_transfer(transfer);
            drop(Box::from_raw(pending));
            return Err(io::Error::other(format!(
                "libusb_submit_transfer returned {result}"
            )));
        }
        transfer
    };
    let start = Instant::now();
    let mut cancelled = false;
    loop {
        if let Ok(result) = done.try_recv() {
            return Ok(result);
        }
        if start.elapsed() > Duration::from_millis(400) {
            // The backend still owns the allocation. A later event dispatch can
            // retire it. Returning must never invalidate a queued transfer.
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "iso cancellation has not completed",
            ));
        }
        if !cancelled && start.elapsed() >= DATA_TIMEOUT {
            // SAFETY: only this loop drives events, and completion was checked above.
            unsafe {
                ffi::libusb_cancel_transfer(transfer);
            }
            cancelled = true;
        }
        if let Err(error) = handle
            .context()
            .handle_events(Some(Duration::from_millis(5)))
            && error != rusb::Error::Interrupted
        {
            // Event handling can deliver a completion even when returning an error.
            if let Ok(result) = done.try_recv() {
                return Ok(result);
            }
            if !cancelled {
                // SAFETY: this private context has a single event dispatcher.
                // The channel check above ruled out completion during the error.
                unsafe {
                    ffi::libusb_cancel_transfer(transfer);
                }
                cancelled = true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
