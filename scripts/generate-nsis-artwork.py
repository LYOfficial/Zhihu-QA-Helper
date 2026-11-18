import os
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont


ROOT = Path(__file__).resolve().parents[1]
SCALE = 3
FONTS = Path(os.environ["WINDIR"]) / "Fonts"
LOGO = Image.open(ROOT / "crates/desktop/icons/icon.png").convert("RGBA")


def artwork(width, height, background):
    image = Image.new("RGBA", (width * SCALE, height * SCALE), background)
    return image, ImageDraw.Draw(image)


def mark(image, center, size):
    icon = LOGO.resize((size * SCALE, size * SCALE), Image.Resampling.LANCZOS)
    image.alpha_composite(icon, tuple(round((value - size / 2) * SCALE) for value in center))


def label(draw, position, text, size, color, *, anchor="mm", bold=False):
    preferred = FONTS / ("msyhbd.ttc" if bold else "msyh.ttc")
    font = ImageFont.truetype(str(preferred if preferred.exists() else FONTS / "simsun.ttc"), size * SCALE)
    draw.text(tuple(value * SCALE for value in position), text, font=font, fill=color, anchor=anchor)


def save(image, output, name):
    width, height = (side // SCALE for side in image.size)
    image.resize((width, height), Image.Resampling.LANCZOS).convert("RGB").save(output / name)


def main(output):
    output.mkdir(parents=True, exist_ok=True)
    header, draw = artwork(150, 57, "#ffffff")
    draw.rectangle((0, 53 * SCALE, 150 * SCALE, 57 * SCALE), fill="#e5efff")
    mark(header, (26, 27), 31)
    label(draw, (48, 22), "知乎答主助手", 14, "#173252", anchor="lm", bold=True)
    label(draw, (48, 39), "安装向导", 10, "#526983", anchor="lm")
    save(header, output, "installer-header.bmp")

    side, draw = artwork(164, 314, "#0c346e")
    for row in range(side.height):
        factor = row / side.height
        color = tuple(round(start * (1 - factor) + end * factor) for start, end in zip((9, 39, 85), (24, 102, 216)))
        draw.line((0, row, side.width, row), fill=color)
    draw.arc((-198, -240, 570, 470), 32, 174, fill="#205798", width=3)
    draw.arc((-232, -290, 610, 512), 37, 177, fill="#1e4c88", width=3)
    draw.rounded_rectangle((36 * SCALE, 51 * SCALE, 128 * SCALE, 143 * SCALE), radius=22 * SCALE, fill="#eff6ff")
    mark(side, (82, 97), 76)
    label(draw, (82, 169), "知乎答主助手", 17, "#ffffff", bold=True)
    label(draw, (82, 193), "等你落笔", 11, "#cce2ff")
    draw.line((30 * SCALE, 255 * SCALE, 134 * SCALE, 255 * SCALE), fill="#5991dc", width=SCALE)
    label(draw, (82, 273), "取材  /  核查  /  拟稿", 10, "#e4efff")
    save(side, output, "installer-sidebar.bmp")


if __name__ == "__main__":
    main(Path(sys.argv[1]))
