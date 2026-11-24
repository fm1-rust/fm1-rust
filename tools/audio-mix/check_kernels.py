"""Check the linked FM-1 mixer using the instruction models fitted to FM-1 hardware.

PI32V2_ISA=/path/to/pi32v2-isa python3 tools/audio-mix/check_kernels.py target/firmware/mix-bench.elf
"""

import os
import random
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(os.environ["PI32V2_ISA"]) / "tools" / "llvm"))
import sim

I32_MIN = -(1 << 31)
I32_MAX = (1 << 31) - 1
LENGTHS = (0, 1, 2, 7, 15, 16, 17, 30, 31, 32, 33, 63, 64, 65, 127, 128, 129, 255, 256, 257)
GAINS = ((16384, 16384), (-32768, 32767), (0, -1), (4251, -7160))


class Memory(sim.Memory):
    """Guard the test buffers, including read bounds and AC791N alignment."""
    regions = None

    def check(self, address, size, write):
        if self.regions is None:
            return
        if not write and 0x02000000 <= address < 0x03000000:
            return  # Instruction fetch from the linked flash image.
        if sim.STACK_TOP - 64 <= address and address + size <= sim.STACK_TOP:
            return  # C ABI register saves.
        assert address % min(4, size) == 0, (address, size)
        assert any(
            start <= address and address + size <= start + length and (writable or not write)
            for start, length, writable in self.regions
        ), (hex(address), size, write)

    def read(self, address, size):
        self.check(address, size, False)
        return super().read(address, size)

    def write(self, address, data):
        self.check(address, len(data), True)
        super().write(address, data)


class Cpu(sim.CPU):
    def step(self):
        instruction, raw, size = self.decode(self.pc)
        if instruction.id != "rep":
            return super().step()
        # The upstream rep model represents the capture harness's r7 += 1 body.
        # Execute the actual encoded body with the upstream instruction models.
        # Register counts follow hw/fm1-2026-11-15-excprobe, groups 4 and 5.
        fields = sim.check.operands(instruction, raw)
        reg = fields["r"]
        count = self.R[reg]
        repeats = count if count < 32 else (count & 31) + 1
        self.R[reg] -= repeats
        start = self.pc + size
        end = start + fields["n"]
        self.steps += 1
        for _ in range(repeats):
            self.pc = start
            while self.pc < end:
                super().step()
            assert self.pc == end
        self.pc = end


def pack(fmt, values):
    return struct.pack(f"<{len(values)}{fmt}", *values)


def check_case(cpu, symbols, rng, name, frames, offset, gains):
    pcm16 = name.endswith("i16")
    mono = name == "mono_i16"
    planar = name == "planar_q23"
    width = 2 if pcm16 else 4
    limit = (1 << (8 * width - 1)) - 1
    edges = [-limit - 1, limit, 0, -1, 1]
    src = [edges[i % len(edges)] if i < 8 else rng.randint(-limit - 1, limit)
           for i in range(frames * (1 if mono else 2))]
    initial = [rng.choice((I32_MIN, I32_MAX, 1, -1, 0, rng.randint(I32_MIN, I32_MAX)))
               for _ in range(2 * frames)]
    dest = 0x30000 + offset * 4  # also cover output pairs at 4 mod 8
    source = 0x20000 + offset * width
    right = source if offset else 0x28000  # shared mono plane is legal
    regions = [(dest, frames * 8, True)]
    cpu.mem.write(dest, pack("i", initial))
    if planar:
        left_plane = src[::2]
        right_plane = left_plane if source == right else src[1::2]
        for address, plane in ((source, left_plane), (right, right_plane)):
            cpu.mem.write(address, pack("i", plane))
            regions.append((address, frames * 4, False))
        src = [sample for pair in zip(left_plane, right_plane) for sample in pair]
    else:
        data = pack("h" if pcm16 else "i", src)
        cpu.mem.write(source, data)
        regions.append((source, len(data), False))

    expected = []
    for i, value in enumerate(initial):
        sample = src[i // 2] if mono else src[i]
        contribution = sample * gains[i % 2] // 64 if pcm16 else sample
        expected.append(max(I32_MIN, min(I32_MAX, value + contribution)))
    packed_gain = (gains[0] & 0xffff) | ((gains[1] & 0xffff) << 16)
    saved = [rng.getrandbits(32) for _ in range(12)]
    cpu.R[4:] = saved
    cpu.mem.regions = regions
    cpu.call(symbols["fm1_audio_mix_v1_" + name], [dest, source, frames, right if planar else packed_gain])
    cpu.mem.regions = None
    assert cpu.mem.read(dest, frames * 8) == pack("i", expected), (name, frames, offset, gains)
    assert cpu.R[4:] == saved and cpu.sp == sim.STACK_TOP


def main():
    cpu = Cpu()
    cpu.mem = Memory()
    symbols = cpu.load_elf(Path(sys.argv[1]).read_bytes())
    rng = random.Random(0xdd5b14)
    cases = 0
    for name in ("mono_i16", "stereo_i16", "stereo_q23", "planar_q23"):
        for n in LENGTHS:
            for offset in (0, 1):
                for gains in GAINS:
                    check_case(cpu, symbols, rng, name, n, offset, gains)
                    cases += 1
    print(f"Checked {cases} mixer cases against scalar arithmetic")


if __name__ == "__main__":
    main()
