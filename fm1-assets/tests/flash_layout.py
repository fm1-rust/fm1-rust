#!/usr/bin/env python3
"""Exercise load addresses and both flash ceilings with the actual pi32v2 linker."""

import os
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory

sys.dont_write_bytecode = True
crate = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(crate / "tools"))
sys.path.insert(0, str(Path(os.environ["PI32V2_ISA"]) / "tools/hwtest"))
from package import load_regions

tools = Path(os.environ["PI32V2_BIN"])


def link(directory, text, initialized, assets, script="fm1-assets.ld", should_fit=True, storage=False):
    directory = Path(directory)
    assembly = directory / "test.s"
    obj = directory / "test.o"
    image = directory / "test.elf"
    assembly.write_text(f"""
        .section .entry,"ax"
        .globl _start
    _start:
        rts
        .text
        .space {text}, 0x22
        .data
        .space {initialized}, 0x55
        .section .fm1_assets,"a"
        .space {assets}, 0x77
    """)
    subprocess.run([tools / "clang", "--target=pi32v2", "-c", assembly, "-o", obj],
                   check=True)
    result = subprocess.run([tools / "ld.lld", "-L", crate, "-L", crate.parent / "fm1/runtime",
                             "-T", script, *(["-T", "fm1-storage.x"] if storage else []), obj, "-o", image], capture_output=True, text=True)
    if should_fit:
        assert result.returncode == 0, result.stderr
        return load_regions(image)
    assert result.returncode != 0 and any(s in result.stderr for s in ("overflow", "exceed")), result.stderr
    if storage:
        assert "storage boundary" in result.stderr, result.stderr


with TemporaryDirectory() as directory:
    # .text fits in the first sector. The initialization image crosses into the second.
    entry, app, address, samples = link(directory, 64, 4096, 11)
    assert entry == 0x02000120
    assert app.endswith(bytes([0x55]) * 4096)
    assert address == 0x2000 and samples == bytes([0x77]) * 11

    # Existing BSP applications keep the stock ceiling. Opted-in app.bin can cross it.
    link(directory, 0x90000, 4, 1, script="fm1.ld", should_fit=False)
    _, app, address, _ = link(directory, 0x90000, 4, 1)
    assert len(app) > 0x8eee0 and address == 0x91000

    # The asset end may equal the exclusive ceiling, but one more byte fails to link.
    last = 0xdc000 - address
    link(directory, 0x90000, 4, last)
    link(directory, 0x90000, 4, last + 1, should_fit=False)

    # Storage builds reserve physical 0xD8000. Include alignment and RAM load
    # bytes in the limit. Plain asset builds above keep their full budget.
    last = 0xd4000 - address
    link(directory, 0x90000, 4, last, storage=True)
    link(directory, 0x90000, 4, last + 1, storage=True, should_fit=False)
    link(directory, 0xc8000, 0xd000, 1, storage=True, should_fit=False)
