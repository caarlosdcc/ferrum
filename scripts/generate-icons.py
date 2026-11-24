#!/usr/bin/env python3
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.
"""Genera el icono de Ferrum (.ico y .png) sin dependencias externas.

Usa la misma geometría que crates/ferrum/src/logo.rs: hexágono color óxido
con una "F" de hierro. Ejecuta desde la raíz del repositorio:

    python scripts/generate-icons.py
"""

import math
import struct
import zlib
from pathlib import Path

HEX_RADIUS = 0.47
LETTER = [
    (0.36, 0.27, 0.47, 0.73),
    (0.36, 0.27, 0.66, 0.37),
    (0.36, 0.455, 0.60, 0.545),
]
TOP = (242, 140, 80)
BOTTOM = (196, 80, 29)
IRON = (21, 23, 28)
SIZES = [16, 24, 32, 48, 64, 128, 256]
OUT = Path(__file__).resolve().parent.parent / "crates" / "ferrum" / "resources"


def in_hexagon(u, v):
    dx, dy = abs(u - 0.5), abs(v - 0.5)
    return dx <= HEX_RADIUS * 0.8660254 and dy <= HEX_RADIUS - dx / math.sqrt(3)


def in_letter(u, v):
    return any(x0 <= u <= x1 and y0 <= v <= y1 for x0, y0, x1, y1 in LETTER)


def mix(a, b, t):
    return tuple(a[i] + (b[i] - a[i]) * t for i in range(3))


def render(size):
    samples = 8 if size <= 32 else 4
    total = samples * samples
    rows = []
    for y in range(size):
        row = bytearray()
        for x in range(size):
            hexagon = letter = 0
            for sy in range(samples):
                for sx in range(samples):
                    u = (x + (sx + 0.5) / samples) / size
                    v = (y + (sy + 0.5) / samples) / size
                    if in_hexagon(u, v):
                        hexagon += 1
                        if in_letter(u, v):
                            letter += 1
            base = mix(TOP, BOTTOM, (y + 0.5) / size)
            share = letter / hexagon if hexagon else 0.0
            color = mix(base, IRON, share)
            row += bytes(round(c) for c in color)
            row.append(round(hexagon / total * 255))
        rows.append(bytes(row))
    return rows


def png(size, rows):
    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    raw = b"".join(b"\x00" + row for row in rows)
    header = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def ico(images):
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = len(header) + 16 * len(images)
    entries, data = b"", b""
    for size, blob in images:
        side = 0 if size >= 256 else size
        entries += struct.pack("<BBBBHHII", side, side, 0, 0, 1, 32, len(blob), offset + len(data))
        data += blob
    return header + entries + data


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    images = [(size, png(size, render(size))) for size in SIZES]
    (OUT / "ferrum.ico").write_bytes(ico(images))
    (OUT / "ferrum-256.png").write_bytes(images[-1][1])
    print(f"Iconos escritos en {OUT}")


if __name__ == "__main__":
    main()
