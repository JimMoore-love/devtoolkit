"""DevToolkit 图标生成器 - 纯标准库绘制 PNG + ICO"""
import math
import struct
import zlib
import os

OUT = os.path.join(os.path.dirname(__file__), "..", "src-tauri", "icons")


def make_png(size):
    S = size
    u = S / 256.0  # scale unit

    def clamp01(x):
        return max(0.0, min(1.0, x))

    def sd_round_rect(px, py, cx, cy, hw, hh, r):
        qx = abs(px - cx) - (hw - r)
        qy = abs(py - cy) - (hh - r)
        return min(max(qx, qy), 0.0) + math.hypot(max(qx, 0.0), max(qy, 0.0)) - r

    def seg_dist(px, py, ax, ay, bx, by):
        abx, aby = bx - ax, by - ay
        ap = (px - ax) * abx + (py - ay) * aby
        ab2 = abx * abx + aby * aby
        t = clamp01(ap / ab2) if ab2 else 0.0
        return math.hypot(px - (ax + t * abx), py - (ay + t * aby))

    cx, cy = S / 2.0, S / 2.0
    hw = hh = S / 2.0 - 2 * u
    rad = 58 * u

    # gradient colors
    top = (59, 130, 246)   # #3B82F6
    bot = (29, 62, 168)    # #1D3EA8

    rows = []
    for y in range(S):
        row = bytearray()
        for x in range(S):
            t = y / max(S - 1, 1)
            br = top[0] + (bot[0] - top[0]) * t
            bg = top[1] + (bot[1] - top[1]) * t
            bb = top[2] + (bot[2] - top[2]) * t

            # glyph coverage
            g = 0.0
            if S >= 64:
                # ">" chevron: A(84,96) -> (124,128) -> (84,160)
                d1 = seg_dist(x + 0.5, y + 0.5, 84 * u, 96 * u, 124 * u, 128 * u) - 11 * u
                d2 = seg_dist(x + 0.5, y + 0.5, 124 * u, 128 * u, 84 * u, 160 * u) - 11 * u
                # cap joins
                g = max(g, clamp01(0.5 - d1), clamp01(0.5 - d2))
                # underscore
                dx = abs(x + 0.5 - 158 * u) - 24 * u
                dy = abs(y + 0.5 - 154 * u) - 11 * u
                d3 = min(max(dx, dy), 0.0) + math.hypot(max(dx, 0.0), max(dy, 0.0))
                g = max(g, clamp01(0.5 - d3))
            else:
                # simplified glyph for tiny sizes: blocky ">_" 
                d1 = seg_dist(x + 0.5, y + 0.5, 84 * u, 96 * u, 124 * u, 128 * u) - 14 * u
                d2 = seg_dist(x + 0.5, y + 0.5, 124 * u, 128 * u, 84 * u, 160 * u) - 14 * u
                g = max(g, clamp01(0.5 - d1), clamp01(0.5 - d2))

            # blend glyph (white)
            r = br + (255 - br) * g
            gg = bg + (255 - bg) * g
            b = bb + (255 - bb) * g

            bg_cov = clamp01(0.5 - sd_round_rect(x + 0.5, y + 0.5, cx, cy, hw, hh, rad))
            row += bytes((int(r), int(gg), int(b), int(bg_cov * 255)))
        rows.append(bytes(row))

    raw = b"".join(b"\x00" + r for r in rows)

    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", S, S, 8, 6, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr)
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def make_ico(png256):
    header = struct.pack("<HHH", 0, 1, 1)
    entry = struct.pack("<BBBBHHII", 0, 0, 0, 0, 1, 32, len(png256), 6 + 16)
    return header + entry + png256


os.makedirs(OUT, exist_ok=True)
p32 = make_png(32)
p128 = make_png(128)
p256 = make_png(256)
p1024 = make_png(1024)

with open(os.path.join(OUT, "32x32.png"), "wb") as f:
    f.write(p32)
with open(os.path.join(OUT, "128x128.png"), "wb") as f:
    f.write(p128)
with open(os.path.join(OUT, "icon.png"), "wb") as f:
    f.write(p1024)
with open(os.path.join(OUT, "icon.ico"), "wb") as f:
    f.write(make_ico(p256))

print("icons written to", os.path.abspath(OUT))
