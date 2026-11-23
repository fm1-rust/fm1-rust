//! Passive observations from the CPU0 USB owner.

/// Last enumeration milestone for the current connection attempt.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EnumerationStage {
    /// Waiting out the initial detach interval before enabling the pull-up.
    #[default]
    Starting,
    /// The controller attached. USB service has yet to observe a bus reset or SETUP.
    Attached,
    /// USB service observed a bus reset and is waiting for SETUP.
    Reset,
    /// EP0 received a SETUP packet at the default address.
    Setup,
    /// The controller applied a nonzero address, or the host cleared its configuration.
    Addressed,
    /// The host selected a configuration and the device queued its status stage.
    Configured,
    /// A configured session is suspended or its frame counter stopped advancing.
    Suspended,
    /// The controller signaled a disconnect after attachment.
    Disconnected,
    /// A controller register access timed out. The device detached and will retry.
    Retry,
}

/// Indexed serial interface engine (SIE) register access that timed out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegisterTimeout {
    /// The controller did not complete a read of this register number.
    Read(u8),
    /// The controller did not complete a write to this register number.
    Write(u8),
}

/// Reason the control engine last asked EP0 to STALL.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlStall {
    /// The control engine rejected this eight-byte SETUP packet.
    Request([u8; 8]),
    /// EP0 reported this byte count when the control engine expected SETUP.
    ByteCount(u8),
}

/// USB service observations retained for the lifetime of one [`super::MidiDevice`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UsbStatus {
    /// Milestone for the current attempt.
    pub stage: EnumerationStage,
    /// Bus reset indications observed, saturating at `u16::MAX`.
    pub resets: u16,
    /// Controller timeouts followed by a detach/retry, saturating at `u16::MAX`.
    pub retries: u16,
    /// Most recent eight-byte SETUP packet, retained across resets and retries.
    pub last_setup: Option<[u8; 8]>,
    /// Last STALL queued by the control engine, retained across later successful requests.
    pub last_stall: Option<ControlStall>,
    /// Most recent SIE timeout, retained even after a later successful enumeration.
    pub last_timeout: Option<RegisterTimeout>,
}
