#!/usr/bin/env python3
"""Check an FM-1 FWSC package and extract the exact app bytes for the emulator."""
import argparse
from binascii import crc_hqx
from pathlib import Path
import struct

RECORD = struct.Struct("<HHIIBBH16s")


def check(condition, message):
    if not condition:
        raise ValueError(message)


def crypt(data, key):
    result = bytearray()
    for b in data:
        result.append(b ^ (key & 255))
        key = ((key << 1) ^ (0x1021 if key & 0x8000 else 0)) & 65535
    return result


def application(pkg):
    marks = pkg[47:20 * 48:48]
    name = bytes((m-i-1) & 255 for i, m in enumerate(marks) if m != 0x7d)
    check(name == b"FM-1_901", "unexpected package identity")
    head_blocks = b"".join(pkg[i:i+47] for i in range(0, 20*48, 48))
    image = head_blocks + pkg[20*48:]
    header = crypt(image[:64], 65535)
    files = crypt(image[64:144], 65535)
    check(header[16:22] == b"AC791N" and files[64:73] == b"flash.bin", "unexpected package records")
    check(struct.unpack_from("<HHI", header) ==
          (crc_hqx(header[2:], 0), crc_hqx(image[64:144], 0), len(image)), "bad package header")
    crc, _, start, size, size2 = struct.unpack_from("<HHIII", files, 4)
    flash = image[start:start+size]
    check(size == size2 == len(flash) and crc_hqx(flash, 0) == crc, "bad flash payload")
    check(flash[:0x4000] == b"\xff"*0x4000, "unexpected boot-area contents")
    data = flash[0x4000:]
    area = b"".join(crypt(data[i:i+32], 0x980f ^ ((i >> 2) & 65535)) for i in range(0, len(data), 32))
    head = RECORD.unpack_from(area)
    rec = RECORD.unpack_from(area, 32)
    check(head[0] == crc_hqx(area[2:32], 0) and rec[0] == crc_hqx(area[34:64], 0), "bad record CRC")
    check(head[-1].rstrip(b"\0") == b"app_area_head" and rec[-1].rstrip(b"\0") == b"app.bin", "missing app")
    check(head[2] == 0x02000120 and head[1] == crc_hqx(area[32:head[3]], 0), "bad app area")
    check(rec[2] == 0x120, "unexpected app load offset")
    app = area[rec[2]:rec[2]+rec[3]]
    check(len(app) == rec[3] and crc_hqx(app, 0) == rec[1], "bad application CRC")
    return app


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("package", type=Path)
    parser.add_argument("app", type=Path)
    args = parser.parse_args()
    data = application(args.package.read_bytes())
    args.app.write_bytes(data)
    print(f"{args.package.name}: {len(data)} application bytes")


if __name__ == "__main__":
    main()
