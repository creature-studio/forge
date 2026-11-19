#!/usr/bin/env python3
"""Render a color-preserving tmux `capture-pane -ep` from a real Forge session.

Usage: python3 scripts/render-terminal-capture.py frame.ansi output.webp
The UI comes from a live terminal frame; this script only rasterizes cells.
Requires Pillow. The optional title strip belongs to the screenshot, not Forge.
"""
from __future__ import annotations
import argparse
import re
import unicodedata
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont

ESC = re.compile(r'\x1b\[([0-9;]*)m')
FG = (212, 212, 212)
BG = (8, 8, 8)
CELL_W, CELL_H = 11, 22
PAD_X, PAD_Y = 23, 44
FONT = '/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf'
FONT_BOLD = '/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf'
CJK = '/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc'
ANSI16 = [(0, 0, 0), (205, 49, 49), (13, 188, 121), (229, 229, 16),
          (36, 114, 200), (188, 63, 188), (17, 168, 205), (229, 229, 229),
          (102, 102, 102), (241, 76, 76), (35, 209, 139), (245, 245, 67),
          (59, 142, 234), (214, 112, 214), (41, 184, 219), (255, 255, 255)]


def color256(i):
    if i < 16: return ANSI16[i]
    if i < 232:
        i -= 16
        return tuple([0, 95, 135, 175, 215, 255][n] for n in (i // 36, i // 6 % 6, i % 6))
    val = 8 + (i - 232) * 10
    return (val, val, val)


def char_width(ch):
    if unicodedata.combining(ch) or unicodedata.category(ch) in ('Cf', 'Mn', 'Me'): return 0
    return 2 if unicodedata.east_asian_width(ch) in ('W', 'F') else 1


def parse_capture(source: str):
    lines = source.splitlines()
    width = max((len(re.sub(ESC, '', line)) for line in lines), default=0)
    # Width is based on terminal cells (CJK may occupy two), not characters.
    width = max((sum(char_width(c) for c in ESC.sub('', line)) for line in lines), default=0)
    result = []
    fg, bg, bold = FG, BG, False
    for line in lines:
        cells = []
        cursor = 0
        for match in ESC.finditer(line):
            for char in line[cursor:match.start()]:
                cw = char_width(char)
                if cw == 0:
                    if cells: cells[-1] = (cells[-1][0] + char, *cells[-1][1:])
                    continue
                cells.append((char, fg, bg, bold, cw))
                if cw == 2: cells.append(('', fg, bg, bold, 0))
            codes = [int(s) if s else 0 for s in match.group(1).split(';')]
            i = 0
            while i < len(codes):
                code = codes[i]
                if code == 0: fg, bg, bold = FG, BG, False
                elif code == 1: bold = True
                elif code == 22: bold = False
                elif code == 39: fg = FG
                elif code == 49: bg = BG
                elif 30 <= code <= 37: fg = ANSI16[code - 30]
                elif 90 <= code <= 97: fg = ANSI16[code - 90 + 8]
                elif 40 <= code <= 47: bg = ANSI16[code - 40]
                elif 100 <= code <= 107: bg = ANSI16[code - 100 + 8]
                elif code in (38, 48) and i + 2 < len(codes):
                    if codes[i+1] == 5:
                        c = color256(codes[i+2]);i += 2
                        if code == 38: fg = c
                        else: bg = c
                    elif codes[i+1] == 2 and i + 4 < len(codes):
                        c = tuple(codes[i+2:i+5]);i += 4
                        if code == 38: fg = c
                        else: bg = c
                i += 1
            cursor = match.end()
        for char in line[cursor:]:
            cw = char_width(char)
            if cw == 0:
                if cells: cells[-1] = (cells[-1][0] + char, *cells[-1][1:])
                continue
            cells.append((char, fg, bg, bold, cw))
            if cw == 2: cells.append(('', fg, bg, bold, 0))
        result.append(cells)
    return result, width


def render(source: Path, output: Path, title: str = 'Forge'):
    cells, width = parse_capture(source.read_text())
    rows = len(cells)
    img = Image.new('RGB', (max(width, 1)*CELL_W + 2*PAD_X,
                            max(rows, 1)*CELL_H + PAD_X + PAD_Y), (16, 19, 25))
    draw = ImageDraw.Draw(img)
    draw.rounded_rectangle((8, 8, img.width-9, img.height-9), radius=12,
                           fill=(8, 8, 8), outline=(56, 67, 81), width=1)
    draw.rounded_rectangle((8, 8, img.width-9, PAD_Y-2), radius=12, fill=(28, 33, 42))
    draw.rectangle((9, PAD_Y-12, img.width-10, PAD_Y-2), fill=(28, 33, 42))
    small = ImageFont.truetype('/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf', 14)
    draw.text((PAD_X+7, 16), title, font=small, fill=(196, 202, 210))
    mono = ImageFont.truetype(FONT, 16)
    bold_font = ImageFont.truetype(FONT_BOLD, 16)
    wide_font = ImageFont.truetype(CJK, 16)
    for y, line in enumerate(cells):
        for x, (ch, fg, bg, is_bold, cw) in enumerate(line):
            if cw == 0: continue
            left = PAD_X + x*CELL_W
            top = PAD_Y + y*CELL_H
            if bg != BG: draw.rectangle((left, top, left+cw*CELL_W-1, top+CELL_H-1), fill=bg)
            if ch.strip():
                font = wide_font if cw == 2 else (bold_font if is_bold else mono)
                draw.text((left, top+1), ch, fill=fg, font=font)
    output.parent.mkdir(parents=True, exist_ok=True)
    if output.suffix == '.webp': img.save(output, 'WEBP', quality=88, method=6)
    else: img.save(output)
    print(f'{output}: {img.width}x{img.height}, {output.stat().st_size:,} bytes')


if __name__ == '__main__':
    a = argparse.ArgumentParser()
    a.add_argument('source', type=Path)
    a.add_argument('output', type=Path)
    a.add_argument('--title', default='Forge · Terminal workspace')
    args = a.parse_args()
    render(args.source, args.output, args.title)
