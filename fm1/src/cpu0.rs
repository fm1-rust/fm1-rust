//! Transfer an owned peripheral between preemptible tasks on CPU0.
#[cfg(test)]
extern crate std;

use core::{
    fmt,
    mem::ManuallyDrop,
    ops::{Deref, DerefMut},
};

use crate::{
    Board, HeadphoneJack,
    display::{Display, DisplayPort},
    panel::{Controls, Matrix, Play, TouchControls},
    time::Timers,
    usb_midi::{MidiDevice, QualifiedMidiPort, UsbPort},
};

/// An FM-1 peripheral that can be handed to an interrupt task requiring `Send`.
///
/// Wrap an owned handle with `Cpu0::from(handle)`. Access and destruction check
/// the executing core. Dropping on another core panics and leaves the raw
/// handle untouched. Use [`Self::into_inner`] on CPU0 to call a consuming
/// method such as [`DisplayPort::init`]. Raw handles retain their CPU affinity.
///
/// ```
/// use fm1::{Cpu0, display::DisplayPort};
/// fn hand_off(port: DisplayPort) -> impl Send { Cpu0::from(port) }
/// ```
///
/// Only owned BSP peripherals can be wrapped.
/// ```compile_fail
/// use core::cell::Cell;
/// use fm1::Cpu0;
/// fn share(cell: &Cell<u32>) -> impl Send { Cpu0::from(cell) }
/// ```
pub struct Cpu0<T>(ManuallyDrop<T>);

// This list contains only owned drivers that can change interrupt priority
// under exclusive ownership on CPU0. In particular, accepting arbitrary
// non-Send values here would allow two tasks to share an Rc or a &Cell.
trait Peripheral {}
macro_rules! peripherals {
    ($($ty:ty),+ $(,)?) => { $(impl Peripheral for $ty {})+ };
}
peripherals!(
    Board,
    HeadphoneJack,
    Display,
    DisplayPort,
    Controls,
    Matrix,
    Play,
    TouchControls,
    crate::power::Power,
    Timers,
    MidiDevice,
    QualifiedMidiPort,
    UsbPort
);
#[cfg(feature = "flash-storage")]
impl Peripheral for crate::storage::StoragePort {}
#[cfg(feature = "audio")]
impl Peripheral for crate::audio::AudioPort {}
#[cfg(feature = "audio")]
impl<const N: usize> Peripheral for crate::audio::Stream<'_, N> {}

impl<T: Peripheral> From<T> for Cpu0<T> {
    fn from(value: T) -> Self {
        assert_cpu0();
        Self(ManuallyDrop::new(value))
    }
}

// SAFETY: safe constructors accept only the audited owned peripherals above.
// Every access, including Debug, extraction and Drop, checks CPU0. Exclusive
// ownership serializes mutation across interrupt priorities. No Sync impl is added.
unsafe impl<T: Peripheral> Send for Cpu0<T> {}

impl<T> Cpu0<T> {
    /// Recover the raw peripheral on CPU0.
    #[must_use]
    pub fn into_inner(self) -> T {
        let mut this = ManuallyDrop::new(self);
        assert_cpu0();
        // SAFETY: we own the wrapper and have suppressed its destructor.
        unsafe { ManuallyDrop::take(&mut this.0) }
    }
}

impl<T> Deref for Cpu0<T> {
    type Target = T;
    fn deref(&self) -> &T {
        assert_cpu0();
        &self.0
    }
}
impl<T> DerefMut for Cpu0<T> {
    fn deref_mut(&mut self) -> &mut T {
        assert_cpu0();
        &mut self.0
    }
}
impl<T: fmt::Debug> fmt::Debug for Cpu0<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}
impl<T> Drop for Cpu0<T> {
    fn drop(&mut self) {
        assert_cpu0();
        // SAFETY: this is the sole destructor and we just verified affinity.
        unsafe { ManuallyDrop::drop(&mut self.0) };
    }
}

#[inline]
pub(super) fn on_cpu0() -> bool {
    #[cfg(fm1)]
    {
        unsafe extern "C" {
            fn fm1_core_id() -> u32;
        }
        // SAFETY: this shim only reads the current core number.
        unsafe { fm1_core_id() == 0 }
    }
    #[cfg(all(not(fm1), test))]
    {
        TEST_CPU0.get()
    }
    #[cfg(all(not(fm1), not(test)))]
    {
        false
    }
}

fn assert_cpu0() {
    assert!(on_cpu0(), "FM-1 peripheral requires CPU0");
}

#[cfg(all(test, not(fm1)))]
std::thread_local! { static TEST_CPU0: core::cell::Cell<bool> = const { core::cell::Cell::new(true) }; }

#[cfg(all(test, not(fm1)))]
mod tests {
    use super::*;
    use core::panic::AssertUnwindSafe;
    use core::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    use std::panic::catch_unwind;

    #[derive(Debug)]
    struct Probe(&'static AtomicUsize);
    impl Peripheral for Probe {}
    impl Drop for Probe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Relaxed);
        }
    }

    #[test]
    fn foreign_core_cannot_access_extract_or_destroy_the_peripheral() {
        static DROPS: AtomicUsize = AtomicUsize::new(0);
        let access = Cpu0::from(Probe(&DROPS));
        let extract = Cpu0::from(Probe(&DROPS));
        let destroy = Cpu0::from(Probe(&DROPS));
        std::thread::spawn(move || {
            TEST_CPU0.set(false);
            assert!(
                catch_unwind(AssertUnwindSafe(|| {
                    let _ = &*access;
                }))
                .is_err()
            );
            assert!(catch_unwind(AssertUnwindSafe(|| extract.into_inner())).is_err());
            assert!(catch_unwind(AssertUnwindSafe(|| drop(destroy))).is_err());
            assert_eq!(DROPS.load(Relaxed), 0);
            TEST_CPU0.set(true);
            drop(access.into_inner());
        })
        .join()
        .expect("core check");
        assert_eq!(DROPS.load(Relaxed), 1);
    }
}
