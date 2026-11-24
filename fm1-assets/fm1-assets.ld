/* Opt in to the FM-1 application area ending at physical 0xE0000.
   In XIP, physical 0x4000 is mapped to 0x02000000. */
__fm1_assets_xip_end = 0x020DC000;
INCLUDE fm1.ld

/* A separate read-only region also works with older BSP linker scripts,
   which limit app.bin itself to the stock application extent. */
MEMORY {
  fm1_asset_window (r) : ORIGIN = 0x02000000, LENGTH = __fm1_assets_xip_end - 0x02000000
}
SECTIONS {
  /* .data runs in RAM. Its initial values and RAM code load from flash. */
  .fm1_assets ALIGN(LOADADDR(.data) + SIZEOF(.data), 0x1000) : {
    *(.fm1_assets)
    KEEP(*(.fm1_assets.keep))
  } > fm1_asset_window
}
ASSERT(ADDR(.fm1_assets) + SIZEOF(.fm1_assets) <= __fm1_assets_xip_end,
       "FM-1 assets exceed the application area")
