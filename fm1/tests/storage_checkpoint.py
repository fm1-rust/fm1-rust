"""Exercise the instructions in the linked CPU1 checkpoint up to its return or SRAM entry."""

import os
from pathlib import Path
import sys

sys.path.insert(0, str(Path(os.environ["PI32V2_ISA"]) / "tools/llvm"))
import sim


class PollMemory(sim.Memory):
    """While polling, the request word is the only permitted data access."""

    request = None

    def read(self, address, size):
        if self.request is not None:
            fetch = size == 6 and 0x02000000 <= address < 0x020d4000
            assert fetch or (address, size) == (self.request, 4), f"checkpoint read {address:#x}"
        return super().read(address, size)

    def write(self, address, data):
        assert self.request is None, f"checkpoint wrote {address:#x}"
        super().write(address, data)


class PollCpu(sim.CPU):
    def step1(self, insn, raw, size):
        # The hardware capture model for special-register moves also stores a
        # probe-harness return address. These two actual instructions only read
        # cnum or sp into a register. Supply the core under test here.
        if insn.id == "mov.r_sr" and raw["s"] in (6, 14):
            self.R[raw["r"]] = self.core if raw["s"] == 6 else self.sp
            self.pc += size
            self.steps += 1
        else:
            super().step1(insn, raw, size)


def check_checkpoint(elf):
    cpu = PollCpu()
    cpu.mem = PollMemory()
    symbols = cpu.load_elf(elf)
    request = symbols["park_request"]
    parked = symbols["park_cpu1"]
    token = 0x5041524b
    saved = list(range(4, 16))
    for core, stack, word, target in [
        (1, sim.STACK_TOP, 0, sim.RETURN),
        (1, sim.STACK_TOP, 0x00200000, sim.RETURN),
        (1, sim.STACK_TOP, token ^ 0x00200000, sim.RETURN),
        (0, sim.STACK_TOP, token, sim.RETURN),
        (1, 0x01bffffc, token, sim.RETURN),
        (1, 0x01c80000, token, sim.RETURN),
        (1, sim.STACK_TOP, token, parked),
    ]:
        cpu.mem.write(request, word.to_bytes(4, "little"))
        cpu.mem.request = request
        cpu.pc, cpu.rets, cpu.sp = symbols["fm1_storage_cpu1_checkpoint"], sim.RETURN, stack
        cpu.core = core
        cpu.R[4:] = saved
        for _ in range(64):
            insn, _, _ = cpu.decode(cpu.pc)
            cpu.step()
            assert cpu.rets == sim.RETURN and cpu.sp == stack, "checkpoint changed rets or sp"
            if cpu.pc in (sim.RETURN, parked):
                assert cpu.pc == target, (core, hex(stack), hex(word), hex(cpu.pc))
                assert cpu.R[4:] == saved, "checkpoint clobbered a callee-saved register"
                expected = "rts" if target == sim.RETURN else "goto.r"
                assert insn.id == expected, (insn.id, expected)
                break
        else:
            raise AssertionError("checkpoint neither returned nor entered its SRAM routine")
        cpu.mem.request = None


if __name__ == "__main__":
    check_checkpoint(Path(sys.argv[1]).read_bytes())
