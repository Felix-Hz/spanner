"""Render the 16x16 pixel mark to assets/windows/spanner.ico and ui/icon.svg.

    python -I scripts/make_icon.py

Standard library only. Larger icon sizes are integer nearest-neighbour upscales, so edges stay crisp.
"""

import struct
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GRID = 16
INK = (0x1D, 0x3E, 0xA8)  # --color-accent
PAPER = (0xFA, 0xFA, 0xF8)  # --color-paper
SIZES = [16, 32, 48, 64, 128, 256]

# Open-ended spanner on the diagonal: jaw top right, handle to bottom left. The head is mirror-symmetric
# about the handle axis (x + y = 15) with a three-cell slot, so the two prongs read as jaws rather than
# an enclosed hole. Bounding box is columns 2-13 and rows 2-13: an equal 2 px margin on every side.
MARK = """
................
................
..........##....
.........##.....
.........##..#..
.........#####..
.........####...
........##......
.......##.......
......##........
.....##.........
....##..........
...##...........
..##............
................
................
"""


def mark() -> set[tuple[int, int]]:
    rows = MARK.strip("\n").split("\n")
    assert len(rows) == GRID and all(len(r) == GRID for r in rows), "mark must be 16x16"
    return {(x, y) for y, row in enumerate(rows) for x, c in enumerate(row) if c == "#"}


def png(size: int, on: set[tuple[int, int]]) -> bytes:
    scale = size // GRID
    raw = bytearray()
    for y in range(size):
        raw.append(0)  # filter: none
        for x in range(size):
            raw += bytes(PAPER if (x // scale, y // scale) in on else INK) + b"\xff"

    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)  # 8-bit RGBA
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(bytes(raw), 9)) + chunk(b"IEND", b"")


def ico(on: set[tuple[int, int]]) -> bytes:
    images = [png(s, on) for s in SIZES]
    out = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    for s, img in zip(SIZES, images):
        dim = 0 if s == 256 else s  # 0 encodes 256
        out += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(img), offset)
        offset += len(img)
    return out + b"".join(images)


def svg(on: set[tuple[int, int]]) -> str:
    path = "".join(f"M{x} {y}h1v1h-1z" for x, y in sorted(on, key=lambda p: (p[1], p[0])))
    hexc = lambda c: "#%02x%02x%02x" % c  # noqa: E731
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" shape-rendering="crispEdges">'
        f'<title>spanner</title><rect width="16" height="16" fill="{hexc(INK)}"/>'
        f'<path fill="{hexc(PAPER)}" d="{path}"/></svg>\n'
    )


def main() -> None:
    on = mark()
    (ROOT / "assets" / "windows" / "spanner.ico").write_bytes(ico(on))
    (ROOT / "ui" / "icon.svg").write_text(svg(on), encoding="utf-8")


if __name__ == "__main__":
    main()
