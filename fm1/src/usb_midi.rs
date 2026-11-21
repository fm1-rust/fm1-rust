// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Leo Kuroshita (@kurogedelic), Hügelton Instruments
//! Full-speed USB-MIDI 1.0 device with one bidirectional virtual cable.
//!
//! Move the board's USB port into one task, call [`MidiDevice::poll`] about
//! every millisecond, then receive and send complete four-byte event packets.
//! USB needs neither an interrupt binding nor an allocator.

mod control;
mod controller;
use crate::{
    Board, Token,
    time::{Timers, cycles},
};
use control::{Control, Interface, Reply};
use controller::{CSR0, INDEX, rd, wr};
use core::sync::atomic::{AtomicBool, Ordering};

/// A USB-MIDI 1.0 event packet. Byte 0 contains the cable (high nibble,
/// zero for this device) and Code Index Number; bytes 1..=3 carry MIDI.
pub type EventPacket = [u8; 4];
/// Maximum number of USB-MIDI events in one full-speed bulk transfer.
pub const MAX_EVENTS: usize = controller::MAX_PACKET / 4;
/// Storage for one transfer from the host.
pub type PacketBuffer = [EventPacket; MAX_EVENTS];

/// Device descriptors supplied by the firmware. Strings use up to 31 UTF-16 code units each.
#[derive(Clone, Copy, Debug)]
pub struct Identity {
    /// USB vendor ID.
    pub vendor_id: u16,
    /// USB product ID allocated by the vendor.
    pub product_id: u16,
    /// Manufacturer string.
    pub manufacturer: &'static str,
    /// Product name shown by the host.
    pub product: &'static str,
}

/// Exclusive CPU0 handle for the USB0 full-speed device port.
#[derive(Debug)]
pub struct UsbPort(Token);

static CLAIMED: AtomicBool = AtomicBool::new(false);

impl Board {
    /// Acquire the USB0 port before moving other board fields into tasks.
    /// Returns `None` while an existing port or MIDI device owns the controller.
    /// Acquiring the port leaves the USB pads alone until [`UsbPort::into_midi`].
    #[must_use]
    pub fn take_usb(&self) -> Option<UsbPort> {
        (!CLAIMED.swap(true, Ordering::Acquire)).then(|| UsbPort(Token::new()))
    }
}

impl Drop for UsbPort {
    fn drop(&mut self) {
        CLAIMED.store(false, Ordering::Release);
    }
}

impl UsbPort {
    /// Start attachment as a USB-MIDI device. The first poll after 25 ms
    /// enables the D+ pull-up, giving the host time to forget the bootloader.
    #[must_use]
    pub fn into_midi(self, identity: Identity) -> MidiDevice {
        controller::detach();
        MidiDevice {
            _port: self,
            identity,
            wait: Some((cycles(), Timers::CRYSTAL_HZ / 40)),
            control: Control::new(),
            interface: Interface::default(),
            address: None,
            epoch: 0,
            suspended: false,
            rx_pending: false,
            frame: 0,
            frame_sample: 0,
            frame_changed: 0,
        }
    }
}

/// One owned USB device. All register access, including indexed endpoint
/// selection, stays in the task that owns this handle. Dropping it detaches.
#[derive(Debug)]
pub struct MidiDevice {
    _port: UsbPort,
    identity: Identity,
    wait: Option<(u32, u32)>,
    control: Control,
    interface: Interface,
    address: Option<u8>,
    epoch: u32,
    suspended: bool,
    rx_pending: bool,
    frame: u16,
    frame_sample: u32,
    frame_changed: u32,
}

impl MidiDevice {
    /// Run one bounded service pass. Attach, enumeration, suspend and cable
    /// reconnection progress even when the application has no MIDI to send.
    pub fn poll(&mut self) {
        if self.service().is_err() {
            self.retry();
        }
    }

    /// Identifies the current host configuration. A reset, suspend or unplug
    /// invalidates it. Release notes and discard queued events from a previous
    /// session when this value changes.
    #[must_use]
    pub fn session(&self) -> Option<u32> {
        (self.interface.configured && !self.suspended).then_some(self.epoch)
    }

    /// Receive up to 16 events from the host. Returns the initialized prefix.
    /// An empty slice means there is currently no transfer to deliver.
    pub fn receive<'a>(&mut self, buffer: &'a mut PacketBuffer) -> &'a [EventPacket] {
        let mut n = 0;
        if self.session().is_some() && !self.interface.out_halted && self.rx_pending {
            self.rx_pending = false;
            match controller::receive(buffer) {
                Ok(count) => n = count,
                Err(_) => self.retry(),
            }
        }
        &buffer[..n]
    }

    /// Queue the longest prefix that fits in one bulk transfer. Retain and
    /// retry the suffix. Returns zero when disconnected or the endpoint is busy.
    #[must_use]
    pub fn send(&mut self, events: &[EventPacket]) -> usize {
        if self.session().is_none() || self.interface.in_halted {
            return 0;
        }
        match controller::send(events) {
            Ok(n) => n,
            Err(_) => {
                self.retry();
                0
            }
        }
    }

    fn retry(&mut self) {
        controller::detach();
        self.bus_reset();
        self.wait = Some((cycles(), Timers::CRYSTAL_HZ / 4));
    }

    fn bus_reset(&mut self) {
        self.interface = Interface::default();
        self.rx_pending = false;
        self.suspended = false;
        self.control.sending = false;
        self.address = None;
    }

    fn service(&mut self) -> controller::Result<()> {
        let now = cycles();
        if let Some((since, delay)) = self.wait {
            if now.wrapping_sub(since) < delay {
                return Ok(());
            }
            controller::attach()?;
            self.wait = None;
        }
        let flags = rd(controller::INTR_USB)?;
        let ep0 = rd(controller::INTR_TX)? & 1 != 0;
        let rx = rd(controller::INTR_RX)? & 2 != 0;
        if flags & 4 != 0 {
            self.bus_reset();
            controller::reset()?;
        }
        if flags & 1 != 0 {
            self.suspend(true);
            self.frame = controller::frame_number()?;
            self.frame_sample = now;
        }
        if flags & 2 != 0 {
            self.suspend(false);
        }
        self.rx_pending |= rx;
        if ep0 {
            self.endpoint_zero(now)?;
        }

        // The SOF pending bit can stay active with the cable out. Felucca
        // checks the 11-bit frame counter, which only advances with a host.
        if self.interface.configured
            && now.wrapping_sub(self.frame_sample) >= Timers::CRYSTAL_HZ / 100
        {
            self.frame_sample = now;
            let frame = controller::frame_number()?;
            if frame != self.frame {
                self.frame = frame;
                self.frame_changed = now;
                self.suspend(false);
            } else if now.wrapping_sub(self.frame_changed) >= Timers::CRYSTAL_HZ / 10 {
                self.suspend(true);
            }
        }
        if self.suspended && self.rx_pending && !self.interface.out_halted {
            // A packet already in the FIFO belongs to the old session. Free
            // it without resetting the bulk endpoint's data toggle.
            controller::discard_rx()?;
            self.rx_pending = false;
        }
        Ok(())
    }

    fn suspend(&mut self, suspended: bool) {
        if suspended && !self.suspended {
            self.epoch = self.epoch.wrapping_add(1);
        }
        self.suspended = suspended;
    }

    fn endpoint_zero(&mut self, now: u32) -> controller::Result<()> {
        wr(INDEX, 0)?;
        let csr = rd(CSR0)?;
        if csr & 0x10 != 0 {
            wr(CSR0, 0x80)?; // SetupEnd: a new SETUP can already be waiting.
            self.control.sending = false;
            self.address = None;
        } else if let Some(address) = self.address.take() {
            // The host completed SET_ADDRESS's IN status stage at the old address.
            wr(controller::FADDR, address)?;
        }
        if csr & 4 != 0 {
            wr(CSR0, 0)?;
            self.control.sending = false;
        }
        if csr & 1 == 0 {
            if self.control.sending && csr & 2 == 0 {
                let (bytes, last) = self.control.chunk();
                controller::control_in(bytes, last)?;
            }
            return Ok(());
        }
        if rd(controller::COUNT)? != 8 {
            return wr(CSR0, 0x60);
        }
        let setup = controller::setup();
        let requested = u16::from_le_bytes([setup[6], setup[7]]);
        self.control.sending = false;
        match self.control.request(&self.identity, self.interface, setup) {
            Reply::Data(available) if requested != 0 => {
                wr(CSR0, 0x40)?;
                self.control.start(available, requested);
                let (bytes, last) = self.control.chunk();
                return controller::control_in(bytes, last);
            }
            Reply::Address(address) => self.address = Some(address),
            Reply::Configure(configured) => {
                controller::configure(configured)?;
                self.interface = Interface {
                    configured,
                    ..Interface::default()
                };
                self.epoch = self.epoch.wrapping_add(1);
                self.rx_pending = false;
                self.suspended = false;
                self.frame_changed = now;
                self.frame_sample = now;
                wr(INDEX, 0)?;
            }
            Reply::Halt { input, halted } => {
                controller::halt(input, halted)?;
                if input {
                    self.interface.in_halted = halted;
                } else {
                    self.interface.out_halted = halted;
                }
                wr(INDEX, 0)?;
            }
            Reply::Ack | Reply::Data(_) => {}
            Reply::Stall => return wr(CSR0, 0x60),
        }
        wr(CSR0, 0x48) // ServicedRxPktRdy | DataEnd, status IN sent by hardware.
    }
}

impl Drop for MidiDevice {
    fn drop(&mut self) {
        controller::detach();
    }
}
