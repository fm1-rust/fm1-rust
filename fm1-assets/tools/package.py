"""Pack a linked FM-1 application and its read-only XIP assets for M-Flasher."""

from pathlib import Path
import sys

import fm1app  # From the same pi32v2-isa checkout as the rest of the FM-1 toolchain.
from pi32v2isa.oracle import read_exec


# Area-relative offsets. The loader maps physical 0x4000 to XIP 0x02000000.
ASSETS_LIMIT = 0xE0000 - fm1app.BOOT_AREA
CHIP_KEY = 0x980F


def load_regions(image):
    """Use load addresses so initialized RAM and RAM-resident code stay in app.bin."""
    data = image.read_bytes()
    elf = read_exec(data)
    base = fm1app.FLASH_WINDOW
    entry = elf["entry"]
    if entry != base + fm1app.APP_AT:
        raise ValueError(f"unexpected entry {entry:#x}")

    sections = [s for s in elf["sections"] if s["name"] == ".fm1_assets"]
    if len(sections) != 1:
        raise ValueError("expected exactly one .fm1_assets section")
    section = sections[0]
    if section["type"] != 1 or not section["size"] or section["flags"] & 7 != 2:
        raise ValueError("expected a nonempty read-only .fm1_assets section")
    assets_at = section["addr"] - base
    if (assets_at % fm1app.SECTOR or assets_at <= fm1app.APP_AT
            or assets_at + section["size"] > ASSETS_LIMIT):
        raise ValueError("asset section is unaligned or outside application flash")

    app = bytearray()
    assets = None
    for kind, offset, vaddr, paddr, size, memsize, flags, _ in sorted(elf["phdrs"], key=lambda h: h[3]):
        if kind != 1 or not size:  # PT_LOAD, excluding uninitialized RAM
            continue
        at = paddr - base
        payload = data[offset:offset + size]
        if len(payload) != size:
            raise ValueError("truncated load segment")
        if at == assets_at and at + size <= ASSETS_LIMIT:
            if (assets is not None or vaddr != paddr or flags & 7 != 4
                    or offset != section["offset"] or size != section["size"] or size != memsize):
                raise ValueError("the upper load segment must contain only factory assets")
            assets = payload
        elif fm1app.APP_AT + len(app) <= at and at + size <= assets_at:
            if not app and (paddr != entry or vaddr != entry or not flags & 1):
                raise ValueError("the first executable segment must start at the entry")
            app.extend(b"\xff" * (at - fm1app.APP_AT - len(app)))
            app.extend(payload)
        else:
            raise ValueError(f"unexpected flash segment at {paddr:#x}, size {size:#x}")
    if not app or not assets:
        raise ValueError("missing application or assets")
    app_end = fm1app.APP_AT + len(app)
    if assets_at != app_end + (-app_end % fm1app.SECTOR):
        raise ValueError("assets must follow the last application load byte at the next sector")
    return entry, app, assets_at, assets


def build(image):
    entry, app, assets_at, assets = load_regions(image)
    end = assets_at + len(assets)
    padded = end + (-end % fm1app.SECTOR)
    # Start with the decoded view of erased flash. After ENC the unused sectors
    # between the two files, and the tail of the final sector, will be physical FF.
    area = bytearray(fm1app.area_crypt(b"\xff" * padded, CHIP_KEY))
    area[:fm1app.APP_AT] = b"\xff" * fm1app.APP_AT
    area[fm1app.APP_AT:fm1app.APP_AT + len(app)] = app
    area[assets_at:end] = assets
    for index, (name, at, contents) in enumerate([
        ("app.bin", fm1app.APP_AT, app), ("assets.bin", assets_at, assets),
    ], start=1):
        area[32 * index:32 * (index + 1)] = fm1app.record(
            name, at, len(contents), 0x82, fm1app.crc16(contents)
        )
    area[:32] = fm1app.record("app_area_head", entry, end, 0x83, fm1app.crc16(area[32:end]))
    flash = b"\xff" * fm1app.BOOT_AREA + fm1app.area_crypt(area, CHIP_KEY)
    image.with_suffix(".fwsc").write_bytes(fm1app.package(flash, "FM-1_901"))
    # Raw emulator images begin at the entry and retain the linked asset addresses.
    image.with_suffix(".bin").write_bytes(area[fm1app.APP_AT:end])
    print(f"app.bin {len(app):,} bytes, "
          f"assets.bin {len(assets):,} bytes at physical {fm1app.BOOT_AREA + assets_at:#x}, "
          f"installation ends at {fm1app.BOOT_AREA + padded:#x}, "
          f"{ASSETS_LIMIT - padded:,} whole-sector bytes free")


if __name__ == "__main__":
    build(Path(sys.argv[1]))
