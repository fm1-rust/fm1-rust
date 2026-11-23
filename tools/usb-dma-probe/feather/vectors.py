"""Emit full-speed DATA0 packets for the pinned Pico-PIO-USB transmitter."""

import argparse
from pathlib import Path

K, J, SE0, COMPLETE = 1, 3, 0, 2
LENGTHS = (64, 69, 1023, 2047, 2048, 2049, 3072, 4096)


def payload(n):
    # Nonperiodic across the 2048 boundary. Neither sentinel appears in any payload.
    x = 0x3191D29F
    out = bytearray()
    for _ in range(n):
        x ^= (x << 13) & 0xFFFFFFFF
        x ^= x >> 17
        x ^= (x << 5) & 0xFFFFFFFF
        b = (x >> 8) & 0xFF
        out.append({0xA5: 0xA4, 0x5A: 0x5B}.get(b, b))
    return bytes(out)


def crc16(data):
    crc = 0xFFFF
    for byte in data:
        crc ^= byte
        for _ in range(8):
            crc = (crc >> 1) ^ (0xA001 if crc & 1 else 0)
    return crc ^ 0xFFFF


def encode(packet):
    level = K  # instruction indices used in the pinned PIO program
    symbols = []
    ones = 0
    for byte in packet:
        for i in range(8):
            if byte & (1 << i):
                ones += 1
            else:
                level = J if level == K else K
                ones = 0
            symbols.append(level)
            if ones == 6:
                level = J if level == K else K
                symbols.append(level)
                ones = 0
    symbols.extend((SE0, COMPLETE))
    symbols.extend([K] * (4 - len(symbols) % 4))
    return bytes(
        (a << 6) | (b << 4) | (c << 2) | d for a, b, c, d in zip(*[iter(symbols)] * 4)
    )


def decode(encoded):
    levels = [b >> shift & 3 for b in encoded for shift in (6, 4, 2, 0)]
    end = levels.index(SE0)
    assert levels[end + 1] == COMPLETE and all(x == K for x in levels[end + 2 :])
    last = K
    bits = []
    ones = 0
    for level in levels[:end]:
        assert level in (K, J)
        bit = int(level == last)
        last = level
        if ones == 6:
            assert bit == 0
            ones = 0
            continue
        bits.append(bit)
        ones = ones + 1 if bit else 0
    assert len(bits) % 8 == 0
    return bytes(
        sum(bit << i for i, bit in enumerate(bits[n : n + 8]))
        for n in range(0, len(bits), 8)
    )


def array(name, data):
    rows = [
        ",".join(f"0x{b:02x}" for b in data[i : i + 16])
        for i in range(0, len(data), 16)
    ]
    return f"static const uint8_t {name}[] = {{\n" + ",\n".join(rows) + "\n};\n"


def generate():
    assert crc16(b"123456789") == 0xB4C8
    out = [array("reference", payload(max(LENGTHS)))]
    table = []
    for n in LENGTHS:
        for bad in [False, True] if n in (2049, 4096) else [False]:
            data = payload(n)
            crc = crc16(data) ^ (0xFFFF if bad else 0)
            packet = bytes([0x80, 0xC3]) + data + crc.to_bytes(2, "little")
            encoded = encode(packet)
            assert decode(encoded) == packet
            name = f"vector_{n}_{'bad' if bad else 'good'}"
            out.append(array(name, encoded))
            table.append(f"    {{{n}, {str(bad).lower()}, sizeof({name}), {name}}},")
    out.append(
        "static const struct vector vectors[] = {\n" + "\n".join(table) + "\n};\n"
    )
    return "".join(out)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=Path)
    parser.parse_args().output.write_text(generate(), encoding="ascii")
