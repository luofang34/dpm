#!/usr/bin/env python3
"""The observer's chart colours, read from the Swift sources, keep the Sokoly workstation palette's rules: the
selection accent and every data colour drawn as an unlabelled mark (planned, critical, gate) stay at least ΔE00 10
apart in normal vision and under protanopia, deuteranopia and tritanopia, and each stands at least 3:1 against the
surfaces it is drawn on, in both appearances; saturated marks also sit at least 45° apart in OKLCH hue, so two of
them never read as one family (a crimson and a plum). A new or changed colour fails here before a person can mistake one
mark for another."""
import itertools
import math
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCES = ROOT / 'native' / 'swift' / 'Sources' / 'DPMObserver'
MARKS = ('planned', 'critical', 'gate')
# The surfaces a chart is drawn on: the Sokoly workstation surfaces and the macOS window backgrounds.
SURFACES = {'light': ['#EFF1F5', '#F7F9FC', '#FFFFFF', '#ECECEC'], 'dark': ['#101316', '#171A1E', '#1D2125', '#25292E', '#1E1F22', '#2A2B2F']}
DISTINCT, GRAPHIC, HUE_APART, NEUTRAL_CHROMA = 10.0, 3.0, 45.0, 0.05
# Machado, Oliveira & Fernandes (2009), severity 1.0, applied in linear RGB.
CVD = {
    'protan': ((0.152286, 1.052583, -0.204868), (0.114503, 0.786281, 0.099216), (-0.003882, -0.048116, 1.051998)),
    'deutan': ((0.367322, 0.860646, -0.227968), (0.280085, 0.672501, 0.047413), (-0.011820, 0.042940, 0.968881)),
    'tritan': ((1.255528, -0.076749, -0.178779), (-0.078411, 0.930809, 0.147602), (0.004733, 0.691367, 0.303900)),
}


def rgb(text):
    value = int(text.lstrip('#').removeprefix('0x'), 16)
    return (value >> 16 & 255, value >> 8 & 255, value & 255)


def linear(c):
    c /= 255
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def contrast(a, b):
    la, lb = (sum(w * linear(c) for w, c in zip((0.2126, 0.7152, 0.0722), x)) for x in (a, b))
    return (max(la, lb) + 0.05) / (min(la, lb) + 0.05)


def simulate(colour, kind):
    out = []
    for row in CVD[kind]:
        v = min(1.0, max(0.0, sum(m * linear(c) for m, c in zip(row, colour))))
        out.append(round(255 * (v * 12.92 if v <= 0.0031308 else 1.055 * v ** (1 / 2.4) - 0.055)))
    return tuple(out)


def lab(colour):
    r, g, b = (linear(c) for c in colour)
    x, y, z = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047, 0.2126 * r + 0.7152 * g + 0.0722 * b, (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883
    f = lambda t: t ** (1 / 3) if t > 216 / 24389 else (24389 / 27 * t + 16) / 116  # noqa: E731
    return 116 * f(y) - 16, 500 * (f(x) - f(y)), 200 * (f(y) - f(z))


def delta_e2000(c1, c2):
    (l1, a1, b1), (l2, a2, b2) = lab(c1), lab(c2)
    cbar = (math.hypot(a1, b1) + math.hypot(a2, b2)) / 2
    g = 0.5 * (1 - math.sqrt(cbar ** 7 / (cbar ** 7 + 25 ** 7)))
    a1p, a2p = a1 * (1 + g), a2 * (1 + g)
    c1p, c2p = math.hypot(a1p, b1), math.hypot(a2p, b2)
    h1p, h2p = math.degrees(math.atan2(b1, a1p)) % 360, math.degrees(math.atan2(b2, a2p)) % 360
    dh = 0.0
    if c1p * c2p:
        dh = h2p - h1p
        dh = dh - 360 if dh > 180 else dh + 360 if dh < -180 else dh
    dhh = 2 * math.sqrt(c1p * c2p) * math.sin(math.radians(dh / 2))
    lbar, cbarp, hbar = (l1 + l2) / 2, (c1p + c2p) / 2, h1p + h2p
    if c1p * c2p:
        hbar = (hbar + 360) / 2 if abs(h1p - h2p) > 180 else hbar / 2
    t = 1 - 0.17 * math.cos(math.radians(hbar - 30)) + 0.24 * math.cos(math.radians(2 * hbar)) + 0.32 * math.cos(math.radians(3 * hbar + 6)) - 0.20 * math.cos(math.radians(4 * hbar - 63))
    sl, sc, sh = 1 + 0.015 * (lbar - 50) ** 2 / math.sqrt(20 + (lbar - 50) ** 2), 1 + 0.045 * cbarp, 1 + 0.015 * cbarp * t
    rt = -2 * math.sqrt(cbarp ** 7 / (cbarp ** 7 + 25 ** 7)) * math.sin(math.radians(60 * math.exp(-(((hbar - 275) / 25) ** 2))))
    return math.sqrt(((l2 - l1) / sl) ** 2 + ((c2p - c1p) / sc) ** 2 + (dhh / sh) ** 2 + rt * ((c2p - c1p) / sc) * (dhh / sh))


def oklch(colour):
    r, g, b = (linear(c) for c in colour)
    l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b) ** (1 / 3)
    m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b) ** (1 / 3)
    s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b) ** (1 / 3)
    a, bb = 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s, 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s
    return 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s, math.hypot(a, bb), math.degrees(math.atan2(bb, a)) % 360


def worst_separation(a, b):
    return min([delta_e2000(a, b)] + [delta_e2000(simulate(a, k), simulate(b, k)) for k in CVD])


def marks(palette, design):
    """{appearance: {name: rgb}} of the accent and the chart marks, as the Swift sources define them."""
    found = {'light': {}, 'dark': {}}
    for name in MARKS:
        match = re.search(rf'static let {name} = dynamic\(light: (0x[0-9A-Fa-f]{{6}}), dark: (0x[0-9A-Fa-f]{{6}})\)', palette)
        if not match:
            raise SystemExit(f'Palette.swift: {name} is not a dynamic(light:dark:) colour this check can read')
        found['light'][name], found['dark'][name] = rgb(match[1]), rgb(match[2])
    accent = re.findall(r'NSColor\(srgbRed: 0x([0-9A-F]{2}) / 255\.0, green: 0x([0-9A-F]{2}) / 255\.0, blue: 0x([0-9A-F]{2}) / 255\.0', design)
    if len(accent) != 2:
        raise SystemExit('Design.swift: the accent is not the dark/light pair this check can read')
    found['dark']['accent'], found['light']['accent'] = (tuple(int(c, 16) for c in pair) for pair in accent)
    return found


def problems(found):
    out = []
    for appearance, colours in found.items():
        for name, colour in colours.items():
            low = min(contrast(colour, rgb(s)) for s in SURFACES[appearance])
            if low < GRAPHIC:
                out.append(f'{appearance} {name}: {low:.2f}:1 on its weakest surface, below {GRAPHIC}:1')
        for a, b in itertools.combinations(colours, 2):
            separation = worst_separation(colours[a], colours[b])
            if separation < DISTINCT:
                out.append(f'{appearance} {a} vs {b}: ΔE00 {separation:.1f} for the weakest reader, below {DISTINCT}')
            (_, ca, ha), (_, cb, hb) = oklch(colours[a]), oklch(colours[b])
            apart = min(abs(ha - hb), 360 - abs(ha - hb))
            if ca >= NEUTRAL_CHROMA and cb >= NEUTRAL_CHROMA and apart < HUE_APART:
                out.append(f'{appearance} {a} vs {b}: {apart:.0f}° apart in hue, below {HUE_APART:.0f}°: one colour family')
    return out


# A blue planned bar beside the blue accent, as the observer once drew, must fail.
assert problems({'light': {}, 'dark': {'accent': (77, 148, 255), 'planned': (57, 135, 229)}}), 'a blue planned bar must fail'
assert not problems({'light': {}, 'dark': {'accent': (77, 148, 255), 'planned': (129, 141, 159)}}), 'a neutral planned bar must pass'
# A plum gate beside the crimson critical, far enough by ΔE00, must fail as one family.
assert problems({'light': {'critical': (173, 39, 77), 'gate': (118, 37, 117)}, 'dark': {}}), 'a plum gate beside crimson must fail'

if __name__ == '__main__':
    found = marks((SOURCES / 'Palette.swift').read_text(), (SOURCES / 'Design.swift').read_text())
    failures = problems(found)
    if failures:
        sys.exit('\n'.join(failures))
    print(f'PASS: the accent and {len(MARKS)} chart marks stay apart and readable in both appearances')
