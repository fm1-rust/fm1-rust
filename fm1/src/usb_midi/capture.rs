//! UAC1 capture on the FM-1 audio clock. USB service owns the read cursor.
#[cfg(test)]
mod tests;
use super::{
    Identity,
    control::{CAPTURE_CONFIG, Control, Interface, Reply},
};
use core::sync::atomic::{AtomicU32, Ordering};

const SLOTS: u32 = 384;
const ACTIVE: u32 = 1 << 16;
const CURSOR: u32 = 0xffff;
const PRIME: u32 = 192;
const MAX_FRAMES: usize = 46;
pub(super) const MAX_BYTES: usize = MAX_FRAMES * 4;

/// CPU-owned stereo FIFO shared by the renderer and the USB service task.
/// USB DMA has a separate permanent packet buffer.
#[derive(Debug)]
pub struct CaptureBuffer {
    frames: [AtomicU32; SLOTS as usize],
    // Low 16 bits are the wrapping producer cursor. Upper bits identify the
    // session so an interrupted write cannot commit across a restart.
    head: AtomicU32,
    tail: AtomicU32,
}

impl Default for CaptureBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl CaptureBuffer {
    /// Create an empty FIFO. Allocate one in static storage per device.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            frames: [const { AtomicU32::new(0) }; SLOTS as usize],
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
        }
    }

    pub(super) fn split(&'static mut self) -> (CaptureWriter, Stream) {
        (
            CaptureWriter(self),
            Stream {
                buffer: self,
                selected: false,
                primed: false,
                phase: 0,
                last: None,
            },
        )
    }

    fn start(&self, running: bool) {
        let epoch =
            (self.head.load(Ordering::Relaxed) & !(CURSOR | ACTIVE)).wrapping_add(2 * ACTIVE);
        self.head.store(epoch, Ordering::Release);
        self.tail.store(0, Ordering::Relaxed);
        if running {
            self.head.store(epoch | ACTIVE, Ordering::Release);
        }
    }

    fn frame(&self, cursor: u32) -> &AtomicU32 {
        let index = if cursor >= SLOTS {
            cursor.wrapping_sub(SLOTS)
        } else {
            cursor
        };
        &self.frames[index as usize]
    }
}

// Cursors make two laps of the storage, distinguishing a full FIFO from an
// empty one while using exactly three 128-frame render blocks of SRAM.
fn advance(cursor: u32) -> u32 {
    let next = cursor.wrapping_add(1);
    if next == 2 * SLOTS { 0 } else { next }
}

fn available(head: u32, tail: u32) -> u32 {
    let cursor = head & CURSOR;
    if cursor >= tail {
        cursor.wrapping_sub(tail)
    } else {
        (2 * SLOTS).wrapping_sub(tail).wrapping_add(cursor)
    }
}

/// Single producer of signed Q23 stereo frames. It never waits for the host.
#[derive(Debug)]
#[allow(
    missing_copy_implementations,
    reason = "One producer exclusively publishes the FIFO write cursor."
)]
pub struct CaptureWriter(&'static CaptureBuffer);

impl CaptureWriter {
    /// Copy the prefix that fits and return its frame count. Returns zero when
    /// the host has closed the stream. Each frame is `[left, right]`; 24-bit
    /// samples are clipped and reduced to signed 16-bit PCM.
    pub fn write_q23(&mut self, frames: &[[i32; 2]]) -> usize {
        let buf = self.0;
        let head = buf.head.load(Ordering::Acquire);
        if head & ACTIVE == 0 {
            return 0;
        }
        let tail = buf.tail.load(Ordering::Acquire);
        let fill = available(head, tail);
        let n = (SLOTS.saturating_sub(fill) as usize).min(frames.len());
        let mut cursor = head & CURSOR;
        for &[left, right] in &frames[..n] {
            let sample = |x: i32| {
                let bytes = x.clamp(-8_388_608, 8_388_607).to_le_bytes();
                [bytes[1], bytes[2]]
            };
            let [a, b] = sample(left);
            let [c, d] = sample(right);
            buf.frame(cursor)
                .store(u32::from_le_bytes([a, b, c, d]), Ordering::Relaxed);
            cursor = advance(cursor);
        }
        let next = (head & !CURSOR) | (cursor & CURSOR);
        if buf
            .head
            .compare_exchange(head, next, Ordering::Release, Ordering::Relaxed)
            .is_ok()
        {
            n
        } else {
            0
        }
    }
}

#[derive(Debug)]
pub(super) struct Stream {
    buffer: &'static CaptureBuffer,
    pub(super) selected: bool,
    primed: bool,
    phase: u8,
    last: Option<u32>,
}

impl Stream {
    pub(super) fn select(&mut self, active: bool) {
        self.selected = active;
        self.restart(active);
    }

    pub(super) fn restart(&mut self, running: bool) {
        self.buffer.start(running);
        self.primed = false;
        self.phase = 0;
        self.last = None;
    }

    pub(super) fn request(
        &self,
        control: &mut Control,
        id: &Identity,
        state: Interface,
        setup: [u8; 8],
    ) -> Option<Reply> {
        let configured = state.configured;
        Some(match setup {
            [0x80, 6, 0, 1, 0, 0, _, _] => control.device(id, 0x10),
            [0x80, 6, 0, 2, 0, 0, _, _] => control.data(&CAPTURE_CONFIG),
            [0x81, 0, 0, 0, 2, 0, 2, 0] if configured => control.data(&[0, 0]),
            [0x81, 10, 0, 0, 1, 0, 1, 0] if configured => control.data(&[u8::from(self.selected)]),
            [0x81, 10, 0, 0, 2, 0, 1, 0] if configured => control.data(&[0]),
            [1, 11, alt @ 0..=1, 0, 1, 0, 0, 0] if configured => Reply::CaptureAlternate(alt == 1),
            [1, 11, 0, 0, 2, 0, 0, 0] if configured => Reply::Ack,
            [0x82, 0, 0, 0, 0x84, 0, 2, 0] if configured && self.selected => control.data(&[0, 0]),
            _ => return None,
        })
    }

    pub(super) fn packet(&mut self, now: u32, out: &mut [u8; MAX_BYTES]) -> u8 {
        // A host can stop IN tokens without selecting alt zero. Start fresh
        // when it resumes rather than recording the old contents of the FIFO.
        if self
            .last
            .is_some_and(|last| now.wrapping_sub(last) > crate::time::Timers::CRYSTAL_HZ / 50)
        {
            self.restart(true);
        }
        self.last = Some(now);
        let buf = self.buffer;
        let head = buf.head.load(Ordering::Acquire);
        let mut tail = buf.tail.load(Ordering::Relaxed);
        let fill = available(head, tail);
        // 24 MHz / 544 gives 44 + 2/17 stereo frames per nominal USB ms.
        // The watermarks correct drift between the host and device crystals
        // without reacting to the normal 128-frame DAC block arrivals.
        self.phase = self.phase.wrapping_add(2);
        let mut n = 44u8;
        if self.phase >= 17 {
            self.phase = self.phase.wrapping_sub(17);
            n = 45;
        }
        self.primed |= fill >= PRIME;
        if self.primed {
            if fill < 96 {
                n = n.saturating_sub(1);
            }
            if fill > SLOTS - 128 {
                n = n.saturating_add(1);
            }
        }
        let bytes = n.saturating_mul(4);
        let packet = &mut out[..usize::from(bytes)];
        if self.primed && fill >= u32::from(n) {
            for word in packet.as_chunks_mut::<4>().0 {
                word.copy_from_slice(&buf.frame(tail).load(Ordering::Relaxed).to_le_bytes());
                tail = advance(tail);
            }
            buf.tail.store(tail, Ordering::Release);
        } else {
            if self.primed {
                buf.tail.store(head & CURSOR, Ordering::Release);
                self.primed = false;
            }
            packet.fill(0);
        }
        bytes
    }
}
