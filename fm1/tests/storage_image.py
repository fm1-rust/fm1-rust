#!/usr/bin/env python3
"""Check that startup copies the linked flash-off code and it references only SRAM."""

import os
from pathlib import Path
import struct
import sys

sys.path.insert(0, os.environ["PI32V2_ISA"])
from pi32v2isa.oracle import read_exec

image = read_exec(Path(sys.argv[1]).read_bytes())
symbols = {name: value for name, value, *_ in image["symbols"]}
start = symbols["__fm1_storage_ram_start"]
end = symbols["__fm1_storage_ram_end"]
assert 0x01c40000 <= symbols["__data_start"] <= start < end <= symbols["__data_end"] <= 0x01c7c000
assert start <= symbols["transfer"] < end
assert start <= symbols["park_cpu1"] < end
gate = {symbols["park_request"], symbols["park_ack"]}
assert len(gate) == 2
assert 0x01c40000 <= symbols["__bss_start"] < symbols["__bss_end"] <= 0x01c7c000
assert all(symbols["__bss_start"] <= addr <= symbols["__bss_end"] - 4
           and addr % 32 == 0 for addr in gate)
peer_state = symbols["fm1_cpu1_live_reply"]
assert symbols["__bss_start"] <= peer_state <= symbols["__bss_end"] - 4 and peer_state % 4 == 0
assert symbols["__fm1_storage_start"] == 0xd8000
assert any(kind == 1 and flags & 1 and va <= start and end <= va + size
           and 0x02000000 <= pa and pa + size <= 0x020d4000
           for kind, _, va, pa, size, _, flags, _ in image["phdrs"])

# This example links with --emit-relocs. The assembler resolves local branches.
# Catch a new reference to a flash helper, jump table, constant or runtime call.
relocations = [s for s in image["sections"] if s["type"] == 4]
assert relocations, "link the storage example with --emit-relocs"
for section in relocations:
    for place, info, addend in struct.iter_unpack("<IIi", section["data"]):
        if start <= place < end:
            name, value, *_ = image["symbols"][info >> 8]
            dest = value + addend
            # Relocation 5 loads a 32-bit address into a register. A direct
            # branch or call must still land in the copied executable range.
            assert start <= dest < end or (info & 255 == 5 and (dest in gate or dest == peer_state)), (
                f"flash-off relocation at {place:#x} to {name}"
            )
