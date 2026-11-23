// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Leo Kuroshita (@kurogedelic), Hügelton Instruments
//! Full-speed USB-MIDI 1.0 device with one bidirectional virtual cable.
//!
//! Move the board's USB port into one task. Enable `async-usb` to wait for USB0
//! interrupts, or call [`MidiDevice::poll`] about every millisecond. Both paths
//! receive and send complete four-byte event packets without an allocator.
//! A board integration can qualify its [`UsbPort`] once and pass a
//! [`QualifiedMidiPort`] to ordinary safe application code.

#[cfg(feature = "async-usb")]
mod asynchronous;
#[cfg(feature = "async-usb")]
pub use asynchronous::on_usb0_interrupt;
mod control;
mod controller;
#[cfg(feature = "usb-dma-probe")]
mod dma_probe;
#[cfg(feature = "usb-dma-probe")]
pub use dma_probe::{ProbeDisplay, ProbeProfile, ProbeReading};
#[cfg(feature = "usb-log")]
mod log;
use crate::{
    Board, Token,
    time::{Timers, cycles},
};
use control::{Control, Interface, Reply};
use controller::{CSR0, INDEX, rd, wr};
use core::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "usb-log")]
pub use log::{InvalidLogLine, UsbLog};

/// A USB-MIDI 1.0 event packet. Byte 0 contains the cable (high nibble,
/// zero for this device) and Code Index Number; bytes 1..=3 carry MIDI.
pub type EventPacket = [u8; 4];
/// Number of four-byte events that fit in one advertised 64-byte EP1 packet.
pub const MAX_EVENTS: usize = controller::MAX_PACKET / 4;
/// Caller-owned storage for events copied by [`MidiDevice::receive`].
/// USB0 writes to a separate DMA staging area maintained by the driver.
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

/// An exclusive port whose owner has established the normal MIDI RX DMA contract.
/// It has no public fields and can be consumed once to start a MIDI device.
/// Dropping it unused releases the port without starting USB0.
#[derive(Debug)]
pub struct QualifiedMidiPort(UsbPort);

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
    /// Pass the board's DMA assurance to code that chooses the MIDI identity
    /// and starts USB later. This only moves the owned port.
    ///
    /// # Safety
    /// Establish the complete [`Self::into_midi`] contract before calling.
    /// Safe code can hold this port through a cable change, start it later, or
    /// forget the active device. Keep the assurance valid for every such use.
    /// Restricting traffic through a controlled fixture requires control of
    /// the physical connection and the host stack throughout that interval.
    ///
    /// ```compile_fail,E0133
    /// # use fm1::usb_midi::UsbPort;
    /// fn qualify(raw: UsbPort) {
    ///     let _port = raw.assume_midi_dma();
    /// }
    /// ```
    #[must_use]
    pub unsafe fn assume_midi_dma(self) -> QualifiedMidiPort {
        QualifiedMidiPort(self)
    }

    /// Start a timing diagnostic with separate EP1 OUT arenas. The MIDI profile
    /// uses the normal class descriptors and receive handling between trials.
    ///
    /// # Panics
    /// A changed staging guard retained from a previous session is fatal.
    ///
    /// # Safety
    /// The caller must uphold the receive DMA contract of [`Self::into_midi`]
    /// for all normal destinations and the probe's EP1 arena (2048 bytes for
    /// RXD3, 8192 for RXS1). With the stress image, an isolated fixture must
    /// finish its bounded EP1 traffic before the snapshot gate.
    /// On the qualified board, a gate and retarget must stop writes to the
    /// old EP1 destination within the three-millisecond drain used for capture
    /// and reuse. RXD3 uses one isochronous packet per transfer. RXS1's
    /// host and raw fixture submit at most 4096 payload bytes per acquisition.
    /// This stimulus limit bounds the experiment, subject to the board's
    /// behavior with retries, trailers and writes already in progress.
    ///
    /// ```compile_fail,E0133
    /// # use fm1::usb_midi::{Identity, ProbeProfile, UsbPort};
    /// fn arm_probe(port: UsbPort, id: Identity) {
    ///     let _device = port.into_dma_probe(id, ProbeProfile::Iso);
    /// }
    /// ```
    #[cfg(feature = "usb-dma-probe")]
    #[must_use]
    pub unsafe fn into_dma_probe(self, identity: Identity, profile: ProbeProfile) -> MidiDevice {
        // SAFETY: this constructor passes the full DMA obligation to its caller.
        // No endpoint is attached until a later service pass.
        let mut device = unsafe { self.into_midi(identity) };
        device.probe = Some(dma_probe::Probe::new(profile));
        device
    }

    /// Start attachment as a USB-MIDI device. The first service pass after 25 ms
    /// enables the D+ pull-up, giving the host time to forget the bootloader.
    ///
    /// # Panics
    /// A changed receive guard retained from a previous session is fatal.
    ///
    /// # Safety
    /// Every USB0 RX store must land in its programmed data area (2048 bytes
    /// each for normal EP0 and EP1, 8192 in the stress build), including CRC,
    /// padding, rejected packets and stores without an interrupt. The trailing
    /// guard words belong to the CPU. A ready EP0 SETUP or EP1
    /// prefix read by the CPU must remain stable until the acknowledgement.
    /// USB0 must leave CPU-owned bytes alone while the CPU prepares a response,
    /// and old transfers must be quiescent before the storage is reused.
    /// These requirements start before enumeration and include reset, suspend,
    /// reconnect and outstanding writes after detach. A forgotten device
    /// can leave USB0 active indefinitely.
    ///
    /// A controlled setup can rely on a qualified board, an OS and host
    /// controller restricted to ordinary control and 64-byte MIDI bulk
    /// traffic, and a physical connection that preserves those assumptions.
    /// A host application such as `fm1-usb` controls the messages it submits.
    /// Wire packetization and the USB0 SRAM write extent are separate steps.
    /// For an unrestricted cable, establish the controller's hardware bounds
    /// and handoffs, or enforce equivalent isolation before enabling DMA.
    /// A changed guard or an oversized COUNT can first be visible after a store.
    ///
    /// [`Board::take_usb`] only acquires the port and does not start DMA.
    ///
    /// ```compile_fail,E0133
    /// # use fm1::usb_midi::{Identity, UsbPort};
    /// fn start(port: UsbPort, identity: Identity) {
    ///     let _device = port.into_midi(identity);
    /// }
    /// ```
    #[must_use]
    pub unsafe fn into_midi(self, identity: Identity) -> MidiDevice {
        // SAFETY: the caller establishes the same RX contract for this device.
        // Consume the qualified port immediately.
        unsafe { self.assume_midi_dma() }.into_midi(identity)
    }
}

impl QualifiedMidiPort {
    /// Start the normal USB-MIDI device on the previously qualified port.
    /// The diagnostic alternate settings require a separate unsafe activation.
    ///
    /// # Panics
    /// A changed receive guard retained from a previous session is fatal.
    ///
    /// ```
    /// use fm1::usb_midi::{Identity, MidiDevice, QualifiedMidiPort};
    ///
    /// fn start_instrument(port: QualifiedMidiPort, identity: Identity) -> MidiDevice {
    ///     port.into_midi(identity)
    /// }
    /// ```
    #[must_use]
    pub fn into_midi(self, identity: Identity) -> MidiDevice {
        controller::detach();
        controller::init_rx_guards();
        MidiDevice {
            _port: self.0,
            #[cfg(feature = "usb-dma-probe")]
            probe: None,
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

/// One owned USB device. The owning task selects the indexed registers and
/// manages the endpoint buffers. Dropping the handle detaches the device.
#[derive(Debug)]
pub struct MidiDevice {
    _port: UsbPort,
    #[cfg(feature = "usb-dma-probe")]
    probe: Option<dma_probe::Probe>,
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
    /// Read the on-board display values without a host transaction.
    #[cfg(feature = "usb-dma-probe")]
    #[must_use]
    pub fn probe_display(&self) -> Option<ProbeDisplay> {
        self.probe
            .as_ref()
            .map(|p| p.display(self.session().is_some()))
    }

    /// On the MIDI diagnostic, arm a local three-second capture at the
    /// production RX_MAX. Captures the first ready packet or the quiet timeout.
    #[cfg(feature = "usb-dma-probe")]
    pub fn probe_listen(&mut self) {
        if self.session().is_none() || self.interface.out_halted {
            return;
        }
        if let Some(p) = &mut self.probe
            && p.profile == ProbeProfile::Midi
            && !p.active()
        {
            let token = p.next_token();
            self.rx_pending = false;
            if p.arm(
                token,
                0xff,
                token.to_le_bytes()[0].wrapping_sub(1) & 1,
                None,
            )
            .is_err()
            {
                self.retry();
            }
        }
    }

    /// Run one bounded service pass. Attach, enumeration, suspend and cable
    /// reconnection progress even when the application has no MIDI to send.
    ///
    /// # Panics
    /// Detaches and panics on a changed receive guard or an oversized packet
    /// count observed on a normal endpoint.
    pub fn poll(&mut self) {
        if self.service(true).is_err() {
            self.retry();
        }
    }

    /// Identifies the current active host configuration. A reset, suspend or
    /// detected disconnect invalidates it. Release notes and discard queued
    /// events from the previous session when this value changes.
    #[must_use]
    pub fn session(&self) -> Option<u32> {
        (self.interface.configured && !self.suspended).then_some(self.epoch)
    }

    /// Receive up to 16 events from the host. Returns the initialized prefix.
    /// An empty slice means there is currently no transfer to deliver.
    ///
    /// # Panics
    /// A changed receive guard or a ready normal MIDI packet over 64 bytes
    /// is fatal before its payload is copied.
    pub fn receive<'a>(&mut self, buffer: &'a mut PacketBuffer) -> &'a [EventPacket] {
        #[cfg(feature = "usb-dma-probe")]
        if self
            .probe
            .as_ref()
            .is_some_and(|p| p.active() || p.profile != ProbeProfile::Midi)
        {
            return &[];
        }
        let mut n = 0;
        if self.session().is_some() && !self.interface.out_halted && self.rx_pending {
            controller::check_rx_guards();
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
    ///
    /// # Panics
    /// A changed receive guard is fatal before queuing an IN transfer.
    #[must_use]
    pub fn send(&mut self, events: &[EventPacket]) -> usize {
        if self.session().is_none() || self.interface.in_halted {
            return 0;
        }
        controller::check_rx_guards();
        match controller::send(events) {
            Ok(n) => n,
            Err(_) => {
                self.retry();
                0
            }
        }
    }

    fn retry(&mut self) {
        #[cfg(feature = "usb-dma-probe")]
        if let Some(p) = &mut self.probe {
            p.retries = p.retries.saturating_add(1);
        }
        controller::detach();
        self.bus_reset();
        self.wait = Some((cycles(), Timers::CRYSTAL_HZ / 4));
    }

    fn bus_reset(&mut self) {
        #[cfg(feature = "usb-dma-probe")]
        if let Some(p) = &mut self.probe {
            p.reset();
        }
        self.interface = Interface::default();
        self.rx_pending = false;
        self.suspended = false;
        self.control.sending = false;
        self.address = None;
    }

    fn service(&mut self, sample_frames: bool) -> controller::Result<()> {
        controller::check_rx_guards();
        let now = cycles();
        if let Some((since, delay)) = self.wait {
            if now.wrapping_sub(since) < delay {
                return Ok(());
            }
            controller::attach()?;
            #[cfg(feature = "usb-dma-probe")]
            if let Some(p) = &mut self.probe {
                p.stage = p.stage.max(1);
            }
            self.wait = None;
        }
        let flags = rd(controller::INTR_USB)?;
        let ep0 = rd(controller::INTR_TX)? & 1 != 0;
        let rx = rd(controller::INTR_RX)? & 2 != 0;
        if flags & (controller::RESET | controller::DISCONNECT) != 0 {
            self.bus_reset();
            controller::reset()?;
            if flags & controller::RESET == 0 {
                return Ok(()); // Endpoint flags from the disconnected host are stale.
            }
        }
        if flags & controller::SUSPEND != 0 {
            self.suspend(true)?;
            if sample_frames {
                self.frame = controller::frame_number()?;
            }
            self.frame_sample = now;
        }
        if flags & controller::RESUME != 0 {
            self.suspend(false)?;
        }
        self.rx_pending |= rx;
        #[cfg(feature = "usb-dma-probe")]
        let normal_midi = self.probe.is_none();
        #[cfg(not(feature = "usb-dma-probe"))]
        let normal_midi = true;
        if rx && normal_midi && self.interface.configured {
            controller::check_ready_midi_count()?;
        }
        if ep0 {
            self.endpoint_zero(now)?;
        }

        // The SOF pending bit can stay active with the cable out. Felucca
        // checks the 11-bit frame counter, which only advances with a host.
        let elapsed = now.wrapping_sub(self.frame_sample);
        if sample_frames && self.interface.configured && elapsed >= Timers::CRYSTAL_HZ / 100 {
            self.frame_sample = now;
            let frame = controller::frame_number()?;
            if frame != self.frame {
                self.frame = frame;
                self.frame_changed = now;
                self.suspend(false)?;
            } else if elapsed >= 2 * Timers::CRYSTAL_HZ {
                // Allow for the 2.048 s frame-counter wrap when sampling resumes after a gap.
                self.frame_changed = now;
            } else if now.wrapping_sub(self.frame_changed) >= Timers::CRYSTAL_HZ / 10 {
                self.suspend(true)?;
            }
        }
        #[cfg(feature = "usb-dma-probe")]
        if let Some(p) = &mut self.probe
            && p.active()
        {
            p.advance(cycles(), &mut self.rx_pending)?;
        }
        if self.suspended && self.rx_pending && !self.interface.out_halted {
            // A packet already in the FIFO belongs to the old session. Free
            // it without resetting the bulk endpoint's data toggle.
            controller::discard_rx()?;
            self.rx_pending = false;
        }
        Ok(())
    }

    fn suspend(&mut self, suspended: bool) -> controller::Result<()> {
        if suspended && !self.suspended {
            #[cfg(feature = "usb-dma-probe")]
            if let Some(p) = &mut self.probe {
                p.cancel()?;
            }
            self.epoch = self.epoch.wrapping_add(1);
        }
        self.suspended = suspended;
        Ok(())
    }

    fn setup_reply(&mut self, setup: [u8; 8]) -> Reply {
        #[cfg(feature = "usb-dma-probe")]
        if let Some(probe) = &mut self.probe {
            probe.stage = probe.stage.max(2);
            if let Some(reply) =
                probe.request(&mut self.control, &self.identity, self.interface, setup)
            {
                return reply;
            }
        }
        self.control.request(&self.identity, self.interface, setup)
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
            #[cfg(feature = "usb-dma-probe")]
            if let Some(p) = &mut self.probe {
                p.stage = p.stage.max(3);
            }
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
        let count = rd(controller::COUNT)?;
        controller::check_packet_count("EP0 byte count", u16::from(count));
        if count != 8 {
            return wr(CSR0, 0x60);
        }
        let setup = controller::setup();
        let requested = u16::from_le_bytes([setup[6], setup[7]]);
        self.control.sending = false;
        let reply = self.setup_reply(setup);
        match reply {
            Reply::Data(available) if requested != 0 => {
                wr(CSR0, 0x40)?;
                self.control.start(available, requested);
                let (bytes, last) = self.control.chunk();
                return controller::control_in(bytes, last);
            }
            Reply::Address(address) => self.address = Some(address),
            Reply::Configure(configured) => {
                let enabled = configured;
                #[cfg(feature = "usb-dma-probe")]
                let enabled = if let Some(p) = &mut self.probe {
                    controller::probe::gate();
                    p.reset();
                    configured && p.endpoint_enabled()
                } else {
                    enabled
                };
                controller::configure(enabled)?;
                self.interface = Interface {
                    configured,
                    ..Interface::default()
                };
                self.epoch = self.epoch.wrapping_add(1);
                self.rx_pending = false;
                self.suspended = false;
                self.frame = controller::frame_number()?;
                self.frame_changed = now;
                self.frame_sample = now;
                wr(INDEX, 0)?;
            }
            #[cfg(feature = "usb-dma-probe")]
            Reply::Arm {
                token,
                max,
                flags,
                requested,
            } => {
                self.probe.as_mut().expect("diagnostic arm").arm(
                    token,
                    max,
                    flags,
                    Some(requested),
                )?;
                self.rx_pending = false;
                wr(INDEX, 0)?; // ARM selected EP1. EP0 owns the control status stage.
            }
            #[cfg(feature = "usb-dma-probe")]
            Reply::Alternate(alt) => {
                let p = self.probe.as_mut().expect("diagnostic alternate");
                controller::probe::gate();
                p.reset();
                p.alt = alt;
                controller::configure(alt != 0)?;
                self.rx_pending = false;
                self.interface.in_halted = false;
                self.interface.out_halted = false;
                wr(INDEX, 0)?;
            }
            Reply::Halt { input, halted } => {
                #[cfg(feature = "usb-dma-probe")]
                if let Some(p) = &mut self.probe {
                    p.cancel()?;
                }
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
        wr(CSR0, 0x48)?; // ServicedRxPktRdy | DataEnd, status IN sent by hardware.
        #[cfg(feature = "usb-dma-probe")]
        if reply == Reply::Configure(true)
            && let Some(p) = &mut self.probe
        {
            p.stage = 4;
        }
        Ok(())
    }
}

impl Drop for MidiDevice {
    fn drop(&mut self) {
        // Disabling USB0 requests detach. The activation contract also
        // covers outstanding writes before a later owner can reuse the buffers.
        controller::detach();
    }
}
