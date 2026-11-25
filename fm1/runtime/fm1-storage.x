/* Link in addition to fm1.ld or fm1-assets.ld. Physical FM-1 offsets.
   Keep below Felucca's 0xDC000 calibration/preset area and 0xE0000 OTA.
   Do not emit a load segment or padding for the reservation. */
__fm1_storage_start = 0xD8000;
__fm1_storage_limit = 0x02000000 + __fm1_storage_start - 0x4000;
ASSERT(LOADADDR(.data) + SIZEOF(.data) <= __fm1_storage_limit,
       "FM-1 application exceeds the storage boundary")
ASSERT(SIZEOF(.fm1_assets) == 0 || ADDR(.fm1_assets) + SIZEOF(.fm1_assets) <= __fm1_storage_limit,
       "FM-1 assets exceed the storage boundary")
