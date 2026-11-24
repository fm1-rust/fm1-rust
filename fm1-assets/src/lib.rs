#![no_std]
#![doc = include_str!("../README.md")]

#[cfg(all(feature = "encode", not(target_os = "none")))]
extern crate alloc;

#[cfg(feature = "flac")]
pub mod flac;
pub mod pcm;

// Select the placement in this crate so a consumer needs no custom cfg.
macro_rules! define_flash_asset {
    ($(#[$placement:meta])*) => {
        /// Place an immutable array in the FM-1 XIP asset section and borrow it.
        ///
        /// The backing array has its natural alignment. The source is a constant
        /// slice of `Copy` elements. Other targets use normal read-only data.
        ///
        /// ```
        /// const FONT: &[u8] = fm1_assets::flash_asset!(u8, &[0x81, 0x7e]);
        /// assert_eq!(FONT, [0x81, 0x7e]);
        /// ```
        #[macro_export]
        macro_rules! flash_asset {
            ($element:ty, $source:expr) => {{
                const SOURCE: &[$element] = $source;
                $(#[$placement])*
                static DATA: [$element; SOURCE.len()] = $crate::__copy_array(SOURCE);
                &DATA
            }};
            ($element:ty, $source:expr,) => { $crate::flash_asset!($element, $source) };
        }
    };
}

#[cfg(fm1_assets_xip)]
// SAFETY: this section holds immutable Copy arrays. The FM-1 linker script
// places it in read-only XIP and the packager installs the bytes at that VMA.
define_flash_asset!(#[unsafe(link_section = ".fm1_assets")]);
#[cfg(not(fm1_assets_xip))]
define_flash_asset!();

// A Copy bound excludes inline interior mutability. References inside an
// element still point to the referenced object's own storage.
#[doc(hidden)]
#[must_use]
pub const fn __copy_array<T: Copy, const N: usize>(source: &[T]) -> [T; N] {
    *source.first_chunk().expect("constant slice length")
}
