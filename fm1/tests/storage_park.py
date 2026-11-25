"""Explore all interleavings of the two single-writer words in storage.c.

Each transition is one ordered SRAM access or a change of execution region.
Timers may expire between any two accesses. CPU1 may enter the RAM routine
after the request that prompted it has already expired.
Each CPU1 sample may also differ from the stored request by one bit.
"""

from collections import deque


def reads(word):
    return {word} | {word ^ (1 << bit) for bit in range(32)}


def explore(recheck, request_word=0x5041524b):
    initial = ("drain", "xip", 0, 0)
    todo = deque([initial])
    seen = {initial}
    while todo:
        cpu0, cpu1, request, ack = todo.popleft()
        if cpu0 == "off" and cpu1 not in ("ack", "parked", "withdraw", "recheck"):
            return False
        steps = []
        if cpu0 == "drain" and not ack:
            steps.append(("request", cpu1, request, ack))
        elif cpu0 == "request":
            steps.append(("wait", cpu1, request_word, ack))
        elif cpu0 == "wait":
            steps.append(("release", cpu1, request, ack))  # timeout
            if ack:
                steps.append(("off", cpu1, request, ack))
        elif cpu0 == "off":
            steps.append(("release", cpu1, request, ack))  # SFC restored
        elif cpu0 == "release":
            steps.append(("drain", cpu1, 0, ack))

        observed = reads(request)
        if cpu1 == "xip" and request_word in observed:
            steps.append((cpu0, "enter", request, ack))  # wrapper observed request
        elif cpu1 == "enter":
            steps.append((cpu0, "ack", request, ack))  # SRAM, both IRQs masked
        elif cpu1 == "ack":
            steps.append((cpu0, "parked", request, 1))
        elif cpu1 == "parked" and 0 in observed:
            steps.append((cpu0, "withdraw", request, ack))
        elif cpu1 == "withdraw":
            steps.append((cpu0, "recheck" if recheck else "xip", request, 0))
        elif cpu1 == "recheck":
            if 0 in observed:
                steps.append((cpu0, "xip", request, ack))
            if observed - {0}:
                steps.append((cpu0, "ack", request, ack))

        for state in steps:
            if state not in seen:
                todo.append(state)
                seen.add(state)
    assert any(cpu0 == "off" for cpu0, *_ in seen)
    return True


if __name__ == "__main__":
    assert explore(recheck=True), "CPU1 could leave SRAM while XIP was off"
    assert not explore(recheck=False), "the model missed a stale acknowledgment on retry"
    assert not explore(recheck=True, request_word=1), "a bad bit could release the old one-bit request"
