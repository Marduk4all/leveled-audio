#!/usr/bin/env python3
"""Erzeugt das Programm-Icon von „Leveled Audio“ als PNG (ohne Zusatzpakete).

Motiv: abgerundetes Quadrat mit Farbverlauf (Indigo → Türkis), darauf
Pegelbalken – links ungleich (Rohaufnahme), rechts gleich hoch (gelevelt),
eingefasst von zwei goldenen Ziellinien.

Aufruf:  scripts/make-icon.py [Größe] [Ziel.png]
  Default: 1024 → assets/icon.png
"""
import math
import os
import struct
import sys
import zlib

SIZE = int(sys.argv[1]) if len(sys.argv) > 1 else 1024
OUT = sys.argv[2] if len(sys.argv) > 2 else os.path.join(
    os.path.dirname(__file__), "..", "assets", "icon.png")

S = SIZE / 1024.0
TOP = (0x4C, 0x3B, 0xCF)     # Indigo
BOTTOM = (0x12, 0xB5, 0xA6)  # Türkis
GOLD = (0xFF, 0xC8, 0x57)
WHITE = (0xFF, 0xFF, 0xFF)


def rrect_sd(px, py, x0, y0, x1, y1, r):
    """Vorzeichenbehafteter Abstand zu einem abgerundeten Rechteck."""
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    hx, hy = (x1 - x0) / 2 - r, (y1 - y0) / 2 - r
    dx, dy = abs(px - cx) - hx, abs(py - cy) - hy
    out = math.hypot(max(dx, 0.0), max(dy, 0.0))
    return out + min(max(dx, dy), 0.0) - r


def cov(sd):
    return min(max(0.5 - sd, 0.0), 1.0)


# RGBA-Puffer, transparent
buf = [[0.0, 0.0, 0.0, 0.0] for _ in range(SIZE * SIZE)]


def blend(x, y, rgb, a):
    if a <= 0:
        return
    p = buf[y * SIZE + x]
    na = a + p[3] * (1 - a)
    for i in range(3):
        p[i] = (rgb[i] * a + p[i] * p[3] * (1 - a)) / na if na > 0 else 0
    p[3] = na


def shape(x0, y0, x1, y1, r, color, alpha=1.0, grad=False):
    """Abgerundetes Rechteck zeichnen (Koordinaten im 1024er-Raster)."""
    x0, y0, x1, y1, r = (v * S for v in (x0, y0, x1, y1, r))
    for y in range(max(0, int(y0) - 1), min(SIZE, int(y1) + 2)):
        t = (y - y0) / max(y1 - y0, 1)
        for x in range(max(0, int(x0) - 1), min(SIZE, int(x1) + 2)):
            a = cov(rrect_sd(x + 0.5, y + 0.5, x0, y0, x1, y1, r)) * alpha
            if a <= 0:
                continue
            if grad:
                u = min(max(0.75 * t + 0.25 * (x - x0) / max(x1 - x0, 1), 0), 1)
                rgb = tuple(TOP[i] + (BOTTOM[i] - TOP[i]) * u for i in range(3))
            else:
                rgb = color
            blend(x, y, rgb, a)


# Schatten, Grundfläche (Apple-Raster: 824 px Körper bei 1024 px)
shape(100, 112, 924, 936, 185, (0, 0, 0), alpha=0.18)
shape(100, 100, 924, 924, 185, None, grad=True)

# Pegelbalken: links roh, rechts gelevelt
W, GAP, MID = 50, 30, 512
heights = [170, 420, 110, 330, 230, 300, 300, 300, 300]
n = len(heights)
x = MID - (n * W + (n - 1) * GAP) / 2
LEVEL = 300
for i, h in enumerate(heights):
    leveled = i >= 5
    a = 1.0 if leveled else 0.62
    shape(x, MID - h / 2, x + W, MID + h / 2, W / 2, WHITE, alpha=a)
    x += W + GAP

# Ziellinien über den gelevelten Balken
lx0 = MID - (n * W + (n - 1) * GAP) / 2 + 5 * (W + GAP) - 22
lx1 = MID + (n * W + (n - 1) * GAP) / 2 + 22
for ly in (MID - LEVEL / 2 - 34, MID + LEVEL / 2 + 22):
    shape(lx0, ly, lx1, ly + 12, 6, GOLD)


def png(path):
    raw = bytearray()
    for y in range(SIZE):
        raw.append(0)
        for x in range(SIZE):
            p = buf[y * SIZE + x]
            raw += bytes((round(p[0]), round(p[1]), round(p[2]), round(p[3] * 255)))

    def chunk(t, d):
        c = struct.pack(">I", len(d)) + t + d
        return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)

    data = b"\x89PNG\r\n\x1a\n"
    data += chunk(b"IHDR", struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0))
    data += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    data += chunk(b"IEND", b"")
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    with open(path, "wb") as f:
        f.write(data)


png(OUT)
print(OUT)
