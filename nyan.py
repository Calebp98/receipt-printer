#!/usr/bin/env python3
"""Render Nyan Cat as a dithered bitmap and print it at full head resolution."""
from escpos import Printer, PrinterError, GS
import sys

W, H = 48, 20          # source pixels
SCALE = 12             # dots per source pixel -> 576 dots, the full head width

# fill densities: char -> how many dots of each 4x4 cell are on
DENSITY = {" ": 0, ".": 1, ":": 2, "%": 3, "#": 4}
# which dots of a 4x4 cell light up at each density level
PATTERN = {
    0: set(),
    1: {(0, 0), (2, 2)},
    2: {(0, 0), (2, 2), (1, 3), (3, 1)},
    3: {(0, 0), (2, 2), (1, 3), (3, 1), (0, 2), (2, 0)},
    4: {(x, y) for x in range(4) for y in range(4)},
}

g = [[" "] * W for _ in range(H)]


def rect(x0, y0, x1, y1, fill, outline=None):
    for y in range(y0, y1 + 1):
        for x in range(x0, x1 + 1):
            if 0 <= x < W and 0 <= y < H:
                edge = x in (x0, x1) or y in (y0, y1)
                g[y][x] = outline if (edge and outline) else fill


def disc(cx, cy, r, fill, outline=None):
    for y in range(H):
        for x in range(W):
            d = ((x - cx) ** 2 + (y - cy) ** 2) ** 0.5
            if d <= r:
                g[y][x] = outline if (outline and d > r - 1) else fill


# --- rainbow trail: six bands, stepped like the original ---------------------
BANDS = "#%:.:%"
for i, ch in enumerate(BANDS):
    for x in range(0, 20):
        step = 1 if (x // 4) % 2 else 0
        for y in (4 + 2 * i + step, 5 + 2 * i + step):
            if 0 <= y < H:
                g[y][x] = ch

# --- pop-tart body -----------------------------------------------------------
rect(20, 4, 37, 17, ".", "#")
for sx, sy in [(23, 7), (27, 6), (31, 9), (25, 12), (30, 14), (34, 8), (22, 15), (35, 12)]:
    g[sy][sx] = "#"

# --- tail --------------------------------------------------------------------
rect(17, 7, 20, 8, "%", "#")

# --- head --------------------------------------------------------------------
disc(41, 10, 6, ":", "#")
# ears
for i, (ex, ey) in enumerate([(38, 4), (45, 4)]):
    for r in range(3):
        for c in range(r + 1):
            g[ey + r][ex + c + (0 if i == 0 else -c)] = "#" if r == 2 else "%"
# eyes
for ex in (39, 43):
    rect(ex, 8, ex + 2, 10, " ", "#")
    g[9][ex + 1] = "#"
# cheeks
rect(37, 12, 38, 13, "%")
rect(44, 12, 45, 13, "%")
# mouth
g[13][41] = "#"
g[14][40] = g[14][41] = g[14][42] = "#"

# --- legs --------------------------------------------------------------------
for lx in (22, 27, 32):
    rect(lx, 18, lx + 2, 19, "%", "#")

# --- rasterise ---------------------------------------------------------------
width_dots = W * SCALE
height_dots = H * SCALE
row_bytes = width_dots // 8
raster = bytearray(row_bytes * height_dots)

for py in range(height_dots):
    for px in range(width_dots):
        level = DENSITY[g[py // SCALE][px // SCALE]]
        if (px % 4, py % 4) in PATTERN[level]:
            raster[py * row_bytes + px // 8] |= 0x80 >> (px % 8)

try:
    p = Printer()
except PrinterError as e:
    sys.exit(f"nyan: {e}")

p.init().align("centre")
p.raw(GS + b"v0\x00" + bytes([row_bytes & 0xFF, row_bytes >> 8,
                              height_dots & 0xFF, height_dots >> 8]) + bytes(raster))
p.feed(1).style(bold=True).text("NYAN NYAN NYAN NYAN").style()
p.align("left").feed(2).cut()

try:
    p.send()
    print(f"sent {width_dots}x{height_dots} dots")
except PrinterError as e:
    sys.exit(f"nyan: {e}")
