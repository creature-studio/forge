#!/usr/bin/env python3
"""Create a looping screenshot tour from the real terminal captures used by the site.

Requires Pillow: python3 -m pip install pillow
Run from the repository root: python3 scripts/build-product-tour.py
This is a screenshot slideshow, NOT a screen recording.
"""
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "docs/assets/forge-tour.gif"
SOURCES = [
    ("forge-workspace-900.webp", "FILES + PREVIEW", "Navigate files and inspect Markdown", 4500),
    ("forge-diff-900.webp", "GIT DIFF", "Review changes side by side", 4500),
    ("forge-theme-900.webp", "CODE + THEMES", "Read highlighted code in your terminal", 4500),
    ("forge-commit-900.webp", "COMMIT", "Finish the Git workflow in one place", 4500),
]
WIDTH, TOP, BOTTOM = 900, 72, 42
BG = (11, 13, 16)
FG = (232, 237, 241)
MUTED = (160, 171, 182)
ACCENT = (77, 206, 202)


def font(size, bold=False):
    suffix = "DejaVuSans-Bold.ttf" if bold else "DejaVuSans.ttf"
    try:
        return ImageFont.truetype("/usr/share/fonts/truetype/dejavu/" + suffix, size)
    except OSError:
        return ImageFont.load_default()


def render():
    frames = []
    duration = []
    for i, (filename, title, caption, ms) in enumerate(SOURCES):
        path = ROOT / "site/assets" / filename
        with Image.open(path) as raw:
            capture = raw.convert("RGB")
        if capture.width != WIDTH:
            capture = capture.resize((WIDTH, round(capture.height * WIDTH / capture.width)), Image.Resampling.LANCZOS)
        image = Image.new("RGB", (WIDTH, TOP + capture.height + BOTTOM), BG)
        image.paste(capture, (0, TOP))
        draw = ImageDraw.Draw(image)
        draw.text((23, 13), title, font=font(23, bold=True), fill=FG)
        draw.text((23, 47), caption, font=font(15), fill=MUTED)
        draw.text((23, TOP + capture.height + 13), "REAL FORGE TUI SCREENSHOTS", font=font(13, bold=True), fill=MUTED)
        draw.text((WIDTH - 65, TOP + capture.height + 13), f"{i + 1} / {len(SOURCES)}", font=font(13), fill=FG)
        draw.rectangle((0, TOP - 3, WIDTH - 1, TOP - 1), fill=(38, 43, 49))
        draw.rectangle((0, TOP - 3, round(WIDTH * (i + 1) / len(SOURCES)), TOP - 1), fill=ACCENT)
        frames.append(image.quantize(colors=128, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE))
        duration.append(ms)
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    frames[0].save(OUTPUT, format="GIF", save_all=True, append_images=frames[1:],
                   duration=duration, loop=0, optimize=True, disposal=2)
    with Image.open(OUTPUT) as final:
        assert final.n_frames == len(SOURCES)
        assert final.size[0] == WIDTH
    print(f"{OUTPUT.relative_to(ROOT)}: {OUTPUT.stat().st_size:,} bytes, {len(SOURCES)} frames, {sum(duration) / 1000:.0f}s")


if __name__ == "__main__":
    render()
